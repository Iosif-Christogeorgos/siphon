//! Emote and badge images: downloaded once into Siphon's cache folder, and
//! decoded (every frame of an animated one) when the chat is drawn.

use std::collections::HashMap;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use futures_util::StreamExt;
use image::{AnimationDecoder, ImageDecoder, RgbaImage};
use sha2::{Digest, Sha256};

/// Images fetched at once.
const PARALLEL: usize = 12;
/// An emote larger than this isn't an emote.
const MAX_BYTES: usize = 8 * 1024 * 1024;
/// Numbers each image's file while it is written (see [`fetch`]).
static PARTIAL: AtomicU64 = AtomicU64::new(0);

/// Where an image is kept in `dir`: named after its address, so the same
/// emote is fetched once for every download.
pub fn cached_path(dir: &Path, url: &str) -> PathBuf {
    let digest = Sha256::digest(url.as_bytes());
    dir.join(format!("{}.img", hex::encode(&digest[..12])))
}

/// Fetches into `dir` whichever of `urls` it doesn't have yet, each tried
/// again after a failure that may pass (see [`super::send`]). Returns how
/// many couldn't be had: those are drawn as their words instead.
pub async fn fetch(http: &reqwest::Client, dir: &Path, urls: Vec<String>) -> usize {
    if tokio::fs::create_dir_all(dir).await.is_err() {
        return urls.len();
    }
    let missing: Vec<String> = urls.into_iter().filter(|url| !cached_path(dir, url).exists()).collect();
    let results = futures_util::stream::iter(missing)
        .map(|url| async move {
            let bytes = async {
                let response = super::send(|| http.get(&url)).await.ok()?.error_for_status().ok()?;
                if response
                    .content_length()
                    .is_some_and(|length| length > MAX_BYTES as u64)
                {
                    return None;
                }
                let bytes = response.bytes().await.ok()?;
                (bytes.len() <= MAX_BYTES && decode(&bytes).is_some()).then_some(bytes)
            }
            .await;
            let Some(bytes) = bytes else { return false };
            let file = cached_path(dir, &url);
            // Written beside it under a name of its own first: two downloads
            // (or a download and a preview) may fetch the same image at once.
            let partial = file.with_extension(format!("{}.part", PARTIAL.fetch_add(1, Ordering::Relaxed)));
            let written =
                tokio::fs::write(&partial, &bytes).await.is_ok() && tokio::fs::rename(&partial, &file).await.is_ok();
            if !written {
                let _ = tokio::fs::remove_file(&partial).await;
            }
            // Should the other fetch have put it in place first, it is there all the same.
            written || file.exists()
        })
        .buffer_unordered(PARALLEL)
        .collect::<Vec<bool>>()
        .await;
    results.iter().filter(|ok| !**ok).count()
}

/// A decoded image: one frame, or an animation's frames, all the same size,
/// in premultiplied RGBA.
#[derive(Clone)]
pub struct Picture {
    pub width: u32,
    pub height: u32,
    pub frames: Vec<RgbaImage>,
    /// When each frame ends, in milliseconds from the start of a loop.
    pub ends: Vec<u32>,
}

impl Picture {
    pub fn is_animated(&self) -> bool {
        self.frames.len() > 1
    }

    /// The frame showing `millis` into the video. Every copy of an emote
    /// moves in step, as in the browser.
    pub fn frame_at(&self, millis: u64) -> &RgbaImage {
        let total = self.ends.last().copied().unwrap_or(0);
        if total == 0 || self.frames.len() == 1 {
            return &self.frames[0];
        }
        let into = (millis % u64::from(total)) as u32;
        let index = self.ends.partition_point(|end| *end <= into);
        &self.frames[index.min(self.frames.len() - 1)]
    }

    /// The same picture `height` pixels high, every frame resized smoothly.
    pub fn scaled(&self, height: u32) -> Picture {
        let height = height.max(1);
        if height == self.height {
            return self.clone();
        }
        let width = ((f64::from(self.width) * f64::from(height) / f64::from(self.height.max(1))).round() as u32).max(1);
        Picture {
            width,
            height,
            frames: self
                .frames
                .iter()
                .map(|frame| image::imageops::resize(frame, width, height, image::imageops::FilterType::CatmullRom))
                .collect(),
            ends: self.ends.clone(),
        }
    }
}

/// Decodes a PNG (APNG too), GIF or WebP, animated or not. Frames are
/// premultiplied before anything scales them, so transparent edges don't
/// darken.
pub fn decode(bytes: &[u8]) -> Option<Picture> {
    let frames: Vec<(RgbaImage, u32)> = if bytes.starts_with(b"GIF8") {
        let decoder = image::codecs::gif::GifDecoder::new(Cursor::new(bytes)).ok()?;
        animation(decoder.into_frames())?
    } else if bytes.len() > 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        let decoder = image::codecs::webp::WebPDecoder::new(Cursor::new(bytes)).ok()?;
        if decoder.has_animation() {
            animation(decoder.into_frames())?
        } else {
            vec![(still(decoder)?, 0)]
        }
    } else if bytes.starts_with(b"\x89PNG") {
        let decoder = image::codecs::png::PngDecoder::new(Cursor::new(bytes)).ok()?;
        if decoder.is_apng().ok()? {
            animation(decoder.apng().ok()?.into_frames())?
        } else {
            vec![(still(decoder)?, 0)]
        }
    } else {
        return None;
    };
    let (width, height) = frames.first()?.0.dimensions();
    if width == 0 || height == 0 || frames.iter().any(|(frame, _)| frame.dimensions() != (width, height)) {
        return None;
    }
    let mut end = 0;
    let mut ends = Vec::with_capacity(frames.len());
    let frames = frames
        .into_iter()
        .map(|(mut frame, delay)| {
            premultiply(&mut frame);
            end += delay;
            ends.push(end);
            frame
        })
        .collect();
    Some(Picture {
        width,
        height,
        frames,
        ends,
    })
}

fn still(decoder: impl ImageDecoder) -> Option<RgbaImage> {
    Some(image::DynamicImage::from_decoder(decoder).ok()?.to_rgba8())
}

/// Every frame of an animation and how long it shows. A delay of 10 ms or
/// less is shown for 100 ms, as browsers do (GIFs made for them rely on it).
fn animation(frames: image::Frames<'_>) -> Option<Vec<(RgbaImage, u32)>> {
    let frames: Vec<(RgbaImage, u32)> = frames
        .take(1000)
        .map(|frame| {
            let frame = frame.ok()?;
            let (numerator, denominator) = frame.delay().numer_denom_ms();
            let millis = numerator / denominator.max(1);
            Some((frame.into_buffer(), if millis <= 10 { 100 } else { millis }))
        })
        .collect::<Option<_>>()?;
    (!frames.is_empty()).then_some(frames)
}

fn premultiply(frame: &mut RgbaImage) {
    for pixel in frame.pixels_mut() {
        let alpha = u16::from(pixel[3]);
        for channel in 0..3 {
            pixel[channel] = ((u16::from(pixel[channel]) * alpha + 127) / 255) as u8;
        }
    }
}

/// Decoded pictures by address, at the heights they are drawn at. A picture
/// that can't be read is remembered as missing, and drawn as its word.
pub struct Pictures {
    dir: PathBuf,
    loaded: HashMap<(String, u32), Option<Picture>>,
}

impl Pictures {
    pub fn new(dir: &Path) -> Self {
        Self {
            dir: dir.to_path_buf(),
            loaded: HashMap::new(),
        }
    }

    /// The picture at `url`, `height` pixels high.
    pub fn get(&mut self, url: &str, height: u32) -> Option<&Picture> {
        let key = (url.to_string(), height);
        if !self.loaded.contains_key(&key) {
            let picture = std::fs::read(cached_path(&self.dir, url))
                .ok()
                .and_then(|bytes| decode(&bytes))
                .map(|picture| picture.scaled(height));
            self.loaded.insert(key.clone(), picture);
        }
        self.loaded.get(&key).and_then(Option::as_ref)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 2×1 GIF with two frames: red for 50 ms, then blue for 10 ms (shown for 100).
    fn two_frame_gif() -> Vec<u8> {
        use image::codecs::gif::{GifEncoder, Repeat};
        use image::{Delay, Frame, Rgba};
        let mut bytes = Vec::new();
        {
            let mut encoder = GifEncoder::new(&mut bytes);
            encoder.set_repeat(Repeat::Infinite).unwrap();
            for (color, millis) in [([255, 0, 0, 255], 50), ([0, 0, 255, 255], 10)] {
                let frame = RgbaImage::from_pixel(2, 1, Rgba(color));
                encoder
                    .encode_frame(Frame::from_parts(frame, 0, 0, Delay::from_numer_denom_ms(millis, 1)))
                    .unwrap();
            }
        }
        bytes
    }

    #[test]
    fn decodes_an_animation_and_finds_the_frame_for_a_moment() {
        let picture = decode(&two_frame_gif()).unwrap();
        assert!(picture.is_animated());
        assert_eq!((picture.width, picture.height), (2, 1));
        assert_eq!(
            picture.ends,
            vec![50, 150],
            "a 10 ms frame lasts 100 ms, as in browsers"
        );
        assert_eq!(picture.frame_at(0).get_pixel(0, 0)[0], 255);
        assert_eq!(picture.frame_at(49).get_pixel(0, 0)[0], 255);
        assert_eq!(picture.frame_at(50).get_pixel(0, 0)[2], 255);
        assert_eq!(picture.frame_at(150).get_pixel(0, 0)[0], 255, "loops");
    }

    #[test]
    fn reads_still_images_premultiplied_and_scales_them() {
        let mut png = Vec::new();
        let half_red = RgbaImage::from_pixel(4, 2, image::Rgba([255, 0, 0, 128]));
        image::DynamicImage::ImageRgba8(half_red)
            .write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let picture = decode(&png).unwrap();
        assert!(!picture.is_animated());
        assert_eq!(picture.frames[0].get_pixel(0, 0).0, [128, 0, 0, 128]);
        let scaled = picture.scaled(4);
        assert_eq!((scaled.width, scaled.height), (8, 4), "keeps its shape");
        assert!(decode(b"not an image").is_none());
    }

    #[test]
    fn the_cache_names_a_file_after_its_address() {
        let dir = Path::new("cache");
        let a = cached_path(dir, "https://cdn.7tv.app/emote/1/2x.webp");
        assert_eq!(a, cached_path(dir, "https://cdn.7tv.app/emote/1/2x.webp"));
        assert_ne!(a, cached_path(dir, "https://cdn.7tv.app/emote/2/2x.webp"));
        assert_eq!(a.extension().unwrap(), "img");
    }
}
