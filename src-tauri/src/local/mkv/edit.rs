//! What the user chose to keep of an MKV, and the FFmpeg arguments that
//! make it: every kept track copied as it is, in the order chosen, with the
//! language, name and flags chosen; nothing converted (docs/mkv-editor.md).
//! Tracks may come from other files too (see `added`): a text subtitle file
//! not in UTF-8 is the one thing re-written, its text, as UTF-8.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::{added, chapter_files, Chapter, Contents, Kind, Track};
use crate::error::{AppError, Result};
use crate::ytdlp::subtitles::iso639_2;

/// The tracks an MKV keeps, and how.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TrackEdit {
    /// The tracks kept, in the order they go in the copy.
    pub tracks: Vec<KeptTrack>,
    /// Keep the chapters (as one plain list: see `chapters`).
    #[serde(default = "yes")]
    pub chapters: bool,
    /// Keep the files attached: the fonts ASS subtitles are drawn with, a cover.
    #[serde(default = "yes")]
    pub attachments: bool,
    /// The chapters the copy gets, as chosen: renamed, some left out, or
    /// taken from another file. None: as the MKV has them (or none, with
    /// `chapters` false).
    #[serde(default)]
    pub chapter_list: Option<Vec<Chapter>>,
    /// The MKV's attached files left out, by their place among its streams.
    #[serde(default)]
    pub left_out_attachments: Vec<usize>,
    /// Font files to attach, by key: what ASS subtitles are drawn with.
    #[serde(default)]
    pub fonts: Vec<String>,
}

/// The font files a copy can attach, by extension, and how Matroska names their kind.
pub const FONTS: &[(&str, &str)] = &[("ttf", "font/ttf"), ("otf", "font/otf"), ("ttc", "font/collection")];

/// What else goes into a copy besides tracks, found when it is made: the
/// file its chapters are written in, and the fonts it attaches.
#[derive(Debug, Clone, Default)]
pub struct Extras {
    pub chapters: Option<PathBuf>,
    pub fonts: Vec<PathBuf>,
}

fn yes() -> bool {
    true
}

/// A track kept, with what the copy says of it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KeptTrack {
    /// Its place among its file's streams (`N:index`).
    pub index: usize,
    /// The file it comes from, by its key, when it is added from another
    /// (see `added`); None for the MKV's own.
    #[serde(default)]
    pub from: Option<String>,
    /// For a text subtitle file not in UTF-8: the encoding it is read in.
    #[serde(default)]
    pub charset: Option<String>,
    /// The language the copy tags it with ("el"); None leaves the tag as it is.
    pub language: Option<String>,
    /// Its name in the copy; empty for none.
    pub name: String,
    pub default: bool,
    pub forced: bool,
    pub sdh: bool,
    pub commentary: bool,
    pub described: bool,
    pub original: bool,
}

/// A file tracks are added from, found again when the copy is made.
#[derive(Debug, Clone)]
pub struct AddedInput {
    pub key: String,
    pub path: PathBuf,
    pub contents: Contents,
    /// Bytes.
    pub size: u64,
}

impl TrackEdit {
    /// The files it adds tracks from, by key, each once, in the order first met.
    pub fn added_keys(&self) -> Vec<&str> {
        let mut keys: Vec<&str> = Vec::new();
        for key in self.tracks.iter().filter_map(|kept| kept.from.as_deref()) {
            if !keys.contains(&key) {
                keys.push(key);
            }
        }
        keys
    }

    /// The track `kept` stands for, in the MKV or the file it is added from.
    fn track<'a>(kept: &KeptTrack, contents: &'a Contents, added: &'a [AddedInput]) -> Option<&'a Track> {
        let source = match &kept.from {
            None => contents,
            Some(key) => &added.iter().find(|input| &input.key == key)?.contents,
        };
        source.tracks.iter().find(|track| track.index == kept.index)
    }

    /// An error if the edit doesn't fit the files: a track they don't have,
    /// one kept twice, an encoding Siphon doesn't know, or no track at all.
    pub fn check(&self, contents: &Contents, added: &[AddedInput]) -> Result<()> {
        if self.tracks.is_empty() {
            return Err(AppError::internal("Keep at least one track."));
        }
        for (place, kept) in self.tracks.iter().enumerate() {
            if Self::track(kept, contents, added).is_none() {
                return Err(AppError::internal(
                    "A file has changed since its tracks were read. Open it again.",
                ));
            }
            if self.tracks[..place]
                .iter()
                .any(|before| before.index == kept.index && before.from == kept.from)
            {
                return Err(AppError::internal("A track was kept twice."));
            }
            if kept
                .charset
                .as_deref()
                .is_some_and(|charset| !added::CHARSETS.contains(&charset))
            {
                return Err(AppError::internal("Siphon doesn't know that encoding."));
            }
        }
        if let Some(chapters) = &self.chapter_list {
            let fine = chapters.len() <= chapter_files::MOST
                && chapters.iter().all(|chapter| {
                    chapter.start.is_finite() && chapter.start >= 0.0 && chapter.title.chars().count() <= 500
                })
                && chapters.windows(2).all(|pair| pair[0].start < pair[1].start);
            if !fine {
                return Err(AppError::internal("The chapters chosen aren't in order."));
            }
        }
        if self.fonts.len() > 100 {
            return Err(AppError::internal("That is too many fonts to attach."));
        }
        Ok(())
    }

    /// The kinds of track the copy holds, in order: what it is checked against.
    pub fn kinds(&self, contents: &Contents, added: &[AddedInput]) -> Vec<Kind> {
        self.tracks
            .iter()
            .filter_map(|kept| Self::track(kept, contents, added))
            .map(|track| track.kind)
            .collect()
    }

    /// About how large the copy is: the kept tracks' bytes, where the file
    /// counts them (MKVToolNix does), else the original's size; and, for
    /// each track added, its bytes or its file's.
    pub fn size(&self, contents: &Contents, added: &[AddedInput]) -> Option<u64> {
        let own: Option<u64> = self
            .tracks
            .iter()
            .filter(|kept| kept.from.is_none())
            .map(|kept| contents.tracks.iter().find(|track| track.index == kept.index)?.bytes)
            .sum();
        let own = own.map(|bytes| bytes + bytes / 100).or(contents.size)?;
        let more: u64 = self
            .tracks
            .iter()
            .filter_map(|kept| {
                let input = added.iter().find(|input| Some(&input.key) == kept.from.as_ref())?;
                let track = input.contents.tracks.iter().find(|track| track.index == kept.index)?;
                Some(
                    track
                        .bytes
                        .unwrap_or(input.size / input.contents.tracks.len().max(1) as u64),
                )
            })
            .sum();
        Some(own + more)
    }
}

/// The arguments that make FFmpeg copy what `edit` keeps of `source`, and
/// adds from `added` and `extras`, into `output`, an MKV, saying how far it
/// has got on its standard output.
pub fn args(
    source: &Path,
    output: &Path,
    edit: &TrackEdit,
    contents: &Contents,
    added: &[AddedInput],
    extras: &Extras,
) -> Vec<OsString> {
    let mut args: Vec<OsString> = [
        "-hide_banner",
        "-nostdin",
        "-y",
        "-loglevel",
        "error",
        "-progress",
        "pipe:1",
        "-nostats",
        "-i",
    ]
    .map(OsString::from)
    .to_vec();
    args.push(source.into());
    // Each file added from is an input of its own, read in its encoding.
    let inputs: Vec<&AddedInput> = edit
        .added_keys()
        .into_iter()
        .filter_map(|key| added.iter().find(|input| input.key == key))
        .collect();
    for input in &inputs {
        let charset = edit
            .tracks
            .iter()
            .find(|kept| kept.from.as_deref() == Some(input.key.as_str()))
            .and_then(|kept| kept.charset.as_deref());
        if let Some(charset) = charset {
            args.extend(["-sub_charenc", charset].map(OsString::from));
        }
        args.push("-i".into());
        args.push(input.path.clone().into());
    }
    // The chapters chosen, written for FFmpeg (see `chapter_files`): an input of their own.
    let chapters_input = extras.chapters.as_ref().map(|path| {
        args.extend(["-f", "ffmetadata", "-i"].map(OsString::from));
        args.push(path.into());
        inputs.len() + 1
    });
    let input_of = |kept: &KeptTrack| match &kept.from {
        None => 0,
        Some(key) => inputs
            .iter()
            .position(|input| &input.key == key)
            .map_or(0, |place| place + 1),
    };
    let mut push = |values: &[&str]| args.extend(values.iter().map(OsString::from));
    for kept in &edit.tracks {
        push(&["-map", &format!("{}:{}", input_of(kept), kept.index)]);
    }
    let mut attached = 0;
    if edit.attachments {
        for attachment in &contents.attachments {
            if !edit.left_out_attachments.contains(&attachment.index) {
                push(&["-map", &format!("0:{}", attachment.index)]);
                attached += 1;
            }
        }
    }
    let chapters = match (&edit.chapter_list, chapters_input) {
        (Some(_), Some(input)) => input.to_string(),
        (Some(_), None) => "-1".to_string(),
        (None, _) if edit.chapters => "0".to_string(),
        (None, _) => "-1".to_string(),
    };
    push(&["-map_chapters", &chapters]);
    push(&["-map_metadata", "0", "-c", "copy"]);
    // Text that has to be re-written: a subtitle file not in UTF-8, read in
    // its encoding; an MP4's timed text, which Matroska doesn't hold.
    for (place, kept) in edit.tracks.iter().enumerate() {
        let Some(track) = TrackEdit::track(kept, contents, added) else {
            continue;
        };
        let rewrite = match track.codec.as_str() {
            "mov_text" => Some("srt"),
            "ass" | "ssa" if kept.charset.is_some() => Some("ass"),
            codec if kept.charset.is_some() && added::TEXT_CODECS.contains(&codec) => Some("srt"),
            _ => None,
        };
        if let Some(codec) = rewrite {
            push(&[&format!("-c:{place}"), codec]);
        }
    }
    // FFmpeg would otherwise mark the first track of a kind to play first
    // when none is (found 2026-11-25: Korra's Greek subtitles were turned on).
    push(&["-default_mode", "passthrough"]);
    for (place, kept) in edit.tracks.iter().enumerate() {
        if let Some(language) = &kept.language {
            // Matroska names languages by ISO 639-2's bibliographic codes ("gre").
            let code = iso639_2(language, true).unwrap_or("und");
            push(&[&format!("-metadata:s:{place}"), &format!("language={code}")]);
        }
        push(&[&format!("-metadata:s:{place}"), &format!("title={}", kept.name.trim())]);
        push(&[&format!("-disposition:{place}"), &disposition(kept)]);
    }
    // Fonts attached, each named by its file and of its kind, after the MKV's own.
    for (place, font) in extras.fonts.iter().enumerate() {
        let mime = font
            .extension()
            .and_then(|ext| ext.to_str())
            .and_then(|ext| FONTS.iter().find(|(known, _)| known.eq_ignore_ascii_case(ext)))
            .map_or("font/ttf", |(_, mime)| mime);
        args.push("-attach".into());
        args.push(font.into());
        // Named by its file alone: FFmpeg would name it by the whole path it was given.
        let name = font
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let stream = format!("-metadata:s:t:{}", attached + place);
        args.extend(
            [
                stream.clone(),
                format!("mimetype={mime}"),
                stream,
                format!("filename={name}"),
            ]
            .map(OsString::from),
        );
    }
    args.extend(["-f", "matroska"].map(OsString::from));
    args.push(output.into());
    args
}

/// A kept track's flags as FFmpeg names them: "default+forced", or "0" for none.
fn disposition(kept: &KeptTrack) -> String {
    let flags: Vec<&str> = [
        (kept.default, "default"),
        (kept.forced, "forced"),
        (kept.sdh, "hearing_impaired"),
        (kept.commentary, "comment"),
        (kept.described, "visual_impaired"),
        (kept.original, "original"),
    ]
    .into_iter()
    .filter_map(|(on, flag)| on.then_some(flag))
    .collect();
    if flags.is_empty() {
        "0".to_string()
    } else {
        flags.join("+")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::local::mkv::{Attachment, Chapters, Track};

    fn track(index: usize, kind: Kind, bytes: Option<u64>) -> Track {
        Track {
            index,
            kind,
            codec: "aac".into(),
            name: None,
            language: None,
            default: false,
            forced: false,
            sdh: false,
            commentary: false,
            described: false,
            original: false,
            channels: None,
            width: None,
            height: None,
            entries: None,
            bytes,
            doubts: Vec::new(),
            notes: Vec::new(),
            read: false,
            certain: true,
        }
    }

    fn korra() -> Contents {
        Contents {
            tracks: vec![
                track(0, Kind::Video, Some(187_570_617)),
                track(1, Kind::Sound, Some(29_760_579)),
                track(2, Kind::Sound, Some(46_209_707)),
                track(3, Kind::Sound, Some(23_105_537)),
                track(4, Kind::Subtitles, Some(21_002)),
                track(5, Kind::Subtitles, Some(7_286_059)),
            ],
            chapters: Chapters::default(),
            attachments: vec![Attachment {
                index: 6,
                name: "Arial.ttf".into(),
                mime: Some("font/ttf".into()),
            }],
            doubts: Vec::new(),
            suggestion: None,
            duration: Some(1444.8),
            size: Some(289_422_396),
        }
    }

    fn kept(index: usize, language: Option<&str>, name: &str) -> KeptTrack {
        KeptTrack {
            index,
            from: None,
            charset: None,
            language: language.map(str::to_string),
            name: name.into(),
            default: false,
            forced: false,
            sdh: false,
            commentary: false,
            described: false,
            original: false,
        }
    }

    fn texts(args: &[OsString]) -> Vec<String> {
        args.iter().map(|arg| arg.to_string_lossy().into_owned()).collect()
    }

    #[test]
    fn copies_the_kept_tracks_in_their_order_with_what_was_chosen() {
        let edit = TrackEdit {
            tracks: vec![
                kept(0, None, ""),
                KeptTrack {
                    default: true,
                    ..kept(1, Some("el"), "")
                },
                KeptTrack {
                    commentary: true,
                    ..kept(3, Some("en"), "Commentary")
                },
                KeptTrack {
                    default: true,
                    forced: true,
                    ..kept(5, Some("en"), "Signs")
                },
                kept(4, Some("el"), ""),
            ],
            chapters: true,
            attachments: true,
            ..Default::default()
        };
        let args = texts(&args(
            Path::new("Korra.mkv"),
            Path::new("Edited/Korra.mkv"),
            &edit,
            &korra(),
            &[],
            &Extras::default(),
        ));
        let joined = args.join(" ");
        assert!(joined.contains("-map 0:0 -map 0:1 -map 0:3 -map 0:5 -map 0:4 -map 0:6 -map_chapters 0"));
        assert!(joined.contains("-c copy -default_mode passthrough"));
        // The video's tag is left as it is; the others say their language, in Matroska's codes.
        assert!(!joined.contains("-metadata:s:0 language"));
        assert!(joined.contains("-metadata:s:1 language=gre"));
        assert!(joined.contains("-metadata:s:2 language=eng -metadata:s:2 title=Commentary -disposition:2 comment"));
        assert!(joined.contains("-disposition:3 default+forced"));
        assert!(joined.contains("-metadata:s:4 title= -disposition:4 0"));
        assert_eq!(&args[args.len() - 3..], ["-f", "matroska", "Edited/Korra.mkv"]);
    }

    #[test]
    fn leaves_out_chapters_and_attachments_when_asked() {
        let edit = TrackEdit {
            tracks: vec![kept(0, None, "")],
            chapters: false,
            attachments: false,
            ..Default::default()
        };
        let joined = texts(&args(
            Path::new("a.mkv"),
            Path::new("b.mkv"),
            &edit,
            &korra(),
            &[],
            &Extras::default(),
        ))
        .join(" ");
        assert!(joined.contains("-map 0:0 -map_chapters -1"));
        assert!(!joined.contains("0:6"));
    }

    #[test]
    fn an_edit_must_fit_the_file() {
        let contents = korra();
        let none = TrackEdit {
            tracks: Vec::new(),
            chapters: true,
            attachments: true,
            ..Default::default()
        };
        assert!(none.check(&contents, &[]).is_err());
        let missing = TrackEdit {
            tracks: vec![kept(9, None, "")],
            ..none.clone()
        };
        assert!(missing.check(&contents, &[]).is_err());
        let twice = TrackEdit {
            tracks: vec![kept(1, None, ""), kept(1, None, "")],
            ..none.clone()
        };
        assert!(twice.check(&contents, &[]).is_err());
        let fine = TrackEdit {
            tracks: vec![kept(0, None, ""), kept(1, None, "")],
            ..none
        };
        assert!(fine.check(&contents, &[]).is_ok());
        assert_eq!(fine.kinds(&contents, &[]), [Kind::Video, Kind::Sound]);
    }

    #[test]
    fn sizes_the_copy_by_its_tracks_where_the_file_counts_them() {
        let mut contents = korra();
        let edit = TrackEdit {
            tracks: vec![kept(0, None, ""), kept(1, None, "")],
            chapters: true,
            attachments: true,
            ..Default::default()
        };
        assert_eq!(edit.size(&contents, &[]), Some(217_331_196 + 2_173_311));
        contents.tracks[1].bytes = None;
        assert_eq!(edit.size(&contents, &[]), Some(289_422_396));
    }

    fn subtitle_file(codec: &str, bytes: Option<u64>) -> Contents {
        let mut track = track(0, Kind::Subtitles, bytes);
        track.codec = codec.into();
        Contents {
            tracks: vec![track],
            chapters: Chapters::default(),
            attachments: Vec::new(),
            doubts: Vec::new(),
            suggestion: None,
            duration: Some(1444.0),
            size: None,
        }
    }

    #[test]
    fn adds_tracks_from_other_files_each_an_input_of_its_own() {
        let added = [
            AddedInput {
                key: "siphon-file:greek".into(),
                path: PathBuf::from("Korra.el.srt"),
                contents: subtitle_file("subrip", None),
                size: 40_000,
            },
            AddedInput {
                key: "siphon-file:signs".into(),
                path: PathBuf::from("Korra.en.forced.sup"),
                contents: subtitle_file("hdmv_pgs_subtitle", Some(500_000)),
                size: 520_000,
            },
        ];
        let edit = TrackEdit {
            tracks: vec![
                kept(0, None, ""),
                kept(1, Some("el"), ""),
                KeptTrack {
                    from: Some("siphon-file:greek".into()),
                    charset: Some("CP1253".into()),
                    default: true,
                    ..kept(0, Some("el"), "")
                },
                kept(4, Some("el"), ""),
                KeptTrack {
                    from: Some("siphon-file:signs".into()),
                    forced: true,
                    ..kept(0, Some("en"), "Signs")
                },
            ],
            chapters: true,
            attachments: false,
            ..Default::default()
        };
        let contents = korra();
        assert!(edit.check(&contents, &added).is_ok());
        assert_eq!(
            edit.kinds(&contents, &added),
            [
                Kind::Video,
                Kind::Sound,
                Kind::Subtitles,
                Kind::Subtitles,
                Kind::Subtitles
            ]
        );
        let joined = texts(&args(
            Path::new("Korra.mkv"),
            Path::new("out.mkv"),
            &edit,
            &contents,
            &added,
            &Extras::default(),
        ))
        .join(" ");
        // The Greek file is read as Windows-1253 and its text written as UTF-8; the pictures are copied.
        assert!(joined.contains("-i Korra.mkv -sub_charenc CP1253 -i Korra.el.srt -i Korra.en.forced.sup"));
        assert!(joined.contains("-map 0:0 -map 0:1 -map 1:0 -map 0:4 -map 2:0 -map_chapters 0"));
        assert!(joined.contains("-c copy -c:2 srt -default_mode passthrough"));
        assert!(!joined.contains("-c:4"));
        assert!(joined.contains("-metadata:s:2 language=gre"));
        assert!(joined.contains("-disposition:2 default"));
        assert!(joined.contains("-disposition:4 forced"));
        // The subtitle file's size counts where it doesn't count its bytes.
        let own = 187_570_617 + 29_760_579 + 21_002;
        assert_eq!(edit.size(&contents, &added), Some(own + own / 100 + 40_000 + 500_000));
        // A file it adds from, gone: the edit doesn't fit.
        assert!(edit.check(&contents, &added[..1]).is_err());
        let unknown = TrackEdit {
            tracks: vec![KeptTrack {
                from: Some("siphon-file:greek".into()),
                charset: Some("UTF-7; rm -rf".into()),
                ..kept(0, None, "")
            }],
            ..edit.clone()
        };
        assert!(unknown.check(&contents, &added).is_err());
    }

    #[test]
    fn writes_the_chapters_chosen_leaves_out_attachments_and_attaches_fonts() {
        let mut contents = korra();
        contents.attachments.push(Attachment {
            index: 7,
            name: "Cover.jpg".into(),
            mime: Some("image/jpeg".into()),
        });
        let edit = TrackEdit {
            tracks: vec![kept(0, None, ""), kept(1, Some("el"), "")],
            chapters: true,
            attachments: true,
            chapter_list: Some(vec![
                Chapter {
                    start: 0.0,
                    title: "Αρχή".into(),
                },
                Chapter {
                    start: 434.225,
                    title: "Part 2".into(),
                },
            ]),
            left_out_attachments: vec![7],
            fonts: vec!["siphon-file:font".into()],
        };
        assert!(edit.check(&contents, &[]).is_ok());
        let extras = Extras {
            chapters: Some(PathBuf::from("work/chapters.txt")),
            fonts: vec![PathBuf::from("Fonts/Comic.OTF")],
        };
        let joined = texts(&args(
            Path::new("Korra.mkv"),
            Path::new("out.mkv"),
            &edit,
            &contents,
            &[],
            &extras,
        ))
        .join(" ");
        assert!(joined.contains("-i Korra.mkv -f ffmetadata -i work/chapters.txt"));
        // The font the MKV has stays; its cover goes; the chapters come from the second input.
        assert!(joined.contains("-map 0:0 -map 0:1 -map 0:6 -map_chapters 1"));
        assert!(!joined.contains("0:7"));
        assert!(joined.contains(
            "-attach Fonts/Comic.OTF -metadata:s:t:1 mimetype=font/otf -metadata:s:t:1 filename=Comic.OTF -f matroska out.mkv"
        ));
        // No chapters chosen: none.
        let none = TrackEdit {
            chapter_list: Some(Vec::new()),
            ..edit.clone()
        };
        let joined = texts(&args(
            Path::new("a.mkv"),
            Path::new("b.mkv"),
            &none,
            &contents,
            &[],
            &Extras::default(),
        ))
        .join(" ");
        assert!(joined.contains("-map_chapters -1"));
        // Chapters out of order don't fit.
        let jumbled = TrackEdit {
            chapter_list: Some(vec![
                Chapter {
                    start: 60.0,
                    title: String::new(),
                },
                Chapter {
                    start: 0.0,
                    title: String::new(),
                },
            ]),
            ..edit
        };
        assert!(jumbled.check(&contents, &[]).is_err());
    }
}
