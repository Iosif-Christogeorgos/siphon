//! What converting a file on this PC makes: whether there is anything to
//! make, and what it is called and where it goes.

use std::fs::OpenOptions;
use std::path::{Path, PathBuf};

use crate::plan::codecs::Container;
use crate::plan::registry::{Group, TargetId, TierId};
use crate::plan::{Choice, Recipe, Trim};
use crate::ytdlp::args;
use crate::ytdlp::metadata::VideoInfo;

/// Why `choice` would make nothing new from a file: what it would make is
/// the file as it already is, its tracks untouched in the same kind of file.
/// Then nothing is made, and the window says so. None when there is
/// something to make (a file that isn't on this PC never has nothing).
pub fn nothing_to_do(video: &VideoInfo, choice: &Choice, recipe: &Recipe) -> Option<String> {
    let format = video.formats.first().filter(|_| video.file.is_some())?;
    let untouched = recipe.trim.is_none()
        && recipe.subtitles.is_none()
        && recipe.chat.is_none()
        && !recipe.strip_audio
        && !recipe.encodes_video()
        && !recipe.encodes_audio();
    // A file with picture and sound keeps both: taking the sound alone is something.
    let same_tracks = recipe.video.is_some() == format.video_codec.is_some()
        && recipe.audio_codec().is_some() == format.audio_codec.is_some();
    if !untouched || !same_tracks {
        return None;
    }
    let preset = match choice {
        Choice::Preset(preset) => Some(preset),
        Choice::Custom(_) => None,
    };
    let group = preset.map(|preset| preset.target.group());
    // A sound file is already its own original sound, whatever file type it is
    // in; only Music, which asks for one, can make another of it.
    let sound_alone = recipe.video.is_none() && preset.is_none_or(|preset| preset.target != TargetId::Music);
    if !sound_alone && !same_kind(&format.ext, recipe.container) {
        return None;
    }
    let said = match (group, preset) {
        (Some(Group::Watch), Some(preset)) => format!("This already plays on {}.", preset.target.info().name),
        (Some(Group::Share), _) => match &recipe.fits {
            Some(fits) => format!("This already {fits}, and plays there."),
            None => "This already fits, and plays there.".to_string(),
        },
        (Some(Group::Audio), Some(preset)) if preset.target == TargetId::Music => {
            format!("This is already {} at that quality.", recipe.container.with_article())
        }
        _ if recipe.video.is_none() => "This is already the sound alone, as it is.".to_string(),
        (Some(Group::Max), _) => "This already has its streams as they are, in the file type that suits them.".into(),
        _ => format!("This is already {} like that.", recipe.container.with_article()),
    };
    Some(format!("{said} Nothing to convert."))
}

/// Whether a file with this extension already is the kind of file `container` makes.
fn same_kind(ext: &str, container: Container) -> bool {
    match container {
        Container::Mp4 => matches!(ext, "mp4" | "m4v"),
        Container::M4a => matches!(ext, "m4a" | "m4b"),
        Container::Ogg => matches!(ext, "ogg" | "oga"),
        other => ext == other.ext(),
    }
}

/// What a file made from the file `stem` is called, beside it: after it,
/// with what it was made for and the part cut out of it, if any.
/// "Clip (Windows).mp4", "Clip (Discord, 1m00s-1m15s).mp4", "Clip.mp3".
/// `trim` is the part as the user chose it, from the file's start.
pub fn output_name(stem: &str, choice: &Choice, recipe: &Recipe, trim: Option<Trim>) -> String {
    let mut marks: Vec<String> = label(choice).map(str::to_string).into_iter().collect();
    if let Some(trim) = trim {
        marks.push(format!("{}-{}", args::clock(trim.start), args::clock(trim.end)));
    }
    let marks = if marks.is_empty() {
        String::new()
    } else {
        format!(" ({})", marks.join(", "))
    };
    format!("{stem}{marks}.{}", recipe.container.ext())
}

/// What a file is made for, in a word, for its name. Sound and the file's
/// own streams in another file type need none: the file type says it.
fn label(choice: &Choice) -> Option<&'static str> {
    let Choice::Preset(preset) = choice else {
        return None;
    };
    Some(match preset.target {
        TargetId::Windows => "Windows",
        TargetId::Apple => "iPhone",
        TargetId::Android => "Android",
        TargetId::Tv => "TV",
        TargetId::Discord => "Discord",
        TargetId::Whatsapp => "WhatsApp",
        TargetId::Social => match preset.tier {
            Some(TierId::Reels) => "Reels",
            Some(TierId::Shorts) => "Shorts",
            _ => "TikTok",
        },
        TargetId::Music | TargetId::Original | TargetId::Max => return None,
    })
}

/// Where a conversion writes until its file is finished and checked:
/// beside it, under the job's own marker (see `args::own_marker`), so it
/// can never be a file that was already there. The extension stays, for
/// ffmpeg picks the file type from it.
pub fn partial_name(output: &Path, job_id: &str) -> PathBuf {
    let stem = output.file_stem().and_then(|stem| stem.to_str()).unwrap_or("file");
    let ext = output.extension().and_then(|ext| ext.to_str()).unwrap_or("mp4");
    output.with_file_name(format!("{stem}{}.{ext}", args::own_marker(job_id)))
}

/// Whether Siphon can write files into `dir`: it makes a file of its own
/// there (under the job's marker) and removes it at once. A folder on a
/// disc, a read-only share, or one Windows guards against unknown apps
/// can't be written to.
pub fn writable(dir: &Path, job_id: &str) -> bool {
    let test = dir.join(format!("{}.tmp", args::own_marker(job_id)));
    match OpenOptions::new().write(true).create_new(true).open(&test) {
        Ok(file) => {
            drop(file);
            let _ = std::fs::remove_file(&test);
            true
        }
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::local::{LocalFile, Original};
    use crate::plan::registry::MusicFormat;
    use crate::plan::{plan, Capabilities, PresetChoice};
    use crate::ytdlp::metadata::{Format, FormatKind};

    fn file(ext: &str, vcodec: Option<&str>, acodec: Option<&str>, kbps: f64) -> VideoInfo {
        let kind = match (vcodec, acodec) {
            (Some(_), Some(_)) => FormatKind::Muxed,
            (Some(_), None) => FormatKind::Video,
            _ => FormatKind::Audio,
        };
        let mut format = Format {
            id: "file".into(),
            ext: ext.into(),
            kind,
            vcodec: vcodec.map(str::to_string),
            acodec: acodec.map(str::to_string),
            video_codec: None,
            audio_codec: None,
            width: vcodec.map(|_| 1920),
            height: vcodec.map(|_| 1080),
            fps: vcodec.map(|_| 30.0),
            tbr: Some(kbps),
            vbr: None,
            abr: acodec.map(|_| 128.0),
            asr: Some(48_000),
            audio_channels: Some(2),
            filesize: Some((kbps * 1000.0 / 8.0 * 60.0) as u64),
            filesize_estimated: false,
            dynamic_range: vcodec.map(|_| "SDR".into()),
            language: None,
            language_preference: None,
            segmented: false,
            stream_url: None,
            http_chunk_size: None,
            http_headers: Vec::new(),
            note: None,
        };
        format.video_codec = vcodec.map(|_| crate::plan::codecs::VideoCodec::of(&format));
        format.audio_codec = acodec.map(|_| crate::plan::codecs::AudioCodec::of(&format));
        let original = Original::new(PathBuf::from(format!(r"C:\Videos\Clip.{ext}")));
        VideoInfo {
            id: original.id().into(),
            title: "Clip".into(),
            url: original.key(),
            thumbnail: None,
            duration: Some(60.0),
            channel: None,
            view_count: None,
            extractor: None,
            formats: vec![format],
            with_account: false,
            live: None,
            subtitles: Vec::new(),
            chat: None,
            saved: None,
            sizes: Default::default(),
            file: Some(LocalFile {
                key: original.key(),
                name: original.name(),
                folder: r"C:\Videos".into(),
                size: 1,
                version: "1-0".into(),
                matroska: false,
                overview: Default::default(),
                start: 0.0,
                cover: false,
                tracks: Default::default(),
            }),
        }
    }

    fn preset(target: TargetId) -> Choice {
        Choice::Preset(PresetChoice {
            target,
            quality: None,
            tier: None,
            music_format: MusicFormat::Mp3,
            subtitles: None,
            chat: None,
        })
    }

    fn nothing(video: &VideoInfo, choice: &Choice) -> Option<String> {
        let recipe = plan(video, choice, None, &Capabilities::BASIC).unwrap();
        nothing_to_do(video, choice, &recipe)
    }

    #[test]
    fn a_file_that_already_is_what_was_asked_for_makes_nothing() {
        let mp4 = file("mp4", Some("h264"), Some("aac"), 4000.0);
        assert_eq!(
            nothing(&mp4, &preset(TargetId::Windows)).as_deref(),
            Some("This already plays on Windows 11. Nothing to convert.")
        );
        assert!(nothing(&mp4, &preset(TargetId::Tv)).is_some());
        // Its sound alone is something.
        assert!(nothing(&mp4, &preset(TargetId::Original)).is_none());
        assert!(nothing(&mp4, &preset(TargetId::Music)).is_none());
        // Small enough for Discord already.
        let small = file("mp4", Some("h264"), Some("aac"), 1500.0);
        assert_eq!(
            nothing(&small, &preset(TargetId::Discord)).as_deref(),
            Some("This already fits Discord (20 MB), and plays there. Nothing to convert.")
        );
        // Too big for it, or in another file type: something to make.
        assert!(nothing(&mp4, &preset(TargetId::Discord)).is_none());
        let mkv = file("mkv", Some("h264"), Some("aac"), 4000.0);
        assert!(nothing(&mkv, &preset(TargetId::Windows)).is_none());
        // Max quality: an OBS recording becomes an MP4, losing nothing; an MP4 stays as it is.
        let ts = file("ts", Some("h264"), Some("aac"), 4000.0);
        assert!(nothing(&ts, &preset(TargetId::Max)).is_none());
        let remux = plan(&ts, &preset(TargetId::Max), None, &Capabilities::BASIC).unwrap();
        assert_eq!(remux.badge, crate::plan::Badge::Lossless);
        assert!(nothing(&mp4, &preset(TargetId::Max)).is_some());
    }

    #[test]
    fn a_sound_file_is_its_own_original_sound() {
        let wav = file("wav", None, Some("pcm_s16le"), 1411.0);
        assert_eq!(
            nothing(&wav, &preset(TargetId::Original)).as_deref(),
            Some("This is already the sound alone, as it is. Nothing to convert.")
        );
        // Every video preset makes a sound file the same sound.
        assert!(nothing(&wav, &preset(TargetId::Windows)).is_some());
        // Music makes an MP3 of it, unless it already is one.
        assert!(nothing(&wav, &preset(TargetId::Music)).is_none());
        let mp3 = file("mp3", None, Some("mp3"), 192.0);
        assert_eq!(
            nothing(&mp3, &preset(TargetId::Music)).as_deref(),
            Some("This is already an MP3 at that quality. Nothing to convert.")
        );
    }

    #[test]
    fn a_link_never_has_nothing_to_do() {
        let mut link = file("mp4", Some("h264"), Some("aac"), 4000.0);
        link.file = None;
        assert!(nothing(&link, &preset(TargetId::Windows)).is_none());
    }

    #[test]
    fn what_is_made_is_named_after_the_file_and_what_it_is_for() {
        let mp4 = file("mkv", Some("h264"), Some("aac"), 4000.0);
        let name = |choice: &Choice, trim: Option<Trim>| {
            let recipe = plan(&mp4, choice, trim, &Capabilities::BASIC).unwrap();
            output_name("Clip", choice, &recipe, trim)
        };
        assert_eq!(name(&preset(TargetId::Windows), None), "Clip (Windows).mp4");
        assert_eq!(name(&preset(TargetId::Tv), None), "Clip (TV).mp4");
        assert_eq!(name(&preset(TargetId::Music), None), "Clip.mp3");
        assert_eq!(name(&preset(TargetId::Original), None), "Clip.m4a");
        assert_eq!(name(&preset(TargetId::Max), None), "Clip.mp4");
        let part = Trim { start: 60.0, end: 75.5 };
        assert_eq!(
            name(&preset(TargetId::Discord), Some(part)),
            "Clip (Discord, 1m00s-1m15s).mp4"
        );
        assert_eq!(name(&preset(TargetId::Max), Some(part)), "Clip (1m00s-1m15s).mp4");
        let reels = Choice::Preset(PresetChoice {
            tier: Some(TierId::Reels),
            ..match preset(TargetId::Social) {
                Choice::Preset(preset) => preset,
                Choice::Custom(_) => unreachable!(),
            }
        });
        assert_eq!(name(&reels, None), "Clip (Reels).mp4");
    }

    #[test]
    fn a_conversion_writes_under_its_own_marker_until_it_is_done() {
        let partial = partial_name(Path::new(r"C:\Videos\Clip (Windows).mp4"), "1a2b3c4d-5e6f");
        assert_eq!(partial, Path::new(r"C:\Videos\Clip (Windows).siphon-1a2b3c4d.mp4"));
    }

    #[test]
    fn a_folder_that_takes_files_is_writable_and_left_as_it_was() {
        let dir = std::env::temp_dir().join(format!("siphon-writable-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(writable(&dir, "job"));
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
        assert!(!writable(&dir.join("missing"), "job"));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
