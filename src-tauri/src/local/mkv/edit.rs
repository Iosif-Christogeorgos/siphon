//! What the user chose to keep of an MKV, and the FFmpeg arguments that
//! make it: every kept track copied as it is, in the order chosen, with the
//! language, name and flags chosen; nothing converted (docs/mkv-editor.md).

use std::ffi::OsString;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::{Contents, Kind};
use crate::error::{AppError, Result};
use crate::ytdlp::subtitles::iso639_2;

/// The tracks an MKV keeps, and how.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TrackEdit {
    /// The tracks kept, in the order they go in the copy.
    pub tracks: Vec<KeptTrack>,
    /// Keep the chapters (as one plain list: see `chapters`).
    #[serde(default = "yes")]
    pub chapters: bool,
    /// Keep the files attached: the fonts ASS subtitles are drawn with, a cover.
    #[serde(default = "yes")]
    pub attachments: bool,
}

fn yes() -> bool {
    true
}

/// A track kept, with what the copy says of it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KeptTrack {
    /// Its place among the original's streams (`0:N`).
    pub index: usize,
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

impl TrackEdit {
    /// An error if the edit doesn't fit the file: a track it doesn't have,
    /// one kept twice, or none at all.
    pub fn check(&self, contents: &Contents) -> Result<()> {
        if self.tracks.is_empty() {
            return Err(AppError::internal("Keep at least one track."));
        }
        for (place, kept) in self.tracks.iter().enumerate() {
            if !contents.tracks.iter().any(|track| track.index == kept.index) {
                return Err(AppError::internal(
                    "The file has changed since its tracks were read. Open it again.",
                ));
            }
            if self.tracks[..place].iter().any(|before| before.index == kept.index) {
                return Err(AppError::internal("A track was kept twice."));
            }
        }
        Ok(())
    }

    /// The kinds of track the copy holds, in order: what it is checked against.
    pub fn kinds(&self, contents: &Contents) -> Vec<Kind> {
        self.tracks
            .iter()
            .filter_map(|kept| contents.tracks.iter().find(|track| track.index == kept.index))
            .map(|track| track.kind)
            .collect()
    }

    /// About how large the copy is: the kept tracks' bytes, where the file
    /// counts them (MKVToolNix does), else the original's size.
    pub fn size(&self, contents: &Contents) -> Option<u64> {
        let kept: Option<u64> = self
            .tracks
            .iter()
            .map(|kept| contents.tracks.iter().find(|track| track.index == kept.index)?.bytes)
            .sum();
        kept.map(|bytes| bytes + bytes / 100).or(contents.size)
    }
}

/// The arguments that make FFmpeg copy what `edit` keeps of `source` into
/// `output`, an MKV, saying how far it has got on its standard output.
pub fn args(source: &Path, output: &Path, edit: &TrackEdit, contents: &Contents) -> Vec<OsString> {
    let mut args: Vec<OsString> = Vec::new();
    let mut push = |values: &[&str]| args.extend(values.iter().map(OsString::from));
    push(&[
        "-hide_banner",
        "-nostdin",
        "-y",
        "-loglevel",
        "error",
        "-progress",
        "pipe:1",
        "-nostats",
        "-i",
    ]);
    args.push(source.into());
    let mut push = |values: &[&str]| args.extend(values.iter().map(OsString::from));
    for kept in &edit.tracks {
        push(&["-map", &format!("0:{}", kept.index)]);
    }
    if edit.attachments {
        for attachment in &contents.attachments {
            push(&["-map", &format!("0:{}", attachment.index)]);
        }
    }
    push(&["-map_chapters", if edit.chapters { "0" } else { "-1" }]);
    push(&["-map_metadata", "0", "-c", "copy"]);
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
    push(&["-f", "matroska"]);
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
        };
        let args = texts(&args(
            Path::new("Korra.mkv"),
            Path::new("Edited/Korra.mkv"),
            &edit,
            &korra(),
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
        };
        let joined = texts(&args(Path::new("a.mkv"), Path::new("b.mkv"), &edit, &korra())).join(" ");
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
        };
        assert!(none.check(&contents).is_err());
        let missing = TrackEdit {
            tracks: vec![kept(9, None, "")],
            ..none.clone()
        };
        assert!(missing.check(&contents).is_err());
        let twice = TrackEdit {
            tracks: vec![kept(1, None, ""), kept(1, None, "")],
            ..none.clone()
        };
        assert!(twice.check(&contents).is_err());
        let fine = TrackEdit {
            tracks: vec![kept(0, None, ""), kept(1, None, "")],
            ..none
        };
        assert!(fine.check(&contents).is_ok());
        assert_eq!(fine.kinds(&contents), [Kind::Video, Kind::Sound]);
    }

    #[test]
    fn sizes_the_copy_by_its_tracks_where_the_file_counts_them() {
        let mut contents = korra();
        let edit = TrackEdit {
            tracks: vec![kept(0, None, ""), kept(1, None, "")],
            chapters: true,
            attachments: true,
        };
        assert_eq!(edit.size(&contents), Some(217_331_196 + 2_173_311));
        contents.tracks[1].bytes = None;
        assert_eq!(edit.size(&contents), Some(289_422_396));
    }
}
