//! Measuring a conversion: a dozen short stretches of the video, spread over
//! it, encoded exactly as the download will encode it. Each stretch's
//! encoded bytes are set against its bytes as the site has them, and that
//! ratio against the whole: the site's stream already holds more bytes where
//! the picture is busier, so the ratio varies far less than the encoded
//! bitrate does. Measured 2026-11-22 against full encodes, with 8 to 12
//! stretches of 4 s: Big Buck Bunny in 4K to HEVC, 1,100–1,298 MB for
//! 1,183; a talking head in 4K to HEVC, 678–739 MB for 730 (the rule of
//! thumb said 450); ten minutes of Twitch 1080p60 at the chat's settings,
//! 654–660 MB for 630. Scaling the stretches' bitrate to the length instead
//! came to 954–1,687 MB for Big Buck Bunny.

use super::stretch::{self, Stretch};

/// Stretches measured, at most, on a graphics card's encoder and on the CPU's.
pub const ON_HARDWARE: usize = 12;
pub const ON_CPU: usize = 4;
/// How long each is, in seconds.
pub const SECONDS_ON_HARDWARE: f64 = 4.0;
pub const SECONDS_ON_CPU: f64 = 3.0;

/// Where to measure in the part from `start` to `end`: up to `count`
/// stretches of `length` seconds, evenly spread, in an order that spreads
/// them from the first (so the first few measured already cover the part).
/// A part no longer than two stretches is measured whole.
pub fn spans(start: f64, end: f64, count: usize, length: f64) -> Vec<(f64, f64)> {
    let part = end - start;
    if part <= length * 2.0 {
        return vec![(start, end)];
    }
    let count = count.min((part / (length * 2.0)).floor() as usize).max(2);
    spread(count)
        .into_iter()
        .map(|index| {
            let middle = start + (index as f64 + 0.5) * part / count as f64;
            (middle - length / 2.0, middle + length / 2.0)
        })
        .collect()
}

/// 0 to `count` - 1 in the order of the van der Corput sequence: each next
/// one in the middle of the widest gap left.
fn spread(count: usize) -> Vec<usize> {
    let mut order: Vec<usize> = Vec::with_capacity(count);
    let mut k: u32 = 0;
    while order.len() < count && k < 1 << 16 {
        // k's bits reversed, as a fraction.
        let fraction = f64::from(k.reverse_bits()) / 2f64.powi(32);
        let index = ((fraction * count as f64) as usize).min(count - 1);
        if !order.contains(&index) {
            order.push(index);
        }
        k += 1;
    }
    order
}

/// What the measured stretches held and came to, summed.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Sums {
    pub video: u64,
    pub audio: u64,
    pub encoded: u64,
    pub seconds: f64,
    pub count: usize,
}

impl Sums {
    pub fn add(&mut self, measured: &Stretch, from: f64, until: f64) {
        self.video += stretch::bytes(&measured.video, from, until);
        self.audio += stretch::bytes(&measured.audio, from, until);
        self.encoded += stretch::bytes(&measured.encoded, from, until);
        self.seconds += until - from;
        self.count += 1;
    }

    /// The bytes the encoded picture comes to over the part, from what the
    /// part holds as the site has it (`source`, the stream's bytes: with its
    /// sound, for a stream that carries both). `keeps_sound`: that sound is
    /// copied into the file too, so it counts on both sides. Without the
    /// part's bytes, the stretches' bytes per second over `seconds`.
    pub fn whole(&self, source: Option<u64>, keeps_sound: bool, seconds: f64) -> Option<f64> {
        if self.video == 0 || self.encoded == 0 {
            return None;
        }
        let sound = if keeps_sound { self.audio as f64 } else { 0.0 };
        match source {
            Some(source) => {
                // The share of the stream that is the picture, and the sound if it stays.
                let held = self.video as f64 + self.audio as f64;
                let kept_share = (self.video as f64 + sound) / held;
                let ratio = (self.encoded as f64 + sound) / (self.video as f64 + sound);
                Some(source as f64 * kept_share * ratio)
            }
            None if self.seconds > 0.0 => Some((self.encoded as f64 + sound) / self.seconds * seconds),
            None => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sizing::stretch::Packet;

    #[test]
    fn spreads_the_stretches_over_the_part() {
        let measured = spans(0.0, 600.0, 12, 4.0);
        assert_eq!(measured.len(), 12);
        // The first ones already cover it: the start, the middle, a quarter, three quarters.
        let middles: Vec<f64> = measured.iter().map(|(from, until)| (from + until) / 2.0).collect();
        assert_eq!(&middles[..4], &[25.0, 325.0, 175.0, 475.0]);
        assert!(measured.iter().all(|(from, until)| *from >= 0.0 && *until <= 600.0));
        let mut sorted = middles.clone();
        sorted.sort_by(f64::total_cmp);
        sorted.dedup();
        assert_eq!(sorted.len(), 12);
        // A short part: fewer, and one no longer than two stretches, whole.
        assert_eq!(spans(100.0, 130.0, 12, 4.0).len(), 3);
        assert_eq!(spans(100.0, 107.0, 12, 4.0), [(100.0, 107.0)]);
    }

    fn packet(pts: f64, size: u64) -> Packet {
        Packet { pts, size, key: true }
    }

    #[test]
    fn scales_the_part_by_what_the_stretches_came_to() {
        let stretch = Stretch {
            video: vec![packet(10.0, 1000), packet(11.0, 1000)],
            audio: vec![packet(10.0, 100), packet(11.0, 100)],
            encoded: vec![packet(10.0, 3000), packet(11.0, 1000)],
        };
        let mut sums = Sums::default();
        sums.add(&stretch, 10.0, 12.0);
        assert_eq!((sums.video, sums.audio, sums.encoded, sums.count), (2000, 200, 4000, 1));
        // A stream of picture alone: twice its bytes.
        let picture_only = Stretch {
            audio: Vec::new(),
            ..stretch.clone()
        };
        let mut alone = Sums::default();
        alone.add(&picture_only, 10.0, 12.0);
        assert_eq!(alone.whole(Some(1_000_000), false, 600.0), Some(2_000_000.0));
        // One with its sound, kept: the sound counts on both sides.
        let kept = sums.whole(Some(1_100_000), true, 600.0).unwrap();
        assert!((kept - 1_100_000.0 * 4200.0 / 2200.0).abs() < 1.0);
        // Its sound converted apart: only the picture's share of it.
        let picture = sums.whole(Some(1_100_000), false, 600.0).unwrap();
        assert!((picture - 2_000_000.0).abs() < 1.0);
        // Without the part's bytes: per second.
        assert_eq!(sums.whole(None, false, 600.0), Some(4000.0 / 2.0 * 600.0));
        assert_eq!(Sums::default().whole(Some(1), false, 1.0), None);
    }
}
