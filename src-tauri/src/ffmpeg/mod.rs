//! Siphon's own ffmpeg pass: converting video and audio after the download.
//!
//! yt-dlp handles lossless work (merging, remuxing). Conversions run here
//! instead, because that is the only way to get hardware encoding and a
//! progress readout.

pub mod encoders;

use std::ffi::OsString;
use std::path::Path;
use std::time::Duration;

use crate::plan::codecs::{AudioCodec, Container, VideoCodec};
use crate::plan::{AudioAction, Recipe, Trim, VideoAction, VideoEncode, VideoStep};
use crate::process;
use encoders::{Encoders, Hardware};

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

/// Builds the ffmpeg arguments that turn the downloaded `input` into the
/// file the recipe describes. Every value is its own argument; nothing is
/// ever passed through a shell.
pub fn convert_args(recipe: &Recipe, encoders: &Encoders, input: &Path, output: &Path) -> Vec<OsString> {
    let mut args: Vec<OsString> = Vec::new();
    let mut push = |values: &[&str]| args.extend(values.iter().map(OsString::from));

    push(&["-hide_banner", "-nostdin", "-y", "-loglevel", "error"]);
    let cut = recipe.exact_cut();
    if cut.is_some() {
        // The section was downloaded with its original timestamps (see
        // `ytdlp::args`), starting at a key frame somewhere before the cut.
        // Keeping those timestamps lets the trim filter find the exact frames.
        push(&["-copyts"]);
    }
    // Machine-readable progress on stdout (see `parse_progress`).
    push(&["-progress", "pipe:1", "-nostats", "-i"]);
    args.push(input.into());
    let mut push = |values: &[&str]| args.extend(values.iter().map(OsString::from));

    // First video track, and the first audio track if we keep one. Drops
    // subtitles, thumbnails and data tracks that some containers reject.
    push(&["-map", "0:v:0"]);
    match recipe.audio_codec() {
        Some(_) => push(&["-map", "0:a:0?"]),
        None => push(&["-an"]),
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
                let hardware = encoders.for_codec(encode.codec);
                for arg in video_encode_args(video, encode, hardware, cut) {
                    push(&[&arg]);
                }
            }
        }
    }

    if let (Some(audio), Some(_)) = (&recipe.audio, recipe.audio_codec()) {
        match audio.action {
            AudioAction::Copy => push(&["-c:a", "copy"]),
            AudioAction::Encode { codec, bitrate_kbps } => {
                for arg in audio_encode_args(codec, bitrate_kbps) {
                    push(&[&arg]);
                }
            }
        }
    }

    if let Some(cut) = cut {
        // Shift everything back so the clip starts at zero.
        push(&["-output_ts_offset", &format!("-{}", cut.start)]);
    }
    if matches!(recipe.container, Container::Mp4 | Container::Mov) {
        // Put the index at the front so playback can start before the whole file is read.
        push(&["-movflags", "+faststart"]);
    }
    args.push(output.into());
    args
}

fn video_encode_args(video: &VideoStep, encode: &VideoEncode, hardware: Hardware, cut: Option<Trim>) -> Vec<String> {
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
    if encode.constant_frame_rate {
        push(&["-fps_mode", "cfr"]);
        if let Some(fps) = output_fps.or(source_fps) {
            push(&["-r", &fps.to_string()]);
        }
    }

    let encoder = hardware.encoder(encode.codec);
    push(&["-c:v", encoder]);

    if encode.codec == VideoCodec::Dnxhr {
        push(&["-profile:v", "dnxhr_hq", "-pix_fmt", "yuv422p"]);
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
        // ── Size-limited: hold an average bitrate ──
        (Some(kbps), hardware) => {
            let rate = format!("{kbps}k");
            let buffer = format!("{}k", kbps * 2);
            match hardware {
                Hardware::Nvenc => push(&["-preset", "p5", "-tune", "hq", "-rc", "vbr"]),
                Hardware::Amf => push(&["-quality", "quality", "-rc", "vbr_peak"]),
                Hardware::Qsv | Hardware::Software => push(&["-preset", "medium"]),
                Hardware::VideoToolbox => {}
            }
            push(&["-b:v", &rate, "-maxrate", &rate, "-bufsize", &buffer]);
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
        (None, Hardware::VideoToolbox) => push(&["-q:v", if hevc { "60" } else { "65" }]),
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

fn audio_encode_args(codec: AudioCodec, bitrate_kbps: Option<u32>) -> Vec<String> {
    let bitrate = |default: u32| format!("{}k", bitrate_kbps.unwrap_or(default));
    let args: Vec<String> = match codec {
        AudioCodec::Mp3 => vec!["-c:a".into(), "libmp3lame".into(), "-b:a".into(), bitrate(192)],
        AudioCodec::Opus => vec!["-c:a".into(), "libopus".into(), "-b:a".into(), bitrate(160)],
        AudioCodec::Vorbis => vec!["-c:a".into(), "libvorbis".into(), "-q:a".into(), "6".into()],
        AudioCodec::Flac => vec!["-c:a".into(), "flac".into()],
        AudioCodec::Pcm => vec!["-c:a".into(), "pcm_s16le".into()],
        AudioCodec::Ac3 => vec!["-c:a".into(), "ac3".into(), "-b:a".into(), bitrate(448)],
        AudioCodec::Aac | AudioCodec::Other => vec!["-c:a".into(), "aac".into(), "-b:a".into(), bitrate(192)],
    };
    args
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
