//! `yt-dlp -J`: what a link points to, and which formats it offers.

use std::time::Duration;

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

use super::{command, errors, validate_url};
use crate::binaries::BinaryManager;
use crate::error::{AppError, ErrorKind, Result};
use crate::plan::codecs::{AudioCodec, VideoCodec};
use crate::process;

const FETCH_TIMEOUT: Duration = Duration::from_secs(90);

// ── What the frontend receives ──────────────────────────────────────────

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Media {
    Video(Box<VideoInfo>),
    Playlist(PlaylistInfo),
}

#[derive(Debug, Clone, Serialize)]
pub struct VideoInfo {
    pub id: String,
    pub title: String,
    /// Canonical page URL; this is what gets downloaded.
    pub url: String,
    pub thumbnail: Option<String>,
    /// Seconds.
    pub duration: Option<f64>,
    pub channel: Option<String>,
    pub view_count: Option<u64>,
    /// yt-dlp's name for the site ("Youtube", "Vimeo", …).
    #[serde(skip)]
    pub extractor: Option<String>,
    pub formats: Vec<Format>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FormatKind {
    /// Video without audio.
    Video,
    /// Audio without video.
    Audio,
    /// Video and audio in one stream.
    Muxed,
}

#[derive(Debug, Clone, Serialize)]
pub struct Format {
    pub id: String,
    pub ext: String,
    pub kind: FormatKind,
    /// Raw codec strings as reported by the site, e.g. "avc1.640028", "opus".
    #[serde(skip)]
    pub vcodec: Option<String>,
    #[serde(skip)]
    pub acodec: Option<String>,
    /// The codecs as Siphon classifies them; `None` when the stream has no such track.
    pub video_codec: Option<VideoCodec>,
    pub audio_codec: Option<AudioCodec>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub fps: Option<f64>,
    /// Bitrates in kbit/s.
    pub tbr: Option<f64>,
    pub vbr: Option<f64>,
    pub abr: Option<f64>,
    /// Audio sample rate in Hz.
    pub asr: Option<u32>,
    pub audio_channels: Option<u32>,
    /// Bytes. See `filesize_estimated`.
    pub filesize: Option<u64>,
    /// True when the site didn't state the size and yt-dlp estimated it.
    pub filesize_estimated: bool,
    /// "SDR", "HDR10", "HLG", …
    pub dynamic_range: Option<String>,
    pub language: Option<String>,
    /// yt-dlp's ranking of audio tracks: the original language scores highest.
    pub language_preference: Option<i32>,
    /// True for segmented streams (HLS). Sites often list the same video both
    /// ways; the plain file downloads faster and reports its size.
    pub segmented: bool,
    /// Direct media URL, used only for the trim preview.
    #[serde(skip)]
    pub stream_url: Option<String>,
    pub note: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct PlaylistInfo {
    pub id: String,
    pub title: String,
    pub url: String,
    pub channel: Option<String>,
    pub entries: Vec<PlaylistEntry>,
}

#[derive(Debug, Serialize)]
pub struct PlaylistEntry {
    pub id: String,
    pub title: String,
    pub url: String,
    pub duration: Option<f64>,
    pub thumbnail: Option<String>,
    pub channel: Option<String>,
}

// ── What yt-dlp emits ───────────────────────────────────────────────────
//
// Extractors are inconsistent: any field may be missing, null, or a number
// where a string is expected. Everything is optional and parsed leniently so
// one odd field never fails the whole lookup.

#[derive(Debug, Deserialize)]
struct RawInfo {
    #[serde(rename = "_type")]
    kind: Option<String>,
    #[serde(default, deserialize_with = "text")]
    id: Option<String>,
    #[serde(default, deserialize_with = "text")]
    title: Option<String>,
    #[serde(default, deserialize_with = "text")]
    webpage_url: Option<String>,
    /// Flat playlist entries carry their page link here instead.
    #[serde(default, deserialize_with = "text")]
    url: Option<String>,
    #[serde(default, deserialize_with = "text")]
    thumbnail: Option<String>,
    #[serde(default)]
    thumbnails: Option<Vec<RawThumbnail>>,
    #[serde(default, deserialize_with = "number")]
    duration: Option<f64>,
    #[serde(default, deserialize_with = "text")]
    channel: Option<String>,
    #[serde(default, deserialize_with = "text")]
    uploader: Option<String>,
    #[serde(default, deserialize_with = "number")]
    view_count: Option<f64>,
    #[serde(default, deserialize_with = "text")]
    extractor_key: Option<String>,
    #[serde(default, deserialize_with = "text")]
    live_status: Option<String>,
    #[serde(default)]
    formats: Option<Vec<RawFormat>>,
    /// Unavailable playlist items come through as `null`.
    #[serde(default)]
    entries: Option<Vec<Option<RawInfo>>>,
}

#[derive(Debug, Deserialize)]
struct RawThumbnail {
    #[serde(default, deserialize_with = "text")]
    url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawFormat {
    #[serde(default, deserialize_with = "text")]
    format_id: Option<String>,
    #[serde(default, deserialize_with = "text")]
    ext: Option<String>,
    #[serde(default, deserialize_with = "text")]
    vcodec: Option<String>,
    #[serde(default, deserialize_with = "text")]
    acodec: Option<String>,
    #[serde(default, deserialize_with = "number")]
    width: Option<f64>,
    #[serde(default, deserialize_with = "number")]
    height: Option<f64>,
    #[serde(default, deserialize_with = "number")]
    fps: Option<f64>,
    #[serde(default, deserialize_with = "number")]
    tbr: Option<f64>,
    #[serde(default, deserialize_with = "number")]
    vbr: Option<f64>,
    #[serde(default, deserialize_with = "number")]
    abr: Option<f64>,
    #[serde(default, deserialize_with = "number")]
    asr: Option<f64>,
    #[serde(default, deserialize_with = "number")]
    audio_channels: Option<f64>,
    #[serde(default, deserialize_with = "number")]
    filesize: Option<f64>,
    #[serde(default, deserialize_with = "number")]
    filesize_approx: Option<f64>,
    #[serde(default, deserialize_with = "text")]
    dynamic_range: Option<String>,
    #[serde(default, deserialize_with = "text")]
    language: Option<String>,
    #[serde(default, deserialize_with = "number")]
    language_preference: Option<f64>,
    #[serde(default, deserialize_with = "text")]
    format_note: Option<String>,
    #[serde(default, deserialize_with = "text")]
    protocol: Option<String>,
    #[serde(default, deserialize_with = "text")]
    url: Option<String>,
    /// "none" when the stream has no video / audio, even if the codec is unknown.
    #[serde(default, deserialize_with = "text")]
    video_ext: Option<String>,
    #[serde(default, deserialize_with = "text")]
    audio_ext: Option<String>,
    #[serde(default)]
    has_drm: Option<Value>,
}

/// A number, or a string holding one; anything else becomes `None`.
fn number<'de, D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Option<f64>, D::Error> {
    Ok(match Value::deserialize(deserializer)? {
        Value::Number(n) => n.as_f64().filter(|n| n.is_finite()),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    })
}

/// A non-empty string, or a number rendered as one; anything else becomes `None`.
fn text<'de, D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Option<String>, D::Error> {
    Ok(match Value::deserialize(deserializer)? {
        Value::String(s) if !s.trim().is_empty() => Some(s),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    })
}

// ── Fetching ────────────────────────────────────────────────────────────

/// Looks up a link. Returns a single video with its formats, or a playlist
/// with a lightweight entry list (no per-video requests).
pub async fn fetch(bins: &BinaryManager, url: &str) -> Result<Media> {
    let url = validate_url(url)?;

    let mut cmd = command(bins);
    cmd.args([
        "--dump-single-json",
        // List playlist items without resolving each one (fast).
        "--flat-playlist",
        // A watch link that also names a playlist means "this video".
        "--no-playlist",
        "--no-warnings",
        "--",
    ])
    .arg(url);

    let output = process::output_within(cmd, FETCH_TIMEOUT).await?.ok_or_else(|| {
        AppError::new(
            ErrorKind::Network,
            "The site took too long to respond. Try again in a moment.",
        )
    })?;
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !output.status.success() {
        return Err(errors::classify(&stderr));
    }

    let raw: RawInfo =
        serde_json::from_slice(&output.stdout).map_err(|err| AppError::from(err).with_details(stderr.to_string()))?;

    if raw.kind.as_deref() == Some("playlist") {
        Ok(Media::Playlist(playlist(raw, url)))
    } else {
        video(raw, url).map(|info| Media::Video(Box::new(info)))
    }
}

fn video(raw: RawInfo, requested_url: &str) -> Result<VideoInfo> {
    if matches!(raw.live_status.as_deref(), Some("is_live" | "is_upcoming")) {
        return Err(AppError::new(
            ErrorKind::Unavailable,
            "This is a live stream. Come back once it has ended and Siphon can download the recording.",
        ));
    }

    let raw_formats = raw.formats.unwrap_or_default();
    let is_drm = |format: &RawFormat| matches!(&format.has_drm, Some(Value::Bool(true)));
    if !raw_formats.is_empty() && raw_formats.iter().all(is_drm) {
        return Err(AppError::new(
            ErrorKind::Drm,
            "This video is protected by DRM, so it can't be downloaded. Siphon doesn't bypass copy protection.",
        ));
    }
    let formats: Vec<Format> = raw_formats
        .into_iter()
        .filter(|f| !is_drm(f))
        .filter_map(format)
        .collect();

    Ok(VideoInfo {
        id: raw.id.unwrap_or_default(),
        title: raw.title.unwrap_or_else(|| "Untitled video".to_string()),
        url: raw.webpage_url.unwrap_or_else(|| requested_url.to_string()),
        thumbnail: raw.thumbnail.or_else(|| last_thumbnail(raw.thumbnails)),
        duration: raw.duration,
        channel: raw.channel.or(raw.uploader),
        view_count: raw.view_count.map(|n| n as u64),
        extractor: raw.extractor_key,
        formats,
    })
}

fn format(raw: RawFormat) -> Option<Format> {
    // yt-dlp writes the string "none" for "this stream has no video/audio".
    // When the codec is unknown, `video_ext` / `audio_ext` still say whether the track exists.
    let present = |codec: &Option<String>, ext: &Option<String>| match codec.as_deref() {
        Some(codec) => codec != "none",
        None => ext.as_deref() != Some("none"),
    };
    let has_video = present(&raw.vcodec, &raw.video_ext);
    let has_audio = present(&raw.acodec, &raw.audio_ext);
    let kind = match (has_video, has_audio) {
        (true, true) => FormatKind::Muxed,
        (true, false) => FormatKind::Video,
        (false, true) => FormatKind::Audio,
        // Storyboards and other non-media entries.
        (false, false) => return None,
    };
    let ext = raw.ext?;
    if ext == "mhtml" {
        return None;
    }

    let codec = |value: Option<String>| value.filter(|codec| codec != "none");
    let whole = |value: Option<f64>| value.filter(|n| *n > 0.0).map(|n| n.round() as u32);
    let rate = |value: Option<f64>| value.filter(|n| *n > 0.0);

    let mut format = Format {
        id: raw.format_id?,
        ext,
        kind,
        vcodec: codec(raw.vcodec),
        acodec: codec(raw.acodec),
        width: whole(raw.width),
        height: whole(raw.height),
        fps: rate(raw.fps),
        tbr: rate(raw.tbr),
        vbr: rate(raw.vbr),
        abr: rate(raw.abr),
        asr: whole(raw.asr),
        audio_channels: whole(raw.audio_channels),
        filesize: rate(raw.filesize).or(rate(raw.filesize_approx)).map(|n| n as u64),
        filesize_estimated: rate(raw.filesize).is_none(),
        dynamic_range: raw.dynamic_range,
        language: raw.language,
        language_preference: raw.language_preference.map(|n| n as i32),
        segmented: raw
            .protocol
            .as_deref()
            .is_some_and(|protocol| protocol.starts_with("m3u8")),
        note: raw.format_note,
        stream_url: raw.url.filter(|url| url.starts_with("http")),
        video_codec: None,
        audio_codec: None,
    };
    format.video_codec = (kind != FormatKind::Audio).then(|| VideoCodec::of(&format));
    format.audio_codec = (kind != FormatKind::Video).then(|| AudioCodec::of(&format));
    Some(format)
}

fn playlist(raw: RawInfo, requested_url: &str) -> PlaylistInfo {
    let entries = raw
        .entries
        .unwrap_or_default()
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            Some(PlaylistEntry {
                url: entry.url.or(entry.webpage_url)?,
                id: entry.id.unwrap_or_default(),
                title: entry.title.unwrap_or_else(|| "Untitled video".to_string()),
                duration: entry.duration,
                thumbnail: entry.thumbnail.or_else(|| last_thumbnail(entry.thumbnails)),
                channel: entry.channel.or(entry.uploader),
            })
        })
        .collect();

    PlaylistInfo {
        id: raw.id.unwrap_or_default(),
        title: raw.title.unwrap_or_else(|| "Untitled playlist".to_string()),
        url: raw.webpage_url.unwrap_or_else(|| requested_url.to_string()),
        channel: raw.channel.or(raw.uploader),
        entries,
    }
}

/// yt-dlp orders thumbnails worst → best.
fn last_thumbnail(thumbnails: Option<Vec<RawThumbnail>>) -> Option<String> {
    thumbnails?.into_iter().rev().find_map(|thumbnail| thumbnail.url)
}
