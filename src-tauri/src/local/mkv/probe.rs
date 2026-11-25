//! An MKV's tracks as ffprobe reads them.

use std::ffi::OsString;
use std::path::Path;

use serde_json::{Map, Value};

use super::{Attachment, Chapters, Contents, Kind, Track};
use crate::ytdlp::subtitles::language_tag;

/// The arguments that make ffprobe print what [`parse`] reads.
pub fn args(file: &Path) -> Vec<OsString> {
    let mut args: Vec<OsString> = [
        "-v",
        "error",
        "-print_format",
        "json",
        "-show_entries",
        concat!(
            "format=format_name,duration,size",
            ":stream=index,codec_type,codec_name,width,height,channels",
            ":stream_tags:stream_disposition",
            ":chapter=start_time",
        ),
    ]
    .map(OsString::from)
    .to_vec();
    args.push(file.into());
    args
}

/// Reads what [`args`] printed; None if it isn't ffprobe's JSON for an MKV.
pub fn parse(json: &str) -> Option<Contents> {
    let value: Value = serde_json::from_str(json).ok()?;
    let format = value["format"]["format_name"].as_str()?;
    if !format.split(',').any(|name| name == "matroska") {
        return None;
    }
    let mut tracks = Vec::new();
    let mut attachments = Vec::new();
    for stream in value["streams"].as_array().map(Vec::as_slice).unwrap_or_default() {
        let tags = stream["tags"].as_object();
        let flag = |name: &str| stream["disposition"][name].as_i64() == Some(1);
        let kind = match stream["codec_type"].as_str() {
            // A cover picture carried in the file is an attachment, not video.
            Some("video") if flag("attached_pic") => None,
            Some("video") => Some(Kind::Video),
            Some("audio") => Some(Kind::Sound),
            Some("subtitle") => Some(Kind::Subtitles),
            _ => None,
        };
        let Some(kind) = kind else {
            if let Some(name) = tag(tags, "filename") {
                attachments.push(Attachment {
                    name: name.to_string(),
                    mime: tag(tags, "mimetype").map(str::to_string),
                });
            }
            continue;
        };
        let name = tag(tags, "title")
            .map(str::to_string)
            .filter(|name| !name.trim().is_empty());
        let named = |words: &[&str]| name.as_deref().is_some_and(|name| says(name, words));
        tracks.push(Track {
            index: whole(&stream["index"]).unwrap_or(tracks.len() as u64) as usize,
            kind,
            codec: stream["codec_name"].as_str().unwrap_or("unknown").to_string(),
            language: tag(tags, "language").and_then(language),
            default: flag("default"),
            forced: flag("forced") || named(&["forced"]),
            sdh: flag("hearing_impaired") || named(&["sdh", "cc", "hearing impaired", "hard of hearing"]),
            commentary: flag("comment") || named(&["commentary"]),
            described: flag("visual_impaired") || named(&["audio description", "descriptive"]),
            original: flag("original"),
            channels: whole(&stream["channels"]).and_then(|n| u32::try_from(n).ok()),
            width: whole(&stream["width"]).and_then(|n| u32::try_from(n).ok()),
            height: whole(&stream["height"]).and_then(|n| u32::try_from(n).ok()),
            entries: tag(tags, "NUMBER_OF_FRAMES").and_then(|n| n.trim().parse().ok()),
            name,
            doubts: Vec::new(),
            notes: Vec::new(),
            read: false,
            certain: false,
        });
    }
    Some(Contents {
        tracks,
        chapters: Chapters {
            count: value["chapters"].as_array().map_or(0, Vec::len),
            ..Chapters::default()
        },
        attachments,
        doubts: Vec::new(),
        duration: number(&value["format"]["duration"]).filter(|seconds| *seconds > 0.0),
        size: number(&value["format"]["size"]).map(|bytes| bytes as u64),
    })
}

/// A tag by its name, whatever its capitals, or with the language Matroska
/// marks it with: ffprobe gives "LANGUAGE" for a track with no language of
/// its own, and MKVToolNix's counts as "NUMBER_OF_FRAMES-eng".
fn tag<'a>(tags: Option<&'a Map<String, Value>>, name: &str) -> Option<&'a str> {
    let tags = tags?;
    let matches = |key: &str| {
        key.eq_ignore_ascii_case(name)
            || key
                .get(..name.len())
                .is_some_and(|start| start.eq_ignore_ascii_case(name) && key[name.len()..].starts_with('-'))
    };
    tags.iter()
        .find(|(key, _)| matches(key))
        .and_then(|(_, value)| value.as_str())
}

/// A track's language tag as Siphon names languages ("gre" → "el"); None for
/// the tags that name none: undetermined, several, none at all.
fn language(tag: &str) -> Option<String> {
    let tag = tag.trim().to_ascii_lowercase();
    (!tag.is_empty() && !["und", "mul", "zxx", "mis"].contains(&tag.as_str())).then(|| language_tag(&tag))
}

/// Whether `name` says one of `words`, as words: "English SDH" says "sdh",
/// "Accent" doesn't say "cc".
fn says(name: &str, words: &[&str]) -> bool {
    let name = name.to_lowercase();
    let spaced: String = name
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect();
    let spaced = format!(" {} ", spaced.split_whitespace().collect::<Vec<_>>().join(" "));
    words.iter().any(|word| spaced.contains(&format!(" {word} ")))
}

/// ffprobe writes numbers as strings: "1444.778000".
fn number(value: &Value) -> Option<f64> {
    match value {
        Value::String(text) => text.trim().parse().ok(),
        Value::Number(number) => number.as_f64(),
        _ => None,
    }
    .filter(|number: &f64| number.is_finite())
}

fn whole(value: &Value) -> Option<u64> {
    value.as_u64()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What ffprobe prints for The Legend of Korra S01E01 (EN-GR, from
    /// MKVToolNix 22), cut to what is read.
    const KORRA: &str = r#"{
        "streams": [
            {"index": 0, "codec_name": "hevc", "codec_type": "video", "width": 1920, "height": 1080,
             "disposition": {"default": 1, "forced": 0},
             "tags": {"BPS-eng": "1039153", "NUMBER_OF_FRAMES-eng": "34622"}},
            {"index": 1, "codec_name": "aac", "codec_type": "audio", "channels": 2,
             "disposition": {"default": 1}, "tags": {"language": "gre", "NUMBER_OF_FRAMES-eng": "67724"}},
            {"index": 2, "codec_name": "aac", "codec_type": "audio", "channels": 6,
             "disposition": {"default": 0}, "tags": {"title": "Surround", "LANGUAGE": "eng"}},
            {"index": 3, "codec_name": "aac", "codec_type": "audio", "channels": 2,
             "disposition": {"default": 0, "comment": 0}, "tags": {"title": "Stereo-Commentary", "LANGUAGE": "eng"}},
            {"index": 4, "codec_name": "subrip", "codec_type": "subtitle",
             "disposition": {"default": 0}, "tags": {"language": "gre", "NUMBER_OF_FRAMES-eng": "338"}},
            {"index": 5, "codec_name": "hdmv_pgs_subtitle", "codec_type": "subtitle",
             "disposition": {"default": 0}, "tags": {"LANGUAGE": "eng", "NUMBER_OF_FRAMES-eng": "589"}}
        ],
        "chapters": [{"start_time": "0.000000"}, {"start_time": "434.225000"},
                     {"start_time": "997.329000"}, {"start_time": "1408.782000"}],
        "format": {"format_name": "matroska,webm"}
    }"#;

    #[test]
    fn reads_every_track_of_a_real_episode() {
        let contents = parse(KORRA).unwrap();
        let summary: Vec<_> = contents
            .tracks
            .iter()
            .map(|t| {
                (
                    t.index,
                    t.kind,
                    t.codec.as_str(),
                    t.language.as_deref(),
                    t.name.as_deref(),
                    t.default,
                )
            })
            .collect();
        assert_eq!(
            summary,
            [
                (0, Kind::Video, "hevc", None, None, true),
                (1, Kind::Sound, "aac", Some("el"), None, true),
                (2, Kind::Sound, "aac", Some("en"), Some("Surround"), false),
                (3, Kind::Sound, "aac", Some("en"), Some("Stereo-Commentary"), false),
                (4, Kind::Subtitles, "subrip", Some("el"), None, false),
                (5, Kind::Subtitles, "hdmv_pgs_subtitle", Some("en"), None, false),
            ]
        );
        // Commentary by its name alone; the counts MKVToolNix wrote.
        assert!(contents.tracks[3].commentary && !contents.tracks[2].commentary);
        assert_eq!(contents.tracks[2].channels, Some(6));
        assert_eq!(contents.tracks[4].entries, Some(338));
        assert_eq!(contents.tracks[5].entries, Some(589));
        assert_eq!(contents.chapters.count, 4);
    }

    #[test]
    fn reads_flags_from_the_file_or_from_names() {
        let json = r#"{"format": {"format_name": "matroska,webm"}, "streams": [
            {"index": 0, "codec_type": "subtitle", "codec_name": "subrip", "disposition": {"forced": 1}},
            {"index": 1, "codec_type": "subtitle", "codec_name": "subrip", "tags": {"title": "English [SDH]"}},
            {"index": 2, "codec_type": "subtitle", "codec_name": "subrip", "disposition": {"hearing_impaired": 1}},
            {"index": 3, "codec_type": "subtitle", "codec_name": "subrip", "tags": {"title": "Accented"}},
            {"index": 4, "codec_type": "audio", "codec_name": "ac3", "tags": {"title": "Audio Description"}},
            {"index": 5, "codec_type": "audio", "codec_name": "truehd", "disposition": {"original": 1}},
            {"index": 6, "codec_type": "subtitle", "codec_name": "ass", "tags": {"title": "Signs & Songs (Forced)"}}
        ]}"#;
        let tracks = parse(json).unwrap().tracks;
        let flags: Vec<_> = tracks
            .iter()
            .map(|t| (t.forced, t.sdh, t.described, t.original))
            .collect();
        assert_eq!(
            flags,
            [
                (true, false, false, false),
                (false, true, false, false),
                (false, true, false, false),
                (false, false, false, false),
                (false, false, true, false),
                (false, false, false, true),
                (true, false, false, false),
            ]
        );
    }

    #[test]
    fn tags_that_name_no_language_are_unknown() {
        let json = r#"{"format": {"format_name": "matroska,webm"}, "streams": [
            {"index": 0, "codec_type": "audio", "codec_name": "aac", "tags": {"language": "und"}},
            {"index": 1, "codec_type": "audio", "codec_name": "aac", "tags": {"language": "zxx"}},
            {"index": 2, "codec_type": "audio", "codec_name": "aac"},
            {"index": 3, "codec_type": "audio", "codec_name": "aac", "tags": {"language": "ger"}},
            {"index": 4, "codec_type": "audio", "codec_name": "aac", "tags": {"language": "pt-BR"}}
        ]}"#;
        let languages: Vec<_> = parse(json).unwrap().tracks.into_iter().map(|t| t.language).collect();
        assert_eq!(languages, [None, None, None, Some("de".into()), Some("pt-br".into())]);
    }

    #[test]
    fn attachments_and_covers_are_not_tracks() {
        let json = r#"{"format": {"format_name": "matroska,webm"}, "streams": [
            {"index": 0, "codec_type": "video", "codec_name": "h264", "width": 1280, "height": 720},
            {"index": 1, "codec_type": "attachment", "codec_name": "ttf",
             "tags": {"filename": "Arial.ttf", "mimetype": "font/ttf"}},
            {"index": 2, "codec_type": "video", "codec_name": "mjpeg", "disposition": {"attached_pic": 1},
             "tags": {"filename": "cover.jpg", "mimetype": "image/jpeg"}}
        ]}"#;
        let contents = parse(json).unwrap();
        assert_eq!(contents.tracks.len(), 1);
        let names: Vec<_> = contents.attachments.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, ["Arial.ttf", "cover.jpg"]);
    }

    #[test]
    fn only_an_mkv() {
        assert!(parse(r#"{"format": {"format_name": "mov,mp4,m4a,3gp,3g2,mj2"}, "streams": []}"#).is_none());
        assert!(parse("not json").is_none());
    }
}
