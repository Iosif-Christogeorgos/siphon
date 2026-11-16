//! Turns "what the user asked for" into "exactly what will happen".
//!
//! [`plan`] is a pure function from a video's formats and the chosen options
//! to a [`Recipe`]: which streams to fetch, whether each is copied or
//! converted, and into which container. The same recipe drives both the
//! plain-language summary shown before downloading and the yt-dlp/ffmpeg
//! commands, so the app can never describe one thing and do another.
//!
//! Guiding rule: copy original streams whenever the target can play them.
//! Converting is slower and never improves quality, so it only happens when
//! the target needs it or the user explicitly asks.

pub mod codecs;
pub mod explain;
pub mod presets;

use std::cmp::Ordering;

use serde::{Deserialize, Serialize};

use crate::error::{AppError, ErrorKind, Result};
use crate::ytdlp::metadata::{Format, FormatKind, VideoInfo};
use codecs::{AudioCodec, Container, VideoCodec};
use presets::{DeviceProfile, PresetId};

// ── Options (from the frontend) ─────────────────────────────────────────

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum DownloadOptions {
    Preset {
        preset: PresetId,
        /// A specific resolution (short side, e.g. 1080). `None` lets the preset decide.
        #[serde(default)]
        max_lines: Option<u32>,
        /// Share preset only: the upload limit to fit under.
        #[serde(default)]
        size_limit_mb: Option<u32>,
    },
    Manual(ManualOptions),
}

/// The part of the video to keep, in seconds from its start.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
pub struct Trim {
    pub start: f64,
    pub end: f64,
    /// Cut on the exact frames, which means re-encoding the video. Otherwise
    /// the original stream is kept and the cut lands on the nearest key frame
    /// before `start` (up to a few seconds early).
    pub exact: bool,
}

impl Trim {
    pub fn duration(&self) -> f64 {
        self.end - self.start
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Content {
    VideoAudio,
    VideoOnly,
    AudioOnly,
}

/// Everything in manual mode is optional: `None` means "best available" for
/// streams and "keep the original" for codecs and container.
#[derive(Debug, Clone, Deserialize)]
pub struct ManualOptions {
    pub content: Content,
    #[serde(default)]
    pub video_format: Option<String>,
    #[serde(default)]
    pub audio_format: Option<String>,
    #[serde(default)]
    pub container: Option<Container>,
    #[serde(default)]
    pub video_codec: Option<VideoCodec>,
    #[serde(default)]
    pub audio_codec: Option<AudioCodec>,
}

// ── Recipe (what will happen) ───────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub struct Recipe {
    pub video: Option<VideoStep>,
    pub audio: Option<AudioStep>,
    pub container: Container,
    /// Drop the audio track of a stream that has video and audio in one.
    pub strip_audio: bool,
    /// Why something is converted, as a clause: "iPhone can't play VP9".
    pub reason: Option<String>,
    /// "Plays natively on iPhone", "Ready for Premiere and DaVinci Resolve", …
    pub promise: String,
    pub hints: Vec<Hint>,
    /// Resolutions the user could pick instead.
    pub qualities: Vec<QualityOption>,
    /// Seconds of output, when known (needed for size estimates and conversion progress).
    pub duration: Option<f64>,
    /// Only this part of the video is downloaded.
    pub trim: Option<Trim>,
    /// Share of each source stream that gets downloaded: 1 unless trimmed.
    pub source_fraction: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct VideoStep {
    pub format_id: String,
    pub source: VideoCodec,
    /// Short side in pixels of the source stream.
    pub lines: u32,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub fps: Option<f64>,
    pub hdr: bool,
    /// Bytes of the source stream.
    pub size: Option<u64>,
    pub action: VideoAction,
}

#[derive(Debug, Clone, PartialEq)]
pub enum VideoAction {
    Copy,
    Encode(VideoEncode),
}

#[derive(Debug, Clone, PartialEq)]
pub struct VideoEncode {
    pub codec: VideoCodec,
    /// Target bitrate for size-limited output; `None` means constant quality.
    pub bitrate_kbps: Option<u32>,
    /// Scale down to this short side (never up).
    pub scale_lines: Option<u32>,
    pub max_fps: Option<f64>,
    /// Force a constant frame rate (editing).
    pub constant_frame_rate: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AudioStep {
    /// `None` when the audio rides along inside the video stream.
    pub format_id: Option<String>,
    pub source: AudioCodec,
    pub source_kbps: Option<f64>,
    pub size: Option<u64>,
    pub action: AudioAction,
}

#[derive(Debug, Clone, PartialEq)]
pub enum AudioAction {
    Copy,
    Encode {
        codec: AudioCodec,
        bitrate_kbps: Option<u32>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HintLevel {
    Info,
    Warning,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Hint {
    pub level: HintLevel,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct QualityOption {
    pub lines: u32,
    /// Picking this resolution means converting the video.
    pub converts: bool,
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

    pub fn encodes_audio(&self) -> bool {
        matches!(
            self.audio,
            Some(AudioStep {
                action: AudioAction::Encode { .. },
                ..
            })
        )
    }

    /// True when Siphon runs its own ffmpeg pass after the download.
    /// (Audio-only conversions are left to yt-dlp's extractor instead.)
    pub fn needs_ffmpeg_pass(&self) -> bool {
        self.video.is_some() && (self.encodes_video() || self.encodes_audio() || self.strip_audio)
    }

    /// The range to cut frame-exactly in the ffmpeg pass, if any. (Audio-only
    /// downloads are cut by yt-dlp: audio has no key frames to worry about.)
    pub fn exact_cut(&self) -> Option<Trim> {
        self.trim.filter(|trim| trim.exact && self.video.is_some())
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

    /// Output short side after any downscale.
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
            VideoAction::Encode(VideoEncode { max_fps: Some(max), .. }) if fps > *max => fps / 2.0,
            _ => fps,
        })
    }

    /// Bytes yt-dlp will fetch: the chosen streams, scaled to the trimmed part.
    pub fn estimated_source_bytes(&self) -> Option<u64> {
        let video = self.video.as_ref().map_or(Some(0), |step| step.size)?;
        let audio = match &self.audio {
            Some(AudioStep {
                format_id: Some(_),
                size,
                ..
            }) => (*size)?,
            _ => 0,
        };
        Some(((video + audio) as f64 * self.source_fraction) as u64)
    }

    /// Best guess at the finished file's size in bytes.
    pub fn estimated_size(&self) -> Option<u64> {
        let duration = self.duration;
        let video = match &self.video {
            None => Some(0),
            Some(step) => match &step.action {
                VideoAction::Copy => step.size.map(|size| (size as f64 * self.source_fraction) as u64),
                VideoAction::Encode(encode) => match (encode.codec, encode.bitrate_kbps) {
                    (_, Some(kbps)) => duration.map(|d| (kbps as f64 * 1000.0 / 8.0 * d) as u64),
                    (VideoCodec::Dnxhr, None) => duration.map(|d| (self.dnxhr_bitrate() / 8.0 * d) as u64),
                    // Constant-quality encodes depend on the content.
                    _ => None,
                },
            },
        }?;
        let audio = match &self.audio {
            None => Some(0),
            Some(_) if self.strip_audio => Some(0),
            Some(step) => match step.action {
                // Audio inside a muxed stream is already counted with the video.
                AudioAction::Copy if step.format_id.is_none() => Some(0),
                AudioAction::Copy => step.size.map(|size| (size as f64 * self.source_fraction) as u64),
                AudioAction::Encode {
                    codec: AudioCodec::Pcm, ..
                } => duration.map(|d| (1536.0 * 1000.0 / 8.0 * d) as u64),
                AudioAction::Encode {
                    bitrate_kbps: Some(kbps),
                    ..
                } => duration.map(|d| (kbps as f64 * 1000.0 / 8.0 * d) as u64),
                AudioAction::Encode { .. } => None,
            },
        }?;
        Some(video + audio)
    }

    /// DNxHR HQ is ~220 Mbit/s at 1080p30 and scales with pixels and frame rate.
    fn dnxhr_bitrate(&self) -> f64 {
        let Some(step) = &self.video else { return 0.0 };
        let pixels = match (step.width, step.height) {
            (Some(w), Some(h)) => (w * h) as f64,
            _ => (step.lines * step.lines) as f64 * 16.0 / 9.0,
        };
        220e6 * (pixels / (1920.0 * 1080.0)) * (step.fps.unwrap_or(30.0) / 30.0)
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

/// Short side of the frame: 1080 for both 1920×1080 and vertical 1080×1920.
fn lines_of(format: &Format) -> u32 {
    match (format.width, format.height) {
        (Some(w), Some(h)) => w.min(h),
        (None, Some(h)) => h,
        _ => 0,
    }
}

fn max_by_key<T: Copy, K: PartialOrd>(items: impl IntoIterator<Item = T>, key: impl Fn(&T) -> K) -> Option<T> {
    items
        .into_iter()
        .max_by(|a, b| key(a).partial_cmp(&key(b)).unwrap_or(Ordering::Equal))
}

fn info(text: impl Into<String>) -> Hint {
    Hint {
        level: HintLevel::Info,
        text: text.into(),
    }
}

fn warning(text: impl Into<String>) -> Hint {
    Hint {
        level: HintLevel::Warning,
        text: text.into(),
    }
}

// ── Planning ────────────────────────────────────────────────────────────

/// Decides what to download and how to process it. Pure: no I/O.
pub fn plan(video: &VideoInfo, options: &DownloadOptions, trim: Option<Trim>) -> Result<Recipe> {
    if trim.is_some_and(|trim| !(trim.start >= 0.0 && trim.end > trim.start)) {
        return Err(AppError::internal("The selected part of the video is empty."));
    }
    let planner = Planner::new(video, trim);
    let mut recipe = planner.dispatch(options)?;
    if recipe.exact_cut().is_some() && recipe.reason.is_none() {
        recipe.reason = Some("an exact cut has to rebuild the frames at its edges".into());
    }
    Ok(recipe)
}

struct Planner<'a> {
    videos: Vec<Video<'a>>,
    audios: Vec<Audio<'a>>,
    /// Seconds of output: the trimmed length when trimming.
    duration: Option<f64>,
    trim: Option<Trim>,
    source_fraction: f64,
}

impl<'a> Planner<'a> {
    fn dispatch(&self, options: &DownloadOptions) -> Result<Recipe> {
        let planner = self;
        if planner.videos.is_empty() && planner.audios.is_empty() {
            return Err(AppError::new(
                ErrorKind::Unavailable,
                "This link has no downloadable video or audio.",
            ));
        }
        match options {
            DownloadOptions::Manual(manual) => planner.manual(manual),
            // An audio-only source (a podcast, a song) makes every preset an audio download.
            DownloadOptions::Preset { .. } if planner.videos.is_empty() => Ok(planner.audio_only()),
            DownloadOptions::Preset {
                preset,
                max_lines,
                size_limit_mb,
            } => Ok(match presets::device(*preset) {
                Some(profile) => planner.device(profile, *max_lines),
                None => match preset {
                    PresetId::Audio => planner.audio_only(),
                    PresetId::Editing => planner.editing(*max_lines),
                    PresetId::Share => planner.share(size_limit_mb.unwrap_or(presets::SHARE_LIMITS_MB[0])),
                    _ => planner.max_quality(*max_lines),
                },
            }),
        }
    }

    /// True when the video will be cut on exact frames, so it is re-encoded
    /// whatever else is decided.
    fn cuts(&self) -> bool {
        self.trim.is_some_and(|trim| trim.exact)
    }

    /// The codec an exact cut is encoded with when the stream would otherwise
    /// have been copied: the same one where a hardware encoder exists.
    fn cut_codec(source: VideoCodec, lines: u32) -> VideoCodec {
        match source {
            VideoCodec::H264 | VideoCodec::Hevc => source,
            _ if lines > 1080 => VideoCodec::Hevc,
            _ => VideoCodec::H264,
        }
    }

    fn encode(codec: VideoCodec) -> VideoAction {
        VideoAction::Encode(VideoEncode {
            codec,
            bitrate_kbps: None,
            scale_lines: None,
            max_fps: None,
            constant_frame_rate: false,
        })
    }

    fn new(info: &'a VideoInfo, trim: Option<Trim>) -> Self {
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
            qualities: Vec::new(),
            duration: self.duration,
            trim: self.trim,
            source_fraction: self.source_fraction,
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
            action,
        }
    }

    /// Distinct resolutions on offer, highest first.
    fn resolutions(videos: &[Video]) -> Vec<u32> {
        let mut lines: Vec<u32> = videos
            .iter()
            .map(|video| video.lines)
            .filter(|lines| *lines > 0)
            .collect();
        lines.sort_unstable_by(|a, b| b.cmp(a));
        lines.dedup();
        lines
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

    /// The pool cut down to one resolution: the requested one, or the closest below it.
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
                AudioAction::Encode {
                    codec: fallback,
                    bitrate_kbps: Some(lossy_target_kbps(kbps)),
                }
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
        // A copyable track beats a slightly higher-bitrate one that needs converting.
        let best = max_by_key(self.audios.iter().copied(), |audio| {
            let (language, plain, direct, kbps) = audio.rank();
            (language, plain, accepted.contains(&audio.codec), direct, kbps)
        })?;
        Some(step(
            Some(best.format.id.clone()),
            best.codec,
            Some(best.kbps).filter(|k| *k > 0.0),
            best.format.filesize,
        ))
    }

    // ── Device presets ──────────────────────────────────────────────────

    fn device(&self, profile: &DeviceProfile, max_lines: Option<u32>) -> Recipe {
        let pool = self.pool(profile.max_lines, profile.max_fps);
        let cut = self.cuts();
        // An exact cut re-encodes the video anyway. Streams whose codec has a
        // hardware encoder are re-encoded as they are; the rest are treated
        // like unplayable ones and converted to the device's own format.
        let native_rank = |video: &Video| {
            profile
                .tier_of(video.codec)
                .filter(|_| !cut || matches!(video.codec, VideoCodec::H264 | VideoCodec::Hevc))
                .map(|(index, _)| profile.tiers.len() - index)
        };

        let playable_lines = pool
            .iter()
            .filter(|v| profile.tier_of(v.codec).is_some())
            .map(|v| v.lines)
            .max();
        let candidates = match (max_lines, playable_lines) {
            (Some(lines), _) => Self::at_resolution(&pool, lines),
            // Cutting must not change which resolution the preset picks.
            (None, Some(lines)) if cut => Self::at_resolution(&pool, lines),
            (None, _) => pool.clone(),
        };
        // Highest resolution and frame rate among natively playable streams …
        let native = max_by_key(
            candidates.iter().copied().filter(|v| native_rank(v).is_some()),
            |video| (video.lines, video.fps as u32, native_rank(video), video.bitrate_rank()),
        );
        // … otherwise the best stream of any kind, to be converted.
        let any = max_by_key(candidates.iter().copied(), |video| {
            (
                video.lines,
                video.fps as u32,
                video.codec.efficiency(),
                video.bitrate_rank(),
            )
        });

        let mut recipe = self.recipe(Container::Mp4, format!("Plays natively on {}", profile.name));
        recipe.qualities = Self::resolutions(&pool)
            .into_iter()
            .map(|lines| QualityOption {
                lines,
                converts: !pool.iter().any(|v| v.lines == lines && native_rank(v).is_some()),
            })
            .collect();

        match (native, any) {
            (Some(video), _) => {
                let (_, tier) = profile.tier_of(video.codec).expect("native videos have a tier");
                recipe.container = tier.container;
                recipe.audio = self.audio_for(&video, tier.audio, tier.container.default_audio());
                if let Some(caveat) = tier.caveat {
                    recipe.hints.push(info(caveat));
                }
                if recipe.encodes_audio() {
                    let source = recipe
                        .audio
                        .as_ref()
                        .map(|audio| audio.source.label())
                        .unwrap_or("audio");
                    recipe.reason = Some(format!("{} can’t play {source} audio in this file type", profile.name));
                }
                // Tell the user when a sharper version exists but would need converting.
                let best_available = pool.iter().map(|v| v.lines).max().unwrap_or(0);
                if max_lines.is_none() && best_available > video.lines {
                    recipe.hints.push(info(format!(
                        "{best_available}p is available too, but {} can’t play it as-is. Pick it above to convert it (slower).",
                        profile.name
                    )));
                }
                let action = if cut {
                    Self::encode(video.codec)
                } else {
                    VideoAction::Copy
                };
                recipe.video = Some(Self::video_step(&video, action));
            }
            (None, Some(video)) => {
                let codec = profile.encode_target(video.lines);
                let accepted = profile
                    .tier_of(codec)
                    .map(|(_, tier)| tier.audio)
                    .unwrap_or(&[AudioCodec::Aac]);
                recipe.audio = self.audio_for(&video, accepted, AudioCodec::Aac);
                recipe.reason = Some(format!(
                    "{} can’t play {} out of the box",
                    profile.name,
                    video.codec.label()
                ));
                recipe.video = Some(Self::video_step(
                    &video,
                    VideoAction::Encode(VideoEncode {
                        codec,
                        bitrate_kbps: None,
                        scale_lines: None,
                        max_fps: None,
                        constant_frame_rate: false,
                    }),
                ));
            }
            (None, None) => unreachable!("plan() checks that videos exist"),
        }
        if recipe.video.as_ref().is_some_and(|video| video.hdr) {
            recipe.hints.push(warning(
                "This video is HDR. On screens without HDR, colours may look washed out.",
            ));
        }
        recipe
    }

    // ── Maximum quality ─────────────────────────────────────────────────

    fn max_quality(&self, max_lines: Option<u32>) -> Recipe {
        let candidates = match max_lines {
            Some(lines) => Self::at_resolution(&self.videos, lines),
            None => self.videos.clone(),
        };
        let video = max_by_key(candidates.iter().copied(), |video| {
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
        let (codec, action) = if self.cuts() {
            let codec = Self::cut_codec(video.codec, video.lines);
            (codec, Self::encode(codec))
        } else {
            (video.codec, VideoAction::Copy)
        };
        recipe.container = Container::natural(Some(codec), recipe.audio.as_ref().map(|audio| audio.source));
        recipe.qualities = Self::resolutions(&self.videos)
            .into_iter()
            .map(|lines| QualityOption { lines, converts: false })
            .collect();
        if matches!(codec, VideoCodec::Av1 | VideoCodec::Vp9) || recipe.container != Container::Mp4 {
            recipe.hints.push(info(
                "Not every device plays this format. VLC does; for a phone or TV, a device preset is the safer pick.",
            ));
        }
        recipe.video = Some(Self::video_step(&video, action));
        recipe
    }

    // ── Audio only ──────────────────────────────────────────────────────

    fn audio_only(&self) -> Recipe {
        let mut recipe = self.recipe(Container::Mkv, "The original audio, untouched");
        recipe.audio = self.best_audio(None);
        recipe.container = Container::natural(None, recipe.audio.as_ref().map(|audio| audio.source));
        recipe
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

    // ── Editing ─────────────────────────────────────────────────────────

    /// DNxHR HQ + PCM in MOV at a constant frame rate: every frame is a key
    /// frame, so Premiere and Resolve scrub it smoothly and cut it exactly.
    fn editing(&self, max_lines: Option<u32>) -> Recipe {
        // 1080p unless asked: editing files are enormous.
        let pool = self.pool(max_lines.unwrap_or(1080), 120.0);
        let video = max_by_key(Self::at_resolution(&pool, max_lines.unwrap_or(1080)), |video| {
            (
                video.lines,
                video.fps as u32,
                video.codec.efficiency(),
                video.bitrate_rank(),
            )
        })
        .expect("plan() checks that videos exist");

        let mut recipe = self.recipe(Container::Mov, "Ready for Premiere Pro and DaVinci Resolve");
        recipe.audio = self.audio_for(&video, &[], AudioCodec::Pcm);
        recipe.reason = Some("editors work best with a constant frame rate and frames that stand on their own".into());
        recipe.qualities = Self::resolutions(&self.pool(2160, 120.0))
            .into_iter()
            .map(|lines| QualityOption { lines, converts: false })
            .collect();
        recipe.video = Some(Self::video_step(
            &video,
            VideoAction::Encode(VideoEncode {
                codec: VideoCodec::Dnxhr,
                bitrate_kbps: None,
                scale_lines: None,
                max_fps: None,
                constant_frame_rate: true,
            }),
        ));
        recipe.hints.push(info(
            "Editing files are large by design: they trade size for smooth scrubbing. Picture quality matches the source; it can’t exceed it.",
        ));
        recipe
    }

    // ── Share (size limit) ──────────────────────────────────────────────

    fn share(&self, limit_mb: u32) -> Recipe {
        // Aim a little under the limit: services count megabytes differently.
        let budget = limit_mb as f64 * 1_000_000.0 * 0.96;
        let promise = format!("Fits under {limit_mb} MB");
        let mut recipe = self.recipe(Container::Mp4, promise);
        let pool = self.pool(1080, 60.0);

        // Sharing wants a small stereo track, not a 384 kbps surround mix that eats the budget.
        let modest: Vec<Audio> = self
            .audios
            .iter()
            .copied()
            .filter(|audio| audio.kbps <= 160.0)
            .collect();
        let lean = Planner {
            videos: Vec::new(),
            audios: if modest.is_empty() { self.audios.clone() } else { modest },
            duration: self.duration,
            trim: self.trim,
            source_fraction: self.source_fraction,
        };

        // First choice: an H.264 + AAC combination that already fits. No conversion at all.
        let aac = max_by_key(
            lean.audios.iter().copied().filter(|a| a.codec == AudioCodec::Aac),
            Audio::rank,
        );
        let fits = pool
            .iter()
            .copied()
            .filter(|video| video.codec == VideoCodec::H264)
            .filter(|video| {
                let total = match (video.has_audio(), video.format.filesize) {
                    (true, Some(size)) if AudioCodec::of(video.format) == AudioCodec::Aac => Some(size),
                    (false, Some(size)) => aac.and_then(|audio| audio.format.filesize).map(|audio| size + audio),
                    _ => None,
                };
                total.is_some_and(|total| total as f64 <= budget)
            });
        // (An exact cut is re-encoded anyway, so it goes straight to the bitrate route.)
        let fits = fits.filter(|_| !self.cuts());
        if let Some(video) = max_by_key(fits, |video| (video.lines, video.fps as u32, video.bitrate_rank())) {
            // Don't settle for a postage stamp when converting would look far better.
            if video.lines >= 480 || video.lines >= pool.iter().map(|v| v.lines).max().unwrap_or(0) {
                recipe.audio = lean.audio_for(&video, &[AudioCodec::Aac], AudioCodec::Aac);
                recipe.video = Some(Self::video_step(&video, VideoAction::Copy));
                return recipe;
            }
        }

        // Otherwise convert to a bitrate that lands under the limit.
        let Some(duration) = self.duration else {
            let video = max_by_key(Self::at_resolution(&pool, 480), |video| video.bitrate_rank())
                .expect("plan() checks that videos exist");
            recipe.audio = lean.audio_for(&video, &[AudioCodec::Aac], AudioCodec::Aac);
            recipe.video = Some(Self::video_step(&video, VideoAction::Copy));
            recipe.promise = "A small version of the video".into();
            recipe.hints.push(warning(format!(
                "The site didn’t say how long this video is, so Siphon can’t guarantee it fits under {limit_mb} MB."
            )));
            return recipe;
        };

        let total_kbps = budget * 8.0 / duration / 1000.0;
        let audio_kbps: u32 = if total_kbps >= 700.0 {
            96
        } else if total_kbps >= 250.0 {
            64
        } else {
            48
        };
        // ~2% of an MP4 at these sizes is container overhead.
        let video_kbps = (total_kbps * 0.98 - audio_kbps as f64).max(40.0) as u32;
        let (target_lines, max_fps) = match video_kbps {
            4500.. => (1080, 60.0),
            2200.. => (720, 60.0),
            1000.. => (720, 30.0),
            500.. => (480, 30.0),
            250.. => (360, 30.0),
            _ => (240, 30.0),
        };

        // Fetch the smallest source that still covers the target resolution.
        let source_lines = pool
            .iter()
            .map(|video| video.lines)
            .filter(|lines| *lines >= target_lines)
            .min()
            .or_else(|| pool.iter().map(|video| video.lines).max())
            .unwrap_or(0);
        let video = max_by_key(pool.iter().copied().filter(|v| v.lines == source_lines), |video| {
            (
                video.fps <= max_fps + 0.5,
                video.codec == VideoCodec::H264,
                video.bitrate_rank(),
            )
        })
        .expect("plan() checks that videos exist");

        recipe.audio = lean.audio_for(&video, &[], AudioCodec::Aac).map(|mut audio| {
            audio.action = AudioAction::Encode {
                codec: AudioCodec::Aac,
                bitrate_kbps: Some(audio_kbps),
            };
            audio
        });
        recipe.reason = Some(format!("no original stream fits under {limit_mb} MB"));
        recipe.video = Some(Self::video_step(
            &video,
            VideoAction::Encode(VideoEncode {
                codec: VideoCodec::H264,
                bitrate_kbps: Some(video_kbps),
                scale_lines: (video.lines > target_lines).then_some(target_lines),
                max_fps: Some(max_fps),
                constant_frame_rate: false,
            }),
        ));
        if video_kbps < 250 {
            recipe.hints.push(warning(format!(
                "This video is long for a {limit_mb} MB limit, so it will look rough. A larger limit, or trimming it, will help."
            )));
        }
        recipe
    }

    // ── Manual mode ─────────────────────────────────────────────────────

    fn manual(&self, options: &ManualOptions) -> Result<Recipe> {
        let unknown = |id: &str| {
            AppError::new(
                ErrorKind::Unavailable,
                format!("The chosen stream ({id}) isn’t offered for this video any more."),
            )
        };
        let mut recipe = self.recipe(Container::Mkv, "Your custom settings");

        // Audio only.
        if options.content == Content::AudioOnly || self.videos.is_empty() {
            let mut audio = self
                .best_audio(options.audio_format.as_deref())
                .ok_or_else(|| AppError::new(ErrorKind::Unavailable, "This video has no audio track."))?;
            let container = options.container.filter(|container| container.is_audio_only());
            let target = options
                .audio_codec
                .or(container
                    .filter(|c| !c.holds_audio(audio.source))
                    .map(Container::default_audio))
                .unwrap_or(audio.source);
            if target != audio.source {
                recipe
                    .hints
                    .extend(audio_conversion_hint(audio.source, target, audio.source_kbps));
                audio.action = AudioAction::Encode {
                    codec: target,
                    bitrate_kbps: None,
                };
            }
            recipe.container = container
                .filter(|container| container.holds_audio(target))
                .unwrap_or_else(|| Container::natural(None, Some(target)));
            recipe.audio = Some(audio);
            return Ok(recipe);
        }

        // Video, with or without audio.
        let want_audio = options.content == Content::VideoAudio;
        let video = match options.video_format.as_deref() {
            Some(id) => self
                .videos
                .iter()
                .copied()
                .find(|video| video.format.id == id)
                .ok_or_else(|| unknown(id))?,
            None => max_by_key(self.videos.iter().copied(), |video| {
                // With "video only", prefer streams that don't carry audio to strip.
                (
                    want_audio || !video.has_audio(),
                    video.lines,
                    video.fps as u32,
                    video.codec.efficiency(),
                    video.bitrate_rank(),
                )
            })
            .expect("videos is not empty"),
        };

        let mut audio = match (want_audio, options.audio_format.as_deref()) {
            (false, _) => None,
            (true, Some(id)) => Some(self.best_audio(Some(id)).ok_or_else(|| unknown(id))?),
            (true, None) => self.audio_for(&video, ALL_AUDIO, AudioCodec::Aac),
        };
        // A combined stream already has sound; a second audio download replaces nothing.
        if video.has_audio()
            && audio
                .as_ref()
                .is_some_and(|step| step.format_id.as_deref() == Some(&video.format.id))
        {
            audio = audio.map(|step| AudioStep {
                format_id: None,
                ..step
            });
        }
        recipe.strip_audio = !want_audio && video.has_audio();

        let cut = self.cuts();
        let kept = if cut {
            Self::cut_codec(video.codec, video.lines)
        } else {
            video.codec
        };
        let wanted_video = options.video_codec.unwrap_or(kept);
        let wanted_audio = audio.as_ref().map(|step| options.audio_codec.unwrap_or(step.source));
        let container = options
            .container
            .filter(|container| !container.is_audio_only())
            .unwrap_or_else(|| Container::natural(Some(wanted_video), wanted_audio));

        // The container has the final say: swap in a codec it can hold.
        let video_target = if container.holds_video(wanted_video) {
            wanted_video
        } else {
            container.default_video()
        };
        let audio_target = wanted_audio.map(|codec| {
            if container.holds_audio(codec) {
                codec
            } else {
                container.default_audio()
            }
        });

        if video_target != wanted_video {
            recipe.hints.push(info(format!(
                "{} files can’t hold {} video, so it will be converted to {}.",
                container.label(),
                wanted_video.label(),
                video_target.label()
            )));
        }
        if let (Some(target), Some(wanted)) = (audio_target, wanted_audio) {
            if target != wanted {
                recipe.hints.push(info(format!(
                    "{} files can’t hold {} audio, so it will be converted to {}.",
                    container.label(),
                    wanted.label(),
                    target.label()
                )));
            }
        }

        let video_action = if video_target == video.codec {
            if cut {
                Self::encode(video_target)
            } else {
                VideoAction::Copy
            }
        } else if options.video_codec.is_none() && cut {
            // Not the user's choice: the cut needs an encode and this codec has no fast encoder.
            Self::encode(video_target)
        } else {
            recipe.hints.push(warning(format!(
                "Converting {} to {} takes time and loses a little quality; it never adds any. Keep the original codec unless a device needs {}.",
                video.codec.label(),
                video_target.label(),
                video_target.label()
            )));
            VideoAction::Encode(VideoEncode {
                codec: video_target,
                bitrate_kbps: None,
                scale_lines: None,
                max_fps: None,
                constant_frame_rate: false,
            })
        };
        if let (Some(step), Some(target)) = (audio.as_mut(), audio_target) {
            if target != step.source {
                recipe
                    .hints
                    .extend(audio_conversion_hint(step.source, target, step.source_kbps));
                let bitrate_kbps = (!target.is_lossless()).then(|| lossy_target_kbps(step.source_kbps));
                step.action = AudioAction::Encode {
                    codec: target,
                    bitrate_kbps,
                };
            }
        }

        recipe.container = container;
        recipe.video = Some(Self::video_step(&video, video_action));
        recipe.audio = audio;
        Ok(recipe)
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
    let described = match source_kbps {
        Some(kbps) if kbps > 0.0 => format!("{} kbps {}", kbps.round(), source.label()),
        _ => source.label().to_string(),
    };
    if source.is_lossless() {
        return None;
    }
    Some(if target.is_lossless() {
        warning(format!(
            "The source is {described}. Converting it to {} can’t bring back detail that was never there; it only makes the file several times larger.",
            target.label()
        ))
    } else {
        info(format!(
            "The source is {described}. Converting it to {} compresses it a second time, which slightly lowers quality.",
            target.label()
        ))
    })
}

#[cfg(test)]
mod tests;
