//! Siphon's own ffmpeg pass: converting video and audio after the download.
//!
//! yt-dlp handles lossless work (merging, remuxing). Conversions run here
//! instead, because that is the only way to get hardware encoding and a
//! progress readout.

pub mod encoders;
pub mod smart_cut;

use std::ffi::OsString;
use std::path::Path;
use std::time::Duration;

use crate::plan::codecs::{AudioCodec, Container, VideoCodec};
use crate::plan::{AudioAction, Recipe, Trim, VideoAction, VideoEncode, VideoStep};
use crate::process;
use encoders::Hardware;

/// Whether this ffmpeg can fetch a stream in requests of a set size
/// (`-request_size`). Builds from before the option existed stop with
/// "Unrecognized option" when given it, so it is only passed after this check.
pub async fn supports_request_size(ffmpeg: &Path) -> bool {
    let mut cmd = process::command(ffmpeg);
    cmd.args(["-hide_banner", "-h", "protocol=http"]);
    match process::output_within(cmd, Duration::from_secs(20)).await {
        Ok(Some(output)) => String::from_utf8_lossy(&output.stdout).contains("-request_size"),
        _ => false,
    }
}

/// Which run of an encode this is. A size-targeted encode on the CPU runs
/// twice (see `VideoEncode::two_pass`); everything else runs once.
#[derive(Debug, Clone, Copy)]
pub enum Pass<'a> {
    Only,
    /// Measures the video and writes what it learnt to `log`; produces no file.
    First {
        log: &'a Path,
    },
    /// Encodes using what the first pass wrote to `log`.
    Second {
        log: &'a Path,
    },
}

/// [`convert_args_shifted`] for a part whose audio and video share a clock.
#[cfg(test)]
pub fn convert_args(recipe: &Recipe, input: &Path, output: &Path, pass: Pass) -> Vec<OsString> {
    convert_args_shifted(recipe, input, output, pass, None)
}

/// Builds the ffmpeg arguments that turn the downloaded `input` into the
/// file the recipe describes. Every value is its own argument; nothing is
/// ever passed through a shell. For a cut, `audio_shift` moves the audio
/// that many seconds onto the video's clock (see `smart_cut::clock_shift`).
pub fn convert_args_shifted(
    recipe: &Recipe,
    input: &Path,
    output: &Path,
    pass: Pass,
    audio_shift: Option<f64>,
) -> Vec<OsString> {
    let mut args: Vec<OsString> = Vec::new();
    let mut push = |values: &[&str]| args.extend(values.iter().map(OsString::from));

    push(&["-hide_banner", "-nostdin", "-y", "-loglevel", "error"]);
    // ── Input options ──
    let cut = recipe.cut();
    if cut.is_some() {
        // The section was downloaded with its original timestamps (see
        // `ytdlp::args`), starting at a key frame somewhere before the cut.
        // Keeping those timestamps lets the trim filter find the exact frames.
        push(&["-copyts"]);
    }
    // Machine-readable progress on stdout (see `parse_progress`).
    push(&["-progress", "pipe:1", "-nostats", "-i"]);
    args.push(input.into());
    // The audio read a second time, moved onto the video's clock.
    let audio_shift = audio_shift.filter(|_| cut.is_some());
    if let Some(shift) = audio_shift {
        args.extend(["-itsoffset".into(), shift.to_string().into(), "-i".into()]);
        args.push(input.into());
    }
    let mut push = |values: &[&str]| args.extend(values.iter().map(OsString::from));

    // ── Streams ──
    // First video track, and the first audio track if we keep one. Drops
    // subtitles, thumbnails and data tracks that some containers reject.
    let first_pass = matches!(pass, Pass::First { .. });
    push(&["-map", "0:v:0"]);
    match recipe.audio_codec() {
        Some(_) if !first_pass && audio_shift.is_some() => push(&["-map", "1:a:0?"]),
        Some(_) if !first_pass => push(&["-map", "0:a:0?"]),
        _ => push(&["-an"]),
    }

    if let Some(video) = &recipe.video {
        match &video.action {
            VideoAction::Copy => {
                push(&["-c:v", "copy"]);
                if video.source == VideoCodec::Hevc && matches!(recipe.container, Container::Mp4 | Container::Mov) {
                    push(&["-tag:v", "hvc1"]);
                }
            }
            VideoAction::Encode(encode) => {
                for arg in video_encode_args(video, encode, cut, pass) {
                    push(&[&arg]);
                }
            }
        }
    }

    if let (Some(audio), Some(_), false) = (&recipe.audio, recipe.audio_codec(), first_pass) {
        match audio.action {
            AudioAction::Copy => {
                push(&["-c:a", "copy"]);
                if let Some(cut) = cut {
                    // The downloaded part runs from a key frame before the cut
                    // to a little after it: keep the packets whose middle falls
                    // inside it, picked by their own timestamps.
                    push(&[
                        "-bsf:a",
                        &format!(
                            "noise=drop=lt(pts*tb+duration*tb/2\\,{})+gte(pts*tb+duration*tb/2\\,{})",
                            cut.start, cut.end
                        ),
                    ]);
                }
            }
            AudioAction::Encode {
                codec,
                bitrate_kbps,
                sample_rate,
                stereo,
            } => {
                if let Some(cut) = cut {
                    push(&["-af", &format!("atrim=start={}:end={}", cut.start, cut.end)]);
                }
                for arg in audio_encode_args(codec, bitrate_kbps, sample_rate, stereo) {
                    push(&[&arg]);
                }
            }
        }
    }

    // ── Output ──
    if let Some(cut) = cut {
        // Shift everything back so the clip starts at zero.
        push(&["-output_ts_offset", &format!("-{}", cut.start)]);
    }
    if first_pass {
        push(&["-f", "null", if cfg!(windows) { "NUL" } else { "/dev/null" }]);
        return args;
    }
    if matches!(recipe.container, Container::Mp4 | Container::Mov) {
        // Put the index at the front so playback can start before the whole file is read.
        push(&["-movflags", "+faststart"]);
    }
    args.push(output.into());
    args
}

fn video_encode_args(video: &VideoStep, encode: &VideoEncode, cut: Option<Trim>, pass: Pass) -> Vec<String> {
    let mut args: Vec<String> = Vec::new();
    let mut push = |values: &[&str]| args.extend(values.iter().map(|value| value.to_string()));

    // Filters: downscale (never up) and cap the frame rate.
    let mut filters: Vec<String> = Vec::new();
    if let Some(cut) = cut {
        // Keep exactly the frames inside the selection (times are absolute, thanks to -copyts).
        filters.push(format!("trim=start={}:end={}", cut.start, cut.end));
    }
    if let Some(lines) = encode.scale_lines.filter(|lines| *lines < video.lines) {
        // The short side becomes `lines`; -2 keeps the aspect ratio with an even size.
        let portrait = matches!((video.width, video.height), (Some(w), Some(h)) if w < h);
        filters.push(if portrait {
            format!("scale={lines}:-2")
        } else {
            format!("scale=-2:{lines}")
        });
    }
    let source_fps = video.fps.filter(|fps| *fps > 0.0);
    let output_fps = match (source_fps, encode.max_fps) {
        // Halve rather than resample: 60 → 30 and 50 → 25 drop every other frame cleanly.
        (Some(fps), Some(max)) if fps > max + 0.5 => Some(fps / 2.0),
        _ => None,
    };
    if let Some(fps) = output_fps {
        filters.push(format!("fps={fps}"));
    }
    if !filters.is_empty() {
        push(&["-vf", &filters.join(",")]);
    }
    let fps = output_fps.or(source_fps);
    if encode.constant_frame_rate {
        push(&["-fps_mode", "cfr"]);
        if let Some(fps) = fps {
            push(&["-r", &fps.to_string()]);
        }
    }
    if let (Some(seconds), Some(fps)) = (encode.keyframe_seconds, fps) {
        // Editors scrub from key frame to key frame; one a second keeps it smooth.
        push(&["-g", &((fps * seconds).round().max(1.0) as u32).to_string()]);
    }

    let hardware = encode.hardware;
    push(&["-c:v", hardware.encoder(encode.codec)]);

    if encode.codec == VideoCodec::Prores {
        // ProRes 422 ("standard"), 10-bit 4:2:2, tagged the way Apple's tools expect.
        push(&["-profile:v", "2", "-vendor", "apl0", "-pix_fmt", "yuv422p10le"]);
        return args;
    }

    // 8-bit 4:2:0 is the only format every player accepts; keep 10 bits only for HDR in HEVC.
    let ten_bit = video.hdr && encode.codec == VideoCodec::Hevc;
    let pixel_format = match (ten_bit, hardware) {
        (true, Hardware::Software) => "yuv420p10le",
        (true, _) => "p010le",
        (false, Hardware::Qsv) => "nv12",
        (false, _) => "yuv420p",
    };
    push(&["-pix_fmt", pixel_format]);

    let hevc = encode.codec == VideoCodec::Hevc;
    match (encode.bitrate_kbps, hardware) {
        // ── Size-targeted: hold an average bitrate ──
        (Some(kbps), hardware) => {
            match hardware {
                // NVENC looks at each frame twice within one run.
                Hardware::Nvenc => push(&["-preset", "p6", "-tune", "hq", "-rc", "vbr", "-multipass", "fullres"]),
                Hardware::Amf => push(&["-quality", "quality", "-rc", "vbr_peak"]),
                Hardware::Qsv | Hardware::Software => push(&["-preset", "medium"]),
            }
            push(&[
                "-b:v",
                &format!("{kbps}k"),
                "-maxrate",
                &format!("{}k", kbps * 3 / 2),
                "-bufsize",
                &format!("{}k", kbps * 2),
            ]);
            match pass {
                Pass::Only => {}
                Pass::First { log } => push(&["-pass", "1", "-passlogfile", &log.to_string_lossy()]),
                Pass::Second { log } => push(&["-pass", "2", "-passlogfile", &log.to_string_lossy()]),
            }
        }
        // ── Constant quality: visually close to the source ──
        (None, Hardware::Nvenc) => push(&[
            "-preset",
            "p5",
            "-tune",
            "hq",
            "-rc",
            "vbr",
            "-cq",
            if hevc { "23" } else { "21" },
            "-b:v",
            "0",
        ]),
        (None, Hardware::Qsv) => push(&["-preset", "medium", "-global_quality", if hevc { "23" } else { "21" }]),
        (None, Hardware::Amf) => {
            let (i, p) = if hevc { ("22", "24") } else { ("20", "22") };
            push(&["-quality", "quality", "-rc", "cqp", "-qp_i", i, "-qp_p", p]);
        }
        (None, Hardware::Software) => match encode.codec {
            VideoCodec::Hevc => push(&["-preset", "medium", "-crf", "22"]),
            VideoCodec::Vp9 | VideoCodec::Vp8 => push(&["-crf", "31", "-b:v", "0", "-row-mt", "1", "-cpu-used", "2"]),
            VideoCodec::Av1 => push(&["-preset", "8", "-crf", "30"]),
            _ => push(&["-preset", "medium", "-crf", "19"]),
        },
    }

    match encode.codec {
        // Apple players only recognise HEVC tagged "hvc1".
        VideoCodec::Hevc => push(&["-tag:v", "hvc1"]),
        VideoCodec::H264 => push(&["-profile:v", "high"]),
        _ => {}
    }
    args
}

pub(crate) fn audio_encode_args(
    codec: AudioCodec,
    bitrate_kbps: Option<u32>,
    sample_rate: Option<u32>,
    stereo: bool,
) -> Vec<String> {
    let bitrate = |default: u32| format!("{}k", bitrate_kbps.unwrap_or(default));
    let mut args: Vec<String> = match codec {
        AudioCodec::Mp3 => vec!["-c:a".into(), "libmp3lame".into(), "-b:a".into(), bitrate(192)],
        AudioCodec::Opus => vec!["-c:a".into(), "libopus".into(), "-b:a".into(), bitrate(160)],
        AudioCodec::Vorbis => vec!["-c:a".into(), "libvorbis".into(), "-q:a".into(), "6".into()],
        AudioCodec::Flac => vec!["-c:a".into(), "flac".into()],
        AudioCodec::Pcm => vec!["-c:a".into(), "pcm_s16le".into()],
        AudioCodec::Ac3 => vec!["-c:a".into(), "ac3".into(), "-b:a".into(), bitrate(448)],
        AudioCodec::Aac | AudioCodec::Other => vec!["-c:a".into(), "aac".into(), "-b:a".into(), bitrate(192)],
    };
    if let Some(rate) = sample_rate {
        args.extend(["-ar".into(), rate.to_string()]);
    }
    if stereo {
        args.extend(["-ac".into(), "2".into()]);
    }
    args
}

/// What a live recording is made from: one recorded file, or a list of them
/// (the stream dropped and came back) for ffmpeg's concat demuxer.
#[derive(Debug, Clone, Copy)]
pub enum Recorded<'a> {
    File(&'a Path),
    List(&'a Path),
}

/// The arguments that make a live recording into the file `recipe`
/// describes: the streams copied into the preset's container where it can
/// take them, converted where the preset needs it (see `downloads::live`).
pub fn finalize_args(recipe: &Recipe, input: Recorded, output: &Path) -> Vec<OsString> {
    let mut args: Vec<OsString> = Vec::new();
    let mut push = |values: &[&str]| args.extend(values.iter().map(OsString::from));
    push(&["-hide_banner", "-nostdin", "-y", "-loglevel", "error"]);
    push(&["-progress", "pipe:1", "-nostats"]);
    match input {
        Recorded::File(file) => {
            args.extend(["-i".into(), file.into()]);
        }
        Recorded::List(list) => {
            // `safe 0`: the list names files by their full paths.
            args.extend(["-f", "concat", "-safe", "0", "-i"].map(OsString::from));
            args.push(list.into());
        }
    }
    let mut push = |values: &[&str]| args.extend(values.iter().map(OsString::from));

    let keeps_audio = recipe.audio.is_some() && !recipe.strip_audio;
    match &recipe.video {
        Some(video) => {
            push(&["-map", "0:v:0"]);
            match &video.action {
                VideoAction::Copy => {
                    push(&["-c:v", "copy"]);
                    if video.source == VideoCodec::Hevc && matches!(recipe.container, Container::Mp4 | Container::Mov) {
                        push(&["-tag:v", "hvc1"]);
                    }
                }
                VideoAction::Encode(encode) => {
                    for arg in video_encode_args(video, encode, None, Pass::Only) {
                        push(&[&arg]);
                    }
                }
            }
        }
        None => push(&["-vn"]),
    }
    match (&recipe.audio, keeps_audio) {
        (Some(audio), true) => {
            push(&["-map", "0:a:0?"]);
            match audio.action {
                AudioAction::Copy => push(&["-c:a", "copy"]),
                AudioAction::Encode {
                    codec,
                    bitrate_kbps,
                    sample_rate,
                    stereo,
                } => {
                    for arg in audio_encode_args(codec, bitrate_kbps, sample_rate, stereo) {
                        push(&[&arg]);
                    }
                }
            }
        }
        _ => push(&["-an"]),
    }
    // Subtitles and data tracks of a live stream (timed metadata, ad markers) don't belong in the file.
    push(&["-sn", "-dn"]);
    args.push(output.into());
    args
}

/// The arguments that join the video and audio files a recording from the
/// start left, when Siphon closed before yt-dlp could, into one MPEG-TS file.
pub fn join_parts_args(parts: &[std::path::PathBuf], output: &Path) -> Vec<OsString> {
    let mut args: Vec<OsString> = ["-hide_banner", "-nostdin", "-y", "-loglevel", "error"]
        .map(OsString::from)
        .to_vec();
    for part in parts {
        args.extend(["-i".into(), part.into()]);
    }
    for index in 0..parts.len() {
        args.extend(["-map".into(), format!("{index}").into()]);
    }
    args.extend(["-c", "copy", "-f", "mpegts"].map(OsString::from));
    args.push(output.into());
    args
}

/// ffprobe's view of a file's length and tracks, as JSON (see [`Probed::parse`]).
pub fn probe_args(file: &Path) -> Vec<OsString> {
    let mut args: Vec<OsString> = [
        "-v",
        "error",
        "-show_entries",
        "format=duration:stream=codec_type,start_time,duration",
        "-of",
        "json",
    ]
    .map(OsString::from)
    .to_vec();
    args.push(file.into());
    args
}

/// Decodes the first or the last few seconds of a file and throws them away:
/// it fails if they don't decode.
pub fn decode_check_args(file: &Path, at_end: bool) -> Vec<OsString> {
    let mut args: Vec<OsString> = ["-hide_banner", "-nostdin", "-v", "error", "-xerror"]
        .map(OsString::from)
        .to_vec();
    if at_end {
        args.extend(["-sseof", "-5"].map(OsString::from));
    }
    args.extend(["-i".into(), file.into()]);
    if !at_end {
        args.extend(["-t", "5"].map(OsString::from));
    }
    args.extend(["-f", "null", if cfg!(windows) { "NUL" } else { "/dev/null" }].map(OsString::from));
    args
}

/// A file's length and the span of its first video and audio track, in seconds.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Probed {
    pub duration: Option<f64>,
    /// Start and length of the first video track.
    pub video: Option<(f64, f64)>,
    pub audio: Option<(f64, f64)>,
}

impl Probed {
    /// Reads what [`probe_args`] printed.
    pub fn parse(json: &str) -> Option<Self> {
        let value: serde_json::Value = serde_json::from_str(json).ok()?;
        let number = |value: &serde_json::Value| -> Option<f64> { value.as_str()?.parse().ok() };
        let mut probed = Probed {
            duration: number(&value["format"]["duration"]),
            ..Probed::default()
        };
        for stream in value["streams"].as_array()? {
            let span = number(&stream["start_time"]).zip(number(&stream["duration"]).or(probed.duration));
            match stream["codec_type"].as_str() {
                Some("video") if probed.video.is_none() => probed.video = span.or(Some((0.0, 0.0))),
                Some("audio") if probed.audio.is_none() => probed.audio = span.or(Some((0.0, 0.0))),
                _ => {}
            }
        }
        Some(probed)
    }

    /// Whether this finished file holds what was recorded: the tracks it
    /// should, as long as the recording (`recorded` seconds), with the sound
    /// in step with the picture. Says what is wrong if not.
    pub fn matches(&self, recorded: f64, video: bool, audio: bool) -> std::result::Result<(), String> {
        let length = self.duration.ok_or("it has no length")?;
        // A second either way, and a little more for long recordings: the
        // two ends of a live stream rarely line up to the frame.
        let slack = (recorded * 0.005).max(1.5);
        if (length - recorded).abs() > slack {
            return Err(format!("it is {length:.1} s long, but {recorded:.1} s were recorded"));
        }
        if video != self.video.is_some() {
            return Err(if video {
                "it has no picture"
            } else {
                "it has a picture it shouldn't"
            }
            .into());
        }
        if audio != self.audio.is_some() {
            return Err(if audio {
                "it has no sound"
            } else {
                "it has sound it shouldn't"
            }
            .into());
        }
        if let (Some((video_start, video_length)), Some((audio_start, audio_length))) = (self.video, self.audio) {
            if (video_start - audio_start).abs() > 0.5 {
                return Err(format!(
                    "its sound starts {:.2} s away from its picture",
                    (video_start - audio_start).abs()
                ));
            }
            let video_end = video_start + video_length;
            let audio_end = audio_start + audio_length;
            if (video_end - audio_end).abs() > slack {
                return Err(format!(
                    "its sound ends {:.1} s away from its picture",
                    (video_end - audio_end).abs()
                ));
            }
        }
        Ok(())
    }
}

/// Reads one line of `-progress` output. Returns how far the conversion is,
/// as seconds of finished output.
pub fn parse_progress(line: &str) -> Option<f64> {
    let micros: f64 = line.trim().strip_prefix("out_time_us=")?.parse().ok()?;
    (micros >= 0.0).then_some(micros / 1_000_000.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_progress_lines() {
        assert_eq!(parse_progress("out_time_us=12500000"), Some(12.5));
        assert_eq!(parse_progress("out_time_us=N/A"), None);
        assert_eq!(parse_progress("frame=120"), None);
    }
}

#[cfg(test)]
mod recording_tests {
    use super::*;

    /// What ffprobe printed for a 38 s YouTube live recording (2026-11-19).
    const RECORDED: &str = r#"{"programs": [], "stream_groups": [],
        "streams": [{"codec_type": "video", "start_time": "1.400000", "duration": "38.033333"},
                    {"codec_type": "audio", "start_time": "1.400000", "duration": "37.825289"}],
        "format": {"duration": "38.033333"}}"#;

    #[test]
    fn a_finished_recording_must_be_as_long_as_what_was_recorded_and_in_sync() {
        let probed = Probed::parse(RECORDED).unwrap();
        assert_eq!(probed.video, Some((1.4, 38.033333)));
        assert!(probed.matches(38.0, true, true).is_ok());
        // Too short: something went missing on the way.
        assert!(probed.matches(60.0, true, true).unwrap_err().contains("long"));
        // An audio file should have no picture, and a video should have sound.
        assert!(probed.matches(38.0, false, true).is_err());
        let silent = Probed {
            audio: None,
            ..probed.clone()
        };
        assert!(silent.matches(38.0, true, true).unwrap_err().contains("no sound"));
        // Sound that starts two seconds late is out of step.
        let late = Probed {
            audio: Some((3.4, 36.0)),
            ..probed
        };
        assert!(late.matches(38.0, true, true).unwrap_err().contains("starts"));
    }

    #[test]
    fn a_recording_is_copied_into_its_container_or_its_audio_taken_out() {
        use crate::plan::{AudioStep, Badge, VideoStep};
        let mut recipe = Recipe {
            video: Some(VideoStep {
                format_id: "270".into(),
                source: VideoCodec::H264,
                lines: 1080,
                width: Some(1920),
                height: Some(1080),
                fps: Some(30.0),
                hdr: false,
                size: None,
                kbps: Some(5421.0),
                action: VideoAction::Copy,
            }),
            audio: Some(AudioStep {
                format_id: Some("234".into()),
                source: AudioCodec::Aac,
                source_kbps: Some(128.0),
                size: None,
                action: AudioAction::Copy,
            }),
            container: Container::Mp4,
            strip_audio: false,
            reason: None,
            promise: String::new(),
            hints: Vec::new(),
            duration: None,
            trim: None,
            source_fraction: 1.0,
            badge: Badge::Lossless,
            size_limit: None,
            fits: None,
            blocked: None,
            section_request_size: None,
            fallback: None,
            cut_encode: None,
        };
        let text = |args: Vec<OsString>| {
            args.iter()
                .map(|arg| arg.to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join(" ")
        };
        let copy = text(finalize_args(
            &recipe,
            Recorded::File(Path::new("a.ts")),
            Path::new("a.mp4"),
        ));
        assert!(
            copy.contains("-i a.ts -map 0:v:0 -c:v copy -map 0:a:0? -c:a copy -sn -dn a.mp4"),
            "{copy}"
        );
        let joined = text(finalize_args(
            &recipe,
            Recorded::List(Path::new("parts.txt")),
            Path::new("a.mp4"),
        ));
        assert!(joined.contains("-f concat -safe 0 -i parts.txt"), "{joined}");

        recipe.video = None;
        recipe.container = Container::Mp3;
        if let Some(audio) = recipe.audio.as_mut() {
            audio.action = AudioAction::Encode {
                codec: AudioCodec::Mp3,
                bitrate_kbps: Some(192),
                sample_rate: None,
                stereo: false,
            };
        }
        let music = text(finalize_args(
            &recipe,
            Recorded::File(Path::new("a.ts")),
            Path::new("a.mp3"),
        ));
        assert!(music.contains("-vn -map 0:a:0? -c:a libmp3lame"), "{music}");
    }
}
