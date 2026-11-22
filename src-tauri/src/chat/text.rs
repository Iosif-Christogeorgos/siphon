//! Lays out and draws a line of chat: shaping, fallback fonts, bidi and line
//! breaking by Parley, glyphs drawn by swash, with Windows' own fonts.
//! Emotes and badges sit in the text as boxes Parley leaves room for.

use std::collections::HashMap;
use std::ops::Range;

use parley::{
    Alignment, AlignmentOptions, BaseDirection, FontContext, FontFamily, FontWeight, InlineBox, InlineBoxKind, Layout,
    LayoutContext, LineHeight, OverflowWrap, PositionedLayoutItem, StyleProperty, VerticalAlign,
};
use swash::scale::image::Content;
use swash::scale::{Render, ScaleContext, Source, StrikeWith};
use swash::zeno::{Format, Vector};

/// The look of chat: Windows' interface font, as clean as Twitch's own.
const FAMILY: &str = "Segoe UI";

/// A text colour, the brush Parley carries through a layout.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Ink(pub [u8; 3]);

/// A stretch of the text drawn in its own colour, bold or not.
pub struct Run {
    pub range: Range<usize>,
    pub ink: Ink,
    pub bold: bool,
}

/// Room for an image at byte `index` of the text, where a U+FFFC stands.
pub struct Slot {
    pub index: usize,
    pub id: u64,
    pub width: f32,
    pub height: f32,
}

/// Where a slot ended up, relative to the layout's top left.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placed {
    pub id: u64,
    pub x: f32,
    pub y: f32,
}

/// Premultiplied RGBA, one `f32` per channel, for drawing a message.
pub struct Canvas {
    pub width: usize,
    pub height: usize,
    pub pixels: Vec<[f32; 4]>,
}

impl Canvas {
    pub fn new(width: usize, height: usize, fill: [f32; 4]) -> Self {
        Self {
            width,
            height,
            pixels: vec![fill; width * height],
        }
    }

    /// Lays `color` (premultiplied) over the pixel at `x`, `y`, if it is inside.
    pub fn blend(&mut self, x: i32, y: i32, color: [f32; 4]) {
        if x < 0 || y < 0 || x as usize >= self.width || y as usize >= self.height || color[3] <= 0.0 {
            return;
        }
        let pixel = &mut self.pixels[y as usize * self.width + x as usize];
        let keep = 1.0 - color[3];
        for (channel, value) in pixel.iter_mut().enumerate() {
            *value = color[channel] + *value * keep;
        }
    }

    /// Lays a premultiplied RGBA image over the canvas with its top left at `x`, `y`.
    pub fn draw_image(&mut self, image: &image::RgbaImage, x: i32, y: i32) {
        for (dx, dy, pixel) in image.enumerate_pixels() {
            if pixel[3] == 0 {
                continue;
            }
            let color = pixel.0.map(|channel| f32::from(channel) / 255.0);
            self.blend(x + dx as i32, y + dy as i32, color);
        }
    }

    /// The canvas as straight (not premultiplied) RGBA bytes, what ffmpeg's
    /// overlay filter takes.
    pub fn to_straight_rgba(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(self.pixels.len() * 4);
        for pixel in &self.pixels {
            let alpha = pixel[3].clamp(0.0, 1.0);
            if alpha <= 0.0 {
                bytes.extend_from_slice(&[0, 0, 0, 0]);
                continue;
            }
            for channel in &pixel[..3] {
                bytes.push(((channel / alpha).clamp(0.0, 1.0) * 255.0).round() as u8);
            }
            bytes.push((alpha * 255.0).round() as u8);
        }
        bytes
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct GlyphKey {
    font: u64,
    index: u32,
    glyph: u32,
    size: u32,
    /// Quarter-pixel horizontal offset, so glyphs keep their spacing.
    offset: u8,
}

/// A glyph drawn by swash: an alpha mask, or colour (emoji).
struct Glyph {
    left: i32,
    top: i32,
    width: usize,
    height: usize,
    color: bool,
    data: Vec<u8>,
}

/// Fonts, layout scratch space and drawn glyphs, kept for a whole render.
pub struct Typesetter {
    fonts: FontContext,
    layouts: LayoutContext<Ink>,
    scaler: ScaleContext,
    glyphs: HashMap<GlyphKey, Option<Glyph>>,
}

impl Typesetter {
    pub fn new() -> Self {
        Self {
            fonts: FontContext::new(),
            layouts: LayoutContext::new(),
            scaler: ScaleContext::new(),
            glyphs: HashMap::new(),
        }
    }

    /// Lays out `text` in lines at most `width` wide: white unless a run
    /// says otherwise, `size` pixels high, always left to right as Twitch's
    /// chat is (a right-to-left message still reads right to left inside).
    pub fn layout(&mut self, text: &str, runs: &[Run], slots: &[Slot], size: f32, width: f32) -> Layout<Ink> {
        let mut builder = self.layouts.ranged_builder(&mut self.fonts, text, 1.0, true);
        builder.set_base_direction(BaseDirection::Ltr);
        builder.push_default(StyleProperty::FontFamily(FontFamily::named(FAMILY)));
        builder.push_default(StyleProperty::FontSize(size));
        builder.push_default(StyleProperty::LineHeight(LineHeight::FontSizeRelative(1.3)));
        builder.push_default(StyleProperty::Brush(Ink([0xEF, 0xEF, 0xF1])));
        // A long link or a word typed as one long run still wraps.
        builder.push_default(StyleProperty::OverflowWrap(OverflowWrap::Anywhere));
        for run in runs {
            builder.push(StyleProperty::Brush(run.ink), run.range.clone());
            if run.bold {
                builder.push(StyleProperty::FontWeight(FontWeight::BOLD), run.range.clone());
            }
        }
        for slot in slots {
            builder.push_inline_box(InlineBox {
                id: slot.id,
                kind: InlineBoxKind::InFlow,
                index: slot.index,
                width: slot.width,
                height: slot.height,
                baseline: None,
                vertical_align: VerticalAlign::MIDDLE,
            });
        }
        let mut layout: Layout<Ink> = builder.build(text);
        layout.break_all_lines(Some(width.max(1.0)));
        layout.set_alignment(Alignment::Left, AlignmentOptions::default());
        layout
    }

    /// Where each slot of `layout` went.
    pub fn slots(layout: &Layout<Ink>) -> Vec<Placed> {
        let mut placed = Vec::new();
        for line in layout.lines() {
            for item in line.items() {
                if let PositionedLayoutItem::InlineBox(slot) = item {
                    placed.push(Placed {
                        id: slot.id,
                        x: slot.x,
                        y: slot.y,
                    });
                }
            }
        }
        placed
    }

    /// Draws the text of `layout` into `canvas` with its top left at `x`,
    /// `y`: plain glyphs in their colour, with no outline or shadow.
    pub fn paint(&mut self, layout: &Layout<Ink>, canvas: &mut Canvas, x: f32, y: f32) {
        for line in layout.lines() {
            for item in line.items() {
                let PositionedLayoutItem::GlyphRun(glyph_run) = item else {
                    continue;
                };
                let run = glyph_run.run();
                let font = run.font();
                let size = run.font_size();
                let Some(font_ref) = swash::FontRef::from_index(font.data.as_ref(), font.index as usize) else {
                    continue;
                };
                let coords: Vec<i16> = run.normalized_coords().iter().map(|c| c.to_bits()).collect();
                let ink = glyph_run.style().brush.0.map(|c| f32::from(c) / 255.0);
                for glyph in glyph_run.positioned_glyphs() {
                    let gx = x + glyph.x;
                    let gy = y + glyph.y;
                    let offset = (gx.fract() * 4.0).floor().clamp(0.0, 3.0) as u8;
                    let key = GlyphKey {
                        font: font.data.id(),
                        index: font.index,
                        glyph: glyph.id,
                        size: size.to_bits(),
                        offset,
                    };
                    let scaler = &mut self.scaler;
                    let drawn = self.glyphs.entry(key).or_insert_with(|| {
                        let mut scaler = scaler
                            .builder(font_ref)
                            .size(size)
                            .hint(false)
                            .normalized_coords(coords.iter())
                            .build();
                        let image = Render::new(&[
                            Source::ColorOutline(0),
                            Source::ColorBitmap(StrikeWith::BestFit),
                            Source::Outline,
                        ])
                        .format(Format::Alpha)
                        .offset(Vector::new(f32::from(offset) / 4.0, 0.0))
                        .render(&mut scaler, glyph.id as u16)?;
                        Some(Glyph {
                            left: image.placement.left,
                            top: image.placement.top,
                            width: image.placement.width as usize,
                            height: image.placement.height as usize,
                            color: image.content == Content::Color,
                            data: image.data,
                        })
                    });
                    let Some(drawn) = drawn else { continue };
                    let left = gx.floor() as i32 + drawn.left;
                    let top = gy.round() as i32 - drawn.top;
                    for row in 0..drawn.height {
                        for column in 0..drawn.width {
                            let color = if drawn.color {
                                let pixel = &drawn.data[(row * drawn.width + column) * 4..][..4];
                                let alpha = f32::from(pixel[3]) / 255.0;
                                // swash's colour glyphs are straight RGBA.
                                [
                                    f32::from(pixel[0]) / 255.0 * alpha,
                                    f32::from(pixel[1]) / 255.0 * alpha,
                                    f32::from(pixel[2]) / 255.0 * alpha,
                                    alpha,
                                ]
                            } else {
                                let coverage = f32::from(drawn.data[row * drawn.width + column]) / 255.0;
                                [ink[0] * coverage, ink[1] * coverage, ink[2] * coverage, coverage]
                            };
                            canvas.blend(left + column as i32, top + row as i32, color);
                        }
                    }
                }
            }
        }
    }
}
