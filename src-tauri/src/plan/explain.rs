//! Describes a [`Recipe`] in plain language for the UI.

use serde::Serialize;

use super::codecs::{AudioCodec, VideoCodec};
use super::{AudioAction, Hint, QualityOption, Recipe, VideoAction};
use crate::ffmpeg::encoders::{Encoders, Hardware};

#[derive(Debug, Clone, Serialize)]
pub struct PlanSummary {
    /// "1080p60 H.264 + AAC in MP4"
    pub headline: String,
    /// "Plays natively on iPhone. Original streams, no re-encoding."
    pub detail: String,
    /// True when anything is converted (slower, slightly lossy).
    pub converts: bool,
    /// Bytes, when it can be predicted.
    pub estimated_size: Option<u64>,
    pub hints: Vec<Hint>,
    /// Resolutions the user can pick, highest first.
    pub qualities: Vec<QualityOption>,
    /// The resolution this plan resolved to (short side).
    pub lines: Option<u32>,
    /// File extension of the result.
    pub ext: &'static str,
}

pub fn summarize(recipe: &Recipe, encoders: &Encoders) -> PlanSummary {
    PlanSummary {
        headline: headline(recipe),
        detail: detail(recipe, encoders),
        converts: recipe.encodes_video() || recipe.encodes_audio(),
        estimated_size: recipe.estimated_size(),
        hints: recipe.hints.clone(),
        qualities: recipe.qualities.clone(),
        lines: recipe.video.as_ref().map(|video| video.lines),
        ext: recipe.container.ext(),
    }
}

/// "1080p60", "2160p", or "" when the site didn't report a resolution.
fn resolution(lines: Option<u32>, fps: Option<f64>) -> String {
    match lines {
        Some(lines) if lines > 0 => {
            let fps = fps
                .filter(|fps| *fps > 30.5)
                .map(|fps| fps.round().to_string())
                .unwrap_or_default();
            format!("{lines}p{fps}")
        }
        _ => String::new(),
    }
}

fn headline(recipe: &Recipe) -> String {
    let container = recipe.container.label();
    let Some(video_codec) = recipe.video_codec() else {
        // Audio only: "Opus 160 kbps in an Opus file" reads oddly, so keep it short.
        let Some(audio) = &recipe.audio else {
            return container.to_string();
        };
        let codec = recipe.audio_codec().unwrap_or(audio.source);
        return match (&audio.action, audio.source_kbps) {
            (AudioAction::Copy, Some(kbps)) => format!("{} · {} kbps", codec.label(), kbps.round()),
            _ => codec.label().to_string(),
        };
    };

    let mut parts = vec![resolution(recipe.output_lines(), recipe.output_fps())];
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

fn detail(recipe: &Recipe, encoders: &Encoders) -> String {
    let promise = &recipe.promise;
    let reason = recipe
        .reason
        .as_deref()
        .map(|reason| format!(" because {reason}"))
        .unwrap_or_default();

    let Some(video) = &recipe.video else {
        return match recipe.audio.as_ref().map(|audio| (&audio.action, audio.source)) {
            Some((AudioAction::Encode { codec, .. }, source)) => {
                format!("{promise}. Converted from {} to {}.", source.label(), codec.label())
            }
            _ => format!("{promise}. No re-encoding."),
        };
    };

    match &video.action {
        VideoAction::Copy if recipe.encodes_audio() => {
            let audio = recipe.audio.as_ref().expect("encodes_audio implies audio");
            let target = recipe.audio_codec().map(AudioCodec::label).unwrap_or("AAC");
            format!(
                "{promise}. The video is untouched; only the audio is converted from {} to {target}{reason}.",
                audio.source.label()
            )
        }
        VideoAction::Copy => format!("{promise}. Original streams, no re-encoding."),
        VideoAction::Encode(encode) => {
            let how = match encode.codec {
                // DNxHR has no hardware encoders; it is fast on any CPU.
                VideoCodec::Dnxhr => String::new(),
                codec => format!(" {}", encoder_phrase(encoders.for_codec(codec))),
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
        Hardware::VideoToolbox => "with Apple’s hardware encoder",
        Hardware::Software => "on the CPU (no hardware encoder found, so this is slower)",
    }
}
