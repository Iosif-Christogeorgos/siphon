//! Where the trim preview plays from.
//!
//! Three levels, tried in order; the frontend asks for the next one when a
//! level fails to play:
//!
//! 0. YouTube's own embedded player (YouTube links only).
//! 1. A low-resolution stream straight from the site.
//! 2. A small low-resolution copy downloaded to the cache folder.

use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::PathBuf;
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Manager};

use crate::binaries::BinaryManager;
use crate::error::{AppError, ErrorKind, Result};
use crate::plan::codecs::VideoCodec;
use crate::process;
use crate::ytdlp::metadata::{Format, FormatKind, VideoInfo};
use crate::ytdlp::{self, errors};

/// The preview only has to be watchable, so aim low: it loads faster.
const PREVIEW_LINES: u32 = 360;
const PROXY_TIMEOUT: Duration = Duration::from_secs(15 * 60);
/// Above this the copy is not worth the wait (two hours of 360p is about 400 MB).
const PROXY_MAX_SIZE: &str = "500M";

pub const LAST_LEVEL: u8 = 2;

#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PreviewSource {
    /// Play through YouTube's IFrame player.
    Youtube { id: String },
    /// Play this URL in a `<video>` element.
    Stream { url: String },
    /// Play this local file (served through the asset protocol).
    File { path: String },
}

#[derive(Debug, Serialize)]
pub struct Preview {
    pub source: PreviewSource,
    /// The level this source came from; ask for `level + 1` if it won't play.
    pub level: u8,
}

/// Finds a preview source at `level` or the next level that has one.
pub async fn source(app: &AppHandle, bins: &BinaryManager, video: &VideoInfo, level: u8) -> Result<Preview> {
    if level == 0 && video.extractor.as_deref() == Some("Youtube") && !video.id.is_empty() {
        return Ok(Preview {
            source: PreviewSource::Youtube { id: video.id.clone() },
            level: 0,
        });
    }
    if level <= 1 {
        if let Some(url) = stream_url(video) {
            return Ok(Preview {
                source: PreviewSource::Stream { url },
                level: 1,
            });
        }
    }
    if level <= LAST_LEVEL {
        let path = download_proxy(app, bins, video).await?;
        return Ok(Preview {
            source: PreviewSource::File {
                path: path.to_string_lossy().into_owned(),
            },
            level: LAST_LEVEL,
        });
    }
    Err(AppError::new(
        ErrorKind::Unavailable,
        "A preview isn’t available for this video.",
    ))
}

/// A direct stream the webview can play, with picture and, ideally, sound,
/// as close to 360p as the site offers. A plain file is preferred, but an HLS
/// stream will do: WebView2 and Safari play those natively. That matters for
/// sites like Twitch, which offer nothing else, and whose videos can be hours
/// long, far too much to download as a preview copy.
fn stream_url(video: &VideoInfo) -> Option<String> {
    let playable = |format: &&Format| {
        format.stream_url.is_some()
            && format.kind != FormatKind::Audio
            && matches!(
                VideoCodec::of(format),
                VideoCodec::H264 | VideoCodec::Vp9 | VideoCodec::Vp8 | VideoCodec::Av1
            )
    };
    video
        .formats
        .iter()
        .filter(playable)
        .max_by_key(|format| {
            let lines = match (format.width, format.height) {
                (Some(w), Some(h)) => w.min(h),
                (_, Some(h)) => h,
                _ => 0,
            };
            // Sound first, then a plain file, then not larger than needed,
            // then closest to the target.
            (
                format.kind == FormatKind::Muxed,
                !format.segmented,
                lines <= PREVIEW_LINES * 4 / 3,
                std::cmp::Reverse(lines.abs_diff(PREVIEW_LINES)),
            )
        })
        .and_then(|format| format.stream_url.clone())
}

fn cache_dir(app: &AppHandle) -> Result<PathBuf> {
    app.path()
        .app_cache_dir()
        .map(|dir| dir.join("preview"))
        .map_err(|err| AppError::internal("Couldn't locate the cache folder.").with_details(err.to_string()))
}

/// Previews are throwaway: start each session with an empty folder.
pub fn clear_cache(app: &AppHandle) {
    if let Ok(dir) = cache_dir(app) {
        let _ = std::fs::remove_dir_all(dir);
    }
}

/// Downloads a ~360p copy of the whole video for previewing.
async fn download_proxy(app: &AppHandle, bins: &BinaryManager, video: &VideoInfo) -> Result<PathBuf> {
    let dir = cache_dir(app)?;
    tokio::fs::create_dir_all(&dir).await?;

    let mut hasher = DefaultHasher::new();
    video.url.hash(&mut hasher);
    let name = format!("{:016x}", hasher.finish());
    let target = dir.join(format!("{name}.mp4"));
    if target.is_file() {
        return Ok(target);
    }

    let mut cmd = ytdlp::command(bins);
    cmd.args(ytdlp::args::TRANSFER_ARGS)
        .args([
            "--no-playlist",
            "--no-warnings",
            // A preview only has to be watchable: the smallest picture near
            // 360p and the smallest sound, never the full-quality file. Where
            // nothing that small exists, the smallest the site has.
            "--format-sort",
            "res:360,+size,+br,vcodec:h264,acodec:aac",
            "--format",
            "bv*[height<=480]+wa/b[height<=480]/wv*+wa/w",
            // And no preview at all rather than gigabytes of one.
            "--max-filesize",
            PROXY_MAX_SIZE,
            "--merge-output-format",
            "mp4",
            "--remux-video",
            "mp4",
        ])
        .arg("--paths")
        .arg(&dir)
        .args(["--output", &format!("{name}.%(ext)s"), "--"])
        .arg(&video.url);

    let _in_use = bins.in_use().await;
    let output = process::output_within(cmd, PROXY_TIMEOUT).await?.ok_or_else(|| {
        AppError::new(
            ErrorKind::Network,
            "Preparing the preview took too long. You can still type the times.",
        )
    })?;
    if !output.status.success() {
        return Err(errors::classify(&String::from_utf8_lossy(&output.stderr)));
    }
    if target.is_file() {
        Ok(target)
    } else {
        Err(AppError::new(
            ErrorKind::Unavailable,
            "A preview isn’t available for this video.",
        ))
    }
}
