//! "My languages": what an MKV keeps when the user's languages choose
//! (docs/mkv-editor.md). Applied as soon as a file opens; every choice can
//! still be changed by hand.
//!
//! - Sound in one of my languages is kept, the first language's plays first.
//!   With none, the original sound alone is kept; if which is the original
//!   isn't known, all of it is. The original isn't kept for being the original.
//! - Subtitles in my languages are kept, full and forced. With sound in my
//!   language, its forced subtitles play first; otherwise the full subtitles
//!   in my first language that has some.
//! - Commentary and description in my languages are kept, never played first.
//! - SDH is left out, unless it is a language's only full subtitles: then it
//!   is kept, never played first.
//! - A track whose language isn't certain is always kept.

use serde::Serialize;

use super::edit::{KeptTrack, TrackEdit};
use super::{Contents, Kind, Track};
use crate::ytdlp::subtitles::{language_name, primary};

/// What keeping the user's languages makes of a file, and what it says of it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Suggestion {
    /// The languages it went by, best first.
    pub languages: Vec<String>,
    pub edit: TrackEdit,
    /// The tracks it leaves out, and why.
    pub left_out: Vec<LeftOut>,
    /// What else it changed or kept, in a line each: "English sound plays first: it is your first language."
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LeftOut {
    /// The track's place among the file's streams.
    pub index: usize,
    /// "Italian sound: not one of your languages."
    pub why: String,
}

/// What `contents` keeps when `languages` (best first) choose.
pub fn suggest(contents: &Contents, languages: &[String]) -> Suggestion {
    let rank = |track: &Track| {
        let language = primary(track.language.as_deref()?);
        languages.iter().position(|mine| primary(mine) == language)
    };
    let mine = |track: &Track| track.certain && rank(track).is_some();
    let main = |track: &Track| !track.commentary && !track.described;
    let mut keep: Vec<bool> = contents.tracks.iter().map(|_| true).collect();
    let mut left_out = Vec::new();
    let mut notes = Vec::new();
    let place = |track: &Track| contents.tracks.iter().position(|t| t.index == track.index).unwrap_or(0);

    // Sound.
    let sounds: Vec<&Track> = contents.tracks.iter().filter(|t| t.kind == Kind::Sound).collect();
    let my_sound: Vec<&Track> = sounds.iter().copied().filter(|t| mine(t) && main(t)).collect();
    let mut first_sound: Option<&Track> = my_sound.iter().copied().min_by_key(|t| (rank(t), !t.default, place(t)));
    if my_sound.is_empty() && !sounds.is_empty() {
        let mains: Vec<&Track> = sounds.iter().copied().filter(|t| main(t)).collect();
        let original = mains
            .iter()
            .copied()
            .find(|t| t.original)
            .or_else(|| mains.iter().copied().find(|t| t.default))
            .or(if mains.len() == 1 { mains.first().copied() } else { None });
        match original {
            Some(original) => {
                notes.push(format!(
                    "None of its sound is in your languages: its original sound{} is kept.",
                    original
                        .language
                        .as_deref()
                        .map(|l| format!(", {},", name(l)))
                        .unwrap_or_default()
                ));
                first_sound = Some(original);
            }
            None => notes.push(
                "None of its sound is in your languages, and which is its original isn't known: all of it is kept."
                    .to_string(),
            ),
        }
    }
    let fallback_all = my_sound.is_empty() && first_sound.is_none();
    for track in &sounds {
        let kept = fallback_all || !track.certain || mine(track) || first_sound.is_some_and(|f| f.index == track.index);
        if !kept {
            keep[place(track)] = false;
            left_out.push(LeftOut {
                index: track.index,
                why: format!("{}: not one of your languages.", describe(track)),
            });
        }
    }

    // Subtitles.
    let subtitles: Vec<&Track> = contents.tracks.iter().filter(|t| t.kind == Kind::Subtitles).collect();
    let full = |t: &Track| !t.forced && !t.commentary && !t.sdh;
    for track in &subtitles {
        if !track.certain {
            continue;
        }
        if !mine(track) {
            keep[place(track)] = false;
            left_out.push(LeftOut {
                index: track.index,
                why: format!("{}: not one of your languages.", describe(track)),
            });
        } else if track.sdh && !track.forced {
            let others = subtitles
                .iter()
                .any(|other| other.index != track.index && mine(other) && full(other) && same_language(other, track));
            if others {
                keep[place(track)] = false;
                left_out.push(LeftOut {
                    index: track.index,
                    why: format!(
                        "{}: for the deaf and hard of hearing; its other subtitles are kept.",
                        describe(track)
                    ),
                });
            }
        }
    }
    let kept_subtitle = |t: &Track| keep[place(t)];
    let first_subtitle: Option<&Track> = match first_sound.filter(|sound| mine(sound)) {
        // Sound in my language: its forced subtitles, for signs and foreign lines.
        Some(sound) => subtitles
            .iter()
            .copied()
            .filter(|t| kept_subtitle(t) && t.forced && same_language(t, sound))
            .min_by_key(|t| (!t.default, place(t))),
        // Otherwise the full subtitles in my first language that has some.
        None => subtitles
            .iter()
            .copied()
            .filter(|t| kept_subtitle(t) && mine(t) && full(t))
            .min_by_key(|t| (rank(t), !t.default, place(t))),
    };

    // What plays first, said where it changes.
    let default_of = |track: &Track| match track.kind {
        Kind::Video => track.default,
        Kind::Sound => match first_sound {
            Some(first) => first.index == track.index,
            None => track.default && main(track),
        },
        Kind::Subtitles => first_subtitle.is_some_and(|first| first.index == track.index),
    };
    if let Some(first) = first_sound.filter(|first| !first.default && mine(first)) {
        notes.push(format!("{} plays first: {}.", describe(first), why_first(rank(first))));
    }
    if let Some(first) = first_subtitle.filter(|first| !first.default) {
        let reason = if first.forced {
            "for signs and foreign lines"
        } else {
            "its sound isn't in your languages"
        };
        notes.push(format!("{} play first: {reason}.", describe(first)));
    }
    let uncertain = contents.tracks.iter().filter(|t| !t.certain).count();
    if uncertain > 0 {
        notes.push(format!(
            "{uncertain} track{} whose language isn't certain {} kept.",
            if uncertain == 1 { "" } else { "s" },
            if uncertain == 1 { "is" } else { "are" }
        ));
    }

    let tracks = contents
        .tracks
        .iter()
        .zip(&keep)
        .filter(|(_, keep)| **keep)
        .map(|(track, _)| KeptTrack {
            index: track.index,
            from: None,
            charset: None,
            language: track.language.clone(),
            name: track.name.clone().unwrap_or_default(),
            default: default_of(track),
            forced: track.forced,
            sdh: track.sdh,
            commentary: track.commentary,
            described: track.described,
            original: track.original,
        })
        .collect();
    Suggestion {
        languages: languages.to_vec(),
        edit: TrackEdit {
            chapters: true,
            attachments: true,
            tracks,
            ..TrackEdit::default()
        },
        left_out,
        notes,
    }
}

fn same_language(a: &Track, b: &Track) -> bool {
    match (&a.language, &b.language) {
        (Some(a), Some(b)) => primary(a) == primary(b),
        _ => false,
    }
}

/// "Italian sound", "English SDH subtitles", "French commentary".
fn describe(track: &Track) -> String {
    let language = track.language.as_deref().map_or_else(|| "Unknown".to_string(), name);
    match track.kind {
        Kind::Video => format!("{language} video"),
        Kind::Sound if track.commentary => format!("{language} commentary"),
        Kind::Sound if track.described => format!("{language} audio description"),
        Kind::Sound => format!("{language} sound"),
        Kind::Subtitles if track.forced => format!("{language} forced subtitles"),
        Kind::Subtitles if track.sdh => format!("{language} SDH subtitles"),
        Kind::Subtitles => format!("{language} subtitles"),
    }
}

fn why_first(rank: Option<usize>) -> &'static str {
    match rank {
        Some(0) => "it is your first language",
        _ => "no sound is in a language you put before it",
    }
}

fn name(tag: &str) -> String {
    language_name(tag).map_or_else(|| tag.to_string(), str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::local::mkv::Chapters;

    fn track(index: usize, kind: Kind, language: Option<&str>) -> Track {
        Track {
            index,
            kind,
            codec: "aac".into(),
            name: None,
            language: language.map(str::to_string),
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
            bytes: None,
            doubts: Vec::new(),
            notes: Vec::new(),
            read: false,
            certain: true,
        }
    }

    fn file(tracks: Vec<Track>) -> Contents {
        Contents {
            container: Default::default(),
            tracks,
            chapters: Chapters::default(),
            attachments: Vec::new(),
            doubts: Vec::new(),
            suggestion: None,
            duration: None,
            size: None,
        }
    }

    fn langs(tags: &[&str]) -> Vec<String> {
        tags.iter().map(|tag| tag.to_string()).collect()
    }

    /// The kept tracks' places, and which of them play first.
    fn kept(suggestion: &Suggestion) -> (Vec<usize>, Vec<usize>) {
        let tracks = &suggestion.edit.tracks;
        (
            tracks.iter().map(|t| t.index).collect(),
            tracks.iter().filter(|t| t.default).map(|t| t.index).collect(),
        )
    }

    /// The Legend of Korra S01E01: Greek dub playing first, English surround,
    /// English commentary; Greek SRT, English PGS.
    fn korra() -> Contents {
        file(vec![
            Track {
                default: true,
                ..track(0, Kind::Video, None)
            },
            Track {
                default: true,
                ..track(1, Kind::Sound, Some("el"))
            },
            track(2, Kind::Sound, Some("en")),
            Track {
                commentary: true,
                ..track(3, Kind::Sound, Some("en"))
            },
            track(4, Kind::Subtitles, Some("el")),
            track(5, Kind::Subtitles, Some("en")),
        ])
    }

    #[test]
    fn korra_for_greek_then_english_keeps_all_and_plays_greek() {
        let suggestion = suggest(&korra(), &langs(&["el", "en"]));
        assert_eq!(kept(&suggestion), (vec![0, 1, 2, 3, 4, 5], vec![0, 1]));
        assert!(suggestion.left_out.is_empty() && suggestion.notes.is_empty());
    }

    #[test]
    fn korra_for_english_first_plays_english_and_says_so() {
        let suggestion = suggest(&korra(), &langs(&["en", "el"]));
        // The commentary never plays first; no subtitles do, as the sound is English.
        assert_eq!(kept(&suggestion), (vec![0, 1, 2, 3, 4, 5], vec![0, 2]));
        assert_eq!(
            suggestion.notes,
            ["English sound plays first: it is your first language."]
        );
    }

    #[test]
    fn korra_for_greek_alone_leaves_the_english_out_and_its_original_is_not_kept_for_being_it() {
        let suggestion = suggest(&korra(), &langs(&["el"]));
        assert_eq!(kept(&suggestion), (vec![0, 1, 4], vec![0, 1]));
        let why: Vec<&str> = suggestion.left_out.iter().map(|l| l.why.as_str()).collect();
        assert_eq!(
            why,
            [
                "English sound: not one of your languages.",
                "English commentary: not one of your languages.",
                "English subtitles: not one of your languages."
            ]
        );
    }

    #[test]
    fn a_film_with_no_sound_in_my_languages_keeps_its_original_and_my_subtitles_play() {
        // A Japanese film with an Italian dub, Greek and Italian subtitles.
        let film = file(vec![
            track(0, Kind::Video, None),
            Track {
                original: true,
                ..track(1, Kind::Sound, Some("ja"))
            },
            Track {
                default: true,
                ..track(2, Kind::Sound, Some("it"))
            },
            track(3, Kind::Subtitles, Some("it")),
            track(4, Kind::Subtitles, Some("el")),
        ]);
        let suggestion = suggest(&film, &langs(&["el", "en"]));
        assert_eq!(kept(&suggestion), (vec![0, 1, 4], vec![1, 4]));
        assert_eq!(
            suggestion.notes,
            [
                "None of its sound is in your languages: its original sound, Japanese, is kept.",
                "Greek subtitles play first: its sound isn't in your languages."
            ]
        );
    }

    #[test]
    fn with_no_original_known_all_the_sound_is_kept() {
        let film = file(vec![
            track(1, Kind::Sound, Some("ja")),
            track(2, Kind::Sound, Some("it")),
        ]);
        let suggestion = suggest(&film, &langs(&["el"]));
        assert_eq!(kept(&suggestion).0, [1, 2]);
        assert!(suggestion.notes[0].contains("all of it is kept"));
        // One sound track is the original, whatever it says.
        let one = file(vec![track(1, Kind::Sound, Some("ja"))]);
        assert_eq!(kept(&suggest(&one, &langs(&["el"]))), (vec![1], vec![1]));
    }

    #[test]
    fn sound_in_my_language_plays_its_forced_subtitles_first() {
        let film = file(vec![
            Track {
                default: true,
                ..track(1, Kind::Sound, Some("en"))
            },
            track(2, Kind::Subtitles, Some("en")),
            Track {
                forced: true,
                ..track(3, Kind::Subtitles, Some("en"))
            },
            Track {
                default: true,
                ..track(4, Kind::Subtitles, Some("el"))
            },
        ]);
        let suggestion = suggest(&film, &langs(&["en", "el"]));
        assert_eq!(kept(&suggestion), (vec![1, 2, 3, 4], vec![1, 3]));
        assert_eq!(
            suggestion.notes,
            ["English forced subtitles play first: for signs and foreign lines."]
        );
    }

    #[test]
    fn sdh_goes_unless_it_is_a_languages_only_full_subtitles_and_never_plays_first() {
        let film = file(vec![
            Track {
                original: true,
                ..track(1, Kind::Sound, Some("ja"))
            },
            track(2, Kind::Subtitles, Some("en")),
            Track {
                sdh: true,
                ..track(3, Kind::Subtitles, Some("en"))
            },
            Track {
                sdh: true,
                ..track(4, Kind::Subtitles, Some("el"))
            },
        ]);
        let suggestion = suggest(&film, &langs(&["el", "en"]));
        // Greek SDH stays (Greek's only), English SDH goes; Greek SDH doesn't play, English full does.
        assert_eq!(kept(&suggestion), (vec![1, 2, 4], vec![1, 2]));
        assert_eq!(
            suggestion.left_out[0].why,
            "English SDH subtitles: for the deaf and hard of hearing; its other subtitles are kept."
        );
    }

    #[test]
    fn what_isnt_certain_is_kept_and_said() {
        let film = file(vec![
            Track {
                default: true,
                ..track(1, Kind::Sound, Some("en"))
            },
            Track {
                certain: false,
                ..track(2, Kind::Sound, Some("it"))
            },
            Track {
                certain: false,
                ..track(3, Kind::Subtitles, None)
            },
            track(4, Kind::Subtitles, Some("fr")),
        ]);
        let suggestion = suggest(&film, &langs(&["en"]));
        assert_eq!(kept(&suggestion), (vec![1, 2, 3], vec![1]));
        assert_eq!(suggestion.notes, ["2 tracks whose language isn't certain are kept."]);
    }

    #[test]
    fn commentary_in_my_language_is_kept_and_never_plays_first() {
        let film = file(vec![
            Track {
                commentary: true,
                default: true,
                ..track(1, Kind::Sound, Some("en"))
            },
            track(2, Kind::Sound, Some("en")),
        ]);
        assert_eq!(kept(&suggest(&film, &langs(&["en"]))), (vec![1, 2], vec![2]));
    }
}
