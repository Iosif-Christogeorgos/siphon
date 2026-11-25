//! What a file on this PC holds, as ffprobe reads it, in the shape a link's
//! lookup has: a [`VideoInfo`] with one format, which the planner plans like
//! any other.

use std::ffi::OsString;
use std::path::Path;

use serde_json::Value;

use super::{LocalFile, Original, Stamp};
use crate::error::{AppError, ErrorKind, Result};
use crate::plan::codecs::{AudioCodec, VideoCodec};
use crate::plan::sizes::SizeFacts;
use crate::plan::tracks::{FileSound, FileSubtitle, FileTracks};
use crate::ytdlp::metadata::{Format, FormatKind, VideoInfo};

/// The arguments that make ffprobe print what [`Probe::parse`] reads.
pub fn probe_args(file: &Path) -> Vec<OsString> {
    let mut args: Vec<OsString> = [
        "-v",
        "error",
        "-print_format",
        "json",
        "-show_entries",
        concat!(
            "format=format_name,start_time,duration,size,bit_rate",
            ":stream=index,codec_type,codec_name,width,height,avg_frame_rate,r_frame_rate,bit_rate,",
            "channels,sample_rate,color_transfer,duration",
            // Matroska states a track's bitrate (if at all) and length as tags,
            // which MKVToolNix names with their language ("BPS-eng"): all of
            // them are asked for, as a list would only match whole names.
            ":stream_tags",
            // An MP3's or M4A's cover art is a picture marked as one, not video.
            ":stream_disposition=default,attached_pic",
            // A phone's video is stored on its side, with how to turn it.
            ":stream_side_data=rotation",
        ),
    ]
    .map(OsString::from)
    .to_vec();
    args.push(file.into());
    args
}

/// What ffprobe says a file holds: its first video track, its main audio
/// track (which a conversion takes as its sound), and its other tracks.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Probe {
    /// ffprobe's names for the file type: "mov,mp4,m4a,3gp,3g2,mj2".
    pub format: String,
    /// Where its timestamps start, in seconds (1.4 for an MPEG-TS from OBS).
    pub start: f64,
    /// Seconds.
    pub duration: Option<f64>,
    /// Bits a second, all tracks together.
    pub bit_rate: Option<f64>,
    pub video: Option<Picture>,
    pub audio: Option<Sound>,
    /// A cover picture (an audio file's art), not video.
    pub cover: bool,
    /// Every sound and subtitle track, and which sound is the main one.
    pub tracks: FileTracks,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Picture {
    /// ffprobe's codec name: "h264", "hevc", "vp9", "prores".
    pub codec: String,
    /// As it is shown: a phone video stored on its side, turned upright.
    pub width: u32,
    pub height: u32,
    pub fps: Option<f64>,
    /// Bits a second, where the file says.
    pub bit_rate: Option<f64>,
    /// "HDR10" or "HLG"; None for SDR.
    pub hdr: Option<&'static str>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Sound {
    /// "aac", "mp3", "opus", "pcm_s16le".
    pub codec: String,
    pub sample_rate: Option<u32>,
    pub channels: Option<u32>,
    pub bit_rate: Option<f64>,
    pub language: Option<String>,
}

impl Probe {
    /// Reads what [`probe_args`] printed. None if it isn't ffprobe's JSON.
    pub fn parse(json: &str) -> Option<Probe> {
        let value: Value = serde_json::from_str(json).ok()?;
        let format = &value["format"];
        let mut probe = Probe {
            format: format["format_name"].as_str().unwrap_or_default().to_string(),
            start: number(&format["start_time"]).unwrap_or(0.0),
            duration: number(&format["duration"]).filter(|seconds| *seconds > 0.0),
            bit_rate: number(&format["bit_rate"]).filter(|bits| *bits > 0.0),
            ..Probe::default()
        };
        let streams = value["streams"].as_array().map(Vec::as_slice).unwrap_or_default();
        let sounds: Vec<&Value> = streams
            .iter()
            .filter(|stream| stream["codec_type"] == "audio")
            .collect();
        // The main sound: the track marked to play first, or the first. A
        // conversion takes it by its place among the sound tracks (`0:a:N`).
        let main = sounds
            .iter()
            .position(|stream| stream["disposition"]["default"].as_i64() == Some(1))
            .unwrap_or(0);
        probe.tracks = FileTracks {
            main,
            sounds: sounds
                .iter()
                .enumerate()
                .map(|(track, stream)| {
                    let sound = sound(stream);
                    FileSound {
                        track,
                        codec: AudioCodec::named(&sound.codec),
                        kbps: sound.bit_rate.map(|bits| bits / 1000.0),
                    }
                })
                .collect(),
            subtitles: streams
                .iter()
                .filter(|stream| stream["codec_type"] == "subtitle")
                .enumerate()
                .map(|(track, stream)| FileSubtitle {
                    track,
                    codec: stream["codec_name"].as_str().unwrap_or_default().to_string(),
                })
                .collect(),
        };
        let audio = sounds.get(main).copied();
        let mut longest = None::<f64>;
        for stream in streams {
            let length = number(&stream["duration"]).or_else(|| tag(&stream["tags"], "DURATION").and_then(clock));
            longest = match (longest, length) {
                (Some(a), Some(b)) => Some(a.max(b)),
                (a, b) => a.or(b),
            };
            let is_cover = stream["disposition"]["attached_pic"].as_i64() == Some(1);
            match stream["codec_type"].as_str() {
                Some("video") if is_cover => probe.cover = true,
                Some("video") if probe.video.is_none() => probe.video = Some(picture(stream)),
                _ => {}
            }
        }
        probe.audio = audio.map(sound);
        probe.duration = probe.duration.or(longest).filter(|seconds| *seconds > 0.0);
        Some(probe)
    }

    /// What the file holds as a video's lookup: one format, with the
    /// file's own picture and sound, which the planner plans like any other.
    /// An error if it holds neither, or is a still picture.
    pub(super) fn video_info(&self, original: &Original, stamp: Stamp) -> Result<VideoInfo> {
        let size = stamp.size;
        // A picture without a length is a still (a photo, a PNG): nothing to convert.
        let still = self.format.ends_with("_pipe") || self.format == "image2";
        let kind = match (&self.video, &self.audio) {
            _ if still || self.duration.is_none() => return Err(not_media()),
            (Some(_), Some(_)) => FormatKind::Muxed,
            (Some(_), None) => FormatKind::Video,
            (None, Some(_)) => FormatKind::Audio,
            (None, None) => return Err(not_media()),
        };
        let kbps = |bits: Option<f64>| bits.map(|bits| bits / 1000.0);
        let video = self.video.as_ref();
        let audio = self.audio.as_ref();
        let ext = original
            .path()
            .extension()
            .map(|ext| ext.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        let mut format = Format {
            // A file has one: the id names nothing to fetch.
            id: "file".into(),
            ext,
            kind,
            vcodec: video.map(|video| video.codec.clone()),
            acodec: audio.map(|audio| audio.codec.clone()),
            video_codec: None,
            audio_codec: None,
            width: video.map(|video| video.width),
            height: video.map(|video| video.height),
            fps: video.and_then(|video| video.fps),
            tbr: kbps(self.bit_rate),
            vbr: kbps(video.and_then(|video| video.bit_rate)),
            abr: kbps(audio.and_then(|audio| audio.bit_rate)),
            asr: audio.and_then(|audio| audio.sample_rate),
            audio_channels: audio.and_then(|audio| audio.channels),
            filesize: Some(size).filter(|size| *size > 0),
            filesize_estimated: false,
            dynamic_range: video.map(|video| video.hdr.unwrap_or("SDR").to_string()),
            language: audio.and_then(|audio| audio.language.clone()),
            language_preference: None,
            segmented: false,
            stream_url: None,
            http_chunk_size: None,
            http_headers: Vec::new(),
            note: None,
        };
        format.video_codec = video.map(|_| VideoCodec::of(&format));
        format.audio_codec = audio.map(|_| AudioCodec::of(&format));
        Ok(VideoInfo {
            id: original.id().to_string(),
            title: original.stem(),
            url: original.key(),
            thumbnail: None,
            duration: self.duration,
            channel: None,
            view_count: None,
            extractor: None,
            formats: vec![format],
            with_account: false,
            live: None,
            subtitles: Vec::new(),
            chat: None,
            saved: None,
            sizes: SizeFacts::default(),
            file: Some(LocalFile {
                matroska: self.format.split(',').any(|name| name == "matroska"),
                start: self.start,
                cover: self.cover,
                tracks: self.tracks.clone(),
                ..original.describe(stamp)
            }),
        })
    }
}

/// The error for a file that holds no video or audio Siphon can read.
pub fn not_media() -> AppError {
    AppError::new(
        ErrorKind::Unsupported,
        "This isn't a video or audio file Siphon can read.",
    )
}

fn picture(stream: &Value) -> Picture {
    let (width, height) = (whole(&stream["width"]), whole(&stream["height"]));
    // A quarter turn either way stands the picture up: show it as it plays.
    let turned = stream["side_data_list"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|data| data["rotation"].as_f64())
        .any(|rotation| (rotation.abs() - 90.0).abs() < 1.0 || (rotation.abs() - 270.0).abs() < 1.0);
    let (width, height) = if turned { (height, width) } else { (width, height) };
    Picture {
        codec: stream["codec_name"].as_str().unwrap_or_default().to_string(),
        width,
        height,
        fps: rate(&stream["avg_frame_rate"]).or_else(|| rate(&stream["r_frame_rate"]).filter(|fps| *fps <= 240.0)),
        bit_rate: bit_rate(stream),
        hdr: match stream["color_transfer"].as_str() {
            Some("smpte2084") => Some("HDR10"),
            Some("arib-std-b67") => Some("HLG"),
            _ => None,
        },
    }
}

fn sound(stream: &Value) -> Sound {
    Sound {
        codec: stream["codec_name"].as_str().unwrap_or_default().to_string(),
        sample_rate: number(&stream["sample_rate"]).map(|rate| rate as u32),
        channels: stream["channels"]
            .as_u64()
            .and_then(|channels| u32::try_from(channels).ok()),
        bit_rate: bit_rate(stream),
        // "und" is ffmpeg's word for none.
        language: tag(&stream["tags"], "language")
            .filter(|language| !language.is_empty() && *language != "und")
            .map(str::to_string),
    }
}

/// A stream's tag by its name, whatever its capitals, or with the language
/// Matroska marks it with: MKVToolNix writes its counts as "BPS-eng" and
/// "DURATION-eng", and ffprobe gives "LANGUAGE" for a track with no language
/// element of its own.
pub(super) fn tag<'a>(tags: &'a Value, name: &str) -> Option<&'a str> {
    let matches = |key: &str| {
        key.eq_ignore_ascii_case(name)
            || key
                .get(..name.len())
                .is_some_and(|start| start.eq_ignore_ascii_case(name) && key[name.len()..].starts_with('-'))
    };
    tags.as_object()?
        .iter()
        .find(|(key, _)| matches(key))
        .and_then(|(_, value)| value.as_str())
}

/// The track's bitrate, or the one Matroska's muxers write as a tag.
fn bit_rate(stream: &Value) -> Option<f64> {
    number(&stream["bit_rate"])
        .or_else(|| tag(&stream["tags"], "BPS").and_then(|bits| bits.trim().parse().ok()))
        .filter(|bits| *bits > 0.0)
}

/// ffprobe writes numbers as strings: "6.000000", "4804946".
fn number(value: &Value) -> Option<f64> {
    match value {
        Value::String(text) => text.trim().parse().ok(),
        Value::Number(number) => number.as_f64(),
        _ => None,
    }
    .filter(|number: &f64| number.is_finite())
}

fn whole(value: &Value) -> u32 {
    value.as_u64().and_then(|value| u32::try_from(value).ok()).unwrap_or(0)
}

/// "30000/1001"; "0/0" when unknown.
fn rate(value: &Value) -> Option<f64> {
    let (numerator, denominator) = value.as_str()?.split_once('/')?;
    let (numerator, denominator): (f64, f64) = (numerator.parse().ok()?, denominator.parse().ok()?);
    (numerator > 0.0 && denominator > 0.0).then(|| numerator / denominator)
}

/// Matroska's length tag: "00:12:44.967000000".
fn clock(value: &str) -> Option<f64> {
    let mut seconds = 0.0;
    for part in value.split(':') {
        seconds = seconds * 60.0 + part.parse::<f64>().ok()?;
    }
    Some(seconds)
}

/// The arguments that take the frame a file's card shows into `out`: at a
/// tenth of the way in (at most five minutes), where a video has usually got
/// going, or an audio file's cover. None for sound without a cover.
pub fn frame_args(file: &Path, probe: &Probe, out: &Path) -> Option<Vec<OsString>> {
    let mut args: Vec<OsString> = ["-hide_banner", "-nostdin", "-y", "-loglevel", "error"]
        .map(OsString::from)
        .to_vec();
    let stream = if probe.video.is_some() {
        let at = probe.duration.map_or(0.0, |seconds| (seconds * 0.1).min(300.0));
        args.extend(["-ss".into(), format!("{at:.3}").into()]);
        // Video, never a cover picture beside it.
        "0:V:0"
    } else if probe.cover {
        "0:v:0"
    } else {
        return None;
    };
    args.extend(["-i".into(), file.into()]);
    args.extend(
        [
            "-map",
            stream,
            "-frames:v",
            "1",
            // Twice what a card shows it at, either way up.
            "-vf",
            "scale=w=640:h=640:force_original_aspect_ratio=decrease:force_divisible_by=2",
            "-q:v",
            "4",
            "-f",
            "image2",
        ]
        .map(OsString::from),
    );
    args.push(out.into());
    Some(args)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What ffprobe printed for files made with the managed FFmpeg
    /// (2026-11-23), cut down to what is read.
    const PHONE_HEVC: &str = r#"{"streams": [
        {"index": 0, "codec_name": "hevc", "codec_type": "video", "width": 1920, "height": 1080,
         "r_frame_rate": "30/1", "avg_frame_rate": "30/1", "bit_rate": "4804946",
         "disposition": {"default": 1, "attached_pic": 0}, "tags": {"language": "und"},
         "side_data_list": [{"rotation": 90}]},
        {"index": 1, "codec_name": "aac", "codec_type": "audio", "sample_rate": "44100", "channels": 1,
         "r_frame_rate": "0/0", "avg_frame_rate": "0/0", "bit_rate": "68568",
         "disposition": {"default": 1, "attached_pic": 0}, "tags": {"language": "und"}}],
        "format": {"format_name": "mov,mp4,m4a,3gp,3g2,mj2", "start_time": "0.000000", "duration": "6.000000",
         "size": "3665955", "bit_rate": "4887940"}}"#;

    const SONG_WITH_COVER: &str = r#"{"streams": [
        {"index": 0, "codec_name": "mp3", "codec_type": "audio", "sample_rate": "44100", "channels": 1,
         "bit_rate": "192000", "disposition": {"default": 0, "attached_pic": 0}},
        {"index": 1, "codec_name": "mjpeg", "codec_type": "video", "width": 640, "height": 360,
         "r_frame_rate": "90000/1", "avg_frame_rate": "0/0", "disposition": {"default": 0, "attached_pic": 1}, "tags": {}}],
        "format": {"format_name": "mp3", "start_time": "0.025056", "duration": "7.000000", "size": "195034", "bit_rate": "222896"}}"#;

    const OBS_MKV: &str = r#"{"streams": [
        {"index": 0, "codec_name": "h264", "codec_type": "video", "width": 1920, "height": 1080,
         "r_frame_rate": "60/1", "avg_frame_rate": "60/1", "disposition": {"default": 0, "attached_pic": 0},
         "tags": {"DURATION": "00:00:08.000000000"}},
        {"index": 1, "codec_name": "aac", "codec_type": "audio", "sample_rate": "44100", "channels": 1,
         "r_frame_rate": "0/0", "avg_frame_rate": "0/0", "disposition": {"default": 0, "attached_pic": 0},
         "tags": {"DURATION": "00:00:08.023000000"}}],
        "format": {"format_name": "matroska,webm", "start_time": "0.000000", "duration": "8.023000",
         "size": "8929801", "bit_rate": "8904201"}}"#;

    /// A film's tracks as ffprobe printed them (2026-11-25): three sound
    /// tracks, the second marked to play first, and two subtitle tracks.
    const FILM: &str = r#"{"streams": [
        {"index": 0, "codec_name": "h264", "codec_type": "video", "width": 1280, "height": 720,
         "r_frame_rate": "30/1", "avg_frame_rate": "30/1", "disposition": {"default": 0, "attached_pic": 0}},
        {"index": 1, "codec_name": "aac", "codec_type": "audio", "sample_rate": "44100", "channels": 1,
         "disposition": {"default": 0, "attached_pic": 0}, "tags": {"language": "eng", "title": "Game"}},
        {"index": 2, "codec_name": "aac", "codec_type": "audio", "sample_rate": "44100", "channels": 1,
         "disposition": {"default": 1, "attached_pic": 0}, "tags": {"language": "eng", "title": "Mic"}},
        {"index": 3, "codec_name": "ac3", "codec_type": "audio", "sample_rate": "44100", "channels": 1,
         "bit_rate": "192000", "disposition": {"default": 0, "attached_pic": 0}, "tags": {"language": "ell"}},
        {"index": 4, "codec_name": "subrip", "codec_type": "subtitle", "disposition": {"default": 0, "attached_pic": 0}},
        {"index": 5, "codec_name": "ass", "codec_type": "subtitle", "disposition": {"default": 0, "attached_pic": 0}}],
        "format": {"format_name": "matroska,webm", "start_time": "0.000000", "duration": "10.000000",
         "size": "3000000", "bit_rate": "2400000"}}"#;

    const STILL: &str = r#"{"streams": [
        {"index": 0, "codec_name": "png", "codec_type": "video", "width": 640, "height": 360,
         "r_frame_rate": "25/1", "avg_frame_rate": "25/1", "disposition": {"default": 0, "attached_pic": 0}}],
        "format": {"format_name": "png_pipe", "size": "26458"}}"#;

    /// The file as it is: of `size` bytes, its time of change unknown.
    fn stamp(size: u64) -> Stamp {
        Stamp { size, modified: None }
    }

    fn original(name: &str) -> Original {
        Original::new(Path::new(r"C:\Videos").join(name))
    }

    #[test]
    fn a_phone_video_reads_as_one_upright_format_with_its_sound() {
        let probe = Probe::parse(PHONE_HEVC).unwrap();
        let info = probe.video_info(&original("Phone.MP4"), stamp(3_665_955)).unwrap();
        assert_eq!(info.title, "Phone");
        assert!(crate::local::is_key(&info.url));
        assert_eq!(info.duration, Some(6.0));
        let format = &info.formats[0];
        assert_eq!(format.kind, FormatKind::Muxed);
        assert_eq!(format.ext, "mp4");
        assert_eq!(format.video_codec, Some(VideoCodec::Hevc));
        assert_eq!(format.audio_codec, Some(AudioCodec::Aac));
        // Stored on its side, shown standing up.
        assert_eq!((format.width, format.height), (Some(1080), Some(1920)));
        assert_eq!(format.fps, Some(30.0));
        assert_eq!(format.filesize, Some(3_665_955));
        assert!(!format.filesize_estimated);
        assert_eq!(format.vbr.map(f64::round), Some(4805.0));
        assert_eq!(format.abr.map(f64::round), Some(69.0));
        assert_eq!(format.dynamic_range.as_deref(), Some("SDR"));
        // "und" says nothing.
        assert_eq!(format.language, None);
        let file = info.file.as_ref().unwrap();
        assert_eq!((file.name.as_str(), file.folder.as_str()), ("Phone.MP4", r"C:\Videos"));
    }

    #[test]
    fn reads_the_tags_mkvtoolnix_writes_with_their_language() {
        // The Legend of Korra S01E01, from MKVToolNix 22: counts as "BPS-eng",
        // and "LANGUAGE" for the tracks with no language element of their own.
        let json = r#"{
            "format": {"format_name": "matroska,webm", "start_time": "0.000000", "size": "289422396"},
            "streams": [
                {"index": 0, "codec_type": "video", "codec_name": "hevc", "width": 1920, "height": 1080,
                 "avg_frame_rate": "24000/1001", "disposition": {"default": 1, "attached_pic": 0},
                 "tags": {"BPS-eng": "1039153", "DURATION-eng": "00:24:04.026000000"}},
                {"index": 1, "codec_type": "audio", "codec_name": "aac", "channels": 2, "sample_rate": "48000",
                 "disposition": {"default": 1, "attached_pic": 0},
                 "tags": {"language": "gre", "BPS-eng": "164789", "DURATION-eng": "00:24:04.778000000"}},
                {"index": 2, "codec_type": "audio", "codec_name": "aac", "channels": 6, "sample_rate": "48000",
                 "disposition": {"default": 0, "attached_pic": 0},
                 "tags": {"title": "Surround", "LANGUAGE": "eng", "BPS-eng": "256000"}}
            ]
        }"#;
        let probe = Probe::parse(json).unwrap();
        assert_eq!(probe.video.as_ref().and_then(|video| video.bit_rate), Some(1_039_153.0));
        let sound = probe.audio.as_ref().unwrap();
        assert_eq!(
            (sound.language.as_deref(), sound.bit_rate),
            (Some("gre"), Some(164_789.0))
        );
        let kbps: Vec<_> = probe
            .tracks
            .sounds
            .iter()
            .map(|sound| sound.kbps.map(f64::round))
            .collect();
        assert_eq!(kbps, [Some(165.0), Some(256.0)]);
        // Its length from the longest track's, as the format doesn't say.
        assert_eq!(probe.duration.map(f64::round), Some(1445.0));
    }

    #[test]
    fn a_songs_cover_is_a_picture_not_video() {
        let probe = Probe::parse(SONG_WITH_COVER).unwrap();
        assert!(probe.cover);
        assert!(probe.video.is_none());
        let info = probe.video_info(&original("Song.mp3"), stamp(195_034)).unwrap();
        let format = &info.formats[0];
        assert_eq!(format.kind, FormatKind::Audio);
        assert_eq!(format.audio_codec, Some(AudioCodec::Mp3));
        assert_eq!(format.video_codec, None);
        assert_eq!(format.abr, Some(192.0));
        // Its card shows the cover.
        let args = frame_args(Path::new("Song.mp3"), &probe, Path::new("out.jpg")).unwrap();
        assert!(args.windows(2).any(|pair| pair[0] == "-map" && pair[1] == "0:v:0"));
        assert!(!args.iter().any(|arg| arg == "-ss"));
    }

    #[test]
    fn a_matroska_recording_takes_its_length_from_its_tags_where_it_must() {
        let probe = Probe::parse(OBS_MKV).unwrap();
        assert_eq!(probe.duration, Some(8.023));
        let json = OBS_MKV.replace(r#""duration": "8.023000","#, "");
        assert_eq!(Probe::parse(&json).unwrap().duration, Some(8.023));
        let info = probe.video_info(&original("Recording.mkv"), stamp(8_929_801)).unwrap();
        let format = &info.formats[0];
        assert_eq!(format.video_codec, Some(VideoCodec::H264));
        assert_eq!(
            (format.width, format.height, format.fps),
            (Some(1920), Some(1080), Some(60.0))
        );
        // No bitrate per track: the file's.
        assert_eq!(format.vbr, None);
        assert_eq!(format.tbr.map(f64::round), Some(8904.0));
        // Its card is a frame a tenth of the way in, of the video.
        let args = frame_args(Path::new("Recording.mkv"), &probe, Path::new("out.jpg")).unwrap();
        assert!(args.windows(2).any(|pair| pair[0] == "-ss" && pair[1] == "0.802"));
        assert!(args.windows(2).any(|pair| pair[0] == "-map" && pair[1] == "0:V:0"));
    }

    #[test]
    fn every_track_is_read_and_the_main_sound_is_the_one_marked_to_play_first() {
        let probe = Probe::parse(FILM).unwrap();
        assert_eq!(probe.tracks.main, 1);
        // The card and the plan describe the main sound, as it is converted.
        assert_eq!(
            probe.audio.as_ref().map(|audio| audio.language.as_deref()),
            Some(Some("eng"))
        );
        let sounds: Vec<(usize, AudioCodec)> = probe.tracks.sounds.iter().map(|s| (s.track, s.codec)).collect();
        assert_eq!(
            sounds,
            [(0, AudioCodec::Aac), (1, AudioCodec::Aac), (2, AudioCodec::Ac3)]
        );
        assert_eq!(probe.tracks.sounds[2].kbps, Some(192.0));
        let subtitles: Vec<(usize, &str)> = probe
            .tracks
            .subtitles
            .iter()
            .map(|s| (s.track, s.codec.as_str()))
            .collect();
        assert_eq!(subtitles, [(0, "subrip"), (1, "ass")]);
        let info = probe.video_info(&original("Film.mkv"), stamp(3_000_000)).unwrap();
        assert_eq!(info.file.unwrap().tracks, probe.tracks);
        // None marked: the first.
        let unmarked = FILM.replace(r#""default": 1"#, r#""default": 0"#);
        assert_eq!(Probe::parse(&unmarked).unwrap().tracks.main, 0);
    }

    #[test]
    fn a_still_picture_or_something_else_isnt_media() {
        let still = Probe::parse(STILL).unwrap();
        assert_eq!(
            still
                .video_info(&original("Cover.png"), stamp(26_458))
                .unwrap_err()
                .kind,
            ErrorKind::Unsupported
        );
        assert!(Probe::parse("{\n\n}")
            .unwrap()
            .video_info(&original("Notes.mp4"), stamp(28))
            .is_err());
        assert!(Probe::parse("moov atom not found").is_none());
    }

    #[test]
    fn hdr_is_read_from_the_transfer() {
        let json = PHONE_HEVC.replace(
            r#""bit_rate": "4804946","#,
            r#""bit_rate": "4804946", "color_transfer": "smpte2084","#,
        );
        let info = Probe::parse(&json)
            .unwrap()
            .video_info(&original("HDR.mp4"), stamp(1))
            .unwrap();
        assert_eq!(info.formats[0].dynamic_range.as_deref(), Some("HDR10"));
    }
}
