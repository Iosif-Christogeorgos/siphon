//! Audio: Music (MP3 or M4A at a chosen bitrate) and the original track as it is.

use super::codecs::{AudioCodec, Container};
use super::registry::MusicFormat;
use super::{audio_conversion_hint, info, AudioAction, AudioStep, Planner, Recipe};

impl Planner<'_> {
    /// The best audio track exactly as the site has it.
    pub(super) fn original_audio(&self) -> Recipe {
        let mut recipe = self.recipe(Container::Mkv, "The original audio, untouched");
        recipe.audio = self.best_audio(None);
        recipe.container = Container::natural(None, recipe.audio.as_ref().map(|audio| audio.source));
        recipe
    }

    /// A compact file every player takes. `kbps` is the chosen bitrate; `None`
    /// keeps the source's own, copying it when it is already the right codec.
    pub(super) fn music(&self, format: MusicFormat, kbps: Option<u32>) -> Recipe {
        let (codec, container) = match format {
            MusicFormat::Mp3 => (AudioCodec::Mp3, Container::Mp3),
            MusicFormat::M4a => (AudioCodec::Aac, Container::M4a),
        };
        let mut recipe = self.recipe(container, "Plays in any music player");
        // A track already in the wanted codec can simply be copied.
        let track = self.best_audio_track(&[codec]).map(|track| AudioStep {
            format_id: Some(track.format.id.clone()),
            source: track.codec,
            source_kbps: Some(track.kbps).filter(|kbps| *kbps > 0.0),
            size: track.format.filesize,
            action: AudioAction::Copy,
        });
        let Some(mut audio) = track.or_else(|| self.best_audio(None)) else {
            return recipe;
        };
        let source_kbps = audio.source_kbps;
        // Re-encoding to a higher bitrate than the source had only adds size.
        let same_codec = audio.source == codec;
        let no_smaller = |kbps: u32| source_kbps.is_none_or(|source| f64::from(kbps) >= source * 0.9);
        let copy = same_codec && kbps.is_none_or(no_smaller);
        if !copy {
            let target = kbps.unwrap_or_else(|| super::lossy_target_kbps(source_kbps).min(320));
            if let Some(source) = source_kbps.filter(|source| f64::from(target) > source * 1.25) {
                recipe.hints.push(info(format!(
                    "The source is about {} kbps, so {target} kbps only makes the file larger, not better.",
                    source.round()
                )));
            } else {
                recipe
                    .hints
                    .extend(audio_conversion_hint(audio.source, codec, source_kbps));
            }
            audio.action = AudioAction::encode(codec, Some(target));
        }
        recipe.audio = Some(audio);
        recipe
    }
}
