//! Translates a [`Recipe`] into yt-dlp arguments.

use crate::plan::codecs::{AudioCodec, Container, VideoCodec};
use crate::plan::{AudioAction, Recipe};

/// How long a trimmed part's download may receive nothing before it counts
/// as stalled. Generous: YouTube pauses between requests now and then.
pub const SECTION_READ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);

/// How far past the end of a cut part the video is fetched. The last frames
/// before the end may be decoded from frames after it (B-frames), and the
/// cut needs them whole.
const CUT_MARGIN: f64 = 1.0;

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

/// What a download's files carry in their names until it is finished:
/// `Title.siphon-1a2b3c4d.mp4`. Unique to the download, so yt-dlp never takes
/// a file that is already in the folder for its own: it would count it as
/// "already downloaded" and work on it. It stays the same for every try at
/// the same download, so a later try carries on from the files an earlier one left.
const OWN_MARKER: &str = ".siphon-";

/// This download's marker (see [`OWN_MARKER`]).
pub fn own_marker(download_id: &str) -> String {
    let own: String = download_id
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .take(8)
        .collect();
    format!("{OWN_MARKER}{}", if own.is_empty() { "0" } else { &own })
}

/// A file's name without what Siphon added while working on it:
/// `Title.siphon-1a2b3c4d` or `Title.source` → `Title`.
pub fn finished_stem(stem: &str) -> &str {
    let stem = stem.strip_suffix(SOURCE_MARKER).unwrap_or(stem);
    match stem.rfind(OWN_MARKER) {
        Some(at) if stem[at + OWN_MARKER.len()..].chars().all(|c| c.is_ascii_alphanumeric()) => &stem[..at],
        _ => stem,
    }
}

/// The full output template: the user's naming style (`name`, see
/// `Settings::filename_template`) plus what Siphon adds itself. A converted
/// source goes to the download's own work folder, so `.source` is enough
/// there; anything written to the download folder carries `own` (see
/// [`own_marker`]) until Siphon gives it its final, free name.
pub fn output_template(recipe: &Recipe, name: &str, own: &str) -> String {
    let marker = if recipe.needs_ffmpeg_pass() { SOURCE_MARKER } else { own };
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

/// The arguments that select streams and do the lossless post-processing
/// (merge, remux, audio extraction). Conversions are not yt-dlp's job; see
/// `crate::ffmpeg`.
///
/// A trimmed part is fetched in `recipe.section_request_size` requests when
/// the site names a size; otherwise ffmpeg asks for everything at once.
pub fn recipe_args(recipe: &Recipe) -> Vec<String> {
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
        let end = if recipe.cut().is_some() {
            trim.end + CUT_MARGIN
        } else {
            trim.end
        };
        push(&["--download-sections", &format!("*{}-{end}", trim.start)]);

        // yt-dlp hands a section to ffmpeg; this is what ffmpeg is told about its inputs.
        // Left alone, ffmpeg waits forever on a connection that stops sending,
        // and the download sits still for good. With a read timeout it fails
        // instead, and is tried again (see `downloads::run`).
        // ffmpeg's errors are what tell such a part apart (see
        // `errors::section_cut_short`), and yt-dlp silences them unless asked.
        let mut input_args: Vec<String> = vec![
            format!("-rw_timeout {}", SECTION_READ_TIMEOUT.as_micros()),
            "-loglevel error".into(),
        ];
        if recipe.cut().is_some() {
            // Keep the original timestamps so the cut can find its
            // frames, counted from the start of the video. Not every stream
            // starts at zero: Twitch's start about a minute in, and without
            // -start_at_zero the cut missed every video frame of the part.
            input_args.push("-copyts".into());
            input_args.push("-start_at_zero".into());
        }
        if let Some(size) = recipe.section_request_size {
            // Left alone, ffmpeg asks for the rest of the stream in one request,
            // and YouTube answers that at about twice playback speed. Measured
            // on a five-minute 1080p part: 172 s that way, 6 s in 10 MB requests.
            input_args.push(format!("-request_size {size}"));
        }
        push(&["--downloader-args", &format!("ffmpeg_i:{}", input_args.join(" "))]);
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
            AudioAction::Encode {
                codec, bitrate_kbps, ..
            } => {
                // A set bitrate, or else the encoder's best variable quality.
                let quality = bitrate_kbps.map_or("0".to_string(), |kbps| format!("{kbps}K"));
                push(&["--audio-format", audio_format(codec), "--audio-quality", &quality]);
            }
            AudioAction::Copy if recipe.container != natural => {
                push(&["--audio-format", audio_format(recipe.container.default_audio())]);
            }
            AudioAction::Copy => {}
        }
        push(audio_tag_args(recipe.container));
    }
    args
}

/// Title, artist, date and the video's thumbnail as cover art, so a music
/// player shows more than a file name. Audio only: writing tags means one
/// more pass over the whole file, which is nothing for a song but a full
/// copy of a long video. WAV gets no cover, because yt-dlp can't embed one
/// there and fails the download if asked.
fn audio_tag_args(container: Container) -> &'static [&'static str] {
    match container {
        Container::Mp3 | Container::M4a | Container::Opus | Container::Ogg | Container::Flac => {
            &["--embed-metadata", "--embed-thumbnail"]
        }
        _ => &["--embed-metadata"],
    }
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

/// The arguments for recording a live stream with `recipe` (see
/// `downloads::live`). What it records is kept as it comes, in a file that a
/// crash, a power cut or a stopped recording leaves playable up to that
/// moment; making it into the finished file is Siphon's own step afterwards.
pub fn record_args(recipe: &Recipe, from_start: bool) -> Vec<String> {
    let mut args: Vec<String> = vec!["--format".into(), recipe.format_selector()];
    let mut push = |values: &[&str]| args.extend(values.iter().map(|value| value.to_string()));
    push(&[
        // MPEG-TS has no index to write at the end: every second recorded
        // stays playable whatever happens to the process. (yt-dlp's default
        // for a live stream already; said here so it stays that way.)
        "--hls-use-mpegts",
        // No fix-ups: the recording is made into its file afterwards.
        "--fixup",
        "never",
        // Written under its own name from the first byte, so it is known
        // where it is if Siphon closes or crashes.
        "--no-part",
        // ffmpeg would otherwise print a status line several times a second,
        // all of it kept in memory for the whole recording.
        "--downloader-args",
        "ffmpeg:-nostats -loglevel warning",
        // A connection that drops is picked up again, within one try.
        "--retries",
        "10",
        "--fragment-retries",
        "10",
        "--retry-sleep",
        "http:exp=1:8",
        "--retry-sleep",
        "fragment:exp=1:8",
    ]);
    if from_start {
        // The site's video and audio arrive as two files, joined at the end.
        // One piece at a time, the default: with several at once, yt-dlp keeps
        // every piece in a file of its own until the stream ends, and the
        // recording's own files stay empty all along (seen 2026-11-19). One at
        // a time still caught up 26 minutes of 720p60 in under a minute.
        push(&["--live-from-start", "--merge-output-format", "mkv"]);
    }
    args
}

/// The output template for part `part` of a recording: the user's naming
/// style, the download's own marker, and which part it is. Recorded from now
/// on, it is one MPEG-TS file; from the start, yt-dlp names the video and
/// audio files it joins at the end itself.
pub fn recording_template(name: &str, own: &str, part: usize, from_start: bool) -> String {
    let ext = if from_start { "%(ext)s" } else { "ts" };
    format!("{name}{own}{RECORDING_MARKER}{part}.{ext}")
}

/// What a recording's files carry in their names after the download's own
/// marker: `Title.siphon-1a2b3c4d.rec1.ts`, `….rec2.ts` after a reconnect.
const RECORDING_MARKER: &str = ".rec";

/// What every file of part `part` of a recording has in its name, whatever
/// yt-dlp adds after it: `.siphon-1a2b3c4d.rec1.` (so `rec1` is not `rec10`).
pub fn recording_needle(own: &str, part: usize) -> String {
    format!("{own}{RECORDING_MARKER}{part}.")
}

/// A recording file's name without what Siphon added: the part, the format
/// yt-dlp added (`.f137`), and the download's marker.
/// `Title.siphon-1a2b3c4d.rec1.f137` → `Title`.
pub fn recording_stem(stem: &str) -> &str {
    let stem = match stem.rfind(RECORDING_MARKER) {
        Some(at) if stem[at..].len() > RECORDING_MARKER.len() => &stem[..at],
        _ => stem,
    };
    finished_stem(stem)
}

#[cfg(test)]
mod recording_tests {
    use super::*;

    #[test]
    fn names_each_part_of_a_recording_and_finds_the_title_again() {
        let own = own_marker("1a2b3c4d-5e6f");
        let first = recording_template("Lofi radio", &own, 1, false);
        assert_eq!(first, "Lofi radio.siphon-1a2b3c4d.rec1.ts");
        assert_eq!(recording_stem("Lofi radio.siphon-1a2b3c4d.rec1"), "Lofi radio");
        assert_eq!(recording_stem("Lofi radio.siphon-1a2b3c4d.rec2.f137"), "Lofi radio");
        assert_eq!(recording_template("A", &own, 1, true), "A.siphon-1a2b3c4d.rec1.%(ext)s");
        // Every file of the first part, and none of the tenth.
        let needle = recording_needle(&own, 1);
        assert!(first.contains(&needle));
        assert!("A.siphon-1a2b3c4d.rec1.f308.mkv".contains(&needle));
        assert!(!"A.siphon-1a2b3c4d.rec10.ts".contains(&needle));
        // A title that happens to contain the marker keeps it.
        assert_eq!(recording_stem("Notes on .records.siphon-ab.rec1"), "Notes on .records");
    }
}
