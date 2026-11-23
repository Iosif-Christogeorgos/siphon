//! The soft shadow under chat, as stream overlays draw chat over video: all
//! of a message (its text, badges and emotes) casts one shadow a little
//! below and right of it, blurred, in black at about two thirds. It keeps
//! white text readable over a white picture, with no outline around it.

use image::RgbaImage;

/// How dark a shadow is where it is darkest: this much black (0–1).
pub const STRENGTH: f32 = 0.69;

/// A shadow's fall and blur, as one kernel used across and then down.
pub struct Shadow {
    /// How far each sample is from the pixel it falls on, and its weight,
    /// in order. Taken across and down alike: the shadow falls diagonally.
    taps: Vec<(i32, f32)>,
}

/// An image's shadow, ready to lay under it.
pub struct Cast {
    /// Where the shadow's top left is, from the image's.
    pub left: i32,
    pub top: i32,
    /// Its width; its height is what the pixels make of it.
    pub width: usize,
    /// How much black each pixel gets (0–255), [`STRENGTH`] included.
    pub alpha: Vec<u8>,
}

impl Shadow {
    /// A shadow `offset` pixels right of and below what casts it, blurred
    /// by a Gaussian `blur` pixels wide (its standard deviation).
    pub fn new(offset: f64, blur: f64) -> Shadow {
        let blur = blur.max(0.05);
        let reach = 3.0 * blur;
        // How much of a Gaussian centred at `offset` falls left of `x`.
        let below = |x: f64| 0.5 * (1.0 + erf((x - offset) / (blur * std::f64::consts::SQRT_2)));
        let mut taps: Vec<(i32, f32)> = ((offset - reach).floor() as i32..=(offset + reach).ceil() as i32)
            .map(|i| {
                let at = f64::from(i);
                (i, (below(at + 0.5) - below(at - 0.5)) as f32)
            })
            .filter(|(_, weight)| *weight > 1e-4)
            .collect();
        let total: f32 = taps.iter().map(|(_, weight)| weight).sum();
        for (_, weight) in &mut taps {
            *weight /= total;
        }
        Shadow { taps }
    }

    /// How far the shadow reaches past what casts it, in pixels, on any side.
    pub fn reach(&self) -> usize {
        self.taps
            .iter()
            .map(|(at, _)| at.unsigned_abs() as usize)
            .max()
            .unwrap_or(0)
    }

    /// The shadow of `coverage` (`width` × `height`, how much of each pixel
    /// is covered, 0–1): how much black falls on each pixel, [`STRENGTH`]
    /// included. Only the part around what is covered is worked out.
    pub fn cast(&self, coverage: &[f32], width: usize, height: usize) -> Vec<f32> {
        let mut shadow = vec![0.0; width * height];
        let Some((left, right, top, bottom)) = covered(coverage, width, height) else {
            return shadow;
        };
        let (first, last) = match (self.taps.first(), self.taps.last()) {
            (Some(first), Some(last)) => (first.0, last.0),
            _ => return shadow,
        };
        // The columns and rows a shadow can fall on.
        let spread = |from: usize, to: usize, size: usize| {
            let start = (from as i32 + first).max(0) as usize;
            let end = ((to as i32 + last).max(0) as usize).min(size - 1);
            (start, end)
        };
        let (x0, x1) = spread(left, right, width);
        let (y0, y1) = spread(top, bottom, height);

        // Across, along the rows that have anything in them.
        let mut across = vec![0.0; width * height];
        for y in top..=bottom {
            let row = &coverage[y * width..][..width];
            let out = &mut across[y * width..][..width];
            for &(at, weight) in &self.taps {
                // out[x] takes row[x - at], for every x where both are inside.
                let start = (x0 as i32).max(at);
                let end = (x1 as i32).min(width as i32 - 1 + at);
                if start > end {
                    continue;
                }
                let source = (start - at) as usize;
                for (to, from) in out[start as usize..=end as usize].iter_mut().zip(&row[source..]) {
                    *to += weight * from;
                }
            }
        }
        // Down, from those rows.
        for y in y0..=y1 {
            for &(at, weight) in &self.taps {
                let from = y as i32 - at;
                if from < top as i32 || from > bottom as i32 {
                    continue;
                }
                let weight = weight * STRENGTH;
                let (out, row) = (y * width, from as usize * width);
                for x in x0..=x1 {
                    shadow[out + x] += weight * across[row + x];
                }
            }
        }
        shadow
    }

    /// The shadow of `image` (premultiplied RGBA), with room around it for
    /// the shadow to fall and blur.
    pub fn cast_image(&self, image: &RgbaImage) -> Cast {
        let reach = self.reach();
        let width = image.width() as usize + 2 * reach;
        let height = image.height() as usize + 2 * reach;
        let mut coverage = vec![0.0; width * height];
        for (x, y, pixel) in image.enumerate_pixels() {
            coverage[(y as usize + reach) * width + x as usize + reach] = f32::from(pixel[3]) / 255.0;
        }
        Cast {
            left: -(reach as i32),
            top: -(reach as i32),
            width,
            alpha: self
                .cast(&coverage, width, height)
                .into_iter()
                .map(|black| (black * 255.0).round().clamp(0.0, 255.0) as u8)
                .collect(),
        }
    }
}

/// The first and last column and row with anything in them, if any.
fn covered(coverage: &[f32], width: usize, height: usize) -> Option<(usize, usize, usize, usize)> {
    let (mut left, mut right, mut top, mut bottom) = (usize::MAX, 0, usize::MAX, 0);
    for y in 0..height {
        for (x, &value) in coverage[y * width..][..width].iter().enumerate() {
            if value > 0.0 {
                left = left.min(x);
                right = right.max(x);
                top = top.min(y);
                bottom = y;
            }
        }
    }
    (left != usize::MAX).then_some((left, right, top, bottom))
}

/// The error function, to within 1.5 × 10⁻⁷ (Abramowitz and Stegun, 7.1.26).
fn erf(x: f64) -> f64 {
    let t = 1.0 / (1.0 + 0.327_591_1 * x.abs());
    let poly =
        t * (0.254_829_592 + t * (-0.284_496_736 + t * (1.421_413_741 + t * (-1.453_152_027 + t * 1.061_405_429))));
    let value = 1.0 - poly * (-x * x).exp();
    if x < 0.0 {
        -value
    } else {
        value
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_kernel_is_whole_and_centred_on_the_offset() {
        let shadow = Shadow::new(1.5, 2.0);
        let total: f32 = shadow.taps.iter().map(|(_, weight)| weight).sum();
        assert!((total - 1.0).abs() < 1e-5);
        let centre: f32 = shadow.taps.iter().map(|(at, weight)| *at as f32 * weight).sum();
        assert!((centre - 1.5).abs() < 0.01, "{centre}");
        assert!(shadow.reach() >= 7, "three blurs past the offset: {}", shadow.reach());
        assert!((erf(0.5) - 0.520_499_877_8).abs() < 2e-7);
        assert!((erf(-1.2) + 0.910_313_978_2).abs() < 2e-7);
    }

    #[test]
    fn falls_below_and_right_of_what_casts_it() {
        let (width, height) = (40, 40);
        let mut coverage = vec![0.0; width * height];
        for y in 15..25 {
            for x in 15..25 {
                coverage[y * width + x] = 1.0;
            }
        }
        let shadow = Shadow::new(1.0, 1.65).cast(&coverage, width, height);
        let at = |x: usize, y: usize| shadow[y * width + x];
        assert!(
            (at(20, 20) - STRENGTH).abs() < 0.01,
            "full under the middle: {}",
            at(20, 20)
        );
        assert!(at(25, 20) > 2.0 * at(14, 20), "more right than left");
        assert!(at(20, 25) > 2.0 * at(20, 14), "more below than above");
        assert!(at(20, 13) > 0.0, "a little spills above");
        assert_eq!(at(20, 0), 0.0);
        assert_eq!(at(39, 39), 0.0);
        let total: f32 = shadow.iter().sum();
        assert!((total - 100.0 * STRENGTH).abs() < 0.01, "nothing is lost: {total}");
    }

    #[test]
    fn an_image_casts_its_shadow_with_room_around_it() {
        let mut image = RgbaImage::new(4, 4);
        image.put_pixel(1, 1, image::Rgba([255, 255, 255, 255]));
        let shadow = Shadow::new(1.0, 1.0);
        let cast = shadow.cast_image(&image);
        let reach = shadow.reach();
        assert_eq!(
            (cast.width, cast.alpha.len() / cast.width),
            (4 + 2 * reach, 4 + 2 * reach)
        );
        assert_eq!((cast.left, cast.top), (-(reach as i32), -(reach as i32)));
        let darkest = (0..cast.alpha.len()).max_by_key(|&i| cast.alpha[i]).unwrap();
        // Darkest one pixel right and down of the pixel that casts it.
        assert_eq!((darkest % cast.width, darkest / cast.width), (reach + 2, reach + 2));
        assert!(cast
            .alpha
            .iter()
            .all(|&alpha| f32::from(alpha) <= STRENGTH * 255.0 + 0.5));
    }

    #[test]
    fn nothing_casts_nothing() {
        let shadow = Shadow::new(1.0, 1.0).cast(&[0.0; 16], 4, 4);
        assert!(shadow.iter().all(|&black| black == 0.0));
    }
}
