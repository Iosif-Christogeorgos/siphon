//! Translates a [`Recipe`] into yt-dlp arguments.

use crate::plan::codecs::{AudioCodec, Container, VideoCodec};
use crate::plan::{AudioAction, Recipe};

/// Output name while a conversion is still to come: `Title [id].source.mkv`.
/// The marker keeps the intermediate apart from the finished file.
pub const SOURCE_MARKER: &str = ".source";

/// File name template. The title is capped in bytes so paths stay within OS limits.
pub fn output_template(recipe: &Recipe) -> String {
    let marker = if recipe.needs_ffmpeg_pass() { SOURCE_MARKER } else { "" };
    format!("%(title).180B [%(id)s]{marker}.%(ext)s")
}

/// The arguments that select streams and do the lossless post-processing
/// (merge, remux, audio extraction). Conversions are not yt-dlp's job; see
/// `crate::ffmpeg`.
pub fn recipe_args(recipe: &Recipe) -> Vec<String> {
    let mut args: Vec<String> = vec!["--format".into(), recipe.format_selector()];
    let mut push = |values: &[&str]| args.extend(values.iter().map(|value| value.to_string()));

    if recipe.needs_ffmpeg_pass() {
        // Matroska holds any codec, so it is a safe stopover before converting.
        push(&["--merge-output-format", "mkv"]);
    } else if recipe.video.is_some() {
        let ext = recipe.container.ext();
        // Covers both shapes: two streams to merge, or one file to re-wrap.
        push(&["--merge-output-format", ext, "--remux-video", ext]);
        if matches!(recipe.container, Container::Mp4 | Container::Mov) {
            let mut flags = "-movflags +faststart".to_string();
            if recipe.video_codec() == Some(VideoCodec::Hevc) {
                // Apple players only recognise HEVC tagged "hvc1".
                flags.push_str(" -tag:v hvc1");
            }
            for postprocessor in ["Merger", "VideoRemuxer"] {
                push(&["--postprocessor-args", &format!("{postprocessor}:{flags}")]);
            }
        }
    } else if let Some(audio) = &recipe.audio {
        // Audio only. Without --audio-format, yt-dlp keeps the codec and just
        // moves it into its natural container (Opus → .opus, AAC → .m4a).
        push(&["--extract-audio"]);
        let natural = Container::natural(None, Some(audio.source));
        match audio.action {
            AudioAction::Encode { codec, .. } => {
                push(&["--audio-format", audio_format(codec), "--audio-quality", "0"]);
            }
            AudioAction::Copy if recipe.container != natural => {
                push(&["--audio-format", audio_format(recipe.container.default_audio())]);
            }
            AudioAction::Copy => {}
        }
    }
    args
}

/// yt-dlp's `--audio-format` name for a codec.
fn audio_format(codec: AudioCodec) -> &'static str {
    match codec {
        AudioCodec::Mp3 => "mp3",
        AudioCodec::Opus => "opus",
        AudioCodec::Vorbis => "vorbis",
        AudioCodec::Flac => "flac",
        AudioCodec::Pcm => "wav",
        AudioCodec::Aac | AudioCodec::Ac3 | AudioCodec::Other => "m4a",
    }
}
