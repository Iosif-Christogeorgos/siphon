//! A file on this PC's own tracks beside its picture and main sound: its
//! other sound tracks (an OBS recording's game, microphone and chat apart; a
//! film's languages and commentary) and its subtitle tracks. A download has
//! one picture and one sound, so all of this is empty for it.
//!
//! What is kept depends on what the file is made for (docs/local-files.md):
//! Watch and Customize keep every sound track, each copied where the device
//! and the file type take it and converted where not, and every subtitle
//! track the file type can hold; a subtitle track it can't (pictures, in
//! anything but an MKV) is left out, and the plan says so before anything
//! starts. Share and Audio take the main sound alone: chat apps play only a
//! file's first sound track, and a sound file holds one. Share leaves the
//! subtitles out too, and says so.

use serde::{Deserialize, Serialize};

use super::codecs::{AudioCodec, Container, VideoCodec};
use super::registry::Group;
use super::{info, lossy_target_kbps, warning, watch, AudioAction, Capabilities, Choice, Recipe, ALL_AUDIO};

/// What a file holds beside its picture, as ffprobe read it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FileTracks {
    /// The main sound track: the one marked to play first, or else the first.
    /// Counted among the file's sound tracks, as ffmpeg's `0:a:N` counts them.
    pub main: usize,
    /// Every sound track, the main one among them, in the file's order.
    pub sounds: Vec<FileSound>,
    /// Every subtitle track, in the file's order.
    pub subtitles: Vec<FileSubtitle>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FileSound {
    /// Its place among the file's sound tracks (`0:a:N`).
    pub track: usize,
    pub codec: AudioCodec,
    pub kbps: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FileSubtitle {
    /// Its place among the file's subtitle tracks (`0:s:N`).
    pub track: usize,
    /// ffprobe's name for its codec: "subrip", "ass", "mov_text", "hdmv_pgs_subtitle".
    pub codec: String,
}

/// The file's own tracks a recipe takes: which sound is the main one, and
/// the other tracks kept beside it. All empty for a download.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Tracks {
    /// The source's main sound track, among its sound tracks (`0:a:N`): the
    /// one a conversion takes as its sound. 0 for a download, which has one.
    #[serde(default)]
    pub main: usize,
    /// The other sound tracks, after the main one in what is made.
    #[serde(default)]
    pub sounds: Vec<KeptSound>,
    /// The file's subtitle tracks that go in.
    #[serde(default)]
    pub subtitles: Vec<KeptSubtitle>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KeptSound {
    /// Its place among the source's sound tracks (`0:a:N`).
    pub track: usize,
    pub source: AudioCodec,
    pub source_kbps: Option<f64>,
    pub action: AudioAction,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KeptSubtitle {
    /// Its place among the source's subtitle tracks (`0:s:N`).
    pub track: usize,
    /// ffprobe's name for its codec in the source.
    pub source: String,
    /// What it is in what is made, by the same name: "mov_text" in an MP4.
    pub codec: String,
}

impl KeptSubtitle {
    /// What a smart cut's piece holds it as: Matroska, which takes it as it
    /// is, unless it is MP4's own kind, which becomes SubRip.
    pub fn in_piece(&self) -> &str {
        match self.source.as_str() {
            "mov_text" | "text" => "subrip",
            source => source,
        }
    }
}

/// ffmpeg's encoder for a subtitle track in `from` that is to be `to`:
/// "copy" when they are one kind.
pub fn subtitle_encoder<'a>(from: &str, to: &'a str) -> &'a str {
    fn same(name: &str) -> &str {
        match name {
            "srt" => "subrip",
            "ssa" => "ass",
            other => other,
        }
    }
    match to {
        _ if same(from) == same(to) => "copy",
        "subrip" => "srt",
        other => other,
    }
}

/// What kind of subtitles a track holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// Timed text, which every file type with video has a kind of track for.
    Text,
    /// Pictures of the words (Blu-ray's PGS, a DVD's VobSub): only an MKV holds them.
    Picture,
    /// Anything else (closed captions inside the picture's stream, teletext).
    Other,
}

fn kind(codec: &str) -> Kind {
    match codec {
        "subrip" | "srt" | "ass" | "ssa" | "webvtt" | "mov_text" | "text" => Kind::Text,
        "hdmv_pgs_subtitle" | "dvd_subtitle" | "dvb_subtitle" | "xsub" => Kind::Picture,
        _ => Kind::Other,
    }
}

/// What a subtitle track in `codec` is in `container`; None when it can't go in.
fn subtitle_in(codec: &str, container: Container) -> Option<String> {
    let to = match (kind(codec), container) {
        (Kind::Text, Container::Mp4 | Container::Mov) => "mov_text",
        (Kind::Text, Container::Webm) => "webvtt",
        (Kind::Text, Container::Mkv) => match codec {
            "ass" | "ssa" => "ass",
            "webvtt" => "webvtt",
            _ => "subrip",
        },
        (Kind::Picture, Container::Mkv) => codec,
        _ => return None,
    };
    Some(to.to_string())
}

/// Takes the file's own tracks into `recipe` (see the module's notes).
pub(super) fn keep(recipe: &mut Recipe, file: &FileTracks, choice: &Choice, caps: &Capabilities) {
    recipe.tracks.main = file.main;
    if let Some(fallback) = recipe.fallback.as_deref_mut() {
        fallback.tracks.main = file.main;
    }
    let group = match choice {
        Choice::Preset(preset) => Some(preset.target.group()),
        Choice::Custom(_) => None,
    };
    let others = file.sounds.len().saturating_sub(1);
    if others > 0 && recipe.audio_codec().is_some() {
        match group {
            Some(Group::Share) => recipe
                .hints
                .push(info("Only its main sound track goes in: chat apps play just one.")),
            Some(Group::Audio) => recipe.hints.push(info(format!(
                "Only its main sound track is taken, of the {} it has.",
                file.sounds.len()
            ))),
            _ => {}
        }
    }
    // Share leaves a file's subtitles out rather than drawing them in, as it
    // does a download's (the owner's call, 2026-11-25): the file's own tracks
    // are its to keep, and a converted copy for a chat is the moment, not the
    // film. Said, as the sound is, but not marked in a list: it is expected.
    if group == Some(Group::Share) && !file.subtitles.is_empty() {
        recipe
            .hints
            .push(info("Its subtitles are left out: chat apps don't show them."));
    }
    let keeps_all = matches!(group, None | Some(Group::Watch | Group::Max));
    let Some(picture) = recipe.video_codec().filter(|_| keeps_all) else {
        return;
    };
    let container = recipe.container;

    if recipe.audio_codec().is_some() {
        let (accepted, fallback) = sound_rule(recipe, choice, caps, picture);
        recipe.tracks.sounds = file
            .sounds
            .iter()
            .filter(|sound| sound.track != file.main)
            .map(|sound| KeptSound {
                track: sound.track,
                source: sound.codec,
                source_kbps: sound.kbps,
                action: if accepted.contains(&sound.codec) && container.holds_audio(sound.codec) {
                    AudioAction::Copy
                } else {
                    AudioAction::encode(fallback, Some(lossy_target_kbps(sound.kbps)))
                },
            })
            .collect();
    }

    // A download's subtitles, fetched from the site, would come instead; a file has none.
    if recipe.subtitles.is_some() {
        return;
    }
    let mut left_out = Vec::new();
    for subtitle in &file.subtitles {
        match subtitle_in(&subtitle.codec, container) {
            Some(codec) => recipe.tracks.subtitles.push(KeptSubtitle {
                track: subtitle.track,
                source: subtitle.codec.clone(),
                codec,
            }),
            None => left_out.push(kind(&subtitle.codec)),
        }
    }
    if !left_out.is_empty() {
        let all = left_out.len() == file.subtitles.len();
        recipe
            .hints
            .push(warning(left_out_note(&left_out, all, container, group.is_none())));
    }
}

/// The sound the other tracks may be copied in, and what any other becomes:
/// as the main track's is chosen for this choice.
fn sound_rule(
    recipe: &Recipe,
    choice: &Choice,
    caps: &Capabilities,
    picture: VideoCodec,
) -> (Vec<AudioCodec>, AudioCodec) {
    let container = recipe.container;
    if let Choice::Preset(preset) = choice {
        if preset.target.group() == Group::Watch {
            let (accepted, fallback) = watch::sound_rule(preset.target, caps, picture, container);
            return (accepted.to_vec(), fallback);
        }
    }
    // Customize and Max quality: the main track's own codec, where the file
    // type holds it, or the one the file type takes.
    let main = recipe
        .audio_codec()
        .filter(|codec| *codec != AudioCodec::Other && container.holds_audio(*codec));
    let fallback = main.unwrap_or_else(|| container.default_audio());
    let asked = matches!(choice, Choice::Custom(options) if options.audio_codec.is_some());
    let accepted = if asked { vec![fallback] } else { ALL_AUDIO.to_vec() };
    (accepted, fallback)
}

/// "Its 2 subtitle tracks will be left out: they're made of pictures, which
/// an MP4 can't hold. Customize can keep them in an MKV."
fn left_out_note(kinds: &[Kind], all: bool, container: Container, customizing: bool) -> String {
    let one = kinds.len() == 1;
    let which = match (one, all) {
        (true, true) => "Its subtitle track".to_string(),
        (true, false) => "One of its subtitle tracks".to_string(),
        (false, true) => format!("Its {} subtitle tracks", kinds.len()),
        (false, false) => format!("{} of its subtitle tracks", kinds.len()),
    };
    if kinds.iter().all(|kind| *kind == Kind::Picture) {
        let them = if one { "it" } else { "them" };
        let tip = if customizing {
            format!("An MKV keeps {them}.")
        } else {
            format!("Customize can keep {them} in an MKV.")
        };
        format!(
            "{which} will be left out: {} made of pictures, which {} can't hold. {tip}",
            if one { "it's" } else { "they're" },
            container.with_article()
        )
    } else {
        format!(
            "{which} will be left out: Siphon can't put {} kind of subtitles in {}.",
            if one { "its" } else { "their" },
            container.with_article()
        )
    }
}

/// "Keeps all 3 sound tracks (2 converted to AAC) and its 2 subtitle
/// tracks.", for the plan's explanation; None when there are none.
pub(super) fn described(tracks: &Tracks) -> Option<String> {
    let sounds = (!tracks.sounds.is_empty()).then(|| {
        let converted: Vec<AudioCodec> = tracks
            .sounds
            .iter()
            .filter_map(|sound| match sound.action {
                AudioAction::Encode { codec, .. } => Some(codec),
                AudioAction::Copy => None,
            })
            .collect();
        let how = match converted.first() {
            Some(codec) => format!(" ({} converted to {})", converted.len(), codec.label()),
            None => String::new(),
        };
        format!("all {} sound tracks{how}", tracks.sounds.len() + 1)
    });
    let subtitles = match tracks.subtitles.len() {
        0 => None,
        1 => Some("its subtitle track".to_string()),
        count => Some(format!("its {count} subtitle tracks")),
    };
    let kept = match (sounds, subtitles) {
        (Some(sounds), Some(subtitles)) => format!("{sounds} and {subtitles}"),
        (Some(one), None) | (None, Some(one)) => one,
        (None, None) => return None,
    };
    Some(format!("Keeps {kept}."))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::local::LocalFile;
    use crate::plan::registry::{MusicFormat, TargetId};
    use crate::plan::{plan, Badge, HintLevel, ManualOptions, PresetChoice, Trim};
    use crate::ytdlp::metadata::{Format, FormatKind, VideoInfo};

    /// An OBS recording or a film: H.264 with its main sound (AAC) the
    /// second of three sound tracks, a Dolby Digital commentary last, and
    /// subtitle tracks in `subtitles`.
    fn film(ext: &str, subtitles: &[&str]) -> VideoInfo {
        let mut format = Format {
            id: "file".into(),
            ext: ext.into(),
            kind: FormatKind::Muxed,
            vcodec: Some("h264".into()),
            acodec: Some("aac".into()),
            video_codec: None,
            audio_codec: None,
            width: Some(1920),
            height: Some(1080),
            fps: Some(30.0),
            tbr: Some(5000.0),
            vbr: None,
            abr: Some(160.0),
            asr: Some(48_000),
            audio_channels: Some(2),
            filesize: Some(5000 * 1000 / 8 * 60),
            filesize_estimated: false,
            dynamic_range: Some("SDR".into()),
            language: None,
            language_preference: None,
            segmented: false,
            stream_url: None,
            http_chunk_size: None,
            http_headers: Vec::new(),
            note: None,
        };
        format.video_codec = Some(VideoCodec::of(&format));
        format.audio_codec = Some(AudioCodec::of(&format));
        let sound = |track, codec, kbps| FileSound {
            track,
            codec,
            kbps: Some(kbps),
        };
        VideoInfo {
            id: "1".into(),
            title: "Film".into(),
            url: "siphon-file:1".into(),
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
                key: "siphon-file:1".into(),
                name: format!("Film.{ext}"),
                folder: r"C:\Videos".into(),
                size: 1,
                version: "1-0".into(),
                matroska: false,
                overview: Default::default(),
                start: 0.0,
                cover: false,
                tracks: FileTracks {
                    main: 1,
                    sounds: vec![
                        sound(0, AudioCodec::Aac, 128.0),
                        sound(1, AudioCodec::Aac, 160.0),
                        sound(2, AudioCodec::Ac3, 384.0),
                    ],
                    subtitles: subtitles
                        .iter()
                        .enumerate()
                        .map(|(track, codec)| FileSubtitle {
                            track,
                            codec: (*codec).into(),
                        })
                        .collect(),
                },
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

    fn planned(video: &VideoInfo, choice: &Choice) -> Recipe {
        plan(video, choice, None, &Capabilities::BASIC).unwrap()
    }

    fn warnings(recipe: &Recipe) -> Vec<&str> {
        recipe
            .hints
            .iter()
            .filter(|hint| hint.level == HintLevel::Warning)
            .map(|hint| hint.text.as_str())
            .collect()
    }

    #[test]
    fn the_main_sound_is_the_one_marked_to_play_first_whatever_it_is_made_for() {
        let mkv = film("mkv", &[]);
        for target in [
            TargetId::Windows,
            TargetId::Discord,
            TargetId::Music,
            TargetId::Original,
        ] {
            let recipe = planned(&mkv, &preset(target));
            assert_eq!(recipe.tracks.main, 1, "{target:?}");
            // Share's fallback, converted should a copy come out too large, takes it too.
            if let Some(fallback) = recipe.fallback {
                assert_eq!(fallback.tracks.main, 1);
            }
        }
    }

    #[test]
    fn watch_keeps_every_sound_track_converting_only_what_the_device_cant_play() {
        let mkv = film("mkv", &[]);
        // TVs play Dolby Digital: every track is copied.
        let tv = planned(&mkv, &preset(TargetId::Tv));
        let actions: Vec<(usize, &AudioAction)> = tv.tracks.sounds.iter().map(|s| (s.track, &s.action)).collect();
        assert_eq!(actions, [(0, &AudioAction::Copy), (2, &AudioAction::Copy)]);
        assert_eq!(tv.badge, Badge::Lossless);
        // Windows' own player doesn't: the commentary becomes AAC.
        let windows = planned(&mkv, &preset(TargetId::Windows));
        assert_eq!(windows.tracks.sounds[0].action, AudioAction::Copy);
        assert!(matches!(
            windows.tracks.sounds[1].action,
            AudioAction::Encode {
                codec: AudioCodec::Aac,
                ..
            }
        ));
        assert!(windows.encodes_audio() && !windows.encodes_main_audio());
        assert_eq!(windows.badge, Badge::Converts);
        assert_eq!(
            described(&windows.tracks).as_deref(),
            Some("Keeps all 3 sound tracks (1 converted to AAC).")
        );
    }

    #[test]
    fn share_and_audio_take_the_main_sound_alone_and_say_so() {
        let mkv = film("mkv", &["subrip"]);
        let discord = planned(&mkv, &preset(TargetId::Discord));
        assert!(discord.tracks.sounds.is_empty() && discord.tracks.subtitles.is_empty());
        assert!(discord
            .hints
            .iter()
            .any(|hint| hint.text.contains("chat apps play just one")));
        // Its subtitles are left out with a note, which isn't a warning, so
        // the list doesn't mark the file for it.
        assert!(discord
            .hints
            .iter()
            .any(|hint| hint.level == HintLevel::Info && hint.text.contains("subtitles are left out")));
        assert!(warnings(&discord).is_empty());
        let without = planned(&film("mkv", &[]), &preset(TargetId::Discord));
        assert!(!without.hints.iter().any(|hint| hint.text.contains("subtitles")));
        let music = planned(&mkv, &preset(TargetId::Music));
        assert!(music.tracks.sounds.is_empty());
        assert!(music.hints.iter().any(|hint| hint.text.contains("of the 3 it has")));
    }

    #[test]
    fn subtitles_go_in_as_the_file_type_holds_them_and_pictures_are_left_out_with_a_note() {
        let mkv = film("mkv", &["subrip", "hdmv_pgs_subtitle", "ass"]);
        let tv = planned(&mkv, &preset(TargetId::Tv));
        let kept: Vec<(usize, &str)> = tv
            .tracks
            .subtitles
            .iter()
            .map(|subtitle| (subtitle.track, subtitle.codec.as_str()))
            .collect();
        assert_eq!(kept, [(0, "mov_text"), (2, "mov_text")]);
        assert_eq!(
            warnings(&tv),
            [
                "One of its subtitle tracks will be left out: it's made of pictures, which an MP4 can't hold. \
              Customize can keep it in an MKV."
            ]
        );
        // Customize, as an MKV: all three, as they are.
        let custom = Choice::Custom(ManualOptions {
            container: Some(Container::Mkv),
            ..ManualOptions::default()
        });
        let as_mkv = planned(&mkv, &custom);
        let kept: Vec<&str> = as_mkv.tracks.subtitles.iter().map(|s| s.codec.as_str()).collect();
        assert_eq!(kept, ["subrip", "hdmv_pgs_subtitle", "ass"]);
        assert!(warnings(&as_mkv).is_empty());
        assert_eq!(as_mkv.tracks.sounds.len(), 2);
        assert!(as_mkv
            .tracks
            .sounds
            .iter()
            .all(|sound| sound.action == AudioAction::Copy));
    }

    fn text(args: Vec<std::ffi::OsString>) -> String {
        args.iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join(" ")
    }

    #[test]
    fn a_conversion_takes_the_main_sound_first_then_the_others_and_the_subtitles() {
        use crate::ffmpeg::{self, Pass};
        use std::path::Path;
        let mkv = film("mkv", &["subrip", "ass"]);
        let windows = planned(&mkv, &preset(TargetId::Windows));
        let args = text(ffmpeg::convert_args(
            &windows,
            Path::new("in.mkv"),
            Path::new("out.mp4"),
            Pass::Only,
        ));
        assert!(
            args.contains("-map 0:V:0 -map 0:a:1? -map 0:a:0 -map 0:a:2 -c:v copy"),
            "{args}"
        );
        assert!(
            args.contains(
                "-c:a:0 copy -c:a:1 copy -c:a:2 aac -b:a:2 256k \
                 -disposition:a:0 default -disposition:a:1 -default -disposition:a:2 -default"
            ),
            "{args}"
        );
        assert!(
            args.contains("-map 0:s:0 -c:s:0 mov_text -tag:s:0 tx3g -map 0:s:1 -c:s:1 mov_text -tag:s:1 tx3g"),
            "{args}"
        );
        // A part, cut in a full conversion: each track keeps only its part.
        let part = Trim { start: 10.0, end: 20.0 };
        let mut cut = plan(&mkv, &preset(TargetId::Windows), Some(part), &Capabilities::BASIC).unwrap();
        cut = cut.with_full_cut();
        let args = text(ffmpeg::convert_args(
            &cut,
            Path::new("in.mkv"),
            Path::new("out.mp4"),
            Pass::Only,
        ));
        assert!(
            args.contains(r"-c:a:0 copy -bsf:a:0 noise=drop=lt(pts*tb+duration*tb/2\,10)"),
            "{args}"
        );
        assert!(args.contains("-filter:a:2 atrim=start=10:end=20 -c:a:2 aac"), "{args}");
        assert!(
            args.contains(r"-bsf:s:1 noise=drop=lt(pts*tb\,10)+gte(pts*tb\,20)"),
            "{args}"
        );
        // A download, with one sound track, is spoken of as before.
        let mut one = windows.clone();
        one.tracks = Tracks::default();
        let args = text(ffmpeg::convert_args(
            &one,
            Path::new("in.mkv"),
            Path::new("out.mp4"),
            Pass::Only,
        ));
        assert!(args.contains("-map 0:a:0? -c:v copy -c:a copy -movflags"), "{args}");
        // Sound alone: the main track.
        let music = planned(&mkv, &preset(TargetId::Music));
        let args = text(ffmpeg::sound_args(
            &music,
            Path::new("in.mkv"),
            Path::new("out.mp3"),
            None,
            false,
        ));
        assert!(args.contains("-map 0:a:1 -vn"), "{args}");
    }

    #[test]
    fn a_smart_cut_carries_every_sound_track_and_adds_the_subtitles_as_it_puts_the_clip_in_its_file_type() {
        use crate::ffmpeg::{self, smart_cut};
        use std::path::Path;
        let mkv = film("mkv", &["mov_text", "hdmv_pgs_subtitle"]);
        let custom = Choice::Custom(ManualOptions {
            container: Some(Container::Mkv),
            ..ManualOptions::default()
        });
        let recipe = plan(
            &mkv,
            &custom,
            Some(Trim { start: 10.0, end: 20.0 }),
            &Capabilities::BASIC,
        )
        .unwrap();
        assert!(recipe.smart_cut());
        let main = (recipe.tracks.main, &recipe.audio.as_ref().unwrap().action);
        let sounds: Vec<(usize, &AudioAction)> = std::iter::once(main)
            .chain(recipe.tracks.sounds.iter().map(|sound| (sound.track, &sound.action)))
            .collect();
        let audio = text(smart_cut::audio_args(
            Path::new("in.mkv"),
            10.0,
            20.0,
            &sounds,
            Path::new("audio.mka"),
        ));
        assert!(
            audio.contains("-map 0:a:1 -map 0:a:0 -map 0:a:2 -vn -sn -dn -c:a:0 copy -bsf:a:0"),
            "{audio}"
        );
        assert!(audio.contains("-disposition:a:0 default"), "{audio}");
        let subtitles = text(smart_cut::subtitles_args(
            Path::new("in.mkv"),
            10.0,
            20.0,
            &recipe.tracks.subtitles,
            Path::new("subtitles.mks"),
        ));
        assert!(
            subtitles.contains(
                r"-map 0:s:0 -map 0:s:1 -vn -an -dn -c:s:0 srt -c:s:1 copy -bsf:s noise=drop=lt(pts*tb\,10)+gte(pts*tb\,20) -output_ts_offset -10 -f matroska"
            ),
            "{subtitles}"
        );
        let rewrap = text(ffmpeg::rewrap_args(
            Path::new("cut.mp4"),
            Path::new("out.mkv"),
            None,
            Container::Mkv,
            Some((Path::new("subtitles.mks"), &recipe.tracks.subtitles)),
        ));
        assert!(
            rewrap.contains("-copyts -i cut.mp4 -i subtitles.mks -map 0:v:0 -map 0:a? -c copy -map 1:s:0 -c:s:0 copy -map 1:s:1 -c:s:1 copy out.mkv"),
            "{rewrap}"
        );
        let join = text(smart_cut::join_args(
            &["head.ts", "middle.ts"],
            Some(("audio.mka", 3)),
            None,
            10.0,
            VideoCodec::H264,
            Container::Mp4,
            smart_cut::Index::Rewrite,
            Path::new("cut.mp4"),
        ));
        assert!(join.contains("-map 0:v:0 -map 1:a -c copy"), "{join}");
    }

    #[test]
    fn a_file_with_every_track_kept_as_it_is_still_has_nothing_to_do() {
        let mp4 = film("mp4", &["mov_text"]);
        let choice = preset(TargetId::Tv);
        let recipe = planned(&mp4, &choice);
        assert!(crate::local::nothing_to_do(&mp4, &choice, &recipe).is_some());
        // Windows can't play its commentary: that is something to make.
        let choice = preset(TargetId::Windows);
        let recipe = planned(&mp4, &choice);
        assert!(crate::local::nothing_to_do(&mp4, &choice, &recipe).is_none());
    }

    #[test]
    fn each_kind_of_subtitles_goes_where_it_can() {
        assert_eq!(subtitle_in("subrip", Container::Mp4).as_deref(), Some("mov_text"));
        assert_eq!(subtitle_in("ass", Container::Mov).as_deref(), Some("mov_text"));
        assert_eq!(subtitle_in("ass", Container::Mkv).as_deref(), Some("ass"));
        assert_eq!(subtitle_in("mov_text", Container::Mkv).as_deref(), Some("subrip"));
        assert_eq!(subtitle_in("subrip", Container::Webm).as_deref(), Some("webvtt"));
        // Pictures go only in an MKV.
        assert_eq!(
            subtitle_in("hdmv_pgs_subtitle", Container::Mkv).as_deref(),
            Some("hdmv_pgs_subtitle")
        );
        assert_eq!(subtitle_in("hdmv_pgs_subtitle", Container::Mp4), None);
        assert_eq!(subtitle_in("dvd_subtitle", Container::Webm), None);
        // Captions carried some other way go nowhere.
        assert_eq!(subtitle_in("eia_608", Container::Mkv), None);
    }

    #[test]
    fn a_subtitle_track_is_copied_when_it_stays_its_kind() {
        assert_eq!(subtitle_encoder("mov_text", "mov_text"), "copy");
        assert_eq!(subtitle_encoder("subrip", "mov_text"), "mov_text");
        assert_eq!(subtitle_encoder("mov_text", "subrip"), "srt");
        assert_eq!(subtitle_encoder("ssa", "ass"), "copy");
        assert_eq!(subtitle_encoder("hdmv_pgs_subtitle", "hdmv_pgs_subtitle"), "copy");
        let mp4s = KeptSubtitle {
            track: 0,
            source: "mov_text".into(),
            codec: "subrip".into(),
        };
        assert_eq!(mp4s.in_piece(), "subrip");
    }

    #[test]
    fn what_is_left_out_is_said_plainly() {
        assert_eq!(
            left_out_note(&[Kind::Picture, Kind::Picture], true, Container::Mp4, false),
            "Its 2 subtitle tracks will be left out: they're made of pictures, which an MP4 can't hold. \
             Customize can keep them in an MKV."
        );
        assert_eq!(
            left_out_note(&[Kind::Picture], false, Container::Mp4, true),
            "One of its subtitle tracks will be left out: it's made of pictures, which an MP4 can't hold. \
             An MKV keeps it."
        );
        assert_eq!(
            left_out_note(&[Kind::Other], true, Container::Mkv, true),
            "Its subtitle track will be left out: Siphon can't put its kind of subtitles in an MKV."
        );
    }
}
