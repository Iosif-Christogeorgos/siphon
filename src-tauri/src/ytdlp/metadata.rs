//! `yt-dlp -J`: what a link points to, and which formats it offers.

use std::collections::HashSet;
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
/// its videos (a channel's Playlists tab) is kept as a link to open. Streams
/// that are live or haven't started can't be downloaded yet, and a video
/// listed twice is kept once.
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
        if matches!(entry.live_status.as_deref(), Some("is_live" | "is_upcoming")) {
            continue;
        }
        let is_playlist = is_playlist_link(&entry);
        let Some(url) = entry.url.or(entry.webpage_url) else { continue };
        if !seen.insert(url.clone()) {
            continue;
        }
        entries.push(PlaylistEntry {
            url,
            id: entry.id.unwrap_or_default(),
            title: entry
                .title
                .unwrap_or_else(|| (if is_playlist { "Untitled playlist" } else { "Untitled video" }).to_string()),
            duration: entry.duration,
            thumbnail: entry.thumbnail.or_else(|| last_thumbnail(entry.thumbnails)),
            channel: entry.channel.or(entry.uploader),
            is_playlist,
            section: section.map(str::to_string),
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
        // are playlists with their videos inside; a live and an upcoming stream are left out,
        // and a video in two tabs is listed once.
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
            .map(|entry| (entry.section.as_deref(), entry.title.as_str(), entry.is_playlist))
            .collect();
        assert_eq!(
            rows,
            [
                (Some("Videos"), "What's New in Blender 5.3!", false),
                (Some("Videos"), "Making of", false),
                (Some("Live"), "Blender Today LIVE #293", false),
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
        assert!(list.entries.iter().all(|entry| entry.section.is_none() && !entry.is_playlist));
    }
}
