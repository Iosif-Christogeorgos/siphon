//! Translates a [`Recipe`] into yt-dlp arguments.

use super::metadata::VideoInfo;
use crate::plan::codecs::{AudioCodec, Container, VideoCodec};
use crate::plan::{AudioAction, Recipe};

/// Output name while a conversion is still to come: `Title [id].source.mkv`.
/// The marker keeps the intermediate apart from the finished file.
pub const SOURCE_MARKER: &str = ".source";

/// For every yt-dlp run that downloads media.
pub const TRANSFER_ARGS: [&str; 11] = [
    // Segmented streams (HLS, DASH), which is what most sites serve, are
    // fetched one piece at a time unless told otherwise. Four at once
    // measured 2.4 times faster (23 → 55 MiB/s); eight added nothing.
    "--concurrent-fragments",
    "4",
    // When a connection drops, yt-dlp picks the transfer up where it broke.
    // By default it does so ten times with no pause, which is over within a
    // second: no use against an outage of any length. Five tries spread over
    // 1, 2, 4, 8 and 8 seconds ride out a short one, and after that Siphon's
    // own retry (see `downloads::run`) takes over.
    "--retries",
    "5",
    "--fragment-retries",
    "5",
    "--retry-sleep",
    "http:exp=1:8",
    "--retry-sleep",
    "fragment:exp=1:8",
    // yt-dlp's default is to leave out a piece it couldn't fetch and carry
    // on, which gives a file with a hole in it. Fail instead, so the download
    // is tried again and continues from that piece.
    "--abort-on-unavailable-fragments",
];

/// The full output template: the user's naming style (`name`, see
/// `Settings::filename_template`) plus what Siphon adds itself.
pub fn output_template(recipe: &Recipe, name: &str) -> String {
    let marker = if recipe.needs_ffmpeg_pass() { SOURCE_MARKER } else { "" };
    // Clips say which part they are, so several from one video don't collide.
    let clip = match recipe.trim {
        Some(trim) => format!(" ({}-{})", clock(trim.start), clock(trim.end)),
        None => String::new(),
    };
    format!("{name}{clip}{marker}.%(ext)s")
}

/// 75.4 → "1m15s". File-name safe (no colons) and free of yt-dlp's "%".
fn clock(seconds: f64) -> String {
    let total = seconds.max(0.0).floor() as u64;
    match (total / 3600, total % 3600 / 60, total % 60) {
        (0, minutes, seconds) => format!("{minutes}m{seconds:02}s"),
        (hours, minutes, seconds) => format!("{hours}h{minutes:02}m{seconds:02}s"),
    }
}

/// The request size the site expects for the streams this recipe downloads,
/// when it names one (YouTube does: 10 MB).
pub fn request_size(recipe: &Recipe, video: &VideoInfo) -> Option<u64> {
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

/// The arguments that select streams and do the lossless post-processing
/// (merge, remux, audio extraction). Conversions are not yt-dlp's job; see
/// `crate::ffmpeg`.
///
/// `section_request_size` is the size of the requests ffmpeg should fetch a
/// trimmed part in; `None` leaves ffmpeg to ask for everything at once.
pub fn recipe_args(recipe: &Recipe, section_request_size: Option<u64>) -> Vec<String> {
    let mut args: Vec<String> = vec!["--format".into(), recipe.format_selector()];
    let mut push = |values: &[&str]| args.extend(values.iter().map(|value| value.to_string()));

    if let Some(trim) = recipe.trim {
        // Fetch only the selected part. yt-dlp cuts the streams without
        // re-encoding them, starting at the key frame before `start`.
        //
        // Deliberately not using --force-keyframes-at-cuts: it makes yt-dlp
        // re-encode video *and* audio with ffmpeg's defaults (for example VP9
        // at CRF 32 and Opus at 96 kbit/s), on the CPU, with no progress.
        // Exact cuts are made afterwards in `crate::ffmpeg` instead: one
        // hardware-accelerated encode of the video, audio left untouched.
        push(&["--download-sections", &format!("*{}-{}", trim.start, trim.end)]);

        // yt-dlp hands a section to ffmpeg; this is what ffmpeg is told about its inputs.
        let mut input_args: Vec<String> = Vec::new();
        if recipe.exact_cut().is_some() {
            // Keep the original timestamps so the exact cut can find its frames.
            input_args.push("-copyts".into());
        }
        if let Some(size) = section_request_size {
            // Left alone, ffmpeg asks for the rest of the stream in one request,
            // and YouTube answers that at about twice playback speed. Measured
            // on a five-minute 1080p part: 172 s that way, 6 s in 10 MB requests.
            input_args.push(format!("-request_size {size}"));
        }
        if !input_args.is_empty() {
            push(&["--downloader-args", &format!("ffmpeg_i:{}", input_args.join(" "))]);
        }
    }

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
