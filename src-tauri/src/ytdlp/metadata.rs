//! `yt-dlp -J`: what a link points to, and which formats it offers.

use std::collections::HashSet;
use std::ffi::OsString;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

use super::saved::SavedLookup;
use super::subtitles::{self, SubtitleTrack};
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
    /// Looked up with the site's sign-in, because it was needed (or the user
    /// asked). The download then uses it too: some sites tie their stream
    /// links to the session that asked for them.
    pub with_account: bool,
    /// A stream that is live now, scheduled, or a channel that is offline:
    /// recorded rather than downloaded (see `downloads::live`).
    pub live: Option<Live>,
    /// The subtitle tracks it offers, people's first (see `ytdlp::subtitles`).
    pub subtitles: Vec<SubtitleTrack>,
    /// What the lookup printed, for the download to start from (see `ytdlp::saved`).
    #[serde(skip)]
    pub saved: Option<Arc<SavedLookup>>,
}

/// Where a live stream stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LiveStatus {
    /// On air now.
    Live,
    /// Scheduled: a YouTube premiere or a stream that hasn't started.
    Upcoming,
    /// A channel link whose channel isn't streaming now.
    Offline,
}

#[derive(Debug, Clone, Serialize)]
pub struct Live {
    pub status: LiveStatus,
    /// Unix seconds: when it went on air (live), or is to (upcoming), if the site says.
    pub since: Option<u64>,
    /// How many are watching, if the site says.
    pub viewers: Option<u64>,
    /// The formats to record it from the start with, when the site keeps what
    /// already aired (`--live-from-start`). Asked for separately, because it
    /// takes a second lookup (see `fetch_from_start`); `None` until then.
    #[serde(skip)]
    pub from_start: Option<Vec<Format>>,
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
    /// Bytes per request the site expects. YouTube slows a stream to about
    /// playback speed when more is asked for at once.
    #[serde(skip)]
    pub http_chunk_size: Option<u64>,
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
    /// A link to another playlist (a channel's Playlists tab), opened rather than downloaded.
    pub is_playlist: bool,
    /// The part of a channel it was listed under ("Videos", "Live", "Shorts"); none for a plain playlist.
    pub section: Option<String>,
    /// A stream that is live now or scheduled: opened to record, rather than queued.
    pub live: Option<LiveStatus>,
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
    /// For a flat entry: the extractor its link belongs to ("Youtube" for a video, "YoutubeTab" for a playlist).
    #[serde(default, deserialize_with = "text")]
    ie_key: Option<String>,
    #[serde(default, deserialize_with = "text")]
    live_status: Option<String>,
    /// For a stream: when it went live, or is scheduled to.
    #[serde(default, deserialize_with = "number")]
    release_timestamp: Option<f64>,
    #[serde(default, deserialize_with = "number")]
    timestamp: Option<f64>,
    #[serde(default, deserialize_with = "number")]
    concurrent_view_count: Option<f64>,
    #[serde(default)]
    formats: Option<Vec<RawFormat>>,
    /// Subtitles by language, as `{"en": [{"ext": "vtt", "url": …}, …]}`; read in `ytdlp::subtitles`.
    #[serde(default)]
    subtitles: Option<Value>,
    #[serde(default)]
    automatic_captions: Option<Value>,
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
    /// How the site wants the stream fetched, e.g. `{"http_chunk_size": 10485760}`.
    #[serde(default)]
    downloader_options: Option<Value>,
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
/// with a lightweight entry list (no per-video requests). `cookies` is the
/// site's sign-in, if any (see `crate::sessions`).
pub async fn fetch(bins: &BinaryManager, url: &str, cookies: Option<&Path>) -> Result<Media> {
    let mut cookie_args: Vec<OsString> = Vec::new();
    if let Some(cookies) = cookies {
        cookie_args.extend(["--cookies".into(), cookies.into()]);
    }
    fetch_with(bins, url, &cookie_args).await
}

/// [`fetch`], with yt-dlp told where to find cookies some other way (a browser, for a sign-in).
///
/// A scheduled stream has no formats yet, and yt-dlp fails on it ("This live
/// event will begin in 27 hours"); it is looked up again, told not to mind,
/// for its title and start time. A channel that isn't streaming fails too
/// ("The channel is not currently live"), and is answered as an offline
/// channel: both can be recorded once they go live.
pub async fn fetch_with(bins: &BinaryManager, url: &str, cookie_args: &[OsString]) -> Result<Media> {
    let url = validate_url(url)?;
    match run_json(bins, url, cookie_args, &[]).await {
        Err(error) if errors::is_scheduled(error.details.as_deref().unwrap_or_default()) => {
            run_json(bins, url, cookie_args, &["--ignore-no-formats-error"]).await
        }
        Err(error) if errors::is_offline(error.details.as_deref().unwrap_or_default()) => {
            Ok(Media::Video(Box::new(offline(url))))
        }
        result => result,
    }
}

/// The formats to record a live stream from its start with, when the site
/// keeps the part that already aired. Its own lookup, because yt-dlp lists
/// other formats for it (YouTube: DASH instead of HLS), and only on request.
pub async fn fetch_from_start(bins: &BinaryManager, url: &str, cookies: Option<&Path>) -> Result<Vec<Format>> {
    let url = validate_url(url)?;
    let mut cookie_args: Vec<OsString> = Vec::new();
    if let Some(cookies) = cookies {
        cookie_args.extend(["--cookies".into(), cookies.into()]);
    }
    match run_json(bins, url, &cookie_args, &["--live-from-start"]).await? {
        Media::Video(video) if video.live.as_ref().is_some_and(|live| live.status == LiveStatus::Live) => {
            Ok(video.formats)
        }
        _ => Ok(Vec::new()),
    }
}

/// One `yt-dlp -J` run, read into a video or a playlist. On failure, the
/// error's details hold yt-dlp's messages.
async fn run_json(bins: &BinaryManager, url: &str, cookie_args: &[OsString], extra: &[&str]) -> Result<Media> {
    let mut cmd = command(bins);
    cmd.args(cookie_args);
    cmd.args(extra);
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

    let _in_use = bins.in_use().await;
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

    let raw = parse(&output.stdout).map_err(|err| AppError::from(err).with_details(stderr.to_string()))?;

    if raw.kind.as_deref() == Some("playlist") {
        Ok(Media::Playlist(playlist(raw, url)))
    } else {
        video(raw, url).map(|mut info| {
            info.saved = Some(Arc::new(SavedLookup::new(output.stdout)));
            Media::Video(Box::new(info))
        })
    }
}

/// A channel link whose channel isn't streaming: named after the channel,
/// which the link is all there is to go on for (`twitch.tv/name`,
/// `youtube.com/@name/live`, `kick.com/name`).
fn offline(url: &str) -> VideoInfo {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    let name = path
        .split('/')
        .skip(3)
        .filter(|part| !part.is_empty() && *part != "live" && *part != "streams")
        .last()
        .map(|part| part.trim_start_matches('@').to_string())
        .filter(|name| !name.is_empty());
    VideoInfo {
        id: name.clone().unwrap_or_default(),
        title: name.clone().unwrap_or_else(|| "Live channel".to_string()),
        url: url.to_string(),
        thumbnail: None,
        duration: None,
        channel: name,
        view_count: None,
        extractor: None,
        formats: Vec::new(),
        with_account: false,
        live: Some(Live {
            status: LiveStatus::Offline,
            since: None,
            viewers: None,
            from_start: None,
        }),
        subtitles: Vec::new(),
        saved: None,
    }
}

/// Reads what `yt-dlp -J` printed. A page that offers a single file (an audio
/// host, many generic pages) gets no format list: yt-dlp puts that one format's
/// fields (`url`, `format_id`, `ext`, `audio_ext`, …) on the result itself.
/// That result is then read as its own only format, or Siphon would find
/// nothing to download in a page that plays.
fn parse(json: &[u8]) -> serde_json::Result<RawInfo> {
    let value: Value = serde_json::from_slice(json)?;
    let mut raw = RawInfo::deserialize(&value)?;
    let single = raw.formats.is_none() && raw.kind.as_deref() != Some("playlist") && raw.url.is_some();
    if single {
        raw.formats = Some(vec![RawFormat::deserialize(&value)?]);
    }
    Ok(raw)
}

fn video(raw: RawInfo, requested_url: &str) -> Result<VideoInfo> {
    let whole = |value: Option<f64>| value.filter(|n| *n > 0.0).map(|n| n as u64);
    let live = match raw.live_status.as_deref() {
        Some("is_live") => Some(Live {
            status: LiveStatus::Live,
            // YouTube's release time is when the stream went on air; Twitch only has `timestamp`.
            since: whole(raw.release_timestamp).or(whole(raw.timestamp)),
            viewers: whole(raw.concurrent_view_count),
            from_start: None,
        }),
        Some("is_upcoming") => Some(Live {
            status: LiveStatus::Upcoming,
            since: whole(raw.release_timestamp),
            viewers: None,
            from_start: None,
        }),
        _ => None,
    };

    let raw_formats = raw.formats.unwrap_or_default();
    let is_drm = |format: &RawFormat| matches!(&format.has_drm, Some(Value::Bool(true)));
    if !raw_formats.is_empty() && raw_formats.iter().all(is_drm) {
        return Err(AppError::new(
            ErrorKind::Drm,
            "This video is protected by DRM, so it can't be downloaded. Siphon doesn't bypass copy protection.",
        ));
    }
    let mut formats: Vec<Format> = raw_formats
        .into_iter()
        .filter(|f| !is_drm(f))
        .filter_map(format)
        .collect();
    for format in &mut formats {
        estimate_size(format, raw.duration);
    }

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
        with_account: false,
        live,
        subtitles: subtitles::read(raw.subtitles.as_ref(), raw.automatic_captions.as_ref()),
        saved: None,
    })
}

/// Some sites (Twitch, for one) state a stream's bitrate but not its size,
/// and yt-dlp doesn't estimate it. Without a size, a trimmed part shows no
/// progress and the size presets can't tell what fits, so work it out.
fn estimate_size(format: &mut Format, duration: Option<f64>) {
    if format.filesize.is_some() {
        return;
    }
    if let (Some(kbps), Some(seconds)) = (format.tbr, duration.filter(|d| *d > 0.0)) {
        format.filesize = Some((kbps * 1000.0 / 8.0 * seconds) as u64);
        format.filesize_estimated = true;
    }
}

fn format(raw: RawFormat) -> Option<Format> {
    let (has_video, has_audio) = tracks(&raw);
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
        http_chunk_size: raw
            .downloader_options
            .as_ref()
            .and_then(|options| options.get("http_chunk_size"))
            .and_then(Value::as_u64)
            .filter(|size| *size > 0),
        video_codec: None,
        audio_codec: None,
    };
    format.video_codec = (kind != FormatKind::Audio).then(|| VideoCodec::of(&format));
    format.audio_codec = (kind != FormatKind::Video).then(|| AudioCodec::of(&format));
    Some(format)
}

/// Whether a format carries video and audio.
///
/// yt-dlp writes the codec "none" for a missing track. When a site reports no
/// codecs at all, `video_ext` still tells a video file from an audio file,
/// but yt-dlp then fills `audio_ext` with "none" even though an ordinary
/// video file almost always has sound. So an unknown audio codec next to
/// video counts as audio being present.
fn tracks(raw: &RawFormat) -> (bool, bool) {
    let has_video = match raw.vcodec.as_deref() {
        Some(codec) => codec != "none",
        None => raw.video_ext.as_deref() != Some("none"),
    };
    let has_audio = match raw.acodec.as_deref() {
        Some(codec) => codec != "none",
        None => has_video || raw.audio_ext.as_deref() != Some("none"),
    };
    (has_video, has_audio)
}

fn playlist(raw: RawInfo, requested_url: &str) -> PlaylistInfo {
    let title = raw.title.unwrap_or_else(|| "Untitled playlist".to_string());
    let mut entries = Vec::new();
    let mut seen = HashSet::new();
    collect_entries(raw.entries.unwrap_or_default(), &title, None, &mut entries, &mut seen);

    PlaylistInfo {
        id: raw.id.unwrap_or_default(),
        url: raw.webpage_url.unwrap_or_else(|| requested_url.to_string()),
        channel: raw.channel.or(raw.uploader),
        title,
        entries,
    }
}

/// Flattens a playlist's entries. A YouTube channel lists its tabs (Videos,
/// Live, Shorts) as playlists with their videos inside; those videos are
/// listed under the tab's name. A link to another playlist that came without
/// its videos (a channel's Playlists tab) is kept as a link to open, and so is
/// a stream that is live or scheduled, to be recorded. A video listed twice is
/// kept once.
fn collect_entries(
    raw: Vec<Option<RawInfo>>,
    parent: &str,
    section: Option<&str>,
    entries: &mut Vec<PlaylistEntry>,
    seen: &mut HashSet<String>,
) {
    for entry in raw.into_iter().flatten() {
        if let Some(inner) = entry.entries {
            let name = entry.title.as_deref().map(|title| section_name(title, parent));
            collect_entries(inner, parent, name.as_deref().or(section), entries, seen);
            continue;
        }
        let live = match entry.live_status.as_deref() {
            Some("is_live") => Some(LiveStatus::Live),
            Some("is_upcoming") => Some(LiveStatus::Upcoming),
            _ => None,
        };
        let is_playlist = is_playlist_link(&entry);
        let Some(url) = entry.url.or(entry.webpage_url) else {
            continue;
        };
        if !seen.insert(url.clone()) {
            continue;
        }
        entries.push(PlaylistEntry {
            url,
            id: entry.id.unwrap_or_default(),
            title: entry.title.unwrap_or_else(|| {
                (if is_playlist {
                    "Untitled playlist"
                } else {
                    "Untitled video"
                })
                .to_string()
            }),
            duration: entry.duration,
            thumbnail: entry.thumbnail.or_else(|| last_thumbnail(entry.thumbnails)),
            channel: entry.channel.or(entry.uploader),
            is_playlist,
            section: section.map(str::to_string),
            live,
        });
    }
}

/// "Blender - Videos" under the channel "Blender" is the section "Videos".
fn section_name(title: &str, parent: &str) -> String {
    title
        .strip_prefix(parent)
        .and_then(|rest| rest.strip_prefix(" - "))
        .unwrap_or(title)
        .to_string()
}

/// Whether a flat entry links to a playlist or channel rather than a video.
fn is_playlist_link(entry: &RawInfo) -> bool {
    entry.kind.as_deref() == Some("playlist")
        || entry
            .ie_key
            .as_deref()
            .is_some_and(|key| ["Tab", "Playlist", "Channel"].iter().any(|end| key.ends_with(end)))
        || entry.url.as_deref().is_some_and(|url| url.contains("/playlist?list="))
}

/// yt-dlp orders thumbnails worst → best.
fn last_thumbnail(thumbnails: Option<Vec<RawThumbnail>>) -> Option<String> {
    thumbnails?.into_iter().rev().find_map(|thumbnail| thumbnail.url)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shape `yt-dlp -J` prints for a page with one audio file (seen
    /// 2026-11-18; the values here are made up): no format list.
    #[test]
    fn a_page_with_a_single_file_is_its_own_format() {
        let json = br#"{"id": "3f9a1c", "title": "A recording",
            "url": "https://media.example.com/sounds/3f9a1c.m4a",
            "ext": "m4a", "format_id": "0", "format": "0 - audio only", "protocol": "https",
            "vcodec": "none", "acodec": null, "audio_ext": "m4a", "video_ext": "none", "vbr": 0,
            "uploader": "someone", "extractor_key": "Generic",
            "webpage_url": "https://example.com/u/someone/a-recording"}"#;
        let info = video(parse(json).unwrap(), "https://example.com/u/someone/a-recording").unwrap();
        assert_eq!(info.formats.len(), 1);
        assert_eq!(info.formats[0].id, "0");
        assert_eq!(info.formats[0].kind, FormatKind::Audio);
        assert_eq!(info.formats[0].ext, "m4a");

        // A list, when there is one, is used as it is; a playlist's link is never read as a format.
        let listed = br#"{"id": "a", "url": "https://x/a.mp4", "formats": [{"format_id": "1", "ext": "mp4", "vcodec": "avc1", "acodec": "mp4a"}]}"#;
        assert_eq!(video(parse(listed).unwrap(), "https://x/a").unwrap().formats[0].id, "1");
        let playlist = br#"{"_type": "playlist", "id": "p", "url": "https://x/p", "entries": []}"#;
        assert!(parse(playlist).unwrap().formats.is_none());
    }

    fn kind_of(json: &str) -> Option<FormatKind> {
        format(serde_json::from_str(json).unwrap()).map(|format| format.kind)
    }

    #[test]
    fn classifies_formats_by_their_tracks() {
        let video = r#"{"format_id": "137", "ext": "mp4", "vcodec": "avc1.640028", "acodec": "none"}"#;
        let audio = r#"{"format_id": "251", "ext": "webm", "vcodec": "none", "acodec": "opus"}"#;
        let storyboard = r#"{"format_id": "sb0", "ext": "mhtml", "vcodec": "none", "acodec": "none"}"#;
        assert_eq!(kind_of(video), Some(FormatKind::Video));
        assert_eq!(kind_of(audio), Some(FormatKind::Audio));
        assert_eq!(kind_of(storyboard), None);
    }

    #[test]
    fn a_plain_video_file_with_unknown_codecs_is_assumed_to_have_sound() {
        // What archive.org and direct file links look like.
        let file = r#"{"format_id": "1", "ext": "mp4", "video_ext": "mp4", "audio_ext": "none", "height": 360}"#;
        assert_eq!(kind_of(file), Some(FormatKind::Muxed));
    }

    #[test]
    fn reads_the_request_size_a_site_asks_for() {
        let chunked = r#"{"format_id": "137", "ext": "mp4", "vcodec": "avc1", "acodec": "none",
            "downloader_options": {"http_chunk_size": 10485760}}"#;
        let plain = r#"{"format_id": "1", "ext": "mp4", "vcodec": "avc1", "acodec": "none"}"#;
        let size = |json: &str| format(serde_json::from_str(json).unwrap()).unwrap().http_chunk_size;
        assert_eq!(size(chunked), Some(10_485_760));
        assert_eq!(size(plain), None);
    }

    #[test]
    fn works_out_a_size_from_the_bitrate_when_the_site_gives_none() {
        // A Twitch VOD format: a bitrate, but no size and no estimate from yt-dlp.
        let twitch = r#"{"format_id": "1080p60", "ext": "mp4", "protocol": "m3u8_native",
            "vcodec": "avc1.64002A", "acodec": "mp4a.40.2", "tbr": 8000}"#;
        let stated = r#"{"format_id": "18", "ext": "mp4", "vcodec": "avc1", "acodec": "mp4a.40.2",
            "tbr": 8000, "filesize": 1234}"#;
        let sized = |json: &str, duration: Option<f64>| {
            let mut format = format(serde_json::from_str(json).unwrap()).unwrap();
            estimate_size(&mut format, duration);
            (format.filesize, format.filesize_estimated)
        };
        // 8000 kbit/s for 100 s is 100 MB.
        assert_eq!(sized(twitch, Some(100.0)), (Some(100_000_000), true));
        assert_eq!(sized(twitch, None), (None, true));
        assert_eq!(sized(stated, Some(100.0)), (Some(1234), false));
    }

    #[test]
    fn an_audio_file_with_unknown_codecs_stays_audio() {
        let file = r#"{"format_id": "773", "ext": "m4a", "video_ext": "none", "audio_ext": "m4a"}"#;
        assert_eq!(kind_of(file), Some(FormatKind::Audio));
    }

    fn listed(json: &str) -> PlaylistInfo {
        playlist(serde_json::from_str(json).unwrap(), "https://example.com/list")
    }

    #[test]
    fn a_channel_lists_the_videos_of_its_tabs_under_their_names() {
        // The shape yt-dlp gives for youtube.com/@BlenderOfficial (2026-11-18): the tabs
        // are playlists with their videos inside; a live and an upcoming stream are kept, marked
        // (they are opened to record), and a video in two tabs is listed once.
        let channel = listed(
            r#"{"_type": "playlist", "title": "Blender", "channel": "Blender", "entries": [
                {"_type": "playlist", "title": "Blender - Videos", "entries": [
                    {"_type": "url", "ie_key": "Youtube", "title": "What's New in Blender 5.3!",
                     "url": "https://www.youtube.com/watch?v=pvUy69I_Xuk", "duration": 1681},
                    {"_type": "url", "ie_key": "Youtube", "title": "Making of",
                     "url": "https://www.youtube.com/watch?v=mHrHLqQj7Ts", "duration": 280}]},
                {"_type": "playlist", "title": "Blender - Live", "entries": [
                    {"_type": "url", "ie_key": "Youtube", "title": "Blender Today LIVE #293",
                     "url": "https://www.youtube.com/watch?v=a", "duration": 4036, "live_status": "was_live"},
                    {"_type": "url", "ie_key": "Youtube", "title": "On now",
                     "url": "https://www.youtube.com/watch?v=b", "live_status": "is_live"},
                    {"_type": "url", "ie_key": "Youtube", "title": "Next week",
                     "url": "https://www.youtube.com/watch?v=c", "live_status": "is_upcoming"}]},
                {"_type": "playlist", "title": "Blender - Shorts", "entries": [
                    {"_type": "url", "ie_key": "Youtube", "title": "Making of",
                     "url": "https://www.youtube.com/watch?v=mHrHLqQj7Ts"}]}
            ]}"#,
        );
        let rows: Vec<_> = channel
            .entries
            .iter()
            .map(|entry| (entry.section.as_deref(), entry.title.as_str(), entry.live))
            .collect();
        assert_eq!(
            rows,
            [
                (Some("Videos"), "What's New in Blender 5.3!", None),
                (Some("Videos"), "Making of", None),
                (Some("Live"), "Blender Today LIVE #293", None),
                (Some("Live"), "On now", Some(LiveStatus::Live)),
                (Some("Live"), "Next week", Some(LiveStatus::Upcoming)),
            ]
        );
    }

    #[test]
    fn links_to_other_playlists_are_kept_as_links() {
        // youtube.com/@BlenderOfficial/playlists: each entry is a whole playlist.
        let tab = listed(
            r#"{"_type": "playlist", "title": "Blender - Playlists", "entries": [
                {"_type": "url", "ie_key": "YoutubeTab", "title": "Blender Conference 2026",
                 "url": "https://www.youtube.com/playlist?list=PLa1F2ddGya_-I9y1nXALrBqrZNdlUE012"},
                null
            ]}"#,
        );
        assert_eq!(tab.entries.len(), 1);
        assert!(tab.entries[0].is_playlist);
        assert_eq!(tab.entries[0].section, None);
    }

    #[test]
    fn a_plain_playlist_has_no_sections_and_only_videos() {
        let list = listed(
            r#"{"_type": "playlist", "title": "Blender Releases", "entries": [
                {"_type": "url", "ie_key": "Youtube", "title": "Blender 4.0",
                 "url": "https://www.youtube.com/watch?v=x", "duration": 120}]}"#,
        );
        assert!(list
            .entries
            .iter()
            .all(|entry| entry.section.is_none() && !entry.is_playlist));
    }
}

#[cfg(test)]
mod live_tests {
    use super::*;

    #[test]
    fn reads_a_live_stream_and_when_it_went_on_air() {
        // YouTube (2026-11-19): the release time is when it went live.
        let json = br#"{"id": "gCNeDWCI0vo", "title": "Al Jazeera English | Live", "live_status": "is_live",
            "timestamp": 1678407878, "release_timestamp": 1678408632, "concurrent_view_count": 10627,
            "webpage_url": "https://www.youtube.com/watch?v=gCNeDWCI0vo", "formats": [
              {"format_id": "270", "ext": "mp4", "protocol": "m3u8_native", "vcodec": "avc1.640028", "acodec": "none", "height": 1080, "tbr": 4561.2}]}"#;
        let live = video(parse(json).unwrap(), "https://youtu.be/gCNeDWCI0vo")
            .unwrap()
            .live
            .unwrap();
        assert_eq!(live.status, LiveStatus::Live);
        assert_eq!(live.since, Some(1678408632));
        assert_eq!(live.viewers, Some(10627));
        // Twitch has only the timestamp, and no viewer count.
        let twitch = br#"{"id": "319376708325", "title": "LofiGirl (live)", "live_status": "is_live",
            "timestamp": 1795020396, "formats": []}"#;
        let live = video(parse(twitch).unwrap(), "https://www.twitch.tv/lofigirl")
            .unwrap()
            .live
            .unwrap();
        assert_eq!((live.since, live.viewers), (Some(1795020396), None));
    }

    #[test]
    fn reads_a_scheduled_stream_without_formats() {
        // What `--ignore-no-formats-error` gave for a NASA stream 27 hours off (2026-11-19).
        let json = br#"{"id": "fTDvp9CBXoQ", "title": "SpaceX CRS-35 Undocking", "live_status": "is_upcoming",
            "release_timestamp": 1795193100, "timestamp": 1794520593, "channel": "NASA", "formats": []}"#;
        let info = video(parse(json).unwrap(), "https://www.youtube.com/watch?v=fTDvp9CBXoQ").unwrap();
        let live = info.live.unwrap();
        assert_eq!((live.status, live.since), (LiveStatus::Upcoming, Some(1795193100)));
        assert!(info.formats.is_empty());
        // An ended stream is an ordinary video.
        let ended = br#"{"id": "a", "live_status": "was_live", "duration": 60, "formats": []}"#;
        assert!(video(parse(ended).unwrap(), "https://x/a").unwrap().live.is_none());
    }

    #[test]
    fn tells_a_scheduled_stream_and_an_offline_channel_from_other_failures() {
        assert!(errors::is_scheduled(
            "ERROR: [youtube] fTDvp9CBXoQ: This live event will begin in 27 hours."
        ));
        assert!(errors::is_scheduled("ERROR: [youtube] abc: Premieres in 5 hours"));
        assert!(errors::is_offline(
            "ERROR: [twitch:stream] monstercat: The channel is not currently live"
        ));
        assert!(!errors::is_offline("ERROR: [youtube] abc: Video unavailable"));
        assert!(!errors::is_scheduled("ERROR: [youtube] abc: Video unavailable"));
    }

    #[test]
    fn names_an_offline_channel_after_its_link() {
        for (url, name) in [
            ("https://www.twitch.tv/monstercat", "monstercat"),
            ("https://www.youtube.com/@NASA/live", "NASA"),
            ("https://kick.com/someone?ref=x", "someone"),
        ] {
            let info = offline(url);
            assert_eq!(info.title, name);
            assert_eq!(info.live.unwrap().status, LiveStatus::Offline);
        }
        assert_eq!(offline("https://example.com/").title, "Live channel");
    }
}
