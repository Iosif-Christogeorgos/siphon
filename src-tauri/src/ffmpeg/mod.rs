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
use crate::plan::subtitles::{Delivery, SubtitleStep};
use crate::plan::{AudioAction, Recipe, Trim, VideoAction, VideoEncode, VideoStep};
use crate::process;
use crate::ytdlp::subtitles::iso639_2;
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

/// [`convert_args_shifted`] for a part whose audio and video share a clock, without subtitles.
#[cfg(test)]
pub fn convert_args(recipe: &Recipe, input: &Path, output: &Path, pass: Pass) -> Vec<OsString> {
    convert_args_shifted(recipe, input, output, pass, None, None, None)
}

/// The chat layer Siphon feeds ffmpeg on its standard input for a recipe
/// with chat (see `crate::chat::render`): raw RGBA frames of the box, `fps`
/// a second, laid over the picture with the box's top left at `x`, `y`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ChatLayer {
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    pub x: u32,
    pub y: u32,
    /// Where on the video's clock the layer's first frame goes.
    pub start: f64,
    /// High definition video, whose colours are BT.709 rather than BT.601.
    pub hd: bool,
    /// Where the picture ends on its clock, when that is known exactly (a
    /// clip, a piece of a recording): the output is cut there. The overlay
    /// goes on for as long as either of its inputs does, so a layer that
    /// lasted longer than the picture added the picture's last frame, frozen
    /// and silent, until the layer ended (found 2026-11-21: a 30 s clip came
    /// out with 31 s of picture).
    pub end: Option<f64>,
    /// The output's frame rate, made first in the graph, before the
    /// picture's own filters (a piece's cut among them) and the cut at
    /// `end`, so that both cuts see the same timestamps and are exact.
    /// Made after the graph (`-r`), it added a frame or two past the cut,
    /// and pieces drawn as a stream records overlapped, which lost the next
    /// piece's first frames when they were joined (found 2026-11-22). Made
    /// after the overlay, it moved each frame onto the rate's grid there,
    /// which for a Twitch recording from the start (its frames on whole
    /// milliseconds, just before the grid's) put each piece's last frame
    /// on the cut, and one frame went missing at every join (found
    /// 2026-11-22, 46 in 96 minutes). `None`: as the output's options say.
    pub rate: Option<f64>,
}

impl ChatLayer {
    /// The input that reads the frames from Siphon.
    fn input_args(&self) -> Vec<String> {
        let size = format!("{}x{}", self.width, self.height);
        let rate = format!("{:.6}", self.fps);
        [
            "-f",
            "rawvideo",
            "-pix_fmt",
            "rgba",
            "-s",
            size.as_str(),
            "-framerate",
            rate.as_str(),
            "-i",
            "pipe:0",
        ]
        .map(str::to_string)
        .to_vec()
    }

    /// The filter graph that draws the layer (input `input`) over the
    /// picture after the picture's own `filters`, as the output `[v]`.
    fn graph(&self, filters: &[String], input: usize) -> String {
        let rate = self.rate.map(|fps| format!("fps={}", frame_rate(fps)));
        let base: Vec<&str> = rate
            .iter()
            .map(String::as_str)
            .chain(filters.iter().map(String::as_str))
            .collect();
        let base = if base.is_empty() {
            "null".to_string()
        } else {
            base.join(",")
        };
        let matrix = if self.hd { "bt709" } else { "smpte170m" };
        let chat = format!(
            "setpts=PTS-STARTPTS+{}/TB,scale=out_color_matrix={matrix}:out_range=tv,format=yuva420p",
            self.start
        );
        let overlay = format!("overlay=x={}:y={}:format=yuv420:eof_action=repeat", self.x, self.y);
        let cut = self.end.map(|end| format!(",trim=end={end:.6}")).unwrap_or_default();
        format!("[0:v]{base}[base];[{input}:v]{chat}[chat];[base][chat]{overlay}{cut}[v]")
    }
}

/// Builds the ffmpeg arguments that turn the downloaded `input` into the
/// file the recipe describes. Every value is its own argument; nothing is
/// ever passed through a shell. For a cut, `audio_shift` moves the audio
/// that many seconds onto the video's clock (see `smart_cut::clock_shift`).
/// `subtitles` is the recipe's subtitle file, prepared on the video's own
/// clock (see `crate::srt`): a cut shifts it with the picture. A file to be
/// drawn into the picture is read from the folder ffmpeg runs in, by its
/// name: a filter's arguments would need a Windows path escaped twice over.
/// `chat` is the chat layer, for a recipe that draws its chat in.
pub fn convert_args_shifted(
    recipe: &Recipe,
    input: &Path,
    output: &Path,
    pass: Pass,
    audio_shift: Option<f64>,
    subtitles: Option<&Path>,
    chat: Option<&ChatLayer>,
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
    let first_pass = matches!(pass, Pass::First { .. });
    // The subtitle file: the input after the source (and its shifted audio),
    // or drawn into the picture by name.
    let subtitle_track = match (subtitles, &recipe.subtitles) {
        (Some(file), Some(step)) if !first_pass && step.delivery == Delivery::Embed => {
            args.extend(["-i".into(), file.into()]);
            Some((step, 1 + usize::from(audio_shift.is_some())))
        }
        _ => None,
    };
    let burn = match (subtitles, &recipe.subtitles) {
        (Some(file), Some(step)) if step.delivery == Delivery::BurnIn => {
            file.file_name().map(|name| name.to_string_lossy().into_owned())
        }
        _ => None,
    };
    // The chat layer, the last input, drawn over the encoded picture.
    let chat = match (chat, recipe.video_encode()) {
        (Some(layer), Some(_)) => {
            let index = 1 + usize::from(audio_shift.is_some()) + usize::from(subtitle_track.is_some());
            args.extend(layer.input_args().into_iter().map(OsString::from));
            Some((layer, index))
        }
        _ => None,
    };
    let mut push = |values: &[&str]| args.extend(values.iter().map(OsString::from));

    // ── Streams ──
    // First video track, and the first audio track if we keep one. Drops the
    // source's own subtitles, thumbnails and data tracks, which some
    // containers reject; Siphon's subtitle file is added on its own.
    match (chat, &recipe.video, recipe.video_encode()) {
        (Some((layer, index)), Some(video), Some(encode)) => {
            let filters = video_filters(video, encode, cut, burn.as_deref());
            push(&["-filter_complex", &layer.graph(&filters, index), "-map", "[v]"]);
        }
        _ => push(&["-map", "0:v:0"]),
    }
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
                // With chat, the picture's filters are part of the chat's graph.
                if chat.is_none() {
                    let filters = video_filters(video, encode, cut, burn.as_deref());
                    if !filters.is_empty() {
                        push(&["-vf", &filters.join(",")]);
                    }
                }
                for arg in video_encode_args(video, encode, pass) {
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
                stereo,
            } => {
                if let Some(cut) = cut {
                    push(&["-af", &format!("atrim=start={}:end={}", cut.start, cut.end)]);
                }
                for arg in audio_encode_args(codec, bitrate_kbps, stereo) {
                    push(&[&arg]);
                }
            }
        }
    }

    if let Some((step, input)) = subtitle_track {
        for arg in subtitle_track_args(step, recipe.container, input) {
            push(&[&arg]);
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

/// The arguments that make the first stream of input `input` the file's
/// subtitle track: in the kind of track its file type holds (MP4's own timed
/// text, WebVTT in WebM, SubRip or the styled original in Matroska), tagged
/// with its language and named, and shown when the video opens, since it was
/// asked for. A file only gets Siphon's own track, so it is always `s:0`.
pub fn subtitle_track_args(step: &SubtitleStep, container: Container, input: usize) -> Vec<String> {
    let mut args: Vec<String> = vec!["-map".into(), format!("{input}:0")];
    args.extend(subtitle_codec_args(step, container));
    args
}

/// The codec, language, name and disposition of the file's subtitle track.
fn subtitle_codec_args(step: &SubtitleStep, container: Container) -> Vec<String> {
    let codec = match container {
        Container::Mp4 | Container::Mov => "mov_text",
        Container::Webm => "webvtt",
        _ if step.styled => "copy",
        _ => "srt",
    };
    let mut args: Vec<String> = vec!["-c:s".into(), codec.into()];
    if codec == "mov_text" {
        // Apple's players show a subtitle track tagged "tx3g" (with the "sbtl"
        // handler that tag brings), and ignore others. ffmpeg tags it so in
        // MP4, but "text" in MOV unless told (checked 2026-11-20).
        args.extend(["-tag:s".into(), "tx3g".into()]);
    }
    // Matroska and WebM name languages the bibliographic way ("gre"), MP4 the other ("ell").
    let bibliographic = matches!(container, Container::Mkv | Container::Webm);
    if let Some(code) = iso639_2(&step.lang, bibliographic) {
        args.extend(["-metadata:s:s:0".into(), format!("language={code}")]);
    }
    args.extend(["-metadata:s:s:0".into(), format!("title={}", step.title)]);
    if matches!(container, Container::Mp4 | Container::Mov) {
        // What MP4 players show as the track's name.
        args.extend(["-metadata:s:s:0".into(), format!("handler_name={}", step.title)]);
    }
    args.extend(["-disposition:s:0".into(), "default".into()]);
    args
}

/// The arguments that copy a finished MP4's streams into `container`: a
/// smart cut is joined and checked as MP4 (its checks read decode times,
/// which Matroska doesn't keep), then put in the file type the plan asked
/// for. Its subtitle track, if any, comes along in that file type's kind.
pub fn rewrap_args(
    input: &Path,
    output: &Path,
    subtitles: Option<&SubtitleStep>,
    container: Container,
) -> Vec<OsString> {
    let mut args: Vec<OsString> = ["-hide_banner", "-nostdin", "-y", "-loglevel", "error", "-i"]
        .map(OsString::from)
        .to_vec();
    args.push(input.into());
    args.extend(["-map", "0:v:0", "-map", "0:a:0?", "-c", "copy"].map(OsString::from));
    if let Some(step) = subtitles {
        args.extend(["-map".into(), "0:s:0?".into()]);
        args.extend(subtitle_codec_args(step, container).into_iter().map(OsString::from));
    }
    args.push(output.into());
    args
}

/// What is done to the picture before it is encoded: cut on the exact
/// frames, subtitles drawn in, scaled down, the frame rate capped. `burn`:
/// the name of a subtitle file in ffmpeg's folder to draw into the picture,
/// on the video's own clock like the frames it is drawn on.
fn video_filters(video: &VideoStep, encode: &VideoEncode, cut: Option<Trim>, burn: Option<&str>) -> Vec<String> {
    let mut filters: Vec<String> = Vec::new();
    if let Some(cut) = cut {
        // Keep exactly the frames inside the selection (times are absolute, thanks to -copyts).
        filters.push(format!("trim=start={}:end={}", cut.start, cut.end));
    }
    if let Some(file) = burn {
        // Before the scaling and frame-rate filters, while each frame still
        // has its own time. Fonts come from Windows (DirectWrite), with its
        // fallbacks for scripts Arial lacks (Japanese came out in Yu Gothic UI).
        filters.push(format!("subtitles=filename={file}"));
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
    filters
}

/// The encoder and its settings.
fn video_encode_args(video: &VideoStep, encode: &VideoEncode, pass: Pass) -> Vec<String> {
    let mut args: Vec<String> = Vec::new();
    let mut push = |values: &[&str]| args.extend(values.iter().map(|value| value.to_string()));

    let hardware = encode.hardware;
    push(&["-c:v", hardware.encoder(encode.codec)]);

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
            if encode.fast { "p1" } else { "p5" },
            "-tune",
            "hq",
            "-rc",
            "vbr",
            "-cq",
            if hevc { "23" } else { "21" },
            "-b:v",
            "0",
        ]),
        (None, Hardware::Qsv) => push(&[
            "-preset",
            if encode.fast { "veryfast" } else { "medium" },
            "-global_quality",
            if hevc { "23" } else { "21" },
        ]),
        (None, Hardware::Amf) => {
            let (i, p) = if hevc { ("22", "24") } else { ("20", "22") };
            let quality = if encode.fast { "speed" } else { "quality" };
            push(&["-quality", quality, "-rc", "cqp", "-qp_i", i, "-qp_p", p]);
        }
        (None, Hardware::Software) => {
            let preset = if encode.fast { "veryfast" } else { "medium" };
            match encode.codec {
                VideoCodec::Hevc => push(&["-preset", preset, "-crf", "22"]),
                VideoCodec::Vp9 | VideoCodec::Vp8 => {
                    push(&["-crf", "31", "-b:v", "0", "-row-mt", "1", "-cpu-used", "2"])
                }
                VideoCodec::Av1 => push(&["-preset", "8", "-crf", "30"]),
                _ => push(&["-preset", preset, "-crf", "19"]),
            }
        }
    }

    // A constant-quality encode held under a cap (see `VideoEncode::max_kbps`).
    if let (None, Some(max)) = (encode.bitrate_kbps, encode.max_kbps) {
        push(&["-maxrate", &format!("{max}k"), "-bufsize", &format!("{}k", max * 2)]);
    }

    match encode.codec {
        // Apple players only recognise HEVC tagged "hvc1".
        VideoCodec::Hevc => push(&["-tag:v", "hvc1"]),
        VideoCodec::H264 => push(&["-profile:v", "high"]),
        _ => {}
    }
    args
}

pub(crate) fn audio_encode_args(codec: AudioCodec, bitrate_kbps: Option<u32>, stereo: bool) -> Vec<String> {
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
/// `chat` is the chat layer, for a recording that draws its chat in.
pub fn finalize_args(recipe: &Recipe, input: Recorded, output: &Path, chat: Option<&ChatLayer>) -> Vec<OsString> {
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
    let chat = chat.filter(|_| recipe.encodes_video());
    if let Some(layer) = chat {
        args.extend(layer.input_args().into_iter().map(OsString::from));
    }
    let mut push = |values: &[&str]| args.extend(values.iter().map(OsString::from));

    let keeps_audio = recipe.audio.is_some() && !recipe.strip_audio;
    match &recipe.video {
        Some(video) => {
            match (chat, recipe.video_encode()) {
                (Some(layer), Some(encode)) => {
                    let filters = video_filters(video, encode, None, None);
                    push(&["-filter_complex", &layer.graph(&filters, 1), "-map", "[v]"]);
                }
                _ => push(&["-map", "0:v:0"]),
            }
            match &video.action {
                VideoAction::Copy => {
                    push(&["-c:v", "copy"]);
                    if video.source == VideoCodec::Hevc && matches!(recipe.container, Container::Mp4 | Container::Mov) {
                        push(&["-tag:v", "hvc1"]);
                    }
                }
                VideoAction::Encode(encode) => {
                    let filters = video_filters(video, encode, None, None);
                    if !filters.is_empty() && chat.is_none() {
                        push(&["-vf", &filters.join(",")]);
                    }
                    for arg in video_encode_args(video, encode, Pass::Only) {
                        push(&[&arg]);
                    }
                    // The stream's own frame rate, not the one the recording
                    // starts with: an MP4 is made at one rate, which ffmpeg
                    // takes from the start, and a Twitch recording starts with
                    // a 30-frame ad, so its 60-frame stream lost every other
                    // frame (found 2026-11-21).
                    if let Some(fps) = recipe.output_fps() {
                        push(&["-r", &frame_rate(fps)]);
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
                    stereo,
                } => {
                    for arg in audio_encode_args(codec, bitrate_kbps, stereo) {
                        push(&[&arg]);
                    }
                }
            }
        }
        _ => push(&["-an"]),
    }
    // Subtitles and data tracks of a live stream (timed metadata, ad markers) don't belong in the file.
    push(&["-sn", "-dn"]);
    // Recorded from the start and stopped while catching up, the sound has run
    // far ahead of the picture (it's much smaller): keep what has both. Its
    // length is the recipe's (see `downloads::live`); `-shortest` alone
    // doesn't trim sound that is converted beside a copied picture (measured
    // 2026-11-19: 765 s of picture came with 3490 s of sound).
    if recipe.video.is_some() && keeps_audio {
        push(&["-shortest"]);
    }
    if let Some(length) = recipe.duration.filter(|length| *length > 0.0) {
        push(&["-t", &format!("{length:.3}")]);
    }
    args.push(output.into());
    args
}

/// The arguments that draw the chat into a piece of a recording while it
/// records (see `downloads::live_draw`): the picture of `input` from `from`
/// to `until` (the end of what is there, if `None`), on the file's own
/// timestamps, which start at `file_start`, encoded as `recipe` says with
/// the chat layer from Siphon (`chat.start` is `from`), and the audio
/// packets whose middle falls in the piece copied, all kept on the
/// recording's timestamps in MPEG-TS. Pieces drawn back to back and joined
/// byte for byte make the recording, drawn: every frame and every audio
/// packet once (tested 2026-11-21).
pub fn draw_piece_args(
    recipe: &Recipe,
    input: &Path,
    file_start: f64,
    from: f64,
    until: Option<f64>,
    chat: &ChatLayer,
    output: &Path,
) -> Vec<OsString> {
    let mut args: Vec<OsString> = Vec::new();
    let mut push = |values: &[&str]| args.extend(values.iter().map(OsString::from));
    push(&[
        "-hide_banner",
        "-y",
        "-loglevel",
        "error",
        "-progress",
        "pipe:1",
        "-nostats",
    ]);
    // Read from well before: MPEG-TS has no index of key frames, so a seek
    // can land just after one, and decoding then starts at the next. Three
    // seconds back was not always enough (a piece began a sixth of a second
    // late, 2026-11-22). The trim keeps exactly the piece.
    push(&["-copyts", "-ss", &seek_from(file_start, from - PIECE_LEAD_IN), "-i"]);
    args.push(input.into());
    args.extend(chat.input_args().into_iter().map(OsString::from));
    let mut push = |values: &[&str]| args.extend(values.iter().map(OsString::from));
    let trim = match until {
        Some(until) => format!("trim=start={from:.6}:end={until:.6}"),
        None => format!("trim=start={from:.6}"),
    };
    if let (Some(video), Some(encode)) = (&recipe.video, recipe.video_encode()) {
        let mut filters = vec![trim];
        filters.extend(video_filters(video, encode, None, None));
        push(&["-filter_complex", &chat.graph(&filters, 1), "-map", "[v]"]);
        for arg in video_encode_args(video, encode, Pass::Only) {
            push(&[&arg]);
        }
        // The rate is made in the graph (see `ChatLayer::rate`): every frame as it comes.
        push(&["-fps_mode", "passthrough"]);
    }
    if recipe.audio.is_some() && !recipe.strip_audio {
        // As recorded, packet by packet: the finished file converts it if its preset says so.
        let middle = "pts*tb+duration*tb/2";
        let drop = match until {
            Some(until) => format!("noise=drop=lt({middle}\\,{from:.6})+gte({middle}\\,{until:.6})"),
            None => format!("noise=drop=lt({middle}\\,{from:.6})"),
        };
        push(&["-map", "0:a:0?", "-c:a", "copy", "-bsf:a", &drop]);
    } else {
        push(&["-an"]);
    }
    // Each piece's packets start a count of their own: marked, so that read
    // one after another the first packet after a join isn't taken for a
    // damaged one, which could cost the join a frame (found 2026-11-22).
    push(&[
        "-sn",
        "-dn",
        "-f",
        "mpegts",
        "-muxdelay",
        "0",
        "-muxpreload",
        "0",
        "-mpegts_flags",
        "+initial_discontinuity",
    ]);
    args.push(output.into());
    args
}

/// How far before a piece drawn as a stream records its recording is read
/// from, in seconds (see `draw_piece_args`).
const PIECE_LEAD_IN: f64 = 10.0;

/// The `-ss` that starts reading a file whose timestamps start at
/// `file_start` from its own timestamp `at` (or its start, if `at` is
/// before it). ffmpeg counts `-ss` from the file's start, not by its
/// timestamps, even with `-copyts` and `-seek_timestamp` (checked
/// 2026-11-22 on a file starting at 101.4 s: `-ss 20` read from 121.4 s).
pub fn seek_from(file_start: f64, at: f64) -> String {
    format!("{:.3}", (at - file_start).max(0.0))
}

/// A frame rate as ffmpeg reads it: NTSC's rates exactly (`60000/1001`),
/// others as a plain number.
fn frame_rate(fps: f64) -> String {
    for base in [24_u32, 30, 48, 60, 120] {
        if (fps - f64::from(base) * 1000.0 / 1001.0).abs() < 0.01 {
            return format!("{}/1001", base * 1000);
        }
    }
    let text = format!("{fps:.3}");
    text.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// The concat demuxer's list for a recording in several parts (the stream
/// dropped and came back), and how long the joined recording is.
///
/// The demuxer lays each part after the last at the part's full length,
/// from its start to whichever track ends last, so a part whose sound ends
/// before its picture would push everything after it later. Each part is
/// ended where its picture and sound both end (`outpoint`), and its length
/// counted from its start to there, which is how the demuxer counts it.
/// A part that couldn't be read is listed whole and counts nothing; the
/// finished file then fails its length check and is kept as recorded.
pub fn concat_list(parts: &[(&Path, Option<&Probed>)]) -> (String, f64) {
    let mut list = String::new();
    let mut length = 0.0;
    for (file, probed) in parts {
        list.push_str(&format!("file '{}'\n", file.to_string_lossy().replace('\'', r"'\''")));
        let span = probed.and_then(|probed| Some((probed.start?, probed.common_end()?)));
        if let Some((start, end)) = span.filter(|(start, end)| end > start) {
            list.push_str(&format!("outpoint {end:.6}\n"));
            length += end - start;
        }
    }
    (list, length)
}

/// The arguments that join the video and audio files a recording from the
/// start left (stopped, or cut short, before yt-dlp joined them) into one
/// Matroska file. Not MPEG-TS: YouTube's best video there is often VP9, which
/// MPEG-TS can't hold.
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
    args.extend(["-c", "copy", "-f", "matroska"].map(OsString::from));
    args.push(output.into());
    args
}

/// ffprobe's view of a file's length and tracks, as JSON (see [`Probed::parse`]).
pub fn probe_args(file: &Path) -> Vec<OsString> {
    let mut args: Vec<OsString> = [
        "-v",
        "error",
        "-show_entries",
        // Matroska and WebM state no length per track, only a DURATION tag.
        "format=duration,start_time:stream=codec_type,start_time,duration:stream_tags=DURATION",
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

/// The arguments that make ffprobe print a file's picture size, frame rate and length.
pub fn picture_probe_args(file: &Path) -> Vec<OsString> {
    let mut args: Vec<OsString> = [
        "-v",
        "error",
        "-select_streams",
        "v:0",
        "-show_entries",
        "stream=width,height,avg_frame_rate,r_frame_rate,duration:format=duration",
        "-of",
        "json",
    ]
    .map(OsString::from)
    .to_vec();
    args.push(file.into());
    args
}

/// A file's picture: its size in pixels, frames a second and length.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Picture {
    pub width: u32,
    pub height: u32,
    pub fps: Option<f64>,
    pub duration: Option<f64>,
}

impl Picture {
    /// Reads what [`picture_probe_args`] printed.
    pub fn parse(json: &str) -> Option<Picture> {
        let value: serde_json::Value = serde_json::from_str(json).ok()?;
        let stream = value["streams"].as_array()?.first()?;
        // "60/1"; "0/0" when unknown.
        let rate = |text: &serde_json::Value| -> Option<f64> {
            let (numerator, denominator) = text.as_str()?.split_once('/')?;
            let (numerator, denominator): (f64, f64) = (numerator.parse().ok()?, denominator.parse().ok()?);
            (numerator > 0.0 && denominator > 0.0).then(|| numerator / denominator)
        };
        Some(Picture {
            width: u32::try_from(stream["width"].as_u64()?).ok()?,
            height: u32::try_from(stream["height"].as_u64()?).ok()?,
            fps: rate(&stream["avg_frame_rate"]).or_else(|| rate(&stream["r_frame_rate"])),
            // The picture's own length where the file says, not the file's,
            // which runs on to the end of the sound.
            duration: stream["duration"]
                .as_str()
                .or_else(|| value["format"]["duration"].as_str())
                .and_then(|d| d.parse().ok()),
        })
        .filter(|picture| picture.width > 0 && picture.height > 0)
    }
}

/// A file's length and the span of its first video and audio track, in seconds.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Probed {
    pub duration: Option<f64>,
    /// Where the file starts: the earliest of its tracks (data tracks too).
    pub start: Option<f64>,
    /// Start and length of the first video track.
    pub video: Option<(f64, f64)>,
    pub audio: Option<(f64, f64)>,
}

impl Probed {
    /// Reads what [`probe_args`] printed.
    pub fn parse(json: &str) -> Option<Self> {
        let value: serde_json::Value = serde_json::from_str(json).ok()?;
        let number = |value: &serde_json::Value| -> Option<f64> { value.as_str()?.parse().ok() };
        // "00:12:44.967000000"
        let clock = |value: &serde_json::Value| -> Option<f64> {
            let mut seconds = 0.0;
            for part in value.as_str()?.split(':') {
                seconds = seconds * 60.0 + part.parse::<f64>().ok()?;
            }
            Some(seconds)
        };
        let mut probed = Probed {
            duration: number(&value["format"]["duration"]),
            start: number(&value["format"]["start_time"]),
            ..Probed::default()
        };
        for stream in value["streams"].as_array()? {
            let length = number(&stream["duration"])
                .or_else(|| clock(&stream["tags"]["DURATION"]))
                .or(probed.duration);
            let span = number(&stream["start_time"]).zip(length);
            match stream["codec_type"].as_str() {
                Some("video") if probed.video.is_none() => probed.video = span.or(Some((0.0, 0.0))),
                Some("audio") if probed.audio.is_none() => probed.audio = span.or(Some((0.0, 0.0))),
                _ => {}
            }
        }
        Some(probed)
    }

    /// How much of the file has picture and sound both (or its length, with
    /// only one of them): what a recording made from it keeps.
    pub fn common_length(&self) -> Option<f64> {
        match (self.video, self.audio) {
            (Some((video_start, video_length)), Some((audio_start, audio_length))) => {
                let end = (video_start + video_length).min(audio_start + audio_length);
                Some((end - video_start.max(audio_start)).max(0.0))
            }
            _ => self.duration,
        }
    }

    /// Where the part of the file with picture and sound both ends, as a
    /// timestamp in the file (with only one of them, where that ends).
    pub fn common_end(&self) -> Option<f64> {
        let end = |(start, length): (f64, f64)| start + length;
        match (self.video, self.audio) {
            (Some(video), Some(audio)) => Some(end(video).min(end(audio))),
            (Some(track), None) | (None, Some(track)) => Some(end(track)),
            (None, None) => Some(self.start? + self.duration?),
        }
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

/// Reads one line of `-progress` output. Returns how many bytes of the file
/// ffmpeg has written so far. It comes just before the line with the time in
/// each report.
pub fn parse_written(line: &str) -> Option<u64> {
    line.trim()
        .strip_prefix("total_size=")?
        .parse()
        .ok()
        .filter(|bytes| *bytes > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_progress_lines() {
        assert_eq!(parse_progress("out_time_us=12500000"), Some(12.5));
        assert_eq!(parse_progress("out_time_us=N/A"), None);
        assert_eq!(parse_progress("frame=120"), None);
        assert_eq!(parse_written("total_size=1048624"), Some(1_048_624));
        assert_eq!(parse_written("total_size=N/A"), None);
        assert_eq!(parse_written("total_size=0"), None);
        assert_eq!(parse_written("out_time_us=12500000"), None);
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
        assert!((probed.common_length().unwrap() - 37.825289).abs() < 1e-6);
        // From the start, stopped while catching up: the sound ran 40 minutes ahead.
        let ahead = Probed {
            video: Some((0.0, 600.0)),
            audio: Some((0.0, 2860.0)),
            duration: Some(2860.0),
            start: Some(0.0),
        };
        assert_eq!(ahead.common_length(), Some(600.0));
        // Matroska: lengths only in tags. A recording from the start that came
        // out with 12:50 of picture and an hour of sound must not pass as an hour.
        let tagged = Probed::parse(
            r#"{"streams":[{"codec_type":"video","start_time":"0.000000","tags":{"DURATION":"00:12:50.000000000"}},
                {"codec_type":"audio","start_time":"0.000000","tags":{"DURATION":"01:04:29.841000000"}}],
                "format":{"duration":"3869.841000"}}"#,
        )
        .unwrap();
        assert_eq!(tagged.video, Some((0.0, 770.0)));
        assert_eq!(tagged.common_length(), Some(770.0));
        assert!(tagged.matches(3869.8, true, true).is_err());
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
    fn each_part_of_a_recording_ends_where_its_picture_and_sound_both_do() {
        // Two parts whose sound ends before their picture (measured 2026-11-20).
        let part = |video: f64, audio: f64| {
            Probed::parse(&format!(
                r#"{{"streams":[{{"codec_type":"video","start_time":"1.466667","duration":"{video}"}},
                    {{"codec_type":"audio","start_time":"1.443444","duration":"{audio}"}}],
                    "format":{{"start_time":"1.443444","duration":"{}"}}}}"#,
                video + 0.023223
            ))
            .unwrap()
        };
        let (first, second) = (part(20.0, 17.902578), part(15.0, 13.026389));
        let (list, length) = concat_list(&[
            (Path::new(r"C:\rec\Show.rec1.ts"), Some(&first)),
            (Path::new(r"C:\rec\Show's.rec2.ts"), Some(&second)),
        ]);
        assert_eq!(
            list,
            "file 'C:\\rec\\Show.rec1.ts'\noutpoint 19.346022\nfile 'C:\\rec\\Show'\\''s.rec2.ts'\noutpoint 14.469833\n"
        );
        // From each part's start to its outpoint, as the demuxer lays them out:
        // not their full lengths (35.05 s), and not only what has both (30.88 s).
        assert!((length - (17.902578 + 13.026389)).abs() < 1e-6);
        // A part that can't be read is listed whole, and counts nothing.
        let (list, length) = concat_list(&[(Path::new(r"C:\rec\x.ts"), None)]);
        assert_eq!((list.as_str(), length), ("file 'C:\\rec\\x.ts'\n", 0.0));
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
            subtitles: None,
            chat: None,
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
            None,
        ));
        assert!(
            copy.contains("-i a.ts -map 0:v:0 -c:v copy -map 0:a:0? -c:a copy -sn -dn -shortest a.mp4"),
            "{copy}"
        );
        recipe.duration = Some(765.0);
        let both = text(finalize_args(
            &recipe,
            Recorded::File(Path::new("a.mkv")),
            Path::new("a.mp4"),
            None,
        ));
        assert!(both.contains("-shortest -t 765.000 a.mp4"), "{both}");
        recipe.duration = None;
        let joined = text(finalize_args(
            &recipe,
            Recorded::List(Path::new("parts.txt")),
            Path::new("a.mp4"),
            None,
        ));
        assert!(joined.contains("-f concat -safe 0 -i parts.txt"), "{joined}");

        // Converted: at the stream's own rate, whatever the recording starts
        // with (Twitch's ads run at 30 frames a second, its streams at 60).
        let mut converted = recipe.clone();
        if let Some(video) = converted.video.as_mut() {
            video.fps = Some(60.0);
            video.action = VideoAction::Encode(crate::plan::VideoEncode {
                codec: VideoCodec::H264,
                hardware: Hardware::Nvenc,
                bitrate_kbps: None,
                scale_lines: None,
                max_fps: None,
                fast: true,
                max_kbps: None,
            });
        }
        let encoded = text(finalize_args(
            &converted,
            Recorded::File(Path::new("a.ts")),
            Path::new("a.mp4"),
            None,
        ));
        assert!(
            encoded.contains("-c:v h264_nvenc") && encoded.contains(" -r 60 "),
            "{encoded}"
        );
        // A piece drawn as it records: the recording's own timestamps, exactly
        // its frames, the audio packets in it as recorded, in MPEG-TS.
        let layer = ChatLayer {
            width: 420,
            height: 520,
            fps: 30.0,
            x: 1480,
            y: 20,
            start: 121.4,
            hd: true,
            end: Some(241.4),
            rate: Some(60.0),
        };
        let piece = text(draw_piece_args(
            &converted,
            Path::new("a.ts"),
            1.4,
            121.4,
            Some(241.4),
            &layer,
            Path::new("a.drawn1.piece.ts"),
        ));
        // Ten seconds before the piece, counted from the file's start as ffmpeg counts `-ss`.
        assert!(piece.contains("-copyts -ss 110.000 -i a.ts"), "{piece}");
        // The stream's rate first, so the piece's cut and the cut after the
        // overlay see the same timestamps.
        assert!(
            piece.contains("[0:v]fps=60,trim=start=121.400000:end=241.400000"),
            "{piece}"
        );
        assert!(piece.contains("setpts=PTS-STARTPTS+121.4/TB"), "{piece}");
        // Cut where the piece ends, however long the layer lasts.
        assert!(piece.contains("eof_action=repeat,trim=end=241.400000[v]"), "{piece}");
        assert!(
            piece.contains(" -fps_mode passthrough ") && !piece.contains(" -r "),
            "{piece}"
        );
        assert!(
            piece.contains(
                r"-c:a copy -bsf:a noise=drop=lt(pts*tb+duration*tb/2\,121.400000)+gte(pts*tb+duration*tb/2\,241.400000)"
            ),
            "{piece}"
        );
        assert!(
            piece.ends_with(
                "-f mpegts -muxdelay 0 -muxpreload 0 -mpegts_flags +initial_discontinuity a.drawn1.piece.ts"
            ),
            "{piece}"
        );
        let last = text(draw_piece_args(
            &converted,
            Path::new("a.ts"),
            1.4,
            1.4,
            None,
            &layer,
            Path::new("p.ts"),
        ));
        assert!(
            last.contains("-ss 0.000 ") && last.contains("trim=start=1.400000[base]"),
            "{last}"
        );
        // A file whose timestamps start far from zero is still read from just before the piece.
        assert_eq!(seek_from(101.4, 131.4 - PIECE_LEAD_IN), "20.000");
        assert_eq!(seek_from(101.4, 50.0), "0.000");
        assert_eq!(frame_rate(59.94), "60000/1001");
        assert_eq!(frame_rate(29.97), "30000/1001");
        assert_eq!(frame_rate(25.0), "25");
        assert_eq!(frame_rate(12.5), "12.5");

        recipe.video = None;
        recipe.container = Container::Mp3;
        if let Some(audio) = recipe.audio.as_mut() {
            audio.action = AudioAction::Encode {
                codec: AudioCodec::Mp3,
                bitrate_kbps: Some(192),
                stereo: false,
            };
        }
        let music = text(finalize_args(
            &recipe,
            Recorded::File(Path::new("a.ts")),
            Path::new("a.mp3"),
            None,
        ));
        assert!(music.contains("-vn -map 0:a:0? -c:a libmp3lame"), "{music}");
    }
}
