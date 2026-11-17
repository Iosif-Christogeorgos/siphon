//! Small caches shared between commands and download jobs.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::binaries::BinaryManager;
use crate::ffmpeg::{self, encoders};
use crate::plan::{Capabilities, WindowsCodecs};
use crate::process;
use crate::ytdlp::metadata::VideoInfo;

/// Enough for a long session and a few playlists' worth of size estimates.
const MAX_CACHED_VIDEOS: usize = 512;

/// Videos looked up this session, by URL. Planning and downloading read the
/// formats from here, so the backend stays the single source of truth and
/// the frontend never has to send them back.
#[derive(Default)]
pub struct MediaCache(Mutex<Cached>);

#[derive(Default)]
pub struct Cached {
    videos: HashMap<String, Arc<VideoInfo>>,
    /// Keys in the order they were added, so the oldest go first when full.
    order: VecDeque<String>,
}

impl MediaCache {
    pub fn get(&self, url: &str) -> Option<Arc<VideoInfo>> {
        self.0.lock().unwrap().videos.get(url).cloned()
    }

    /// Stores a video under its canonical URL and the URL the user pasted.
    pub fn insert(&self, requested_url: &str, video: Arc<VideoInfo>) {
        let mut cache = self.0.lock().unwrap();
        for key in [requested_url.to_string(), video.url.clone()] {
            if cache.videos.insert(key.clone(), Arc::clone(&video)).is_none() {
                cache.order.push_back(key);
            }
        }
        while cache.order.len() > MAX_CACHED_VIDEOS {
            if let Some(oldest) = cache.order.pop_front() {
                cache.videos.remove(&oldest);
            }
        }
    }
}

/// What this machine can do, worked out once on first use: the hardware
/// encoders that really work, whether ffmpeg can fetch in set request sizes,
/// and which of Microsoft's video extensions are installed.
#[derive(Default)]
pub struct CapabilityCache(tokio::sync::OnceCell<Capabilities>);

impl CapabilityCache {
    pub async fn get(&self, bins: &BinaryManager) -> Capabilities {
        *self
            .0
            .get_or_init(|| async {
                let _in_use = bins.in_use().await;
                let ffmpeg = bins.ffmpeg();
                let (encoders, chunked_requests, windows) = tokio::join!(
                    encoders::detect(&ffmpeg),
                    ffmpeg::supports_request_size(&ffmpeg),
                    windows_codecs(),
                );
                Capabilities {
                    encoders,
                    chunked_requests,
                    windows,
                }
            })
            .await
    }
}

/// Which of Microsoft's optional video extensions this PC has, so the
/// Windows preset copies VP9, AV1 or HEVC only when Media Player can play it.
async fn windows_codecs() -> WindowsCodecs {
    let mut cmd = process::command("powershell");
    cmd.args([
        "-NoProfile",
        "-NonInteractive",
        "-Command",
        "Get-AppxPackage -Name 'Microsoft.*VideoExtension*' | ForEach-Object Name",
    ]);
    let names = match process::output_within(cmd, Duration::from_secs(15)).await {
        Ok(Some(output)) if output.status.success() => String::from_utf8_lossy(&output.stdout).into_owned(),
        // Unknown means "not installed": the safe answer.
        _ => return WindowsCodecs::default(),
    };
    let has = |name: &str| names.lines().any(|line| line.trim() == name);
    WindowsCodecs {
        vp9: has("Microsoft.VP9VideoExtensions"),
        av1: has("Microsoft.AV1VideoExtension"),
        hevc: has("Microsoft.HEVCVideoExtension") || has("Microsoft.HEVCVideoExtensions"),
    }
}
