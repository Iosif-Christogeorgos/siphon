//! Turns "what the user asked for" into "exactly what will happen".
//!
//! [`plan`] is a pure function from a video's formats, the chosen preset (or
//! custom settings), the trim range and what this machine can do, to a
//! [`Recipe`]: which streams to fetch, whether each is copied or converted
//! and how, into which container. The same recipe drives the summary shown
//! before downloading and the yt-dlp/ffmpeg arguments, so the app can never
//! describe one thing and do another.
//!
//! Every decision is made from the formats this video actually offers, never
//! from what a site usually has: the same preset copies a stream on one site
//! and converts on another.
//!
//! Guiding rule: copy original streams whenever the target can play them.
//! Converting is slower and never improves quality, so it only happens when
//! the target needs it or the user explicitly asks.

mod audio;
pub mod codecs;
mod custom;
pub mod explain;
pub mod live;
pub mod registry;
mod share;
pub mod sizes;
pub mod subtitles;
pub mod tracks;
mod watch;

use std::cmp::Ordering;

use serde::{Deserialize, Serialize};

use crate::chat::{ChatSource, ChatStep, Placement};
use crate::error::{AppError, ErrorKind, Result};
use crate::ffmpeg::encoders::{Encoders, Hardware};
use crate::ytdlp::metadata::{Format, FormatKind, VideoInfo};
use codecs::{AudioCodec, Container, VideoCodec};
use registry::{Group, MusicFormat, QualityKind, TargetId, TierId};

// ── What the user chose (from the frontend) ─────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum Choice {
    Preset(PresetChoice),
    Custom(ManualOptions),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PresetChoice {
    pub target: TargetId,
    /// The quality row: a resolution (e.g. 1080, see `lines_for`), or a bitrate in
    /// kbit/s for Music. `None` is "Best".
    #[serde(default)]
    pub quality: Option<u32>,
    /// Share targets: the account tier. `None` is the target's first tier.
    #[serde(default)]
    pub tier: Option<TierId>,
    #[serde(default)]
    pub music_format: MusicFormat,
    /// The language subtitles are wanted in ("en", "pt-br"); `None` is none.
    /// See [`subtitles`].
    #[serde(default)]
    pub subtitles: Option<String>,
    /// Draw the video's chat into the picture, here; `None` is without chat.
    /// Only Twitch's chat, with Max quality and Watch (see `crate::chat`).
    #[serde(default)]
    pub chat: Option<Placement>,
}

impl Choice {
    /// Where the chat is to be drawn, if it is.
    pub fn chat(&self) -> Option<&Placement> {
        match self {
            Choice::Preset(preset) => preset.chat.as_ref(),
            Choice::Custom(_) => None,
        }
    }
}

/// The part of the video to keep, in seconds from its start. Cuts are
/// always on the exact frames.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Trim {
    pub start: f64,
    pub end: f64,
}

impl Trim {
    pub fn duration(&self) -> f64 {
        self.end - self.start
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Content {
    #[default]
    VideoAudio,
    VideoOnly,
    AudioOnly,
}

/// Custom mode. Everything is optional: `None` means "best available" for
/// streams and "keep the original" for codecs and container.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ManualOptions {
    pub content: Content,
    pub video_format: Option<String>,
    pub audio_format: Option<String>,
    pub container: Option<Container>,
    pub video_codec: Option<VideoCodec>,
    pub audio_codec: Option<AudioCodec>,
    /// The language subtitles are wanted in, as for presets.
    pub subtitles: Option<String>,
}

// ── What this machine can do ────────────────────────────────────────────

/// Which of Microsoft's optional video extensions are installed (Windows only).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WindowsCodecs {
    pub vp9: bool,
    pub av1: bool,
    pub hevc: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Capabilities {
    pub encoders: Encoders,
    /// Whether ffmpeg can fetch a trimmed part in requests of a set size.
    pub chunked_requests: bool,
    pub windows: WindowsCodecs,
}

impl Capabilities {
    /// No hardware encoders and nothing optional installed.
    #[cfg(test)]
    pub const BASIC: Capabilities = Capabilities {
        encoders: Encoders::SOFTWARE,
        chunked_requests: false,
        windows: WindowsCodecs {
            vp9: false,
            av1: false,
            hevc: false,
        },
    };

    fn hardware(&self, codec: VideoCodec) -> Hardware {
        self.encoders.for_codec(codec)
    }

    fn has_hardware(&self, codec: VideoCodec) -> bool {
        self.hardware(codec) != Hardware::Software
    }
}

// ── Recipe (what will happen) ───────────────────────────────────────────

/// How the summary line marks a plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Badge {
    /// Original streams, copied.
    Lossless,
    Converts,
    /// Encoded to fit under an upload limit.
    SizeOptimized,
    /// The best streams as they are, in whatever format they come (Max quality).
    MayNotPlay,
}

/// Serialized only to keep a live recording's plan until it is finished
/// (see `downloads::live`), so it can be finished after a crash.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Recipe {
    pub video: Option<VideoStep>,
    pub audio: Option<AudioStep>,
    pub container: Container,
    /// Drop the audio track of a stream that has video and audio in one.
    pub strip_audio: bool,
    /// Why something is converted, as a clause: "Apple devices can't play VP9".
    pub reason: Option<String>,
    /// "Plays natively on Apple devices", "Plays in any music player", …
    pub promise: String,
    pub hints: Vec<Hint>,
    /// Seconds of output, when known (needed for size estimates and conversion progress).
    pub duration: Option<f64>,
    /// Only this part of the video is downloaded.
    pub trim: Option<Trim>,
    /// Share of each source stream that gets downloaded: 1 unless trimmed.
    pub source_fraction: f64,
    pub badge: Badge,
    /// Share: the finished file must not be larger than this, in bytes.
    pub size_limit: Option<u64>,
    /// Share: "fits Discord (10 MB)", for the headline.
    pub fits: Option<String>,
    /// Why this can't be downloaded as chosen; the plan is shown but Download is not offered.
    pub blocked: Option<String>,
    /// The request size ffmpeg should fetch a trimmed part in, when the site names one.
    pub section_request_size: Option<u64>,
    /// Share: how to convert a copied file that turns out larger than the limit.
    pub fallback: Option<Box<Recipe>>,
    /// A copied H.264 or HEVC stream that is cut: how the frames at the cuts
    /// are re-encoded, and how the whole part is should the smart cut not
    /// pass its checks (see `ffmpeg::smart_cut`).
    pub cut_encode: Option<VideoEncode>,
    /// The subtitle track to add, and how (see [`subtitles`]).
    #[serde(default)]
    pub subtitles: Option<subtitles::SubtitleStep>,
    /// The chat drawn into the picture (see `crate::chat`).
    #[serde(default)]
    pub chat: Option<ChatStep>,
    /// What measuring the video found out about this recipe's size (see
    /// [`sizes`]): nothing until it has been measured.
    #[serde(skip)]
    pub measured: sizes::Measured,
    /// For a part of a file on this PC: where reading it starts (see
    /// [`ReadFrom`]). None reads from the start: a part yt-dlp downloaded
    /// starts near the cut anyway.
    #[serde(default)]
    pub read_from: Option<ReadFrom>,
    /// A file on this PC's own tracks: which sound is its main one, and the
    /// other sound and subtitle tracks kept beside it (see [`tracks`]).
    #[serde(default)]
    pub tracks: tracks::Tracks,
}

/// Where a cut reads a whole file from: a little before the part, rather
/// than from the file's start. ffmpeg seeks to a key frame before it, so a
/// part an hour into a file isn't read, or decoded, all the way from its
/// start; the part's frames are still picked by their own timestamps, so a
/// key frame landed on early costs nothing.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ReadFrom {
    /// Seconds from the file's start, as ffmpeg's `-ss` counts them.
    pub seek: f64,
    /// The same moment on the file's own timestamps (an OBS recording's start a second or so in).
    pub at: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VideoStep {
    pub format_id: String,
    pub source: VideoCodec,
    /// What the source stream is called ("2160p", see `lines_for`).
    pub lines: u32,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub fps: Option<f64>,
    pub hdr: bool,
    /// Bytes of the source stream.
    pub size: Option<u64>,
    /// The source stream's bitrate in kbit/s, when known.
    pub kbps: Option<f64>,
    pub action: VideoAction,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum VideoAction {
    Copy,
    Encode(VideoEncode),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VideoEncode {
    pub codec: VideoCodec,
    /// The encoder, picked from what this machine has.
    pub hardware: Hardware,
    /// Target bitrate for size-limited output; `None` means constant quality.
    pub bitrate_kbps: Option<u32>,
    /// Scale down to this resolution, keeping the picture's shape (`scaled_to`); never up.
    pub scale_lines: Option<u32>,
    pub max_fps: Option<f64>,
    /// The encoder's fastest setting, at the same quality: a file a little
    /// larger, made two to three times sooner. For drawing chat into hours
    /// of video, where the wait matters more than the size (docs/chat.md).
    #[serde(default)]
    pub fast: bool,
    /// The most a constant-quality encode may spend at any moment, in kbit/s.
    /// Twitch encodes streams live at a fixed bitrate, so its busiest scenes
    /// are starved, and keeping their every artefact took 2.3 times the
    /// source's bitrate (measured 2026-11-21, docs/chat.md).
    #[serde(default)]
    pub max_kbps: Option<u32>,
}

impl VideoEncode {
    fn quality(codec: VideoCodec, hardware: Hardware) -> Self {
        Self {
            codec,
            hardware,
            bitrate_kbps: None,
            scale_lines: None,
            max_fps: None,
            fast: false,
            max_kbps: None,
        }
    }

    /// A size-targeted encode on the CPU runs twice: the first pass measures
    /// the video so the second can spend the bits where they show. Hardware
    /// encoders do their own look-ahead in one run.
    pub fn two_pass(&self) -> bool {
        self.bitrate_kbps.is_some() && self.hardware == Hardware::Software && self.codec == VideoCodec::H264
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AudioStep {
    /// `None` when the audio rides along inside the video stream.
    pub format_id: Option<String>,
    pub source: AudioCodec,
    pub source_kbps: Option<f64>,
    pub size: Option<u64>,
    pub action: AudioAction,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum AudioAction {
    Copy,
    Encode {
        codec: AudioCodec,
        bitrate_kbps: Option<u32>,
        /// Mix down to stereo (Share: surround is wasted on a phone).
        stereo: bool,
    },
}

impl AudioAction {
    fn encode(codec: AudioCodec, bitrate_kbps: Option<u32>) -> Self {
        AudioAction::Encode {
            codec,
            bitrate_kbps,
            stereo: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HintLevel {
    Info,
    Warning,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Hint {
    pub level: HintLevel,
    /// A warning's name in a few words ("HDR colours"), for when the
    /// warnings are folded into one row; notes have none.
    #[serde(default)]
    pub title: Option<String>,
    pub text: String,
}

impl Recipe {
    pub fn encodes_video(&self) -> bool {
        matches!(
            self.video,
            Some(VideoStep {
                action: VideoAction::Encode(_),
                ..
            })
        )
    }

    /// The main sound, or another of a file's sound tracks, is converted.
    pub fn encodes_audio(&self) -> bool {
        self.encodes_main_audio()
            || self
                .tracks
                .sounds
                .iter()
                .any(|sound| matches!(sound.action, AudioAction::Encode { .. }))
    }

    pub fn encodes_main_audio(&self) -> bool {
        matches!(
            self.audio,
            Some(AudioStep {
                action: AudioAction::Encode { .. },
                ..
            })
        )
    }

    pub fn video_encode(&self) -> Option<&VideoEncode> {
        match &self.video.as_ref()?.action {
            VideoAction::Encode(encode) => Some(encode),
            VideoAction::Copy => None,
        }
    }

    /// True when Siphon runs its own ffmpeg pass after the download.
    /// (Audio-only conversions are left to yt-dlp's extractor instead.)
    /// Subtitles inside the file are added in that pass, even to streams
    /// that are only copied.
    pub fn needs_ffmpeg_pass(&self) -> bool {
        self.video.is_some()
            && (self.encodes_video()
                || self.encodes_audio()
                || self.strip_audio
                || self.smart_cut()
                || self.embeds_subtitles())
    }

    /// True when Siphon's ffmpeg pass only adds a subtitle track to streams it copies.
    pub fn adds_only_subtitles(&self) -> bool {
        self.embeds_subtitles()
            && !(self.encodes_video() || self.encodes_audio() || self.strip_audio || self.smart_cut())
    }

    /// True when a subtitle track goes inside the file.
    pub fn embeds_subtitles(&self) -> bool {
        self.subtitles
            .as_ref()
            .is_some_and(|step| step.delivery == subtitles::Delivery::Embed)
    }

    /// True when the video is copied and cut on the exact frames by
    /// re-encoding only the frames at the cuts.
    pub fn smart_cut(&self) -> bool {
        self.cut().is_some()
            && self.cut_encode.is_some()
            && matches!(
                self.video,
                Some(VideoStep {
                    action: VideoAction::Copy,
                    ..
                })
            )
    }

    /// The same recipe, cutting by re-encoding the whole part: what a smart
    /// cut falls back to.
    pub fn with_full_cut(&self) -> Recipe {
        let mut recipe = self.clone();
        if let (Some(video), Some(encode)) = (recipe.video.as_mut(), recipe.cut_encode.take()) {
            video.action = VideoAction::Encode(encode);
        }
        recipe
    }

    /// The range to cut on the exact frames in the ffmpeg pass, if any.
    /// (Audio-only downloads are cut by yt-dlp: audio has no key frames to
    /// worry about.)
    pub fn cut(&self) -> Option<Trim> {
        self.trim.filter(|_| self.video.is_some())
    }

    /// yt-dlp `--format` value: explicit stream ids, so it downloads exactly
    /// what the summary promised.
    pub fn format_selector(&self) -> String {
        let video = self.video.as_ref().map(|step| step.format_id.as_str());
        let audio = self.audio.as_ref().and_then(|step| step.format_id.as_deref());
        match (video, audio) {
            (Some(video), Some(audio)) => format!("{video}+{audio}"),
            (Some(id), None) | (None, Some(id)) => id.to_string(),
            (None, None) => "b".to_string(),
        }
    }

    pub fn video_codec(&self) -> Option<VideoCodec> {
        self.video.as_ref().map(|step| match &step.action {
            VideoAction::Copy => step.source,
            VideoAction::Encode(encode) => encode.codec,
        })
    }

    pub fn audio_codec(&self) -> Option<AudioCodec> {
        if self.strip_audio {
            return None;
        }
        self.audio.as_ref().map(|step| match step.action {
            AudioAction::Copy => step.source,
            AudioAction::Encode { codec, .. } => codec,
        })
    }

    /// The output's resolution after any downscale (see `lines_for`).
    pub fn output_lines(&self) -> Option<u32> {
        self.video.as_ref().map(|step| match &step.action {
            VideoAction::Encode(VideoEncode {
                scale_lines: Some(lines),
                ..
            }) => (*lines).min(step.lines),
            _ => step.lines,
        })
    }

    pub fn output_fps(&self) -> Option<f64> {
        let step = self.video.as_ref()?;
        let fps = step.fps?;
        Some(match &step.action {
            VideoAction::Encode(VideoEncode { max_fps: Some(max), .. }) if fps > *max + 0.5 => fps / 2.0,
            _ => fps,
        })
    }

    /// Bytes yt-dlp will fetch: the chosen streams, as measured in the
    /// trimmed part, or scaled to it.
    pub fn estimated_source_bytes(&self) -> Option<u64> {
        let scaled = |size: Option<u64>| size.map(|size| (size as f64 * self.source_fraction) as u64);
        let video = match &self.video {
            Some(step) => self.measured.video.or_else(|| scaled(step.size))?,
            None => 0,
        };
        let audio = match &self.audio {
            Some(AudioStep {
                format_id: Some(_),
                size,
                ..
            }) => self.measured.audio.or_else(|| scaled(*size))?,
            _ => 0,
        };
        Some(video + audio)
    }

    /// Best guess at the finished file's size in bytes (the likely value of [`Recipe::estimate`]).
    pub fn estimated_size(&self) -> Option<u64> {
        self.estimate().map(|estimate| estimate.likely)
    }

    /// The finished file's size: the likely value, and how far either way it
    /// can reasonably land. Copies and size-targeted encodes are near-certain;
    /// a constant-quality encode depends on the picture, so it gets a range,
    /// until it has been measured on the video (see [`sizes`]).
    pub fn estimate(&self) -> Option<SizeEstimate> {
        let duration = self.duration;
        let bitrate = |kbps: f64| duration.map(|d| kbps * 1000.0 / 8.0 * d);
        let video = match &self.video {
            None => Some(SizeEstimate::exact(0.0)),
            Some(step) => match &step.action {
                VideoAction::Copy => self
                    .measured
                    .video
                    .map(|bytes| bytes as f64)
                    .or_else(|| step.size.map(|size| size as f64 * self.source_fraction))
                    // A smart cut re-encodes its ends, visually lossless: larger
                    // than what they replace; and in MP4, it leaves padding
                    // where room was kept for the index.
                    .map(|bytes| bytes + self.measured.cut_ends.unwrap_or(0) as f64 + self.smart_cut_padding() as f64)
                    .map(|bytes| SizeEstimate::exact(bytes.max(0.0))),
                VideoAction::Encode(encode) => match (encode.codec, encode.bitrate_kbps) {
                    (_, Some(kbps)) => bitrate(f64::from(kbps)).map(SizeEstimate::exact),
                    (_, None) => self.quality_encode_estimate(step, encode),
                },
            },
        }?;
        let audio = match &self.audio {
            None => Some(0.0),
            Some(_) if self.strip_audio => Some(0.0),
            Some(step) => match step.action {
                // Audio inside a muxed stream is already counted with the video.
                AudioAction::Copy if step.format_id.is_none() => Some(0.0),
                AudioAction::Copy => self
                    .measured
                    .audio
                    .map(|bytes| bytes as f64)
                    .or_else(|| step.size.map(|size| size as f64 * self.source_fraction)),
                AudioAction::Encode {
                    codec: AudioCodec::Pcm, ..
                } => bitrate(1536.0),
                AudioAction::Encode {
                    bitrate_kbps: Some(kbps),
                    ..
                } => bitrate(f64::from(kbps)),
                AudioAction::Encode { .. } => step.source_kbps.and_then(bitrate),
            },
        }?;
        // A file's other sound tracks: copied with copied video, they are in
        // its size already.
        let video_copied = matches!(&self.video, Some(step) if step.action == VideoAction::Copy);
        let others: f64 = self
            .tracks
            .sounds
            .iter()
            .filter_map(|sound| match sound.action {
                AudioAction::Copy if video_copied => Some(0.0),
                AudioAction::Copy => sound.source_kbps.and_then(bitrate),
                AudioAction::Encode {
                    codec: AudioCodec::Pcm, ..
                } => bitrate(1536.0),
                AudioAction::Encode {
                    bitrate_kbps: Some(kbps),
                    ..
                } => bitrate(f64::from(kbps)),
                AudioAction::Encode { .. } => sound.source_kbps.and_then(bitrate),
            })
            .sum();
        Some(video.plus(audio + others))
    }

    /// A constant-quality encode's size: by the rule of thumb below, with a
    /// range, or as measured on the video. Measured on this very encode, it
    /// is near-certain (within about a tenth, docs/decisions.md#size-estimates);
    /// measured on another of the same video by the same encoder, close.
    fn quality_encode_estimate(&self, step: &VideoStep, encode: &VideoEncode) -> Option<SizeEstimate> {
        let duration = self.duration?;
        let (kbps, low, high) = self.quality_encode_kbps(step, encode)?;
        let bytes = |kbps: f64| kbps * 1000.0 / 8.0 * duration;
        let likely = bytes(kbps);
        let Some(factor) = self.measured.encode else {
            return Some(SizeEstimate::spread(likely, low, high));
        };
        let mut measured = likely * factor;
        if let Some(max) = encode.max_kbps {
            // Held under a cap: never more than the cap allows.
            measured = measured.min(bytes(f64::from(max)));
        }
        Some(if self.measured.encode_exact {
            SizeEstimate::exact(measured)
        } else {
            let (low, high) = sizes::RELATED;
            SizeEstimate::spread(measured, low, high)
        })
    }

    /// The padding a smart cut leaves in an MP4 or MOV (see
    /// `smart_cut::Index::unused`); one into another file type is copied out
    /// of it, without.
    fn smart_cut_padding(&self) -> u64 {
        let mp4 = matches!(self.container, Container::Mp4 | Container::Mov);
        let (Some(duration), true, true) = (self.duration, self.smart_cut(), mp4) else {
            return 0;
        };
        let fps = self.video.as_ref().and_then(|video| video.fps).unwrap_or(30.0);
        let audio = if self.audio_codec().is_some() { duration } else { 0.0 };
        crate::ffmpeg::smart_cut::Index::unused((duration * fps) as usize, audio)
    }

    /// What the rule of thumb takes a constant-quality encode of the video to
    /// come to, in bytes, before anything was measured: what a measurement
    /// is set against (see [`sizes::Measured::encode`]).
    pub fn rule_of_thumb_video(&self) -> Option<f64> {
        let step = self.video.as_ref()?;
        let encode = self.video_encode().filter(|encode| encode.bitrate_kbps.is_none())?;
        let (kbps, _, _) = self.quality_encode_kbps(step, encode)?;
        Some(kbps * 1000.0 / 8.0 * self.duration?)
    }

    /// What a constant-quality encode is likely to spend, in kbit/s, with the
    /// factors below and above it that cover most videos.
    ///
    /// Calibrated on this machine (2026-11-17, docs/decisions.md#size-estimates):
    /// the source's own bitrate says how complex the picture is, far better
    /// than resolution alone (a talking head needed a tenth of the bits per
    /// pixel of Big Buck Bunny). Scaled to the new size by pixels per second
    /// to the power 0.75, an encode at Siphon's quality settings lands at
    /// about 1.4× (a talking head) to 2.7× (busy animation) it for H.264 on
    /// the GPU, and about 1.5× for HEVC. Sites state an average bitrate, and
    /// a busy part of the same video can need twice the average.
    fn quality_encode_kbps(&self, step: &VideoStep, encode: &VideoEncode) -> Option<(f64, f64, f64)> {
        let full = self.duration.filter(|d| *d > 0.0)? / self.source_fraction.max(0.001);
        let source_kbps = step
            .kbps
            .or_else(|| step.size.map(|size| size as f64 * 8.0 / 1000.0 / full))?;
        let source_rate = f64::from(step.lines.max(1)).powi(2) * step.fps.unwrap_or(30.0);
        let output_rate =
            f64::from(self.output_lines().unwrap_or(step.lines).max(1)).powi(2) * self.output_fps().unwrap_or(30.0);
        let scale = (output_rate / source_rate).powf(0.75);
        let software = encode.hardware == Hardware::Software;
        let codec = match encode.codec {
            VideoCodec::H264 if software => 1.5,
            VideoCodec::H264 => 2.2,
            VideoCodec::Hevc if software => 1.0,
            VideoCodec::Hevc => 1.4,
            _ => 1.0,
        };
        // A newer codec needs fewer bits for the same picture, so its bitrate understates it.
        let source = match step.source {
            VideoCodec::Av1 => 1.1,
            VideoCodec::Hevc => 0.85,
            VideoCodec::H264 => 0.75,
            _ => 1.0,
        };
        // Held under a cap (drawing chat into Twitch's streams): a calm
        // stretch needs far less than the source, a busy one reaches the cap.
        // Measured 2026-11-21 on Twitch 1080p60: H.264 came to 0.6 times the
        // source calm and the cap busy; HEVC 0.4 and 1.2 (docs/chat.md).
        if let Some(max) = encode.max_kbps.map(f64::from).filter(|max| *max > 0.0) {
            let typical = if encode.codec == VideoCodec::Hevc { 0.75 } else { 1.0 };
            let likely = (source_kbps * scale * typical).min(max);
            return Some((likely, 0.5, max / likely));
        }
        // The fastest setting spends about a quarter more for the same
        // picture (measured 2026-11-21 on Twitch 1080p60, docs/chat.md).
        let speed = if encode.fast { 1.25 } else { 1.0 };
        // Content swings it two ways: half to double covers every case measured.
        Some((source_kbps * scale * codec * source * speed, 0.5, 2.0))
    }

    /// The same recipe, encoding on the CPU: for when a hardware encoder
    /// refuses a job it passed the probe for (driver limits, too many sessions).
    pub fn on_software(&self) -> Recipe {
        let mut recipe = self.clone();
        if let Some(VideoStep {
            action: VideoAction::Encode(encode),
            ..
        }) = &mut recipe.video
        {
            encode.hardware = Hardware::Software;
        }
        recipe
    }

    /// The same recipe for a picture already encoded as it says (a recording
    /// whose chat was drawn in as it recorded): the picture copied as it is.
    pub fn with_video_encoded(&self) -> Recipe {
        let mut recipe = self.clone();
        if let Some(video) = recipe.video.as_mut() {
            if let VideoAction::Encode(encode) = &video.action {
                video.source = encode.codec;
                video.action = VideoAction::Copy;
            }
        }
        recipe
    }

    /// Share: the same recipe aiming lower, after an encode came out at
    /// `actual` bytes, over the limit. `None` when there is nothing to lower.
    pub fn shrunk(&self, actual: u64) -> Option<Recipe> {
        let limit = self.size_limit?;
        let kbps = self.video_encode()?.bitrate_kbps?;
        // Scale by how far over it came out, aiming for the usual margin, and
        // always by a little at least.
        let scale = (limit as f64 * share::SAFETY / actual.max(1) as f64).min(0.95);
        let lower = ((f64::from(kbps) * scale) as u32).max(40);
        let mut recipe = self.clone();
        if let Some(VideoStep {
            action: VideoAction::Encode(encode),
            ..
        }) = &mut recipe.video
        {
            encode.bitrate_kbps = Some(lower);
        }
        Some(recipe)
    }
}

/// A size in bytes: the likely value, and the range it can reasonably land in.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SizeEstimate {
    pub low: u64,
    pub likely: u64,
    pub high: u64,
}

impl SizeEstimate {
    fn exact(bytes: f64) -> Self {
        Self::spread(bytes, 1.0, 1.0)
    }

    fn spread(likely: f64, low: f64, high: f64) -> Self {
        Self {
            low: (likely * low) as u64,
            likely: likely as u64,
            high: (likely * high) as u64,
        }
    }

    fn plus(self, bytes: f64) -> Self {
        let bytes = bytes as u64;
        Self {
            low: self.low + bytes,
            likely: self.likely + bytes,
            high: self.high + bytes,
        }
    }

    /// True when the range is wide enough to show instead of one number.
    pub fn is_uncertain(&self) -> bool {
        self.high > self.likely + self.likely / 10
    }
}

// ── Stream views ────────────────────────────────────────────────────────

#[derive(Clone, Copy)]
struct Video<'a> {
    format: &'a Format,
    codec: VideoCodec,
    lines: u32,
    fps: f64,
    hdr: bool,
}

impl Video<'_> {
    fn has_audio(&self) -> bool {
        self.format.kind == FormatKind::Muxed
    }

    /// Tie-breaker between streams of the same resolution and codec: a plain
    /// file beats a segmented stream (faster, and its size is known), then
    /// the higher bitrate wins.
    fn bitrate_rank(&self) -> (bool, f64) {
        (!self.format.segmented, self.format.tbr.unwrap_or(0.0))
    }

    /// Bytes of the whole stream, with its sound if a separate track goes with it.
    fn size_with(&self, audio: Option<&Audio>) -> Option<u64> {
        let video = self.format.filesize?;
        match (self.has_audio(), audio) {
            (true, _) => Some(video),
            (false, Some(audio)) => audio.format.filesize.map(|audio| video + audio),
            (false, None) => Some(video),
        }
    }
}

#[derive(Clone, Copy)]
struct Audio<'a> {
    format: &'a Format,
    codec: AudioCodec,
    kbps: f64,
}

impl Audio<'_> {
    /// Original-language tracks first, then ones without levelled ("DRC")
    /// volume, then plain files over segmented streams, then bitrate.
    fn rank(&self) -> (i32, bool, bool, f64) {
        let drc = self.format.id.contains("drc");
        // yt-dlp scores unmarked tracks -1, the same as "not the original language".
        (
            self.format.language_preference.unwrap_or(-1),
            !drc,
            !self.format.segmented,
            self.kbps,
        )
    }
}

fn lines_of(format: &Format) -> u32 {
    match (format.width, format.height) {
        (Some(w), Some(h)) if w > 0 && h > 0 => lines_for(w, h),
        (_, Some(h)) => h,
        _ => 0,
    }
}

/// The sizes frames are named by, smallest first.
const USUAL_LINES: [u32; 11] = [144, 240, 360, 480, 540, 576, 720, 1080, 1440, 2160, 4320];
/// How far a frame may be from a usual size and still take its name.
const USUAL_WITHIN: f64 = 1.08;

/// What a `width`×`height` frame is called ("1080p"), the way sites, players
/// and releases name it: by the 16:9 picture its width belongs to, or by its
/// own short side where that is taller. 1920×1080, vertical 1080×1920 and
/// 1440×1080 (4:3) are all 1080p; a film cropped to 3840×1600 is 2160p, as
/// YouTube and every release call it, not 1600p. Within 8% of a usual size it
/// takes that size's name: 4096×2160 is 2160p, a crop to 1916×798 is 1080p.
/// Every resolution in Siphon is one of these: what a stream is listed as,
/// what a preset or the quality row chooses, and what is scaled down to
/// (`scaled_to`), so what is shown is what is made.
pub fn lines_for(width: u32, height: u32) -> u32 {
    let (long, short) = (width.max(height), width.min(height));
    let lines = short.max((f64::from(long) * 9.0 / 16.0).round() as u32);
    let off = |usual: u32| (f64::from(lines) / f64::from(usual)).ln().abs();
    USUAL_LINES
        .iter()
        .copied()
        .min_by(|a, b| off(*a).total_cmp(&off(*b)))
        .filter(|usual| off(*usual) <= USUAL_WITHIN.ln())
        .unwrap_or(lines)
}

/// A `width`×`height` frame brought down to `lines` (as `lines_for` names
/// sizes), keeping its shape: it fits in the 16:9 picture of that name, the
/// right way up. 3840×1600 at 1080 is 1920×800, 1920×1080 at 720 is
/// 1280×720, vertical 1080×1920 at 720 is 720×1280. Never larger than it
/// was, and each side even, as encoders need.
pub fn scaled_to(width: u32, height: u32, lines: u32) -> (u32, u32) {
    let (long, short) = (width.max(height).max(1), width.min(height).max(1));
    let fit = (f64::from(lines) * 16.0 / 9.0 / f64::from(long))
        .min(f64::from(lines) / f64::from(short))
        .min(1.0);
    let even = |side: u32| ((f64::from(side) * fit / 2.0).round() as u32 * 2).max(2);
    (even(width), even(height))
}

fn max_by_key<T: Copy, K: PartialOrd>(items: impl IntoIterator<Item = T>, key: impl Fn(&T) -> K) -> Option<T> {
    items
        .into_iter()
        .max_by(|a, b| key(a).partial_cmp(&key(b)).unwrap_or(Ordering::Equal))
}

fn info(text: impl Into<String>) -> Hint {
    Hint {
        level: HintLevel::Info,
        title: None,
        text: text.into(),
    }
}

/// A warning: `title` names it in a few words, `text` says it in full.
fn warning(title: &str, text: impl Into<String>) -> Hint {
    Hint {
        level: HintLevel::Warning,
        title: Some(title.to_string()),
        text: text.into(),
    }
}

// ── Planning ────────────────────────────────────────────────────────────

/// Decides what to download and how to process it. Pure: no I/O.
pub fn plan(video: &VideoInfo, choice: &Choice, trim: Option<Trim>, caps: &Capabilities) -> Result<Recipe> {
    // A part ends where the video does: one asked past it was sized, and named
    // ("0m00s-0m30s"), for more than the file held. The trimmer never asks for one.
    let trim = trim.map(|trim| match video.duration {
        Some(duration) => Trim {
            end: trim.end.min(duration),
            ..trim
        },
        None => trim,
    });
    if trim.is_some_and(|trim| !(trim.start >= 0.0 && trim.end > trim.start)) {
        return Err(AppError::internal("The selected part of the video is empty."));
    }
    let planner = Planner::new(video, trim, caps);
    if planner.videos.is_empty() && planner.audios.is_empty() {
        return Err(AppError::new(
            ErrorKind::Unavailable,
            "This link has no downloadable video or audio.",
        ));
    }
    let mut recipe = match choice {
        Choice::Custom(manual) => planner.custom(manual)?,
        Choice::Preset(preset) => planner.preset(preset),
    };
    // Subtitles go with a file, not with a stream as it airs.
    if video.live.is_none() {
        subtitles::attach(&mut recipe, &video.subtitles, choice);
    }
    if let (Some(placement), Some(source)) = (choice.chat(), &video.chat) {
        let group = match choice {
            Choice::Preset(preset) => Some(preset.target.group()),
            Choice::Custom(_) => None,
        };
        // A VOD's replay goes with the video; a channel's chat, with a recording of it.
        let fits = match source {
            ChatSource::Vod { .. } => video.live.is_none(),
            ChatSource::Live { .. } => video.live.is_some(),
        };
        if matches!(group, Some(Group::Max | Group::Watch)) && fits {
            planner.draw_chat(
                &mut recipe,
                ChatStep {
                    source: source.clone(),
                    placement: placement.sanitized(),
                },
            );
        }
    }
    // A copied stream that is cut keeps its codec: only the frames at the
    // cuts are re-encoded (the planner only copies codecs that can be).
    if recipe.cut().is_some() {
        if let Some(video) = recipe.video.as_ref().filter(|video| video.action == VideoAction::Copy) {
            debug_assert!(Planner::smart_cuttable(video.source));
            recipe.cut_encode = Some(planner.encode(video.source));
        }
    }
    if recipe.trim.is_some() && caps.chunked_requests {
        recipe.section_request_size = request_size(&recipe, video);
    }
    if let Some(fallback) = recipe.fallback.as_deref_mut() {
        fallback.section_request_size = recipe.section_request_size;
    }
    // A file on this PC's other sound and subtitle tracks, and its main sound.
    if let Some(file) = &video.file {
        tracks::keep(&mut recipe, &file.tracks, choice, caps);
    }
    recipe.badge = badge(&recipe, choice);
    // A file on this PC already is what it is: keeping its own streams in
    // another file type changes nothing about where it plays.
    if video.file.is_some() && recipe.badge == Badge::MayNotPlay {
        recipe.badge = Badge::Lossless;
    }
    // What was measured on this video, if anything (see `sizes`).
    video.sizes.apply(&mut recipe);
    Ok(recipe)
}

/// The values the quality row offers for this choice and video, best first:
/// resolutions the video has, or bitrates for Music. Empty when there is no row.
pub fn quality_options(video: &VideoInfo, choice: &Choice, caps: &Capabilities) -> (QualityKind, Vec<u32>) {
    let Choice::Preset(preset) = choice else {
        return (QualityKind::None, Vec::new());
    };
    let planner = Planner::new(video, None, caps);
    let kind = preset.target.info().quality;
    let values = match kind {
        QualityKind::Resolution if planner.videos.is_empty() => Vec::new(),
        QualityKind::Resolution => {
            let (max_lines, max_fps) = match preset.target.group() {
                Group::Watch => (watch::MAX_LINES, watch::MAX_FPS),
                _ => (u32::MAX, f64::MAX),
            };
            resolutions(&planner.pool(max_lines, max_fps))
        }
        QualityKind::Bitrate => registry::MUSIC_BITRATES.iter().rev().copied().collect(),
        QualityKind::None => Vec::new(),
    };
    let kind = if values.is_empty() { QualityKind::None } else { kind };
    (kind, values)
}

/// Distinct resolutions on offer, highest first, without the tiny ones
/// (unless they are all there is).
fn resolutions(videos: &[Video]) -> Vec<u32> {
    let mut lines: Vec<u32> = videos.iter().map(|video| video.lines).filter(|l| *l > 0).collect();
    lines.sort_unstable_by(|a, b| b.cmp(a));
    lines.dedup();
    let useful: Vec<u32> = lines.iter().copied().filter(|lines| *lines >= 360).collect();
    if useful.is_empty() {
        lines
    } else {
        useful
    }
}

fn badge(recipe: &Recipe, choice: &Choice) -> Badge {
    let group = match choice {
        Choice::Preset(preset) => Some(preset.target.group()),
        Choice::Custom(_) => None,
    };
    let converts = recipe.encodes_video() || recipe.encodes_audio();
    match group {
        Some(Group::Max) if !converts => Badge::MayNotPlay,
        Some(Group::Share) if converts => Badge::SizeOptimized,
        _ if converts => Badge::Converts,
        _ => Badge::Lossless,
    }
}

/// The request size the site expects for the streams this recipe downloads,
/// when it names one (YouTube does: 10 MB).
fn request_size(recipe: &Recipe, video: &VideoInfo) -> Option<u64> {
    let chosen = [
        recipe.video.as_ref().map(|step| step.format_id.as_str()),
        recipe.audio.as_ref().and_then(|step| step.format_id.as_deref()),
    ];
    video
        .formats
        .iter()
        .filter(|format| chosen.contains(&Some(format.id.as_str())))
        .filter_map(|format| format.http_chunk_size)
        .min()
}

struct Planner<'a> {
    videos: Vec<Video<'a>>,
    audios: Vec<Audio<'a>>,
    /// Seconds of output: the trimmed length when trimming.
    duration: Option<f64>,
    trim: Option<Trim>,
    source_fraction: f64,
    caps: &'a Capabilities,
}

impl<'a> Planner<'a> {
    fn new(info: &'a VideoInfo, trim: Option<Trim>, caps: &'a Capabilities) -> Self {
        let videos = info
            .formats
            .iter()
            .filter(|format| format.kind != FormatKind::Audio)
            .map(|format| Video {
                format,
                codec: VideoCodec::of(format),
                lines: lines_of(format),
                fps: format.fps.unwrap_or(30.0),
                hdr: format.dynamic_range.as_deref().is_some_and(|range| range != "SDR"),
            })
            .collect();
        let audios = info
            .formats
            .iter()
            .filter(|format| format.kind == FormatKind::Audio)
            .map(|format| Audio {
                format,
                codec: AudioCodec::of(format),
                kbps: format.abr.or(format.tbr).unwrap_or(0.0),
            })
            .collect();
        let full = info.duration.filter(|d| *d > 0.0);
        Self {
            videos,
            audios,
            duration: trim.map(|trim| trim.duration()).or(full),
            trim,
            source_fraction: match (trim, full) {
                (Some(trim), Some(full)) => (trim.duration() / full).clamp(0.0, 1.0),
                _ => 1.0,
            },
            caps,
        }
    }

    fn preset(&self, choice: &PresetChoice) -> Recipe {
        let target = choice.target.info();
        // An audio-only source (a podcast, a song) makes every video preset an audio download.
        if self.videos.is_empty() && target.group != Group::Audio {
            return self.original_audio();
        }
        match target.group {
            Group::Watch => self.watch(target, choice.quality),
            Group::Audio if target.id == TargetId::Music => self.music(choice.music_format, choice.quality),
            Group::Audio => self.original_audio(),
            Group::Max => self.max_quality(),
            Group::Share => self.share(target, target.tier(choice.tier).expect("share targets have tiers")),
        }
    }

    /// True when a part of the video is cut out.
    fn cuts(&self) -> bool {
        self.trim.is_some()
    }

    /// Whether a copied stream of this codec can be cut on the exact frames
    /// by re-encoding only the frames at the cuts. Other codecs are
    /// converted when cut.
    fn smart_cuttable(codec: VideoCodec) -> bool {
        matches!(codec, VideoCodec::H264 | VideoCodec::Hevc)
    }

    /// A constant-quality encode to `codec` on the best encoder this machine has.
    fn encode(&self, codec: VideoCodec) -> VideoEncode {
        VideoEncode::quality(codec, self.caps.hardware(codec))
    }

    /// Draws the chat into the picture, which means encoding the video
    /// whatever the preset would have done with it: in the codec it was going
    /// to be converted to, or the one it has (H.264 rather than HEVC without
    /// an HEVC encoder on the graphics card), at the encoder's fastest setting.
    fn draw_chat(&self, recipe: &mut Recipe, step: ChatStep) {
        let Some(video) = recipe.video.as_mut() else {
            return;
        };
        let encode = match &video.action {
            VideoAction::Encode(encode) => encode.clone(),
            VideoAction::Copy => {
                let codec = match Self::cut_codec(video.source, video.lines) {
                    VideoCodec::Hevc if !self.caps.has_hardware(VideoCodec::Hevc) => VideoCodec::H264,
                    codec => codec,
                };
                self.encode(codec)
            }
        };
        // Never more than half again the source's bitrate: a busy scene comes
        // out a little softer than it went in (VMAF 94.8 against 96.6
        // uncapped) at less than half the size; a calm one doesn't reach it.
        let max_kbps = video.kbps.filter(|kbps| *kbps > 0.0).map(|kbps| (kbps * 1.5) as u32);
        video.action = VideoAction::Encode(VideoEncode {
            fast: true,
            max_kbps,
            ..encode
        });
        // The whole part is encoded anyway: nothing to cut around.
        recipe.cut_encode = None;
        recipe
            .reason
            .get_or_insert_with(|| "the chat is drawn into the picture".to_string());
        recipe.chat = Some(step);
    }

    /// The codec a cut stream is converted to when it can't be cut as it is.
    fn cut_codec(source: VideoCodec, lines: u32) -> VideoCodec {
        match source {
            VideoCodec::H264 | VideoCodec::Hevc => source,
            _ if lines > 1080 => VideoCodec::Hevc,
            _ => VideoCodec::H264,
        }
    }

    fn recipe(&self, container: Container, promise: impl Into<String>) -> Recipe {
        Recipe {
            video: None,
            audio: None,
            container,
            strip_audio: false,
            reason: None,
            promise: promise.into(),
            hints: Vec::new(),
            duration: self.duration,
            trim: self.trim,
            source_fraction: self.source_fraction,
            badge: Badge::Lossless,
            size_limit: None,
            fits: None,
            blocked: None,
            section_request_size: None,
            fallback: None,
            cut_encode: None,
            subtitles: None,
            chat: None,
            measured: sizes::Measured::default(),
            read_from: None,
            tracks: Default::default(),
        }
    }

    fn video_step(video: &Video, action: VideoAction) -> VideoStep {
        VideoStep {
            format_id: video.format.id.clone(),
            source: video.codec,
            lines: video.lines,
            width: video.format.width,
            height: video.format.height,
            fps: video.format.fps,
            hdr: video.hdr,
            size: video.format.filesize,
            kbps: video.format.vbr.or(video.format.tbr),
            action,
        }
    }

    /// Videos to choose from for a playback target: within its limits and,
    /// where the site offers both, SDR rather than HDR.
    fn pool(&self, max_lines: u32, max_fps: f64) -> Vec<Video<'a>> {
        let narrow = |videos: Vec<Video<'a>>, keep: &dyn Fn(&Video) -> bool| -> Vec<Video<'a>> {
            let kept: Vec<Video<'a>> = videos.iter().copied().filter(|v| keep(v)).collect();
            if kept.is_empty() {
                videos
            } else {
                kept
            }
        };
        let pool = narrow(self.videos.clone(), &|v| v.lines <= max_lines);
        let pool = narrow(pool, &|v| v.fps <= max_fps + 0.5);
        narrow(pool, &|v| !v.hdr)
    }

    /// The pool cut down to one resolution: the requested one, or the closest
    /// below it. Never above it: a lower choice downloads a lower stream.
    fn at_resolution(pool: &[Video<'a>], lines: u32) -> Vec<Video<'a>> {
        let target = pool
            .iter()
            .map(|video| video.lines)
            .filter(|available| *available <= lines)
            .max()
            .or_else(|| pool.iter().map(|video| video.lines).min());
        pool.iter()
            .copied()
            .filter(|video| Some(video.lines) == target)
            .collect()
    }

    /// Audio to go with a chosen video. `accepted` are the codecs that can be
    /// copied; anything else is converted to `fallback`.
    fn audio_for(&self, video: &Video, accepted: &[AudioCodec], fallback: AudioCodec) -> Option<AudioStep> {
        let step = |format_id: Option<String>, codec: AudioCodec, kbps: Option<f64>, size: Option<u64>| {
            let action = if accepted.contains(&codec) {
                AudioAction::Copy
            } else {
                AudioAction::encode(fallback, Some(lossy_target_kbps(kbps)))
            };
            AudioStep {
                format_id,
                source: codec,
                source_kbps: kbps,
                size,
                action,
            }
        };

        if video.has_audio() {
            return Some(step(None, AudioCodec::of(video.format), video.format.abr, None));
        }
        let best = self.best_audio_track(accepted)?;
        Some(step(
            Some(best.format.id.clone()),
            best.codec,
            Some(best.kbps).filter(|k| *k > 0.0),
            best.format.filesize,
        ))
    }

    /// The best separate audio track; a copyable one beats a slightly
    /// higher-bitrate one that would need converting.
    fn best_audio_track(&self, accepted: &[AudioCodec]) -> Option<Audio<'a>> {
        max_by_key(self.audios.iter().copied(), |audio| {
            let (language, plain, direct, kbps) = audio.rank();
            (language, plain, accepted.contains(&audio.codec), direct, kbps)
        })
    }

    /// The best audio track, or the one with `id`. Falls back to the audio
    /// inside a combined stream when the site has no separate audio.
    fn best_audio(&self, id: Option<&str>) -> Option<AudioStep> {
        let separate = match id {
            Some(id) => self.audios.iter().copied().find(|audio| audio.format.id == id),
            None => max_by_key(self.audios.iter().copied(), Audio::rank),
        };
        if let Some(audio) = separate {
            return Some(AudioStep {
                format_id: Some(audio.format.id.clone()),
                source: audio.codec,
                source_kbps: Some(audio.kbps).filter(|kbps| *kbps > 0.0),
                size: audio.format.filesize,
                action: AudioAction::Copy,
            });
        }
        // Smallest combined stream: the picture is discarded anyway.
        let muxed = self.videos.iter().copied().filter(Video::has_audio);
        let carrier = match id {
            Some(id) => muxed.clone().find(|video| video.format.id == id),
            None => max_by_key(muxed, |video| {
                (video.format.abr.unwrap_or(0.0), std::cmp::Reverse(video.lines))
            }),
        }?;
        Some(AudioStep {
            format_id: Some(carrier.format.id.clone()),
            source: AudioCodec::of(carrier.format),
            source_kbps: carrier.format.abr,
            size: None,
            action: AudioAction::Copy,
        })
    }

    // ── Max quality ─────────────────────────────────────────────────────

    fn max_quality(&self) -> Recipe {
        let video = max_by_key(self.videos.iter().copied(), |video| {
            (
                video.lines,
                video.fps as u32,
                video.hdr,
                video.codec.efficiency(),
                video.bitrate_rank(),
            )
        })
        .expect("plan() checks that videos exist");

        let mut recipe = self.recipe(Container::Mkv, "The best streams the site offers");
        // Every audio codec is accepted: nothing gets converted here.
        recipe.audio = self.audio_for(&video, ALL_AUDIO, AudioCodec::Aac);
        let (codec, action) = if self.cuts() && !Self::smart_cuttable(video.codec) {
            let codec = Self::cut_codec(video.codec, video.lines);
            recipe.reason = Some(format!(
                "{} can only be cut on the exact frames by converting it",
                video.codec.label()
            ));
            (codec, VideoAction::Encode(self.encode(codec)))
        } else {
            (video.codec, VideoAction::Copy)
        };
        recipe.container = Container::natural(Some(codec), recipe.audio.as_ref().map(|audio| audio.source));
        // (The group says it may not play everywhere; the badge repeats it.)
        recipe.video = Some(Self::video_step(&video, action));
        recipe
    }
}

const ALL_AUDIO: &[AudioCodec] = &[
    AudioCodec::Aac,
    AudioCodec::Opus,
    AudioCodec::Mp3,
    AudioCodec::Vorbis,
    AudioCodec::Flac,
    AudioCodec::Ac3,
    AudioCodec::Pcm,
    AudioCodec::Other,
];

/// Bitrate for converting lossy audio: close to the source, because bits
/// beyond what the source had are wasted, within sane bounds.
fn lossy_target_kbps(source_kbps: Option<f64>) -> u32 {
    match source_kbps {
        Some(kbps) if kbps > 0.0 => ((kbps * 1.2 / 32.0).ceil() as u32 * 32).clamp(96, 256),
        _ => 192,
    }
}

/// The honest note that goes with an audio conversion.
fn audio_conversion_hint(source: AudioCodec, target: AudioCodec, source_kbps: Option<f64>) -> Option<Hint> {
    // What the source is, where there is something to say: not "The source is audio."
    // for a codec Siphon doesn't name, of a bitrate nobody said.
    let described = match (source_kbps, source) {
        (Some(kbps), _) if kbps > 0.0 => Some(format!("{} kbps {}", kbps.round(), source.label())),
        (_, AudioCodec::Other) => None,
        _ => Some(source.label().to_string()),
    };
    let lead = described.map_or_else(String::new, |described| format!("The source is {described}. "));
    if source.is_lossless() {
        return None;
    }
    Some(if target.is_lossless() {
        warning(
            "Larger, not better",
            format!(
            "{lead}Converting it to {} can’t bring back detail that was never there; it only makes the file several times larger.",
            target.label()
        ),
        )
    } else {
        info(format!(
            "{lead}Converting it to {} compresses it a second time, which slightly lowers quality.",
            target.label()
        ))
    })
}

#[cfg(test)]
mod tests;
