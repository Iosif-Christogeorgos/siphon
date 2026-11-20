//! Describes a plan in plain language for the summary line.

use serde::Serialize;

use super::codecs::{AudioCodec, VideoCodec};
use super::registry::{Group, QualityKind};
use super::subtitles::{self, Delivery};
use super::{
    plan, quality_options, AudioAction, Badge, Capabilities, Choice, Hint, PresetChoice, Recipe, Trim, VideoAction,
};
use crate::error::Result;
use crate::ffmpeg::encoders::Hardware;
use crate::ytdlp::metadata::VideoInfo;

#[derive(Debug, Clone, Serialize)]
pub struct PlanSummary {
    pub badge: Badge,
    /// "1080p60 H.264 + AAC in MP4", "720p30 · fits Discord (10 MB)"
    pub headline: String,
    /// "Plays natively on Windows 11. Original streams, no re-encoding."
    pub detail: String,
    /// Bytes, when it can be predicted: the likely value.
    pub estimated_size: Option<u64>,
    /// Bytes, low and high, when the size depends on the picture (a
    /// constant-quality conversion): show this range rather than one number.
    pub size_range: Option<[u64; 2]>,
    /// Roughly how long converting takes, in seconds, when anything is converted.
    pub convert_seconds: Option<f64>,
    pub hints: Vec<Hint>,
    /// What the quality row shows, and its choices, best first.
    pub quality: QualityKind,
    pub options: Vec<QualityOption>,
    /// The quality row value this plan came to (a resolution, or a bitrate for Music).
    pub resolved: Option<u32>,
    /// File extension of the result.
    pub ext: &'static str,
    /// Why Download isn't offered (Share: too long to look decent).
    pub blocked: Option<String>,
    /// How subtitles would reach the file with this choice, in any language;
    /// None when it can take none (audio, Share).
    pub subtitle_delivery: Option<Delivery>,
    /// The subtitle track the plan adds (its key), if any.
    pub subtitle_track: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct QualityOption {
    pub value: u32,
    /// Picking this means converting.
    pub converts: bool,
    pub estimated_size: Option<u64>,
    pub size_range: Option<[u64; 2]>,
}

/// Plans `choice` and describes the result, along with the size each other
/// quality would come to.
pub fn summarize(video: &VideoInfo, choice: &Choice, trim: Option<Trim>, caps: &Capabilities) -> Result<PlanSummary> {
    let recipe = plan(video, choice, trim, caps)?;
    let (quality, values) = quality_options(video, choice, caps);
    let options = match choice {
        Choice::Preset(preset) => values
            .into_iter()
            .filter_map(|value| {
                let other = Choice::Preset(PresetChoice {
                    quality: Some(value),
                    ..preset.clone()
                });
                let other = plan(video, &other, trim, caps).ok()?;
                Some(QualityOption {
                    value,
                    converts: other.encodes_video() || other.encodes_audio(),
                    estimated_size: other.estimated_size(),
                    size_range: range(&other),
                })
            })
            .collect(),
        Choice::Custom(_) => Vec::new(),
    };
    let resolved = match quality {
        QualityKind::Resolution => recipe.output_lines(),
        QualityKind::Bitrate => recipe.audio.as_ref().and_then(|audio| match audio.action {
            AudioAction::Encode { bitrate_kbps, .. } => bitrate_kbps,
            AudioAction::Copy => audio.source_kbps.map(|kbps| kbps.round() as u32),
        }),
        QualityKind::None => None,
    };
    let is_share = matches!(choice, Choice::Preset(preset) if preset.target.group() == Group::Share);
    Ok(PlanSummary {
        badge: recipe.badge,
        headline: headline(&recipe, is_share),
        detail: detail(&recipe),
        estimated_size: recipe.estimated_size(),
        size_range: range(&recipe),
        convert_seconds: convert_seconds(&recipe),
        hints: recipe.hints.clone(),
        quality,
        options,
        resolved,
        ext: recipe.container.ext(),
        blocked: recipe.blocked.clone(),
        subtitle_delivery: subtitles::delivery(choice, &recipe),
        subtitle_track: recipe.subtitles.as_ref().map(|step| step.key.clone()),
    })
}

fn range(recipe: &Recipe) -> Option<[u64; 2]> {
    recipe
        .estimate()
        .filter(|estimate| estimate.is_uncertain())
        .map(|estimate| [estimate.low, estimate.high])
}

/// "1080p60", "2160p", or "" when the site didn't report a resolution.
/// `always_fps` shows the frame rate even at 30 or below ("720p30").
fn resolution(lines: Option<u32>, fps: Option<f64>, always_fps: bool) -> String {
    match lines {
        Some(lines) if lines > 0 => {
            let fps = fps
                .filter(|fps| always_fps || *fps > 30.5)
                .map(|fps| fps.round().to_string())
                .unwrap_or_default();
            format!("{lines}p{fps}")
        }
        _ => String::new(),
    }
}

fn headline(recipe: &Recipe, is_share: bool) -> String {
    let container = recipe.container.label();
    let Some(video_codec) = recipe.video_codec() else {
        // Audio only: "Opus 160 kbps in an Opus file" reads oddly, so keep it short.
        let Some(audio) = &recipe.audio else {
            return container.to_string();
        };
        let codec = recipe.audio_codec().unwrap_or(audio.source);
        let kbps = match audio.action {
            AudioAction::Encode { bitrate_kbps, .. } => bitrate_kbps.map(f64::from),
            AudioAction::Copy => audio.source_kbps,
        };
        return match kbps {
            Some(kbps) => format!("{} · {} kbps", codec.label(), kbps.round()),
            None => codec.label().to_string(),
        };
    };

    if let (true, Some(fits)) = (is_share, &recipe.fits) {
        let size = resolution(recipe.output_lines(), recipe.output_fps(), true);
        return [size.as_str(), fits]
            .iter()
            .filter(|part| !part.is_empty())
            .copied()
            .collect::<Vec<_>>()
            .join(" · ");
    }

    let mut parts = vec![resolution(recipe.output_lines(), recipe.output_fps(), false)];
    if recipe.video.as_ref().is_some_and(|video| video.hdr) {
        parts.push("HDR".into());
    }
    parts.push(video_codec.label().into());
    let video = parts
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ");

    match recipe.audio_codec() {
        Some(AudioCodec::Pcm) => format!("{video} + uncompressed audio in {container}"),
        Some(audio) => format!("{video} + {} in {container}", audio.label()),
        None => format!("{video} in {container}, no audio"),
    }
}

/// What the plan does, in a sentence or two, and where its subtitles go.
fn detail(recipe: &Recipe) -> String {
    let streams = streams_detail(recipe);
    match &recipe.subtitles {
        Some(step) => match step.delivery {
            Delivery::Embed => format!(
                "{streams} {} subtitles are inside the file, as a track you can turn on and off.",
                step.title
            ),
            Delivery::File => format!(
                "{streams} {} subtitles come as an SRT file beside the video, to import as captions.",
                step.title
            ),
            Delivery::BurnIn => format!("{streams} {} subtitles are drawn into the picture.", step.title),
        },
        None => streams,
    }
}

fn streams_detail(recipe: &Recipe) -> String {
    let promise = &recipe.promise;
    let reason = recipe
        .reason
        .as_deref()
        .map(|reason| format!(" because {reason}"))
        .unwrap_or_default();

    let Some(video) = &recipe.video else {
        return match recipe.audio.as_ref().map(|audio| (&audio.action, audio.source)) {
            Some((AudioAction::Encode { codec, .. }, source)) if *codec != source => {
                format!("{promise}. Converted from {} to {}.", source.label(), codec.label())
            }
            Some((AudioAction::Encode { .. }, _)) => format!("{promise}. Re-encoded at a lower bitrate to save space."),
            _ => format!("{promise}. No re-encoding."),
        };
    };

    // A smart cut re-encodes the few frames at its cuts and nothing else.
    let cut = if recipe.smart_cut() {
        " Cut on the exact frames: only the frames at the cuts are re-encoded."
    } else {
        ""
    };
    match &video.action {
        VideoAction::Copy if recipe.encodes_audio() => {
            let audio = recipe.audio.as_ref().expect("encodes_audio implies audio");
            let target = recipe.audio_codec().map(AudioCodec::label).unwrap_or("AAC");
            format!(
                "{promise}. The video is untouched; only the audio is converted from {} to {target}{reason}.{cut}",
                audio.source.label()
            )
        }
        VideoAction::Copy if recipe.smart_cut() => format!("{promise}. Original streams.{cut}"),
        VideoAction::Copy => format!("{promise}. Original streams, no re-encoding."),
        VideoAction::Encode(encode) => {
            let how = match encode.codec {
                // ProRes has no hardware encoder here; it is quick on any CPU.
                VideoCodec::Prores => String::new(),
                _ => format!(" {}", encoder_phrase(encode.hardware)),
            };
            let scaled = match encode.scale_lines {
                Some(lines) if lines < video.lines => format!(" and scaled down from {}p", video.lines),
                _ => String::new(),
            };
            format!(
                "{promise}. Converted from {}{scaled}{how}{reason}.",
                video.source.label()
            )
        }
    }
}

fn encoder_phrase(hardware: Hardware) -> &'static str {
    match hardware {
        Hardware::Nvenc => "on your NVIDIA graphics card",
        Hardware::Qsv => "with Intel Quick Sync",
        Hardware::Amf => "on your AMD graphics card",
        Hardware::Software => "on the CPU (no hardware encoder found, so this is slower)",
    }
}

/// A rough idea of how long the conversion takes, in seconds. Speeds are
/// typical multiples of real time at 1080p30, scaled by pixels per second.
fn convert_seconds(recipe: &Recipe) -> Option<f64> {
    if recipe.smart_cut() {
        // A few seconds for the frames at the cuts and the checks, then
        // copying the part about twice over, at disk speed.
        let bytes = recipe.estimated_source_bytes().unwrap_or(0) as f64;
        return Some(3.0 + bytes / 250_000_000.0);
    }
    let duration = recipe.duration?;
    let Some(encode) = recipe.video_encode() else {
        // Audio on its own converts at about sixty times real time.
        return recipe.encodes_audio().then_some(duration / 60.0);
    };
    let lines = f64::from(recipe.output_lines().unwrap_or(1080).max(1));
    let load = (lines / 1080.0).powi(2) * recipe.output_fps().unwrap_or(30.0) / 30.0;
    let speed = match (encode.codec, encode.hardware) {
        (VideoCodec::Prores, _) => 4.0,
        (_, Hardware::Nvenc) => 10.0,
        (_, Hardware::Qsv | Hardware::Amf) => 7.0,
        (VideoCodec::H264, Hardware::Software) => 2.5,
        (VideoCodec::Hevc, Hardware::Software) => 0.5,
        (VideoCodec::Av1, Hardware::Software) => 0.8,
        (_, Hardware::Software) => 0.4,
    };
    let passes = if encode.two_pass() { 1.6 } else { 1.0 };
    Some(duration * load / speed * passes)
}
