//! Custom: the user picks the streams, file type and codecs themselves.

use super::codecs::{AudioCodec, Container};
use super::{
    audio_conversion_hint, info, lossy_target_kbps, max_by_key, warning, AudioAction, AudioStep, Content,
    ManualOptions, Planner, Recipe, VideoAction, ALL_AUDIO,
};
use crate::error::{AppError, ErrorKind, Result};

impl Planner<'_> {
    pub(super) fn custom(&self, options: &ManualOptions) -> Result<Recipe> {
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
                audio.action = AudioAction::encode(target, None);
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
        let kept = if cut && !Self::smart_cuttable(video.codec) {
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
            VideoAction::Copy
        } else if options.video_codec.is_none() && cut {
            // Not the user's choice: this codec can only be cut by converting it.
            VideoAction::Encode(self.encode(video_target))
        } else {
            recipe.hints.push(warning(format!(
                "Converting {} to {} takes time and loses a little quality; it never adds any. Keep the original codec unless a device needs {}.",
                video.codec.label(),
                video_target.label(),
                video_target.label()
            )));
            VideoAction::Encode(self.encode(video_target))
        };
        if let (Some(step), Some(target)) = (audio.as_mut(), audio_target) {
            if target != step.source {
                recipe
                    .hints
                    .extend(audio_conversion_hint(step.source, target, step.source_kbps));
                let bitrate_kbps = (!target.is_lossless()).then(|| lossy_target_kbps(step.source_kbps));
                step.action = AudioAction::encode(target, bitrate_kbps);
            }
        }

        recipe.container = container;
        recipe.video = Some(Self::video_step(&video, video_action));
        recipe.audio = audio;
        Ok(recipe)
    }
}
