//! Where the trim preview plays from.
//!
//! Three levels, tried in order; the frontend asks for the next one when a
//! level fails to play:
//!
//! 0. YouTube's own embedded player (YouTube links only).
//! 1. A low-resolution stream straight from the site.
//! 2. A small low-resolution copy downloaded to the cache folder.
//!
//! A file on this PC plays as it is where WebView2 can play it, and from a
//! small copy made of it otherwise (see [`file_source`]).

use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Manager};

use crate::binaries::BinaryManager;
use crate::error::{AppError, ErrorKind, Result};
use crate::local::Grants;
use crate::plan::codecs::{AudioCodec, VideoCodec};
use crate::process;
use crate::sessions::Sessions;
use crate::throttle::Throttle;
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
    if video.file.is_some() {
        return file_source(app, bins, video, level).await;
    }
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
    // A stream as it airs is never downloaded for a preview: it doesn't end,
    // so yt-dlp would record it until the time limit.
    if level <= LAST_LEVEL && video.live.is_none() {
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

/// A file on this PC: itself, where WebView2 plays it (level 1), or a small
/// copy made of it (level 2): Matroska, HEVC and an OBS recording's MPEG-TS
/// it doesn't. The file is let through the asset protocol on its own; the
/// protocol's scope is otherwise only Siphon's own folders.
async fn file_source(app: &AppHandle, bins: &BinaryManager, video: &VideoInfo, level: u8) -> Result<Preview> {
    let original = app
        .state::<Grants>()
        .get(&video.url)
        .ok_or_else(|| AppError::internal("Siphon lost track of this file. Drop it on the window again."))?;
    if level <= 1 && plays_as_it_is(video) {
        app.asset_protocol_scope()
            .allow_file(original.path())
            .map_err(|err| AppError::internal("Couldn't show the file.").with_details(err.to_string()))?;
        return Ok(Preview {
            source: PreviewSource::File {
                path: original.path().to_string_lossy().into_owned(),
            },
            level: 1,
        });
    }
    if level <= LAST_LEVEL {
        let copy = copy_of_file(app, bins, video, original.path()).await?;
        return Ok(Preview {
            source: PreviewSource::File {
                path: copy.to_string_lossy().into_owned(),
            },
            level: LAST_LEVEL,
        });
    }
    Err(AppError::new(
        ErrorKind::Unavailable,
        "A preview isn’t available for this file.",
    ))
}

/// Whether WebView2 plays a file as it is: MP4, MOV or WebM with H.264, VP9,
/// VP8 or AV1, or a sound file of a common kind.
fn plays_as_it_is(video: &VideoInfo) -> bool {
    let Some(format) = video.formats.first() else {
        return false;
    };
    let picture = match format.video_codec {
        None => true,
        Some(codec) => matches!(
            codec,
            VideoCodec::H264 | VideoCodec::Vp9 | VideoCodec::Vp8 | VideoCodec::Av1
        ),
    };
    let sound = format.audio_codec.is_none_or(|codec| {
        matches!(
            codec,
            AudioCodec::Aac | AudioCodec::Mp3 | AudioCodec::Opus | AudioCodec::Vorbis | AudioCodec::Flac
        )
    });
    let container = matches!(
        format.ext.as_str(),
        "mp4" | "m4v" | "mov" | "webm" | "m4a" | "mp3" | "ogg" | "oga" | "opus" | "flac"
    );
    picture && sound && container
}

/// A small copy of a file on this PC for previewing: its picture at 360p
/// (the short side) and its sound, made here, which takes moments rather
/// than a download's minutes. Its timestamps start at zero where the file
/// starts, as a part to cut is counted (see `downloads::files`).
async fn copy_of_file(app: &AppHandle, bins: &BinaryManager, video: &VideoInfo, file: &Path) -> Result<PathBuf> {
    let dir = cache_dir(app)?;
    tokio::fs::create_dir_all(&dir).await?;
    let version = video
        .file
        .as_ref()
        .map(|file| file.version.as_str())
        .unwrap_or_default();
    let target = dir.join(format!("{}-{version}.mp4", video.id));
    if target.is_file() {
        return Ok(target);
    }
    let partial = dir.join(format!("{}-{version}.making.mp4", video.id));
    // Its main sound, as a conversion of it takes (see `plan::tracks`).
    let main = video.file.as_ref().map_or(0, |file| file.tracks.main);
    let mut cmd = process::command(bins.ffmpeg());
    process::below_others(&mut cmd);
    cmd.args(["-hide_banner", "-nostdin", "-y", "-loglevel", "error", "-i"])
        .arg(file)
        .args(["-map", "0:V:0?", "-map", &format!("0:a:{main}?")])
        .args([
            // The short side at 360, either way up.
            "-vf",
            "scale=w='if(gt(iw,ih),-2,360)':h='if(gt(iw,ih),360,-2)'",
            "-c:v",
            "libx264",
            "-preset",
            "veryfast",
            "-crf",
            "28",
            // A key frame every second: the timeline seeks quickly.
            "-g",
            "30",
            "-pix_fmt",
            "yuv420p",
            "-c:a",
            "aac",
            "-b:a",
            "96k",
            "-movflags",
            "+faststart",
        ])
        .arg(&partial);
    let _in_use = bins.in_use().await;
    let output = process::output_within(cmd, PROXY_TIMEOUT).await?.ok_or_else(|| {
        AppError::new(
            ErrorKind::Unavailable,
            "Making the preview took too long. You can still type the times.",
        )
    })?;
    if !output.status.success() || !partial.is_file() {
        let _ = tokio::fs::remove_file(&partial).await;
        return Err(
            AppError::new(ErrorKind::Unavailable, "A preview isn’t available for this file.")
                .with_details(String::from_utf8_lossy(&output.stderr).into_owned()),
        );
    }
    tokio::fs::rename(&partial, &target).await?;
    Ok(target)
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
        .arg(&dir);
    // A download like any other, so it keeps to the speed limit too.
    let flow = app.state::<std::sync::Arc<Throttle>>().flow().await?;
    flow.route(&mut cmd);
    // A video that needed signing in needs it for the preview too; one that didn't goes without.
    let cookies = if video.with_account {
        app.state::<std::sync::Arc<Sessions>>().cookie_file(&video.url)
    } else {
        None
    };
    if let Some(cookies) = &cookies {
        cmd.arg("--cookies").arg(cookies.path());
    }
    cmd.args(["--output", &format!("{name}.%(ext)s"), "--"]).arg(&video.url);

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
