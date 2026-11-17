//! Smart cut: a trim on the exact frames that re-encodes only what it must.
//!
//! A stream can only be cut losslessly at a key frame, and a trim rarely
//! starts on one. So the frames from the start of the trim up to the next
//! key frame are re-encoded, everything from that key frame up to the last
//! key frame before the end is copied untouched, and the frames from that
//! last key frame to the end are re-encoded too (they may depend on frames
//! after the end). The three pieces are joined on the video's own
//! timestamps, with the audio of the same span.
//!
//! Re-encoded pieces match the copied stream: codec, profile, level, pixel
//! format, colour tags, size and every frame's timestamp. They have no
//! reordered frames, and their decode times are shifted by the source's
//! reordering delay, so decode order runs on smoothly across the joins.
//!
//! Every result is checked ([`check_video`], [`check_audio`], and a decode
//! of the joins run by the caller). Anything uncertain falls back to
//! re-encoding the whole part, which is what [`Fallback`] reports.
//!
//! Only H.264 and HEVC are cut this way; see docs/decisions.md for VP9 and AV1.

use std::ffi::OsString;
use std::fmt;
use std::path::Path;

use crate::ffmpeg::encoders::Hardware;
use crate::plan::codecs::{Container, VideoCodec};
use crate::plan::Trim;

/// Two timestamps closer than this are the same frame. Containers store
/// times in milliseconds or 1/90000 s, so a frame's time can move by up to
/// a millisecond on its way through.
const SAME_TIME: f64 = 0.0015;

/// How many packets either side of a key frame are checked for frames that
/// are shown on the other side of it (an open GOP).
const REORDER_WINDOW: usize = 32;

/// Re-encoded frames are expected at visually lossless quality.
pub const MIN_EDGE_PSNR: f64 = 38.0;

/// Copied frames checked for being decoded exactly as in the source, right after the first join.
pub const CHECKED_COPIED_FRAMES: usize = 30;

/// Why the smart cut wasn't used. The whole part is re-encoded instead.
#[derive(Debug, Clone, PartialEq)]
pub enum Fallback {
    /// The downloaded part doesn't cover the selection on the video's own timeline.
    NotCovered,
    /// The selection holds no frames.
    NoFrames,
    /// No two key frames inside the selection that a cut can be made at:
    /// there is nothing worth copying.
    TooShort,
    /// The key frames inside the selection belong to open GOPs (frames on one
    /// side depend on frames on the other).
    OpenGop,
    /// Decode times are missing, or the reordering delay differs between the key frames.
    Timing,
    /// The stream is something the encoder can't match (a profile or pixel format).
    Unsupported(String),
    /// A step of the cut failed.
    Failed(String),
    /// The result didn't pass a check.
    Invalid(String),
}

impl fmt::Display for Fallback {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Fallback::NotCovered => write!(f, "the downloaded part doesn't cover the selection"),
            Fallback::NoFrames => write!(f, "the selection holds no frames"),
            Fallback::TooShort => write!(f, "no two key frames inside the selection"),
            Fallback::OpenGop => write!(f, "the key frames inside the selection are open"),
            Fallback::Timing => write!(f, "the decode times can't be lined up"),
            Fallback::Unsupported(what) => write!(f, "unsupported stream: {what}"),
            Fallback::Failed(what) => write!(f, "a step failed: {what}"),
            Fallback::Invalid(what) => write!(f, "check failed: {what}"),
        }
    }
}

// ── Reading the source ──────────────────────────────────────────────────

/// One packet as ffprobe lists it, in decode order.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Packet {
    /// Presentation time, seconds.
    pub pts: f64,
    /// Decode time, seconds, when the container has one.
    pub dts: Option<f64>,
    pub key: bool,
}

/// ffprobe arguments that list a stream's packets into `list`: `pts,dts,flags`
/// per line. `stream` is "v:0" or "a:0". (Into a file: an hour of video is
/// half a million lines, far quicker read at once than line by line.)
pub fn packets_args(input: &Path, stream: &str, list: &Path) -> Vec<OsString> {
    let mut args: Vec<OsString> = [
        "-v",
        "error",
        "-select_streams",
        stream,
        "-show_entries",
        "packet=pts_time,dts_time,flags",
        "-of",
        "csv=p=0",
        "-o",
    ]
    .iter()
    .map(OsString::from)
    .collect();
    args.push(list.into());
    args.push(input.into());
    args
}

/// Parses one line of [`packets_args`] output. Packets without a
/// presentation time are skipped: nothing is shown for them.
pub fn parse_packet(line: &str) -> Option<Packet> {
    let mut fields = line.trim().split(',');
    let pts = fields.next()?.parse::<f64>().ok().filter(|t| t.is_finite())?;
    let dts = fields
        .next()
        .and_then(|t| t.parse::<f64>().ok())
        .filter(|t| t.is_finite());
    let key = fields.next().is_some_and(|flags| flags.starts_with('K'));
    Some(Packet { pts, dts, key })
}

/// What the re-encoded pieces have to match.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct StreamParams {
    pub codec: String,
    pub profile: String,
    /// As ffprobe reports it: 42 for H.264 level 4.2, 153 for HEVC level 5.1.
    pub level: i32,
    pub pix_fmt: String,
    pub color_range: String,
    pub color_space: String,
    pub color_transfer: String,
    pub color_primaries: String,
}

/// ffprobe arguments that describe the first video stream, one `key=value` per line.
pub fn params_args(input: &Path) -> Vec<OsString> {
    let mut args: Vec<OsString> = [
        "-v",
        "error",
        "-select_streams",
        "v:0",
        "-show_entries",
        "stream=codec_name,profile,level,pix_fmt,color_range,color_space,color_transfer,color_primaries",
        "-of",
        "default=noprint_wrappers=1",
    ]
    .iter()
    .map(OsString::from)
    .collect();
    args.push(input.into());
    args
}

pub fn parse_params(text: &str) -> StreamParams {
    let mut params = StreamParams::default();
    for line in text.lines() {
        let Some((key, value)) = line.trim().split_once('=') else {
            continue;
        };
        let value = value.trim().to_string();
        match key {
            "codec_name" => params.codec = value,
            "profile" => params.profile = value,
            "level" => params.level = value.parse().unwrap_or(0),
            "pix_fmt" => params.pix_fmt = value,
            "color_range" => params.color_range = value,
            "color_space" => params.color_space = value,
            "color_transfer" => params.color_transfer = value,
            "color_primaries" => params.color_primaries = value,
            _ => {}
        }
    }
    params
}

/// ffprobe arguments that print the times of a stream's first few packets.
pub fn first_times_args(input: &Path, stream: &str) -> Vec<OsString> {
    let mut args: Vec<OsString> = [
        "-v",
        "error",
        "-select_streams",
        stream,
        "-read_intervals",
        "%+#12",
        "-show_entries",
        "packet=pts_time",
        "-of",
        "csv=p=0",
    ]
    .iter()
    .map(OsString::from)
    .collect();
    args.push(input.into());
    args
}

/// How far to move the audio of a downloaded part onto its video's clock,
/// from the times of each stream's first packets, when the two disagree.
///
/// Both start at the key frame the part was cut from, a fraction of a second
/// apart at most. Some containers (AVI) keep the video on the source's clock
/// and restart the audio at zero; cutting both at the same times would then
/// put the sound in the wrong place.
pub fn clock_shift(video: &[f64], audio: &[f64]) -> Option<f64> {
    // The earliest time, ignoring a stray packet far before the rest (AVI
    // starts its video with one at zero).
    let start = |times: &[f64]| -> Option<f64> {
        let mut sorted: Vec<f64> = times.iter().copied().filter(|t| t.is_finite()).collect();
        sorted.sort_by(f64::total_cmp);
        let middle = *sorted.get(sorted.len() / 2)?;
        sorted.into_iter().find(|t| middle - t < 1.0)
    };
    let shift = start(video)? - start(audio)?;
    (shift.abs() > 2.0).then_some(shift)
}

// ── Deciding the pieces ─────────────────────────────────────────────────

/// How a selection is split up.
#[derive(Debug, Clone, PartialEq)]
pub struct CutPlan {
    /// Presentation time of every frame of the selection, in order.
    pub frames: Vec<f64>,
    /// The key frame copying starts at …
    pub copy_from: f64,
    /// … and the key frame it stops before.
    pub copy_until: f64,
    /// How far decoding runs ahead of presentation in the source, seconds.
    pub delay: f64,
    /// One frame's duration, seconds.
    pub frame: f64,
}

impl CutPlan {
    /// When the first frame of the clip is shown: the clip's zero.
    pub fn first(&self) -> f64 {
        self.frames[0]
    }

    pub fn last(&self) -> f64 {
        *self.frames.last().expect("a plan has frames")
    }

    /// When the clip ends: the end of its last frame.
    pub fn end(&self) -> f64 {
        self.last() + self.frame
    }

    /// How many frames are re-encoded before the first join.
    pub fn head_frames(&self) -> usize {
        self.frames
            .iter()
            .filter(|pts| **pts < self.copy_from - self.frame / 2.0)
            .count()
    }

    /// Whether the clip starts with re-encoded frames (it may start on a key frame).
    pub fn has_head(&self) -> bool {
        self.head_frames() > 0
    }

    /// Half a frame before `t`: a boundary that can't round onto the wrong frame.
    pub fn before(&self, t: f64) -> f64 {
        t - self.frame / 2.0
    }
}

/// Splits `trim` up from the source's video packets (in decode order).
pub fn plan(packets: &[Packet], trim: Trim) -> Result<CutPlan, Fallback> {
    let frame = frame_duration(packets).ok_or(Fallback::NoFrames)?;
    let half = frame / 2.0;
    // The part must hold the whole selection, on the video's own clock: the
    // download starts at a key frame before the start and runs to the end.
    let earliest = packets.iter().map(|p| p.pts).fold(f64::INFINITY, f64::min);
    let latest = packets.iter().map(|p| p.pts).fold(f64::NEG_INFINITY, f64::max);
    if earliest > trim.start + half || latest + frame < trim.end - frame * 1.5 {
        return Err(Fallback::NotCovered);
    }
    let mut frames: Vec<f64> = packets
        .iter()
        .map(|packet| packet.pts)
        .filter(|pts| *pts >= trim.start - SAME_TIME.min(half) && *pts < trim.end - SAME_TIME.min(half))
        .collect();
    frames.sort_by(f64::total_cmp);
    frames.dedup_by(|a, b| (*a - *b).abs() < SAME_TIME);
    if frames.is_empty() {
        return Err(Fallback::NoFrames);
    }

    let first = frames[0];
    let last = *frames.last().expect("not empty");
    let keys: Vec<usize> = (0..packets.len())
        .filter(|&i| packets[i].key && packets[i].pts >= first - half && packets[i].pts <= last + half)
        .collect();
    if keys.len() < 2 {
        return Err(Fallback::TooShort);
    }
    let closed: Vec<usize> = keys.iter().copied().filter(|&i| is_closed(packets, i)).collect();
    let (Some(&from), Some(&until)) = (closed.first(), closed.last()) else {
        return Err(Fallback::OpenGop);
    };
    if packets[until].pts <= packets[from].pts + half {
        return Err(if closed.len() < keys.len() {
            Fallback::OpenGop
        } else {
            Fallback::TooShort
        });
    }

    let delay_at = |i: usize| decode_time(packets, i, frame).map(|dts| packets[i].pts - dts);
    let (Some(delay), Some(delay_end)) = (delay_at(from), delay_at(until)) else {
        return Err(Fallback::Timing);
    };
    if delay < -SAME_TIME || (delay - delay_end).abs() > SAME_TIME {
        return Err(Fallback::Timing);
    }

    Ok(CutPlan {
        frames,
        copy_from: packets[from].pts,
        copy_until: packets[until].pts,
        delay: delay.max(0.0),
        frame,
    })
}

/// The typical gap between frames: the median of the gaps in presentation order.
fn frame_duration(packets: &[Packet]) -> Option<f64> {
    let mut times: Vec<f64> = packets.iter().map(|p| p.pts).collect();
    times.sort_by(f64::total_cmp);
    let mut gaps: Vec<f64> = times
        .windows(2)
        .map(|pair| pair[1] - pair[0])
        .filter(|gap| *gap > SAME_TIME / 2.0)
        .collect();
    if gaps.is_empty() {
        return None;
    }
    gaps.sort_by(f64::total_cmp);
    Some(gaps[gaps.len() / 2])
}

/// True when no frame decoded after key frame `i` is shown before it, and no
/// frame decoded before it is shown after it: the stream can be split there.
fn is_closed(packets: &[Packet], i: usize) -> bool {
    let at = packets[i].pts;
    let after = packets[i + 1..].iter().take(REORDER_WINDOW);
    let before = packets[..i].iter().rev().take(REORDER_WINDOW);
    after.clone().all(|p| p.pts > at - SAME_TIME) && before.clone().all(|p| p.pts < at - SAME_TIME)
}

/// The packet's decode time, or, if the container left it out, the next
/// packet's less one frame (decode times step by a frame).
fn decode_time(packets: &[Packet], i: usize, frame: f64) -> Option<f64> {
    packets[i]
        .dts
        .or_else(|| packets.get(i + 1)?.dts.map(|next| next - frame))
}

// ── Commands ────────────────────────────────────────────────────────────

fn os(values: &[&str]) -> Vec<OsString> {
    values.iter().map(OsString::from).collect()
}

/// The same settings as the source, for the encoder that re-encodes the edges.
pub fn matching_args(params: &StreamParams, hardware: Hardware) -> Result<Vec<String>, Fallback> {
    let codec = match params.codec.as_str() {
        "h264" => VideoCodec::H264,
        "hevc" => VideoCodec::Hevc,
        other => return Err(Fallback::Unsupported(format!("{other} video"))),
    };
    let unsupported = |what: &str| Fallback::Unsupported(format!("{} {what}", params.codec));
    let profile = match (codec, params.profile.as_str()) {
        (VideoCodec::H264, "High") => "high",
        (VideoCodec::H264, "Main") => "main",
        (VideoCodec::H264, "Baseline" | "Constrained Baseline") => "baseline",
        (VideoCodec::Hevc, "Main") => "main",
        (VideoCodec::Hevc, "Main 10") => "main10",
        (_, profile) => return Err(unsupported(&format!("profile {profile:?}"))),
    };
    let ten_bit = match params.pix_fmt.as_str() {
        "yuv420p" | "yuvj420p" => false,
        "yuv420p10le" if codec == VideoCodec::Hevc => true,
        other => return Err(unsupported(&format!("pixel format {other:?}"))),
    };
    let pixel_format = match (ten_bit, hardware) {
        (true, Hardware::Software) => "yuv420p10le",
        (true, _) => "p010le",
        (false, Hardware::Qsv) => "nv12",
        (false, _) => "yuv420p",
    };
    let level = match codec {
        // 42 → "4.2"
        VideoCodec::H264 if params.level > 0 => Some(level_name(f64::from(params.level) / 10.0)),
        // 153 → "5.1"
        VideoCodec::Hevc if params.level > 0 => Some(level_name(f64::from(params.level) / 30.0)),
        _ => None,
    };

    let mut args: Vec<String> = vec!["-c:v".into(), hardware.encoder(codec).into()];
    let mut push = |values: &[&str]| args.extend(values.iter().map(|value| value.to_string()));
    // Visually lossless, no reordered frames, one key frame at the start.
    match (hardware, codec) {
        (Hardware::Nvenc, _) => push(&["-preset", "p5", "-tune", "hq", "-rc", "vbr", "-cq", "17", "-b:v", "0"]),
        (Hardware::Qsv, _) => push(&["-preset", "medium", "-global_quality", "17"]),
        (Hardware::Amf, _) => push(&["-quality", "quality", "-rc", "cqp", "-qp_i", "16", "-qp_p", "16"]),
        (Hardware::Software, VideoCodec::Hevc) => push(&[
            "-preset",
            "fast",
            "-crf",
            "16",
            "-x265-params",
            "bframes=0:log-level=error",
        ]),
        (Hardware::Software, _) => push(&["-preset", "fast", "-crf", "15"]),
    }
    push(&[
        "-bf",
        "0",
        "-g",
        "9999",
        "-profile:v",
        profile,
        "-pix_fmt",
        pixel_format,
    ]);
    if let Some(level) = &level {
        push(&["-level:v", level]);
    }
    for (option, value) in [
        ("-color_range", &params.color_range),
        ("-colorspace", &params.color_space),
        ("-color_trc", &params.color_transfer),
        ("-color_primaries", &params.color_primaries),
    ] {
        let value = if option == "-color_range" && params.pix_fmt == "yuvj420p" {
            "pc"
        } else {
            value.as_str()
        };
        if !value.is_empty() && value != "unknown" {
            push(&[option, value]);
        }
    }
    Ok(args)
}

/// "4.2", or "5" rather than "5.0", the way encoders name levels.
fn level_name(level: f64) -> String {
    let rounded = (level * 10.0).round() / 10.0;
    if rounded.fract() == 0.0 {
        format!("{}", rounded as i64)
    } else {
        format!("{rounded:.1}")
    }
}

/// MPEG-TS that keeps every timestamp as it is: the pieces are joined on them.
const PIECE_OUTPUT: &[&str] = &[
    "-f",
    "mpegts",
    "-mpegts_copyts",
    "1",
    "-muxdelay",
    "0",
    "-muxpreload",
    "0",
];

/// Re-encodes the frames shown in `[from, until)` (source times) with `encoder`
/// (see [`matching_args`]), keeping their timestamps, decode times shifted by `delay`.
pub fn edge_args(source: &Path, from: f64, until: f64, encoder: &[String], delay: f64, output: &Path) -> Vec<OsString> {
    let mut args = os(&["-hide_banner", "-nostdin", "-y", "-loglevel", "error", "-copyts", "-i"]);
    args.push(source.into());
    args.extend(os(&[
        "-map",
        "0:v:0",
        "-an",
        "-sn",
        "-dn",
        "-vf",
        &format!("trim=start={from}:end={until}"),
        // Every frame keeps its own time, in the source's time base.
        "-fps_mode",
        "passthrough",
        "-enc_time_base:v",
        "demux",
    ]));
    args.extend(encoder.iter().map(OsString::from));
    // No reordering here, so decode time equals presentation time; moving it
    // back by the source's delay keeps decode order rising across the joins.
    args.extend(os(&["-bsf:v", &format!("setts=dts=DTS-round({delay}/TB)")]));
    args.extend(os(PIECE_OUTPUT));
    args.push(output.into());
    args
}

/// Copies the video packets shown from `from` (source times) up to `until`,
/// or to the end of the file, untouched.
pub fn copy_args(source: &Path, from: f64, until: Option<f64>, output: &Path) -> Vec<OsString> {
    let mut args = os(&["-hide_banner", "-nostdin", "-y", "-loglevel", "error", "-copyts", "-i"]);
    args.push(source.into());
    // Picked by their own timestamps: no seeking, which lands on the wrong
    // key frame when a file doesn't start at zero.
    let drop = match until {
        Some(until) => format!("noise=drop=lt(pts*tb\\,{from})+gte(pts*tb\\,{until})"),
        None => format!("noise=drop=lt(pts*tb\\,{from})"),
    };
    args.extend(os(&["-map", "0:v:0", "-c", "copy", "-bsf:v", &drop]));
    args.extend(os(PIECE_OUTPUT));
    args.push(output.into());
    args
}

/// How the clip's audio is made.
#[derive(Debug, Clone, PartialEq)]
pub enum AudioPiece {
    /// Copy the packets whose middle falls inside the clip.
    Copy,
    /// Encode exactly the clip's span with these encoder arguments.
    Encode(Vec<String>),
}

/// The audio of `[from, until)` (source times), kept on the source's timestamps.
pub fn audio_args(source: &Path, from: f64, until: f64, piece: &AudioPiece, output: &Path) -> Vec<OsString> {
    let mut args = os(&["-hide_banner", "-nostdin", "-y", "-loglevel", "error", "-copyts", "-i"]);
    args.push(source.into());
    args.extend(os(&["-map", "0:a:0", "-vn", "-sn", "-dn"]));
    match piece {
        AudioPiece::Copy => args.extend(os(&[
            "-c:a",
            "copy",
            "-bsf:a",
            &format!("noise=drop=lt(pts*tb+duration*tb/2\\,{from})+gte(pts*tb+duration*tb/2\\,{until})"),
        ])),
        AudioPiece::Encode(encoder) => {
            args.extend(os(&["-af", &format!("atrim=start={from}:end={until}")]));
            args.extend(encoder.iter().map(OsString::from));
        }
    }
    args.extend(os(&["-f", "matroska"]));
    args.push(output.into());
    args
}

/// Joins the video pieces (file names in the working folder, in order) and
/// the audio, and moves the clip to start at zero.
/// Where an MP4's index goes. It belongs at the front, so playback can
/// start before the whole file is read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Index {
    /// Reserve this many bytes for it at the front, and write the file once.
    Reserve(u64),
    /// Write it at the end, then rewrite the file with it moved to the front:
    /// half a minute more for an hour of video.
    Rewrite,
}

impl Index {
    /// Room for the index of `video` frames and `audio` packets. It takes
    /// about 17 bytes each (6.7 MB for an hour of 60 fps video with AAC);
    /// twice that, and a megabyte, leaves room to spare.
    pub fn room_for(video: usize, audio: usize) -> Index {
        Index::Reserve(1_048_576 + 32 * (video + audio) as u64)
    }
}

pub fn join_args(
    pieces: &[&str],
    audio: Option<&str>,
    start: f64,
    codec: VideoCodec,
    container: Container,
    index: Index,
    output: &Path,
) -> Vec<OsString> {
    let mut args = os(&["-hide_banner", "-nostdin", "-y", "-loglevel", "error", "-copyts", "-i"]);
    // The concat protocol reads the pieces as one stream, timestamps as they are.
    args.push(format!("concat:{}", pieces.join("|")).into());
    if let Some(audio) = audio {
        args.extend(os(&["-i", audio]));
    }
    args.extend(os(&["-map", "0:v:0"]));
    if audio.is_some() {
        args.extend(os(&["-map", "1:a:0"]));
    }
    args.extend(os(&["-c", "copy", "-output_ts_offset", &format!("-{start}")]));
    if matches!(container, Container::Mp4 | Container::Mov) {
        if codec == VideoCodec::Hevc {
            // The pieces carry their own parameter sets in the stream, which
            // "hev1" allows. (Apple's players, which need "hvc1", get a full
            // re-encode instead: see the planner.)
            args.extend(os(&["-tag:v", "hev1"]));
        }
        match index {
            Index::Reserve(bytes) => args.extend(os(&["-moov_size", &bytes.to_string()])),
            Index::Rewrite => args.extend(os(&["-movflags", "+faststart"])),
        }
    }
    args.push(output.into());
    args
}

/// Decodes the start of the clip and the source side by side and compares
/// every frame (`psnr` stats into `stats`): the re-encoded frames and the
/// first copied ones. `-xerror` stops at the first decoding error.
pub fn head_check_args(clip: &Path, source: &Path, plan: &CutPlan, stats: &str) -> Vec<OsString> {
    let checked = plan.head_frames() + CHECKED_COPIED_FRAMES;
    let span_end = plan.frames.get(checked).copied().unwrap_or(plan.end());
    let mut args = os(&[
        "-hide_banner",
        "-nostdin",
        "-loglevel",
        "error",
        "-xerror",
        "-copyts",
        // The clip is put back on the source's clock. ffmpeg reads whichever
        // input is behind, so otherwise it would read the whole clip (from
        // zero) before it got to the source (an hour in, say).
        "-itsoffset",
        &plan.first().to_string(),
        "-i",
    ]);
    args.push(clip.into());
    args.push("-i".into());
    args.push(source.into());
    args.extend(os(&[
        "-lavfi",
        &format!(
            "[0:v]trim=start={from}:end={to},setpts=PTS-STARTPTS[clip];[1:v]trim=start={from}:end={to},setpts=PTS-STARTPTS[source];[clip][source]psnr=stats_file={stats}",
            from = plan.before(plan.first()),
            to = plan.before(span_end),
        ),
        // Stop once they are compared, rather than decode both to the end.
        "-frames:v",
        &checked.min(plan.frames.len()).to_string(),
        "-f",
        "null",
        "-",
    ]));
    args
}

/// Decodes the end of the clip, from a little before the last join.
pub fn tail_check_args(clip: &Path, plan: &CutPlan) -> Vec<OsString> {
    let from = (plan.copy_until - plan.first() - 2.0).max(0.0);
    let mut args = os(&[
        "-hide_banner",
        "-nostdin",
        "-loglevel",
        "error",
        "-xerror",
        "-ss",
        &from.to_string(),
        "-i",
    ]);
    args.push(clip.into());
    args.extend(os(&["-map", "0:v:0", "-f", "null", "-"]));
    args
}

// ── Checking the result ─────────────────────────────────────────────────

/// The clip's video: every frame of the selection, each at its own time
/// (moved to start at zero), and decode times that only go up.
pub fn check_video(plan: &CutPlan, clip: &[Packet]) -> Result<(), Fallback> {
    if clip.len() != plan.frames.len() {
        return Err(Fallback::Invalid(format!(
            "{} frames instead of {}",
            clip.len(),
            plan.frames.len()
        )));
    }
    let mut times: Vec<f64> = clip.iter().map(|p| p.pts).collect();
    times.sort_by(f64::total_cmp);
    for (index, (got, want)) in times.iter().zip(&plan.frames).enumerate() {
        let want = want - plan.first();
        if (got - want).abs() > SAME_TIME {
            return Err(Fallback::Invalid(format!(
                "frame {index} at {got:.4} s instead of {want:.4} s"
            )));
        }
    }
    let decode: Vec<f64> = clip.iter().filter_map(|p| p.dts).collect();
    if decode.len() != clip.len() || decode.windows(2).any(|pair| pair[1] <= pair[0]) {
        return Err(Fallback::Invalid("decode times don't keep rising".into()));
    }
    Ok(())
}

/// The clip's audio against the source's: copied packets each sit at their
/// own time (no drift anywhere), and the audio spans the video.
pub fn check_audio(plan: &CutPlan, source: Option<&[Packet]>, clip: &[Packet]) -> Result<(), Fallback> {
    let (Some(first), Some(last)) = (clip.first(), clip.last()) else {
        return Err(Fallback::Invalid("the clip has no audio".into()));
    };
    let mut gaps: Vec<f64> = clip.windows(2).map(|pair| pair[1].pts - pair[0].pts).collect();
    gaps.sort_by(f64::total_cmp);
    // One audio frame: 21 ms for AAC at 48 kHz.
    let audio_frame = gaps.get(gaps.len() / 2).copied().unwrap_or(0.0).max(0.001);
    // Encoders start a little early (priming), so the start may be up to one frame before zero.
    let span_ok = first.pts > -audio_frame * 1.5
        && first.pts < audio_frame
        && (last.pts + audio_frame - (plan.end() - plan.first())).abs() < audio_frame + plan.frame;
    if !span_ok {
        return Err(Fallback::Invalid(format!(
            "audio runs {:.3}–{:.3} s for video of {:.3} s",
            first.pts,
            last.pts + audio_frame,
            plan.end() - plan.first()
        )));
    }
    if gaps
        .iter()
        .any(|gap| *gap > audio_frame * 1.5 || *gap < audio_frame * 0.5)
    {
        return Err(Fallback::Invalid("the audio has a gap or an overlap".into()));
    }
    let Some(source) = source else { return Ok(()) };
    // Copied: every packet at its source time, start, middle and end alike.
    let mut source_times: Vec<f64> = source.iter().map(|p| p.pts).collect();
    source_times.sort_by(f64::total_cmp);
    for packet in clip {
        let at = packet.pts + plan.first();
        let nearest = match source_times.binary_search_by(|t| t.total_cmp(&at)) {
            Ok(_) => 0.0,
            Err(i) => [i.checked_sub(1), Some(i)]
                .into_iter()
                .flatten()
                .filter_map(|i| source_times.get(i))
                .map(|t| (t - at).abs())
                .fold(f64::INFINITY, f64::min),
        };
        if nearest > SAME_TIME {
            return Err(Fallback::Invalid(format!(
                "audio at {:.4} s is {:.1} ms off its source time",
                packet.pts,
                nearest * 1000.0
            )));
        }
    }
    Ok(())
}

/// Reads the per-frame comparison written by [`head_check_args`]: the
/// re-encoded frames must look the same, and the first copied ones must be
/// identical to the source.
pub fn check_head(plan: &CutPlan, stats: &str) -> Result<(), Fallback> {
    let errors: Vec<f64> = stats
        .lines()
        .filter_map(|line| {
            line.split_whitespace()
                .find_map(|field| field.strip_prefix("mse_avg:"))
                .and_then(|mse| mse.parse::<f64>().ok())
        })
        .collect();
    let head = plan.head_frames();
    let expected = (head + CHECKED_COPIED_FRAMES).min(plan.frames.len());
    if errors.len() != expected {
        return Err(Fallback::Invalid(format!(
            "compared {} frames at the start instead of {expected}",
            errors.len()
        )));
    }
    // PSNR of 8-bit video from the mean squared error.
    let psnr = |mse: f64| {
        if mse <= 0.0 {
            f64::INFINITY
        } else {
            10.0 * (255.0f64.powi(2) / mse).log10()
        }
    };
    if let Some((index, worst)) = errors[..head]
        .iter()
        .map(|mse| psnr(*mse))
        .enumerate()
        .find(|(_, value)| *value < MIN_EDGE_PSNR)
    {
        return Err(Fallback::Invalid(format!("re-encoded frame {index} at {worst:.1} dB")));
    }
    if let Some(index) = errors[head..].iter().position(|mse| *mse > 0.0) {
        return Err(Fallback::Invalid(format!(
            "copied frame {} isn't decoded as in the source",
            head + index
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 60 fps stream from `start` with a key frame every `gop` frames and
    /// B-frames: each P frame is decoded before the two B frames shown ahead
    /// of it (K P B B P B B …), so decoding runs two frames ahead. `open`
    /// makes every GOP after the first open: a frame decoded after its key
    /// frame is shown before it.
    fn stream(start: f64, frames: usize, gop: usize, open: bool) -> Vec<Packet> {
        let f = 1.0 / 60.0;
        let mut shown: Vec<usize> = Vec::new();
        for gop_start in (0..frames).step_by(gop) {
            let gop_end = (gop_start + gop).min(frames);
            shown.push(gop_start);
            let mut next = gop_start + 1;
            while next < gop_end {
                if next + 2 < gop_end {
                    shown.extend([next + 2, next, next + 1]);
                    next += 3;
                } else {
                    shown.push(next);
                    next += 1;
                }
            }
        }
        let mut packets: Vec<Packet> = shown
            .iter()
            .enumerate()
            .map(|(decoded, &n)| Packet {
                pts: start + n as f64 * f,
                dts: Some(start + decoded as f64 * f - 2.0 * f),
                key: n % gop == 0,
            })
            .collect();
        if open {
            for i in 1..packets.len() {
                if packets[i].key && i + 2 < packets.len() {
                    packets[i + 2].pts = packets[i].pts - f / 2.0;
                }
            }
        }
        packets
    }

    fn trim(start: f64, end: f64) -> Trim {
        Trim { start, end }
    }

    #[test]
    fn copies_from_the_first_key_frame_after_the_start_to_the_last_before_the_end() {
        // Key frames every 2 s (120 frames), from 3590 s: at 3590, 3592, …
        let packets = stream(3590.0, 60 * 30, 120, false);
        let plan = plan(&packets, trim(3600.5, 3610.25)).unwrap();
        assert!((plan.copy_from - 3602.0).abs() < 1e-6, "{}", plan.copy_from);
        assert!((plan.copy_until - 3610.0).abs() < 1e-6, "{}", plan.copy_until);
        // 3600.5 … 3610.233: 585 frames, the first 90 before the first key frame.
        assert_eq!(plan.frames.len(), 585);
        assert_eq!(plan.head_frames(), 90);
        assert!((plan.delay - 2.0 / 60.0).abs() < 1e-6);
        assert!((plan.frame - 1.0 / 60.0).abs() < 1e-6);
    }

    #[test]
    fn a_start_on_a_key_frame_needs_no_re_encoded_start() {
        let packets = stream(0.0, 600, 120, false);
        let plan = plan(&packets, trim(2.0, 9.0)).unwrap();
        assert!((plan.copy_from - 2.0).abs() < 1e-6);
        assert!(!plan.has_head());
    }

    #[test]
    fn falls_back_when_nothing_can_be_copied() {
        let packets = stream(0.0, 600, 120, false);
        // Only one key frame (at 2 s) inside the selection.
        assert_eq!(plan(&packets, trim(1.5, 3.5)), Err(Fallback::TooShort));
        // None at all.
        assert_eq!(plan(&packets, trim(0.5, 1.5)), Err(Fallback::TooShort));
    }

    #[test]
    fn falls_back_on_open_gops() {
        let packets = stream(0.0, 600, 120, true);
        assert_eq!(plan(&packets, trim(0.5, 9.5)), Err(Fallback::OpenGop));
    }

    #[test]
    fn falls_back_when_the_part_isnt_on_the_videos_own_clock() {
        // The part starts at zero, though the selection is an hour in: its
        // timestamps were reset, so nothing says where the selection is.
        let packets = stream(0.0, 600, 120, false);
        assert_eq!(plan(&packets, trim(3600.0, 3605.0)), Err(Fallback::NotCovered));
        // Or the part ends well before the selection does.
        assert_eq!(plan(&packets, trim(1.0, 30.0)), Err(Fallback::NotCovered));
    }

    #[test]
    fn falls_back_without_decode_times_or_with_a_changing_delay() {
        let mut packets = stream(0.0, 600, 120, false);
        for packet in &mut packets {
            packet.dts = None;
        }
        assert_eq!(plan(&packets, trim(0.5, 9.5)), Err(Fallback::Timing));

        // Decoding runs further ahead at the last key frame than at the first.
        let mut packets = stream(0.0, 600, 120, false);
        let last_key = packets.iter().rposition(|p| p.key && p.pts < 9.5).unwrap();
        packets[last_key].dts = packets[last_key].dts.map(|dts| dts - 1.0 / 60.0);
        assert_eq!(plan(&packets, trim(0.5, 9.5)), Err(Fallback::Timing));
    }

    #[test]
    fn falls_back_on_an_empty_selection() {
        // Between two frames.
        let packets = stream(0.0, 600, 120, false);
        assert_eq!(plan(&packets, trim(5.004, 5.012)), Err(Fallback::NoFrames));
        assert_eq!(plan(&[], trim(0.0, 1.0)), Err(Fallback::NoFrames));
    }

    #[test]
    fn moves_audio_onto_the_videos_clock_only_when_they_disagree() {
        // A normal part: both start at the key frame, a few milliseconds apart.
        assert_eq!(
            clock_shift(&[3590.466, 3590.516, 3590.483], &[3590.485, 3590.506]),
            None
        );
        // An AVI part: the video keeps the source's clock (after a stray packet
        // at zero), the audio starts again at zero.
        let shift = clock_shift(&[0.0, 56.12491, 56.166577, 56.208243, 56.25], &[0.0, 0.032, 0.064]);
        assert!(shift.is_some_and(|s| (s - 56.12491).abs() < 1e-6), "{shift:?}");
        assert_eq!(clock_shift(&[], &[0.0]), None);
    }

    #[test]
    fn reads_ffprobe_output() {
        assert_eq!(
            parse_packet("3590.465667,3590.432667,K__"),
            Some(Packet {
                pts: 3590.465667,
                dts: Some(3590.432667),
                key: true
            })
        );
        assert_eq!(
            parse_packet("0.050000,N/A,___"),
            Some(Packet {
                pts: 0.05,
                dts: None,
                key: false
            })
        );
        assert_eq!(parse_packet("N/A,1.0,K__"), None);

        let params = parse_params(
            "codec_name=h264\nprofile=High\nlevel=42\npix_fmt=yuv420p\ncolor_range=tv\ncolor_space=bt709\ncolor_transfer=bt709\ncolor_primaries=bt709\n",
        );
        assert_eq!(params.codec, "h264");
        assert_eq!(params.level, 42);
        assert_eq!(params.color_primaries, "bt709");
    }

    fn params(codec: &str, profile: &str, level: i32, pix_fmt: &str) -> StreamParams {
        StreamParams {
            codec: codec.into(),
            profile: profile.into(),
            level,
            pix_fmt: pix_fmt.into(),
            color_range: "tv".into(),
            color_space: "bt709".into(),
            color_transfer: "bt709".into(),
            color_primaries: "bt709".into(),
        }
    }

    fn has(args: &[String], pair: [&str; 2]) -> bool {
        args.windows(2).any(|window| window == pair)
    }

    #[test]
    fn re_encoded_edges_match_the_source() {
        let args = matching_args(&params("h264", "High", 42, "yuv420p"), Hardware::Nvenc).unwrap();
        for pair in [
            ["-c:v", "h264_nvenc"],
            ["-profile:v", "high"],
            ["-level:v", "4.2"],
            ["-pix_fmt", "yuv420p"],
            ["-bf", "0"],
            ["-color_range", "tv"],
            ["-colorspace", "bt709"],
            ["-color_trc", "bt709"],
            ["-color_primaries", "bt709"],
        ] {
            assert!(has(&args, pair), "{pair:?} in {args:?}");
        }

        let hevc = matching_args(&params("hevc", "Main 10", 153, "yuv420p10le"), Hardware::Nvenc).unwrap();
        assert!(has(&hevc, ["-c:v", "hevc_nvenc"]));
        assert!(has(&hevc, ["-profile:v", "main10"]));
        assert!(has(&hevc, ["-level:v", "5.1"]));
        assert!(has(&hevc, ["-pix_fmt", "p010le"]));

        let software = matching_args(&params("h264", "Main", 40, "yuv420p"), Hardware::Software).unwrap();
        assert!(has(&software, ["-c:v", "libx264"]));
        assert!(has(&software, ["-level:v", "4"]));

        // Unknown colour tags are left for the encoder to decide.
        let mut untagged = params("h264", "High", 41, "yuv420p");
        untagged.color_primaries = "unknown".into();
        let args = matching_args(&untagged, Hardware::Nvenc).unwrap();
        assert!(!args.iter().any(|arg| arg == "-color_primaries"));
    }

    #[test]
    fn falls_back_on_streams_the_encoder_cant_match() {
        for (codec, profile, pix_fmt) in [
            ("vp9", "Profile 0", "yuv420p"),
            ("h264", "High 10", "yuv420p10le"),
            ("h264", "High 4:4:4 Predictive", "yuv444p"),
            ("hevc", "Rext", "yuv422p10le"),
        ] {
            assert!(
                matches!(
                    matching_args(&params(codec, profile, 41, pix_fmt), Hardware::Nvenc),
                    Err(Fallback::Unsupported(_))
                ),
                "{codec} {profile}"
            );
        }
    }

    fn clip_from(plan: &CutPlan) -> Vec<Packet> {
        plan.frames
            .iter()
            .enumerate()
            .map(|(i, pts)| Packet {
                pts: pts - plan.first(),
                dts: Some(i as f64 * plan.frame - plan.delay),
                key: i == 0,
            })
            .collect()
    }

    fn a_plan() -> CutPlan {
        plan(&stream(100.0, 60 * 20, 120, false), trim(102.5, 112.3)).unwrap()
    }

    #[test]
    fn checks_every_frame_of_the_clip() {
        let plan = a_plan();
        assert_eq!(check_video(&plan, &clip_from(&plan)), Ok(()));

        let mut missing = clip_from(&plan);
        missing.remove(40);
        assert!(matches!(check_video(&plan, &missing), Err(Fallback::Invalid(_))));

        // A frame off by two milliseconds.
        let mut shifted = clip_from(&plan);
        shifted[200].pts += 0.002;
        assert!(matches!(check_video(&plan, &shifted), Err(Fallback::Invalid(_))));

        // Decode order that steps back at a join.
        let mut backwards = clip_from(&plan);
        backwards[plan.head_frames()].dts = Some(-1.0);
        assert!(matches!(check_video(&plan, &backwards), Err(Fallback::Invalid(_))));

        // A whole key frame's worth of frames too many: the off-by-one-key-frame mistake.
        let mut extra = clip_from(&plan);
        for i in 0..120 {
            extra.push(Packet {
                pts: 20.0 + i as f64 / 60.0,
                dts: Some(20.0 + i as f64 / 60.0),
                key: false,
            });
        }
        assert!(matches!(check_video(&plan, &extra), Err(Fallback::Invalid(_))));
    }

    fn audio(from: f64, to: f64) -> Vec<Packet> {
        let frame = 1024.0 / 48000.0;
        (0..)
            .map(|i| from + i as f64 * frame)
            .take_while(|t| *t < to)
            .map(|pts| Packet {
                pts,
                dts: Some(pts),
                key: true,
            })
            .collect()
    }

    #[test]
    fn checks_the_audio_against_the_source() {
        let plan = a_plan();
        let source = audio(99.99, 125.0);
        let in_clip: Vec<Packet> = source
            .iter()
            .filter(|p| p.pts + 0.0107 >= plan.first() && p.pts + 0.0107 < plan.end())
            .map(|p| Packet {
                pts: p.pts - plan.first(),
                ..*p
            })
            .collect();
        assert_eq!(check_audio(&plan, Some(&source), &in_clip), Ok(()));

        // Drifted by 5 ms in the middle.
        let mut drifted = in_clip.clone();
        let middle = drifted.len() / 2;
        for packet in &mut drifted[middle..] {
            packet.pts += 0.005;
        }
        assert!(matches!(
            check_audio(&plan, Some(&source), &drifted),
            Err(Fallback::Invalid(_))
        ));

        // Starting late, or stopping early.
        assert!(matches!(
            check_audio(&plan, Some(&source), &in_clip[3..]),
            Err(Fallback::Invalid(_))
        ));
        assert!(matches!(
            check_audio(&plan, Some(&source), &in_clip[..in_clip.len() - 3]),
            Err(Fallback::Invalid(_))
        ));
        // No audio at all.
        assert!(matches!(
            check_audio(&plan, Some(&source), &[]),
            Err(Fallback::Invalid(_))
        ));
    }

    #[test]
    fn checks_the_frames_at_the_first_join() {
        let plan = a_plan();
        let head = plan.head_frames();
        let line = |mse: f64| format!("n:1 mse_avg:{mse:.2} mse_y:0 psnr_avg:0\n");
        let good: String = (0..head)
            .map(|_| line(0.5))
            .chain((0..CHECKED_COPIED_FRAMES).map(|_| line(0.0)))
            .collect();
        assert_eq!(check_head(&plan, &good), Ok(()));

        // A blurry re-encoded frame (30 dB).
        let blurry: String = (0..head)
            .map(|i| line(if i == 3 { 65.0 } else { 0.5 }))
            .chain((0..CHECKED_COPIED_FRAMES).map(|_| line(0.0)))
            .collect();
        assert!(matches!(check_head(&plan, &blurry), Err(Fallback::Invalid(_))));

        // A copied frame decoded differently: the join broke it.
        let broken: String = (0..head)
            .map(|_| line(0.5))
            .chain((0..CHECKED_COPIED_FRAMES).map(|i| line(if i == 0 { 3.0 } else { 0.0 })))
            .collect();
        assert!(matches!(check_head(&plan, &broken), Err(Fallback::Invalid(_))));

        // Fewer frames compared than expected.
        assert!(matches!(check_head(&plan, &line(0.0)), Err(Fallback::Invalid(_))));
    }

    #[test]
    fn commands_pick_packets_by_their_own_times() {
        let middle = copy_args(Path::new("src.mp4"), 3601.99, Some(3609.99), Path::new("mid.ts"));
        let middle: Vec<String> = middle.iter().map(|a| a.to_string_lossy().into_owned()).collect();
        assert!(middle
            .iter()
            .any(|a| a == "noise=drop=lt(pts*tb\\,3601.99)+gte(pts*tb\\,3609.99)"));
        assert!(!middle.iter().any(|a| a == "-ss"));
        assert!(middle.windows(2).any(|w| w == ["-mpegts_copyts", "1"]));
        let rest = copy_args(Path::new("src.mp4"), 3609.99, None, Path::new("rest.ts"));
        assert!(rest.iter().any(|a| a == "noise=drop=lt(pts*tb\\,3609.99)"));

        let join = join_args(
            &["head.ts", "mid.ts", "tail.ts"],
            Some("audio.mka"),
            3600.5,
            VideoCodec::H264,
            Container::Mp4,
            Index::room_for(216_000, 168_750),
            Path::new("out.mp4"),
        );
        let join: Vec<String> = join.iter().map(|a| a.to_string_lossy().into_owned()).collect();
        assert!(join.iter().any(|a| a == "concat:head.ts|mid.ts|tail.ts"));
        assert!(join.windows(2).any(|w| w == ["-output_ts_offset", "-3600.5"]));
        // 6.7 MB was needed for this hour; twice that is reserved.
        assert!(join.windows(2).any(|w| w == ["-moov_size", "13360576"]));
        assert!(!join.iter().any(|a| a == "+faststart"));
        assert!(!join.iter().any(|a| a == "hev1"));
        let hevc = join_args(
            &["mid.ts", "tail.ts"],
            None,
            1.0,
            VideoCodec::Hevc,
            Container::Mp4,
            Index::Rewrite,
            Path::new("out.mp4"),
        );
        assert!(hevc.windows(2).any(|w| w[0] == "-tag:v" && w[1] == "hev1"));
        assert!(hevc.windows(2).any(|w| w[0] == "-movflags" && w[1] == "+faststart"));
    }
}
