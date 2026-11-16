//! Small caches shared between commands and download jobs.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::binaries::BinaryManager;
use crate::ffmpeg::{
    self,
    encoders::{self, Encoders},
};
use crate::ytdlp::metadata::VideoInfo;

/// Enough for a long session; the cache is simply emptied when it fills up.
const MAX_CACHED_VIDEOS: usize = 64;

/// Videos looked up this session, by URL. Planning and downloading read the
/// formats from here, so the backend stays the single source of truth and
/// the frontend never has to send them back.
#[derive(Default)]
pub struct MediaCache(Mutex<HashMap<String, Arc<VideoInfo>>>);

impl MediaCache {
    pub fn get(&self, url: &str) -> Option<Arc<VideoInfo>> {
        self.0.lock().unwrap().get(url).cloned()
    }

    /// Drops a cached lookup, so the next use fetches it afresh.
    pub fn forget(&self, url: &str) {
        self.0.lock().unwrap().remove(url);
    }

    /// Stores a video under its canonical URL and the URL the user pasted.
    pub fn insert(&self, requested_url: &str, video: Arc<VideoInfo>) {
        let mut cache = self.0.lock().unwrap();
        if cache.len() >= MAX_CACHED_VIDEOS {
            cache.clear();
        }
        cache.insert(requested_url.to_string(), Arc::clone(&video));
        cache.insert(video.url.clone(), video);
    }
}

/// Result of the hardware-encoder probe, run once on first use.
#[derive(Default)]
pub struct EncoderCache(tokio::sync::OnceCell<Encoders>);

impl EncoderCache {
    pub async fn get(&self, bins: &BinaryManager) -> Encoders {
        *self
            .0
            .get_or_init(|| async {
                match bins.ffmpeg() {
                    Some(ffmpeg) => encoders::detect(&ffmpeg).await,
                    None => Encoders::SOFTWARE,
                }
            })
            .await
    }
}

/// Whether the installed ffmpeg can fetch a stream in requests of a set size,
/// checked once on first use (see `ffmpeg::supports_request_size`).
#[derive(Default)]
pub struct ChunkedRequests(tokio::sync::OnceCell<bool>);

impl ChunkedRequests {
    pub async fn supported(&self, bins: &BinaryManager) -> bool {
        *self
            .0
            .get_or_init(|| async {
                let _in_use = bins.in_use().await;
                match bins.ffmpeg() {
                    Some(ffmpeg) => ffmpeg::supports_request_size(&ffmpeg).await,
                    None => false,
                }
            })
            .await
    }
}
