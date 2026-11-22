//! Draws a chat log as a layer to lay over the video: the box the user
//! placed, the newest message at its bottom, older ones scrolling up and
//! out, as Twitch's chat shows them.
//!
//! Each message is laid out and drawn once, when it arrives, over the box's
//! shade. A frame is then the messages' rows copied into place, plus the
//! frame of each animated emote on screen, so a frame costs about one copy
//! of the box (docs/chat.md has the numbers).

use std::collections::{HashMap, VecDeque};
use std::path::Path;

use super::images::Pictures;
use super::text::{Canvas, Ink, Run, Slot, Typesetter};
use super::{image_for, twitch_emote_images, Badge, BadgeRef, ChatLog, Emote, Message, Part, Placement};

/// The chat's design is drawn for a box this wide, and scaled to the real one.
const DESIGN_WIDTH: f64 = 300.0;
/// The character Parley leaves room for an image at.
const SLOT: char = '\u{FFFC}';
/// Twitch's own lines (a sub, a raid) are drawn in its purple, lightened to read over video.
const NOTICE_INK: Ink = Ink([0xC9, 0xA8, 0xFF]);

/// Where the box is on the frame, in pixels. Even numbers throughout:
/// 4:2:0 video keeps its colour at half size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Geometry {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl Geometry {
    pub fn new(placement: &Placement, frame_width: u32, frame_height: u32) -> Geometry {
        let placement = placement.sanitized();
        let even = |value: f64| ((value / 2.0).round().max(1.0) as u32) * 2;
        let frame_width = frame_width.max(2) & !1;
        let frame_height = frame_height.max(2) & !1;
        let width = even(placement.width * f64::from(frame_width)).min(frame_width);
        let height = even(placement.height * f64::from(frame_height)).min(frame_height);
        Geometry {
            x: even(placement.x * f64::from(frame_width)).min(frame_width - width) & !1,
            y: even(placement.y * f64::from(frame_height)).min(frame_height - height) & !1,
            width,
            height,
        }
    }
}

/// Sizes in pixels for a box of a given width: the design's sizes scaled.
#[derive(Debug, Clone, Copy)]
pub struct Style {
    scale: f64,
}

impl Style {
    pub fn for_width(width: u32) -> Style {
        Style {
            scale: f64::from(width) / DESIGN_WIDTH,
        }
    }

    pub fn font(&self) -> f64 {
        15.0 * self.scale
    }

    /// Emotes stand about two lines tall, as in Twitch's chat.
    pub fn emote(&self) -> f64 {
        28.0 * self.scale
    }

    pub fn badge(&self) -> f64 {
        18.0 * self.scale
    }

    fn badge_gap(&self) -> f64 {
        3.0 * self.scale
    }

    /// Room inside the box's edges.
    fn padding(&self) -> f64 {
        8.0 * self.scale
    }

    /// Room between messages.
    fn spacing(&self) -> f64 {
        4.0 * self.scale
    }

    fn corner(&self) -> f64 {
        6.0 * self.scale
    }

    /// How far down the box's top edge fades in, so a message scrolling out
    /// goes softly rather than cut through a line.
    fn fade(&self) -> f64 {
        18.0 * self.scale
    }
}

/// A picture at the height it's drawn: its address and its height in pixels.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct Key {
    url: String,
    height: u32,
}

/// A piece of a message, as it will be drawn.
#[derive(Debug, Clone, PartialEq)]
enum Token {
    Text(String),
    /// An emote, with the emotes drawn over it ("zero-width").
    Emote {
        word: String,
        base: Vec<super::Image>,
        overlays: Vec<Vec<super::Image>>,
    },
}

/// Splits a message into text and emotes: Twitch's own emotes as the
/// message marks them, third-party ones by their words. An overlay emote
/// right after another emote is drawn over it, as 7TV does, and the space
/// between them goes.
fn tokens(parts: &[Part], emotes: &HashMap<&str, &Emote>) -> Vec<Token> {
    let mut tokens: Vec<Token> = Vec::new();
    let push_text = |tokens: &mut Vec<Token>, text: &str| match tokens.last_mut() {
        Some(Token::Text(last)) => last.push_str(text),
        _ => tokens.push(Token::Text(text.to_string())),
    };
    for part in parts {
        match part {
            Part::Emote { id, name } => tokens.push(Token::Emote {
                word: name.clone(),
                base: twitch_emote_images(id),
                overlays: Vec::new(),
            }),
            Part::Text { text } => {
                for (index, word) in text.split(' ').enumerate() {
                    if index > 0 {
                        push_text(&mut tokens, " ");
                    }
                    let Some(emote) = emotes.get(word).filter(|_| !word.is_empty()) else {
                        push_text(&mut tokens, word);
                        continue;
                    };
                    if emote.overlay {
                        // After an emote, with nothing but spaces between: over it.
                        let spaces = match tokens.last() {
                            Some(Token::Text(text)) if text.trim().is_empty() => tokens.len() - 1,
                            _ => tokens.len(),
                        };
                        if let Some(Token::Emote { overlays, .. }) =
                            spaces.checked_sub(1).and_then(|i| tokens.get_mut(i))
                        {
                            overlays.push(emote.images.clone());
                            tokens.truncate(spaces);
                            continue;
                        }
                    }
                    tokens.push(Token::Emote {
                        word: word.to_string(),
                        base: emote.images.clone(),
                        overlays: Vec::new(),
                    });
                }
            }
        }
    }
    tokens
}

/// What goes in a slot.
enum Content {
    Badge(Key),
    Emote { base: Key, overlays: Vec<Key> },
}

/// An animated emote on a message, drawn afresh every frame.
struct Moving {
    x: i32,
    y: i32,
    width: u32,
    base: Key,
    overlays: Vec<Key>,
}

/// A message on screen: its rows, ready to copy, and its moving emotes.
struct Shown {
    height: usize,
    /// Straight RGBA, the box's width across, with the shade behind.
    pixels: Vec<u8>,
    moving: Vec<Moving>,
}

/// The chat layer of a video. Frames must be asked for in time order.
pub struct Layer {
    geometry: Geometry,
    style: Style,
    shade: f32,
    messages: Vec<Message>,
    next: usize,
    emotes: HashMap<String, Emote>,
    badges: HashMap<BadgeRef, Badge>,
    pictures: Pictures,
    typesetter: Typesetter,
    shown: VecDeque<Shown>,
    frame: Vec<u8>,
    drawn: bool,
    /// How much of each corner pixel shows: the box has rounded corners.
    corners: Vec<(usize, usize, f32)>,
    /// How much of each row at the top shows (see [`Style::fade`]).
    fade: Vec<f32>,
}

impl Layer {
    /// The layer for `log`, placed by `placement` on a frame of
    /// `frame_width` × `frame_height`. Its images come from the cache
    /// folder `images` (see [`super::images::fetch`]).
    pub fn new(log: ChatLog, placement: &Placement, frame_width: u32, frame_height: u32, images: &Path) -> Layer {
        let geometry = Geometry::new(placement, frame_width, frame_height);
        let style = Style::for_width(geometry.width);
        let shade = placement.sanitized().shade as f32;
        let ChatLog {
            mut messages,
            emotes,
            badges,
            ..
        } = log;
        messages.sort_by(|a, b| a.time.total_cmp(&b.time));
        let mut by_name: HashMap<String, Emote> = HashMap::new();
        for emote in emotes {
            by_name.entry(emote.name.clone()).or_insert(emote);
        }
        // The channel's badges come last and win (see `twitch::Twitch::badges`).
        let badges = badges
            .into_iter()
            .map(|badge| {
                (
                    BadgeRef {
                        set: badge.set.clone(),
                        version: badge.version.clone(),
                    },
                    badge,
                )
            })
            .collect();
        let corners = corner_mask(geometry.width as usize, geometry.height as usize, style.corner());
        let fade_rows = (style.fade().round() as usize).min(geometry.height as usize / 2);
        let fade = (0..fade_rows)
            .map(|row| {
                // Smooth at both ends.
                let t = (row as f32 + 0.5) / fade_rows as f32;
                t * t * (3.0 - 2.0 * t)
            })
            .collect();
        Layer {
            geometry,
            style,
            shade,
            messages,
            next: 0,
            emotes: by_name,
            badges,
            pictures: Pictures::new(images),
            typesetter: Typesetter::new(),
            shown: VecDeque::new(),
            frame: vec![0; geometry.width as usize * geometry.height as usize * 4],
            drawn: false,
            corners,
            fade,
        }
    }

    pub fn geometry(&self) -> Geometry {
        self.geometry
    }

    /// The layer at `seconds` into the video: straight RGBA, the box's size.
    pub fn frame(&mut self, seconds: f64) -> &[u8] {
        let arrived = self.admit(seconds);
        let moving = self.shown.iter().any(|shown| !shown.moving.is_empty());
        if arrived || moving || !self.drawn {
            self.compose(seconds);
            self.drawn = true;
        }
        &self.frame
    }

    /// Draws the messages sent by `seconds` that haven't been, and lets go
    /// of those scrolled out of the box. True if anything arrived.
    fn admit(&mut self, seconds: f64) -> bool {
        let start = self.next;
        let end = start + self.messages[start..].partition_point(|message| message.time <= seconds);
        if end == start {
            return false;
        }
        self.next = end;
        // Newest first, until the box is full: a burst bigger than the box
        // (a jump in a preview, a clip's first frame) only shows its last
        // messages, and the others needn't be drawn at all.
        let room = self.geometry.height as usize;
        let mut used = self.style.padding() as usize;
        let mut arrived = Vec::new();
        for index in (start..end).rev() {
            if used >= room {
                break;
            }
            let message = self.messages[index].clone();
            let shown = self.draw_message(&message);
            used += shown.height;
            arrived.push(shown);
        }
        if used >= room {
            self.shown.clear();
        }
        self.shown.extend(arrived.into_iter().rev());
        self.forget_hidden();
        true
    }

    /// Starts over with `messages`, keeping the fonts and images loaded so
    /// far: for a preview that jumps about the video.
    pub fn restart(&mut self, mut messages: Vec<Message>) {
        messages.sort_by(|a, b| a.time.total_cmp(&b.time));
        self.messages = messages;
        self.next = 0;
        self.shown.clear();
        self.drawn = false;
    }

    fn forget_hidden(&mut self) {
        let room = self.geometry.height as usize;
        let mut used = self.style.padding() as usize;
        let mut keep = 0;
        for shown in self.shown.iter().rev() {
            if used >= room {
                break;
            }
            used += shown.height;
            keep += 1;
        }
        while self.shown.len() > keep {
            self.shown.pop_front();
        }
    }

    fn compose(&mut self, seconds: f64) {
        let width = self.geometry.width as usize;
        let height = self.geometry.height as usize;
        let shade = [0, 0, 0, (self.shade * 255.0).round() as u8];
        for pixel in self.frame.as_chunks_mut::<4>().0 {
            *pixel = shade;
        }
        let millis = (seconds.max(0.0) * 1000.0) as u64;
        let mut bottom = height as i64 - self.style.padding().round() as i64;
        for shown in self.shown.iter().rev() {
            let top = bottom - shown.height as i64;
            for row in 0..shown.height {
                let y = top + row as i64;
                if y < 0 || y >= height as i64 {
                    continue;
                }
                let start = y as usize * width * 4;
                self.frame[start..start + width * 4].copy_from_slice(&shown.pixels[row * width * 4..][..width * 4]);
            }
            for moving in &shown.moving {
                let y = top as i32 + moving.y;
                for key in std::iter::once(&moving.base).chain(&moving.overlays) {
                    if let Some(picture) = self.pictures.get(&key.url, key.height) {
                        let frame = picture.frame_at(millis);
                        // Overlays are centred on the emote under them.
                        let x = moving.x + (moving.width as i32 - picture.width as i32) / 2;
                        blend_straight(&mut self.frame, width, height, frame, x, y);
                    }
                }
            }
            bottom = top;
            if bottom <= 0 {
                break;
            }
        }
        for (row, &shows) in self.fade.iter().enumerate() {
            for alpha in self.frame[row * width * 4..(row + 1) * width * 4]
                .iter_mut()
                .skip(3)
                .step_by(4)
            {
                *alpha = (f32::from(*alpha) * shows).round() as u8;
            }
        }
        for &(x, y, coverage) in &self.corners {
            let alpha = &mut self.frame[(y * width + x) * 4 + 3];
            *alpha = (f32::from(*alpha) * coverage).round() as u8;
        }
    }

    /// The image of `images` to draw `height` pixels high: the size that
    /// suits it if that was fetched, otherwise any size that was (the frame
    /// can be a little bigger or smaller than the lookup said).
    fn picture_for(&mut self, images: &[super::Image], height: u32) -> Option<Key> {
        let preferred = image_for(images, f64::from(height)).map(|image| image.url.clone());
        let others = images.iter().rev().map(|image| image.url.clone());
        preferred.into_iter().chain(others).find_map(|url| {
            self.pictures.get(&url, height)?;
            Some(Key { url, height })
        })
    }

    /// Lays out and draws one message over the box's shade.
    fn draw_message(&mut self, message: &Message) -> Shown {
        let style = self.style;
        let width = self.geometry.width as usize;
        let padding = style.padding();
        let emote_height = style.emote().round().max(1.0) as u32;
        let badge_height = style.badge().round().max(1.0) as u32;

        let mut text = String::new();
        let mut runs = Vec::new();
        let mut slots = Vec::new();
        let mut contents: Vec<Content> = Vec::new();
        let mut add_slot =
            |text: &mut String, contents: &mut Vec<Content>, content: Content, width: f64, height: f64| {
                slots.push(Slot {
                    index: text.len(),
                    id: contents.len() as u64,
                    width: width as f32,
                    height: height as f32,
                });
                contents.push(content);
                text.push(SLOT);
            };

        for badge in &message.badges {
            let Some(images) = self.badges.get(badge).map(|badge| badge.images.clone()) else {
                continue;
            };
            let Some(key) = self.picture_for(&images, badge_height) else {
                continue;
            };
            if let Some(picture) = self.pictures.get(&key.url, key.height) {
                let room = f64::from(picture.width) + style.badge_gap();
                add_slot(
                    &mut text,
                    &mut contents,
                    Content::Badge(key),
                    room,
                    f64::from(badge_height),
                );
            }
        }
        // Twitch's own line (a sub, a raid) has no name: it is about the user.
        let notice_start = text.len();
        if !message.notice {
            text.push_str(&message.name);
            runs.push(Run {
                range: notice_start..text.len(),
                ink: name_ink(message),
                bold: true,
            });
            text.push_str(": ");
        }

        let tokens = {
            let emotes: HashMap<&str, &Emote> =
                self.emotes.iter().map(|(name, emote)| (name.as_str(), emote)).collect();
            tokens(&message.parts, &emotes)
        };
        for token in tokens {
            match token {
                Token::Text(words) => text.push_str(&words),
                Token::Emote { word, base, overlays } => {
                    // Couldn't be had: the word, as a viewer without the image sees it.
                    let Some(base) = self.picture_for(&base, emote_height) else {
                        text.push_str(&word);
                        continue;
                    };
                    let Some(picture) = self.pictures.get(&base.url, base.height) else {
                        text.push_str(&word);
                        continue;
                    };
                    let picture_width = f64::from(picture.width);
                    let overlays = overlays
                        .iter()
                        .filter_map(|images| self.picture_for(images, emote_height))
                        .collect();
                    add_slot(
                        &mut text,
                        &mut contents,
                        Content::Emote { base, overlays },
                        picture_width,
                        f64::from(emote_height),
                    );
                }
            }
        }

        if message.notice {
            runs.push(Run {
                range: notice_start..text.len(),
                ink: NOTICE_INK,
                bold: false,
            });
        }
        let inner = width as f64 - 2.0 * padding;
        let layout = self
            .typesetter
            .layout(&text, &runs, &slots, style.font() as f32, inner as f32);
        let spacing = style.spacing().round() as usize;
        let height = layout.height().ceil() as usize + spacing;
        let mut canvas = Canvas::new(width, height, [0.0, 0.0, 0.0, self.shade]);
        let top = spacing as f32;
        self.typesetter.paint(&layout, &mut canvas, padding as f32, top);

        let mut moving = Vec::new();
        for placed in Typesetter::slots(&layout) {
            let x = (padding as f32 + placed.x).round() as i32;
            let y = (top + placed.y).round() as i32;
            match &contents[placed.id as usize] {
                Content::Badge(key) => {
                    if let Some(picture) = self.pictures.get(&key.url, key.height) {
                        canvas.draw_image(&picture.frames[0], x, y);
                    }
                }
                Content::Emote { base, overlays } => {
                    let keys: Vec<&Key> = std::iter::once(base).chain(overlays).collect();
                    let mut base_width = 0;
                    let mut animated = false;
                    for (index, key) in keys.iter().enumerate() {
                        if let Some(picture) = self.pictures.get(&key.url, key.height) {
                            if index == 0 {
                                base_width = picture.width;
                            }
                            animated |= picture.is_animated();
                        }
                    }
                    if animated {
                        moving.push(Moving {
                            x,
                            y,
                            width: base_width,
                            base: base.clone(),
                            overlays: overlays.clone(),
                        });
                    } else {
                        for key in keys {
                            if let Some(picture) = self.pictures.get(&key.url, key.height) {
                                let centred = x + (base_width as i32 - picture.width as i32) / 2;
                                canvas.draw_image(&picture.frames[0], centred, y);
                            }
                        }
                    }
                }
            }
        }
        Shown {
            height,
            pixels: canvas.to_straight_rgba(),
            moving,
        }
    }
}

/// Lays a premultiplied RGBA image over a straight RGBA frame `width`
/// pixels wide, with its top left at `x`, `y`; what falls outside is cut.
fn blend_straight(frame: &mut [u8], width: usize, height: usize, image: &image::RgbaImage, x: i32, y: i32) {
    for (dx, dy, pixel) in image.enumerate_pixels() {
        let source_alpha = u32::from(pixel[3]);
        if source_alpha == 0 {
            continue;
        }
        let (px, py) = (x + dx as i32, y + dy as i32);
        if px < 0 || py < 0 || px as usize >= width || py as usize >= height {
            continue;
        }
        let at = (py as usize * width + px as usize) * 4;
        let target = &mut frame[at..at + 4];
        let target_alpha = u32::from(target[3]);
        let keep = 255 - source_alpha;
        let alpha = source_alpha + (target_alpha * keep + 127) / 255;
        if alpha == 0 {
            continue;
        }
        for channel in 0..3 {
            let under = u32::from(target[channel]) * target_alpha / 255;
            let premultiplied = u32::from(pixel[channel]) + (under * keep + 127) / 255;
            target[channel] = (premultiplied * 255 / alpha).min(255) as u8;
        }
        target[3] = alpha.min(255) as u8;
    }
}

/// For each pixel in the box's corners outside a radius-`radius` rounding,
/// how much of it shows (0–1).
fn corner_mask(width: usize, height: usize, radius: f64) -> Vec<(usize, usize, f32)> {
    let radius = radius.min(width as f64 / 2.0).min(height as f64 / 2.0);
    let size = radius.ceil() as usize;
    let mut mask = Vec::new();
    for dy in 0..size {
        for dx in 0..size {
            // Distance from the rounding's centre to this pixel's centre.
            let (cx, cy) = (radius - dx as f64 - 0.5, radius - dy as f64 - 0.5);
            let coverage = (radius - (cx * cx + cy * cy).sqrt() + 0.5).clamp(0.0, 1.0) as f32;
            if coverage < 1.0 {
                for (x, y) in [
                    (dx, dy),
                    (width - 1 - dx, dy),
                    (dx, height - 1 - dy),
                    (width - 1 - dx, height - 1 - dy),
                ] {
                    mask.push((x, y, coverage));
                }
            }
        }
    }
    mask
}

/// The colour a name is drawn in: the one its owner picked, or the one
/// Twitch gives those who never did, lightened where it would be too dark
/// to read over video, as Twitch's own chat does in its dark theme.
fn name_ink(message: &Message) -> Ink {
    let picked = message.color.as_deref().and_then(parse_hex);
    Ink(readable(picked.unwrap_or_else(|| default_color(&message.login))))
}

fn parse_hex(color: &str) -> Option<[u8; 3]> {
    let hex = color.strip_prefix('#')?;
    if hex.len() != 6 {
        return None;
    }
    let value = u32::from_str_radix(hex, 16).ok()?;
    Some([(value >> 16) as u8, (value >> 8) as u8, value as u8])
}

/// Twitch's colours for names nobody picked, chosen by the name.
fn default_color(login: &str) -> [u8; 3] {
    const COLORS: [[u8; 3]; 15] = [
        [255, 0, 0],
        [0, 0, 255],
        [0, 128, 0],
        [178, 34, 34],
        [255, 127, 80],
        [154, 205, 50],
        [255, 69, 0],
        [46, 139, 87],
        [218, 165, 32],
        [210, 105, 30],
        [95, 158, 160],
        [30, 144, 255],
        [255, 105, 180],
        [138, 43, 226],
        [0, 255, 127],
    ];
    let sum: usize = login.bytes().map(usize::from).sum();
    COLORS[sum % COLORS.len()]
}

/// `rgb` made light enough to read over dark video: its lightness raised,
/// keeping its hue and saturation, until it is at least as bright as a mid grey.
fn readable(rgb: [u8; 3]) -> [u8; 3] {
    const LEAST: f64 = 0.3;
    let (hue, saturation, mut lightness) = to_hsl(rgb);
    let mut color = rgb;
    while luminance(color) < LEAST && lightness < 1.0 {
        lightness = (lightness + 0.02).min(1.0);
        color = from_hsl(hue, saturation, lightness);
    }
    color
}

fn luminance(rgb: [u8; 3]) -> f64 {
    let linear = |c: u8| {
        let c = f64::from(c) / 255.0;
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * linear(rgb[0]) + 0.7152 * linear(rgb[1]) + 0.0722 * linear(rgb[2])
}

fn to_hsl(rgb: [u8; 3]) -> (f64, f64, f64) {
    let [r, g, b] = rgb.map(|c| f64::from(c) / 255.0);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let lightness = (max + min) / 2.0;
    if max == min {
        return (0.0, 0.0, lightness);
    }
    let delta = max - min;
    let saturation = if lightness > 0.5 {
        delta / (2.0 - max - min)
    } else {
        delta / (max + min)
    };
    let hue = if max == r {
        (g - b) / delta + if g < b { 6.0 } else { 0.0 }
    } else if max == g {
        (b - r) / delta + 2.0
    } else {
        (r - g) / delta + 4.0
    };
    (hue / 6.0, saturation, lightness)
}

fn from_hsl(hue: f64, saturation: f64, lightness: f64) -> [u8; 3] {
    if saturation == 0.0 {
        let grey = (lightness * 255.0).round() as u8;
        return [grey; 3];
    }
    let q = if lightness < 0.5 {
        lightness * (1.0 + saturation)
    } else {
        lightness + saturation - lightness * saturation
    };
    let p = 2.0 * lightness - q;
    let channel = |mut t: f64| {
        if t < 0.0 {
            t += 1.0;
        }
        if t > 1.0 {
            t -= 1.0;
        }
        let value = if t < 1.0 / 6.0 {
            p + (q - p) * 6.0 * t
        } else if t < 0.5 {
            q
        } else if t < 2.0 / 3.0 {
            p + (q - p) * (2.0 / 3.0 - t) * 6.0
        } else {
            p
        };
        (value * 255.0).round().clamp(0.0, 255.0) as u8
    };
    [channel(hue + 1.0 / 3.0), channel(hue), channel(hue - 1.0 / 3.0)]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat::{images, Image};

    fn emote(name: &str, overlay: bool) -> Emote {
        Emote {
            name: name.into(),
            images: vec![Image {
                height: 32,
                url: format!("https://cdn.example/{name}"),
            }],
            overlay,
        }
    }

    fn message(id: &str, time: f64, text: &str) -> Message {
        Message {
            id: id.into(),
            time,
            name: "Viewer".into(),
            login: "viewer".into(),
            color: Some("#1E90FF".into()),
            badges: Vec::new(),
            parts: vec![Part::Text { text: text.into() }],
            notice: false,
        }
    }

    #[test]
    fn places_the_box_on_even_pixels_inside_the_frame() {
        let geometry = Geometry::new(&Placement::DEFAULT, 1920, 1080);
        assert_eq!(
            geometry,
            Geometry {
                x: 1468,
                y: 324,
                width: 422,
                height: 692
            }
        );
        let corner = Placement {
            x: 0.9,
            y: 0.9,
            width: 0.3,
            height: 0.3,
            shade: 0.0,
        };
        let geometry = Geometry::new(&corner, 1281, 721);
        assert!(geometry.x + geometry.width <= 1280 && geometry.y + geometry.height <= 720);
        assert!([geometry.x, geometry.y, geometry.width, geometry.height]
            .iter()
            .all(|v| v % 2 == 0));
    }

    #[test]
    fn finds_emotes_by_word_and_puts_overlays_over_the_emote_before() {
        let kekw = emote("KEKW", false);
        let rain = emote("RainTime", true);
        let emotes: HashMap<&str, &Emote> = [("KEKW", &kekw), ("RainTime", &rain)].into_iter().collect();
        let parts = vec![
            Part::Text {
                text: "lol KEKW  RainTime ok RainTime".into(),
            },
            Part::Emote {
                id: "25".into(),
                name: "Kappa".into(),
            },
            Part::Text {
                text: " RainTime".into(),
            },
        ];
        let tokens = tokens(&parts, &emotes);
        let summary: Vec<String> = tokens
            .iter()
            .map(|token| match token {
                Token::Text(text) => format!("text {text:?}"),
                Token::Emote { word, overlays, .. } => format!("{word}+{}", overlays.len()),
            })
            .collect();
        assert_eq!(
            summary,
            vec![
                "text \"lol \"",
                "KEKW+1",
                "text \" ok \"",
                // An overlay after words is an emote of its own.
                "RainTime+0",
                "Kappa+1",
            ]
        );
    }

    #[test]
    fn dark_names_are_lightened_and_bright_ones_kept() {
        let lightened = readable([0x1E, 0x2D, 0x1D]);
        assert!(luminance(lightened) >= 0.3, "{lightened:?}");
        let (hue, _, _) = to_hsl([0x1E, 0x2D, 0x1D]);
        let (new_hue, _, _) = to_hsl(lightened);
        assert!((hue - new_hue).abs() < 0.02, "keeps its hue");
        assert_eq!(readable([255, 105, 180]), [255, 105, 180]);
        assert!(luminance(readable([0, 0, 255])) >= 0.3);
        assert_eq!(default_color("viewer"), default_color("viewer"));
        assert_eq!(parse_hex("#1E90FF"), Some([0x1E, 0x90, 0xFF]));
        assert_eq!(parse_hex("1E90FF"), None);
    }

    #[test]
    fn rounds_the_corners_of_the_box() {
        let mask = corner_mask(100, 50, 6.0);
        assert!(
            mask.iter().any(|&(x, y, c)| (x, y) == (0, 0) && c == 0.0),
            "the very corner is clear"
        );
        assert!(mask
            .iter()
            .all(|&(x, y, _)| !(6..94).contains(&x) || !(6..44).contains(&y)));
        assert!(corner_mask(100, 50, 0.0).is_empty());
    }

    /// Draws a chat with one real image in a cache folder of its own.
    fn layer_with(messages: Vec<Message>, shade: f64) -> (Layer, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("siphon-chat-test-{}-{}", std::process::id(), messages.len()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut png = Vec::new();
        image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(32, 32, image::Rgba([255, 200, 0, 255])))
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        std::fs::write(images::cached_path(&dir, "https://cdn.example/KEKW"), png).unwrap();
        let log = ChatLog {
            messages,
            emotes: vec![emote("KEKW", false)],
            ..ChatLog::default()
        };
        let placement = Placement {
            shade,
            ..Placement::DEFAULT
        };
        (Layer::new(log, &placement, 1280, 720, &dir), dir)
    }

    #[test]
    fn draws_messages_from_the_bottom_up_as_they_arrive() {
        let messages = vec![
            message("1", 1.0, "first message KEKW"),
            message("2", 2.0, "second message"),
        ];
        let (mut layer, dir) = layer_with(messages, 0.25);
        let geometry = layer.geometry();
        let (width, height) = (geometry.width as usize, geometry.height as usize);
        let opaque_rows = |frame: &[u8]| -> Vec<usize> {
            (0..height)
                .filter(|row| frame[row * width * 4..][..width * 4].chunks(4).any(|p| p[3] == 255))
                .collect()
        };

        let empty = layer.frame(0.5).to_vec();
        assert_eq!(empty.len(), width * height * 4);
        assert!(opaque_rows(&empty).is_empty(), "nothing said yet: only the shade");
        assert_eq!(empty[(height / 2 * width + width / 2) * 4 + 3], 64, "a quarter dark");

        let one = layer.frame(1.5).to_vec();
        let rows = opaque_rows(&one);
        assert!(!rows.is_empty());
        assert!(
            rows.iter().all(|row| *row > height / 2),
            "the first message sits at the bottom"
        );
        // The emote is drawn: its yellow shows.
        assert!(one.chunks(4).any(|p| p == [255, 200, 0, 255]));

        let two = layer.frame(2.5).to_vec();
        let first_row = |frame: &[u8]| opaque_rows(frame)[0];
        assert!(
            first_row(&two) < first_row(&one),
            "the first message moved up for the second"
        );
        assert_eq!(layer.frame(2.6), &two[..], "nothing new, nothing moved");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_burst_draws_only_what_fits_and_a_restart_starts_over() {
        let messages: Vec<Message> = (0..500)
            .map(|i| message(&i.to_string(), 1.0, "all at once, as after a jump"))
            .collect();
        let (mut layer, dir) = layer_with(messages, 0.0);
        let burst = layer.frame(2.0).to_vec();
        assert!(layer.shown.len() < 40, "{}", layer.shown.len());
        layer.restart(vec![message("x", 5.0, "a fresh start")]);
        assert!(
            layer.frame(1.0).iter().skip(3).step_by(4).all(|alpha| *alpha == 0),
            "nothing said yet"
        );
        assert_ne!(layer.frame(6.0), &burst[..]);
        assert_eq!(layer.shown.len(), 1);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_full_box_lets_go_of_messages_scrolled_out() {
        let messages: Vec<Message> = (0..200)
            .map(|i| {
                message(
                    &i.to_string(),
                    f64::from(i) / 10.0,
                    "a message long enough to take a whole line or two",
                )
            })
            .collect();
        let (mut layer, dir) = layer_with(messages, 0.0);
        layer.frame(30.0);
        assert_eq!(layer.next, 200);
        assert!(layer.shown.len() < 60, "only what fits is kept: {}", layer.shown.len());
        let _ = std::fs::remove_dir_all(dir);
    }
}
