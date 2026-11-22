//! Reads a stretch of a stream straight from the site, and encodes its
//! picture the way a download would: what the stretch holds, and what it
//! comes to encoded, packet by packet. One ffmpeg run, two outputs, each
//! listing its packets (`framecrc`): the stream copied, on standard output,
//! and the picture encoded, on standard error.

use std::ffi::OsString;
use std::path::Path;
use std::time::Duration;

use crate::process;
use crate::throttle::Flow;
use crate::ytdlp::metadata::Format;

use super::parts::{header_args, READ_TIMEOUT_US};

/// How long one stretch may take, read and encoded.
const LIMIT: Duration = Duration::from_secs(90);

/// A packet: when it is shown (seconds, on the stream's own clock from its
/// start), its bytes, and whether it is a key frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Packet {
    pub pts: f64,
    pub size: u64,
    pub key: bool,
}

/// What a stretch of the stream holds, and its picture encoded.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Stretch {
    pub video: Vec<Packet>,
    /// The sound, for a stream that carries it.
    pub audio: Vec<Packet>,
    /// Empty when nothing was encoded.
    pub encoded: Vec<Packet>,
}

/// The bytes of the packets shown from `from` up to `until`.
pub fn bytes(packets: &[Packet], from: f64, until: f64) -> u64 {
    // A frame's time is rounded to its time base: a hair's margin.
    const EDGE: f64 = 0.0005;
    packets
        .iter()
        .filter(|packet| packet.pts >= from - EDGE && packet.pts < until - EDGE)
        .map(|packet| packet.size)
        .sum()
}

/// How the picture is encoded: the filters before it and the encoder's arguments.
pub struct Encoding<'a> {
    pub filters: &'a [String],
    pub encoder: &'a [String],
}

/// Reads `format` from `from` to `until` seconds and, with `encoding`,
/// encodes its picture. `chunk`: the request size the site expects (YouTube's
/// 10 MB; a larger request comes at about twice playback speed). A read the
/// site refused is tried once more a moment later: YouTube now and then
/// refuses one (HTTP 403) that goes through the next time. The error is
/// ffmpeg's, for the log.
#[allow(clippy::too_many_arguments)]
pub async fn read(
    ffmpeg: &Path,
    flow: &Flow,
    format: &Format,
    chunk: Option<u64>,
    from: f64,
    until: f64,
    encoding: Option<&Encoding<'_>>,
) -> Result<Stretch, String> {
    match read_once(ffmpeg, flow, format, chunk, from, until, encoding).await {
        Ok(stretch) => Ok(stretch),
        Err(_) => {
            tokio::time::sleep(Duration::from_secs(1)).await;
            read_once(ffmpeg, flow, format, chunk, from, until, encoding).await
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn read_once(
    ffmpeg: &Path,
    flow: &Flow,
    format: &Format,
    chunk: Option<u64>,
    from: f64,
    until: f64,
    encoding: Option<&Encoding<'_>>,
) -> Result<Stretch, String> {
    let url = format.stream_url.as_deref().ok_or("no link to the stream")?;
    let mut cmd = process::command(ffmpeg);
    cmd.args(args(format, url, chunk, from, until, encoding));
    flow.route_ffmpeg(&mut cmd);
    let output = process::output_within(cmd, LIMIT)
        .await
        .map_err(|err| err.to_string())?
        .ok_or("took too long")?;
    // ffmpeg's errors share standard error with the encoded packets' list.
    let errors = String::from_utf8_lossy(&output.stderr);
    let error = || {
        errors
            .lines()
            .rfind(|line| {
                !line.starts_with('#') && !line.starts_with(|c: char| c.is_ascii_digit()) && !line.trim().is_empty()
            })
            .unwrap_or("no packets")
            .trim()
            .to_string()
    };
    if !output.status.success() {
        return Err(error());
    }
    let source = listed(&String::from_utf8_lossy(&output.stdout));
    let encoded = listed(&errors);
    let stretch = Stretch {
        video: source.video,
        audio: source.audio,
        encoded: encoded.video,
    };
    let encoded_enough = encoding.is_none() || !stretch.encoded.is_empty();
    if stretch.video.is_empty() || !encoded_enough {
        return Err(error());
    }
    Ok(stretch)
}

fn args(
    format: &Format,
    url: &str,
    chunk: Option<u64>,
    from: f64,
    until: f64,
    encoding: Option<&Encoding<'_>>,
) -> Vec<OsString> {
    let mut args: Vec<OsString> = [
        "-hide_banner",
        "-nostdin",
        "-nostats",
        "-v",
        "error",
        "-rw_timeout",
        READ_TIMEOUT_US,
    ]
    .map(OsString::from)
    .to_vec();
    args.extend(header_args(format));
    if let Some(size) = chunk.filter(|_| !format.segmented) {
        args.extend(["-request_size".into(), size.to_string().into()]);
    }
    // The stream's own times, counted from its start (Twitch's start about a
    // minute in), as a trimmed download keeps them (see `ytdlp::args`).
    let length = format!("{:.3}", until - from + 1.0);
    args.extend(
        [
            "-copyts",
            "-start_at_zero",
            "-ss",
            &format!("{from:.3}"),
            "-t",
            &length,
            "-i",
        ]
        .map(OsString::from),
    );
    args.push(url.into());
    // What the stretch holds, as it is.
    args.extend(
        [
            "-map", "0:v:0", "-map", "0:a:0?", "-c", "copy", "-f", "framecrc", "pipe:1",
        ]
        .map(OsString::from),
    );
    if let Some(encoding) = encoding {
        args.extend(["-map", "0:v:0"].map(OsString::from));
        if !encoding.filters.is_empty() {
            args.extend(["-vf".into(), encoding.filters.join(",").into()]);
        }
        args.extend(encoding.encoder.iter().map(OsString::from));
        args.extend(["-f", "framecrc", "pipe:2"].map(OsString::from));
    }
    args
}

/// The times of the key frames between `from` and `until` that decoding
/// restarts at (H.264 IDR frames; HEVC IDR and CRA frames), which is where a
/// smart cut can start copying: YouTube marks other key frames too. Only the
/// key frames are read past, each one's headers traced (see
/// `smart_cut::parse_restarts`); their times come out on the stream's own
/// clock, its base read off the packet list.
pub async fn restarts(
    ffmpeg: &Path,
    flow: &Flow,
    format: &Format,
    chunk: Option<u64>,
    from: f64,
    until: f64,
    codec: crate::plan::codecs::VideoCodec,
) -> Option<Vec<f64>> {
    let url = format.stream_url.as_deref()?;
    let mut cmd = process::command(ffmpeg);
    cmd.args(restart_args(format, url, chunk, from, until));
    flow.route_ffmpeg(&mut cmd);
    let output = process::output_within(cmd, LIMIT).await.ok()??;
    if !output.status.success() {
        return None;
    }
    let listed = String::from_utf8_lossy(&output.stdout);
    let base = listed.lines().find_map(|line| {
        let (num, den) = line.strip_prefix("#tb 0:")?.trim().split_once('/')?;
        let (num, den) = (num.trim().parse::<f64>().ok()?, den.trim().parse::<f64>().ok()?);
        (den > 0.0).then_some(num / den)
    })?;
    let trace = String::from_utf8_lossy(&output.stderr);
    Some(crate::ffmpeg::smart_cut::parse_restarts(&trace, codec, base))
}

fn restart_args(format: &Format, url: &str, chunk: Option<u64>, from: f64, until: f64) -> Vec<OsString> {
    // The headers' trace is logged at the "info" level.
    let mut args: Vec<OsString> = [
        "-hide_banner",
        "-nostdin",
        "-nostats",
        "-v",
        "info",
        "-rw_timeout",
        READ_TIMEOUT_US,
    ]
    .map(OsString::from)
    .to_vec();
    args.extend(header_args(format));
    if let Some(size) = chunk.filter(|_| !format.segmented) {
        args.extend(["-request_size".into(), size.to_string().into()]);
    }
    let length = format!("{:.3}", until - from);
    args.extend(
        [
            "-copyts",
            "-start_at_zero",
            "-ss",
            &format!("{from:.3}"),
            "-t",
            &length,
            "-i",
        ]
        .map(OsString::from),
    );
    args.push(url.into());
    args.extend(
        [
            "-map",
            "0:v:0",
            "-c",
            "copy",
            "-bsf:v",
            "noise=drop=not(key),trace_headers",
            "-f",
            "framecrc",
            "pipe:1",
        ]
        .map(OsString::from),
    );
    args
}

/// The packets `framecrc` listed, by kind. Each stream's time base and kind
/// come first (`#tb 0: 1/1000`, `#media_type 0: video`); then a line per
/// packet: stream, decode time, presentation time, duration, size, checksum,
/// and `F=0x…` for flags other than a plain key frame.
fn listed(text: &str) -> Stretch {
    let mut bases: Vec<(usize, f64)> = Vec::new();
    let mut kinds: Vec<(usize, String)> = Vec::new();
    let mut stretch = Stretch::default();
    for line in text.lines().map(str::trim) {
        if let Some(rest) = line.strip_prefix("#tb ") {
            if let Some((stream, base)) = rest.split_once(':') {
                let base = base.trim().split_once('/').and_then(|(num, den)| {
                    let (num, den) = (num.trim().parse::<f64>().ok()?, den.trim().parse::<f64>().ok()?);
                    (den > 0.0).then_some(num / den)
                });
                if let (Ok(stream), Some(base)) = (stream.trim().parse(), base) {
                    bases.push((stream, base));
                }
            }
            continue;
        }
        if let Some(rest) = line.strip_prefix("#media_type ") {
            if let Some((stream, kind)) = rest.split_once(':') {
                if let Ok(stream) = stream.trim().parse() {
                    kinds.push((stream, kind.trim().to_string()));
                }
            }
            continue;
        }
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        let fields: Vec<&str> = line.split(',').map(str::trim).collect();
        let (Some(stream), Some(pts), Some(size)) = (
            fields.first().and_then(|value| value.parse::<usize>().ok()),
            fields.get(2).and_then(|value| value.parse::<i64>().ok()),
            fields.get(4).and_then(|value| value.parse::<u64>().ok()),
        ) else {
            continue;
        };
        let Some(base) = bases.iter().find(|(index, _)| *index == stream).map(|(_, base)| *base) else {
            continue;
        };
        let flags = fields
            .iter()
            .find_map(|field| field.strip_prefix("F=0x"))
            .and_then(|hex| u32::from_str_radix(hex, 16).ok());
        let packet = Packet {
            pts: pts as f64 * base,
            size,
            key: flags.is_none_or(|flags| flags & 1 == 1),
        };
        let kind = kinds
            .iter()
            .find(|(index, _)| *index == stream)
            .map(|(_, kind)| kind.as_str());
        match kind {
            Some("audio") => stretch.audio.push(packet),
            _ => stretch.video.push(packet),
        }
    }
    stretch
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_what_framecrc_lists() {
        // As ffmpeg 8 lists a copied YouTube stretch (2026-11-22), shortened.
        let text = "#software: Lavf62.3.100\n#tb 0: 1/1000\n#media_type 0: video\n#codec_id 0: h264\n\
            #dimensions 0: 1920x1080\n#sar 0: 1/1\n#tb 1: 1/44100\n#media_type 1: audio\n#codec_id 1: aac\n\
            0,     119952,     120019,       16,    85211, 0x5a3c1f22\n\
            0,     119969,     120036,       16,     3311, 0x9bd0a1e3, F=0x0\n\
            1,    5292000,    5292000,     1024,      372, 0x0ac5bf1d\n\
            0,     119986,     120053,       16,     2210, 0x1e2d3c4b, F=0x0, S=1,        8\n";
        let stretch = listed(text);
        assert_eq!(stretch.video.len(), 3);
        assert!(stretch.video[0].key && !stretch.video[1].key);
        assert!((stretch.video[0].pts - 120.019).abs() < 1e-9);
        assert_eq!(
            stretch.audio,
            [Packet {
                pts: 120.0,
                size: 372,
                key: true
            }]
        );
        assert_eq!(bytes(&stretch.video, 120.0, 120.05), 85211 + 3311);
    }

    #[test]
    fn copies_the_stretch_and_encodes_its_picture() {
        let format = Format {
            stream_url: Some("https://rr1.googlevideo.com/videoplayback?id=1".into()),
            ..super::super::test_format("137")
        };
        let encoder = ["-c:v".to_string(), "h264_nvenc".to_string()];
        let filters = ["fps=30".to_string()];
        let encoding = Encoding {
            filters: &filters,
            encoder: &encoder,
        };
        let args: Vec<String> = args(&format, "https://x/v", Some(10_485_760), 120.0, 124.0, Some(&encoding))
            .into_iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        let joined = args.join(" ");
        assert!(joined.contains("-request_size 10485760 -copyts -start_at_zero -ss 120.000 -t 5.000 -i https://x/v"));
        assert!(joined.contains("-map 0:v:0 -map 0:a:0? -c copy -f framecrc pipe:1"));
        assert!(joined.ends_with("-map 0:v:0 -vf fps=30 -c:v h264_nvenc -f framecrc pipe:2"));
    }
}
