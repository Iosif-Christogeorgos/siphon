//! Subtitles in a plan: which of the video's tracks the language asked for
//! gets, and how that track reaches the finished file.
//!
//! Siphon remembers a language, not a track: "Greek" carries over to every
//! video that has Greek. People's subtitles beat speech recognition whatever
//! the region ("en-GB" written by people over "en-US" by machine); within a
//! kind, the exact region, then the plain language. A video without the
//! language gets none: another language is never put in its place.

use serde::{Deserialize, Serialize};

use super::codecs::Container;
use super::registry::{Group, TargetId};
use super::{info, Choice, Recipe};
use crate::ytdlp::subtitles::{language_name, language_tag, primary, SubtitleKind, SubtitleTrack};

/// How subtitles reach the finished file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Delivery {
    /// A track inside the file, which players can turn on and off.
    Embed,
    /// A SubRip file beside the video, named after it: editors import captions from one.
    File,
    /// Drawn into the picture, for Share: chat apps show no subtitle tracks.
    /// It can't be turned off, and the video has to be encoded for it.
    #[serde(rename = "burn")]
    BurnIn,
}

/// The subtitle track a recipe adds, and how.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SubtitleStep {
    /// yt-dlp's name for the track: "en", "en-orig".
    pub key: String,
    /// Listed with the machine-made ones, which yt-dlp writes only with `--write-auto-subs`.
    pub automatic: bool,
    /// Its language as a lowercase tag ("en", "pt-br").
    pub lang: String,
    /// What players call the track: the site's name for it, or the language's.
    pub title: String,
    pub kind: SubtitleKind,
    pub delivery: Delivery,
    /// Kept as SubStation Alpha, with its fonts, colours and placement, rather
    /// than made into SubRip: into Matroska, which holds it, or drawn into the
    /// picture, which libass does styling and all; and only for the whole
    /// video, since a cut works on SubRip.
    pub styled: bool,
}

impl Choice {
    /// The language subtitles are wanted in, if any.
    pub fn subtitles(&self) -> Option<&str> {
        match self {
            Choice::Preset(preset) => preset.subtitles.as_deref(),
            Choice::Custom(manual) => manual.subtitles.as_deref(),
        }
        .filter(|language| !language.trim().is_empty())
    }
}

/// How subtitles would reach the file `recipe` makes for `choice`, whatever
/// the language. None for audio, which can take none.
pub fn delivery(choice: &Choice, recipe: &Recipe) -> Option<Delivery> {
    recipe.video.as_ref()?;
    match choice {
        Choice::Preset(preset) => match preset.target.group() {
            Group::Edit => Some(Delivery::File),
            Group::Share => Some(Delivery::BurnIn),
            Group::Audio => None,
            Group::Watch | Group::Max => Some(Delivery::Embed),
        },
        Choice::Custom(_) => Some(Delivery::Embed),
    }
}

/// The track that best answers `wanted` (a language tag), if the video has
/// one in that language: people's over speech recognition, then the exact
/// region, then the plain language ("en" over "en-gb").
pub fn pick<'a>(tracks: &'a [SubtitleTrack], wanted: &str) -> Option<&'a SubtitleTrack> {
    let wanted = language_tag(wanted);
    tracks
        .iter()
        .filter(|track| primary(&track.lang) == primary(&wanted))
        .min_by_key(|track| (track.kind, track.lang != wanted, track.lang.len(), track.key.as_str()))
}

/// Adds the subtitles `choice` asks for to `recipe`, when the video has them
/// in that language and the file can take them.
pub fn attach(recipe: &mut Recipe, tracks: &[SubtitleTrack], choice: &Choice) {
    let (Some(wanted), Some(delivery)) = (choice.subtitles(), delivery(choice, recipe)) else {
        return;
    };
    let Some(track) = pick(tracks, wanted) else {
        return;
    };
    match delivery {
        Delivery::Embed => place_track(recipe, choice),
        Delivery::BurnIn if !recipe.encodes_video() => {
            // Drawing them in means encoding the picture: Share's own encode,
            // the one it keeps for an original that turns out too large.
            let Some(encode) = recipe.fallback.take() else {
                recipe.hints.push(info(
                    "No subtitles: drawing them in needs the video's length, which the site didn't say.",
                ));
                return;
            };
            *recipe = *encode;
            recipe.reason = Some("the subtitles are drawn into the picture".into());
        }
        Delivery::BurnIn | Delivery::File => {}
    }
    if delivery == Delivery::BurnIn {
        let app = match choice {
            Choice::Preset(preset) => preset.target.info().name,
            Choice::Custom(_) => "the app",
        };
        recipe.hints.push(info(format!(
            "The subtitles are drawn into the picture, since {app} shows no subtitle tracks. They can't be turned off."
        )));
    }
    let has_styling = track.offers("ass") || track.offers("ssa");
    let whole = recipe.trim.is_none();
    let styled = has_styling
        && whole
        && match delivery {
            Delivery::Embed => recipe.container == Container::Mkv,
            Delivery::BurnIn => true,
            Delivery::File => false,
        };
    if has_styling && !styled {
        let into = match delivery {
            Delivery::File => "an SRT file".to_string(),
            _ if !whole => "a clip".to_string(),
            Delivery::Embed | Delivery::BurnIn => format!("an {} file", recipe.container.label()),
        };
        recipe.hints.push(info(format!(
            "The subtitles' fonts, colours and placement can't go in {into}; their words and timings do."
        )));
    }
    match track.kind {
        SubtitleKind::Uploaded => {}
        SubtitleKind::Automatic => recipe.hints.push(info(
            "These subtitles come from speech recognition, so expect mistakes.",
        )),
    }
    recipe.subtitles = Some(SubtitleStep {
        key: track.key.clone(),
        automatic: track.kind != SubtitleKind::Uploaded,
        lang: track.lang.clone(),
        title: track
            .name
            .clone()
            .or_else(|| language_name(&track.lang).map(str::to_string))
            .unwrap_or_else(|| track.lang.clone()),
        kind: track.kind,
        delivery,
        styled,
    });
}

/// Puts the subtitle track in a file type where the target's players show
/// it. Holding a track isn't showing it: Windows' own playback (what Media
/// Player is built on) found the track in a Matroska file but none in MP4 or
/// WebM, though both played to the end (tested 2026-11-20). So Windows gets
/// Matroska, which it plays natively, and so do TVs, whose spec sheets all
/// list SubRip in Matroska among the subtitles they read, and Max quality,
/// where Matroska also keeps styled subtitles as they are. Apple's players
/// can't open Matroska and show MP4's own timed text (Apple's format), so
/// Apple devices and Android keep MP4. Custom settings keep the file type
/// chosen, and say where it falls short.
fn place_track(recipe: &mut Recipe, choice: &Choice) {
    let container = recipe.container;
    match choice {
        Choice::Preset(preset) => {
            let why = match preset.target {
                TargetId::Windows => "Windows' own players show subtitles from MKV files, not from MP4 or WebM",
                TargetId::Tv => "MKV is the file type smart TVs read subtitles from",
                TargetId::Max => "MKV holds subtitles as they are, and every player shows them from it",
                _ => return,
            };
            if container != Container::Mkv {
                recipe.container = Container::Mkv;
                recipe
                    .hints
                    .push(info(format!("In MKV rather than {}: {why}.", container.label())));
            }
        }
        Choice::Custom(_) if container != Container::Mkv => recipe.hints.push(info(format!(
            "Windows' own players don't show subtitles inside {} files; VLC does. In MKV they show in both.",
            container.label()
        ))),
        Choice::Custom(_) => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use SubtitleKind::*;

    fn track(key: &str, kind: SubtitleKind, exts: &[&str]) -> SubtitleTrack {
        SubtitleTrack {
            key: key.into(),
            lang: language_tag(key),
            name: None,
            kind,
            exts: exts.iter().map(|ext| ext.to_string()).collect(),
        }
    }

    fn keys<'a>(tracks: &'a [SubtitleTrack], wanted: &[&str]) -> Vec<Option<&'a str>> {
        wanted
            .iter()
            .map(|wanted| pick(tracks, wanted).map(|track| track.key.as_str()))
            .collect()
    }

    #[test]
    fn people_beat_machines_whatever_the_region() {
        let tracks = [
            track("en-GB", Uploaded, &["srt"]),
            track("en-orig", Automatic, &["srt"]),
            track("el", Automatic, &["srt"]),
            track("de", Automatic, &["srt"]),
            track("de-DE", Uploaded, &["vtt"]),
        ];
        assert_eq!(
            keys(&tracks, &["en", "en-US", "el", "de", "de-AT", "fr"]),
            [
                Some("en-GB"),
                Some("en-GB"),
                Some("el"),
                Some("de-DE"),
                Some("de-DE"),
                // Never another language in its place.
                None,
            ]
        );
    }

    #[test]
    fn within_a_kind_the_exact_region_then_the_plain_language() {
        let tracks = [
            track("pt-PT", Uploaded, &["vtt"]),
            track("pt-BR", Uploaded, &["vtt"]),
            track("pt", Uploaded, &["vtt"]),
            track("zh-Hant", Uploaded, &["vtt"]),
            track("zh-Hans", Uploaded, &["vtt"]),
        ];
        assert_eq!(
            keys(&tracks, &["pt-br", "pt", "pt-AO", "zh-Hans", "zh-hant", "zh"]),
            [
                Some("pt-BR"),
                Some("pt"),
                Some("pt"),
                Some("zh-Hans"),
                Some("zh-Hant"),
                Some("zh-Hans")
            ]
        );
        // However the language is written.
        assert_eq!(keys(&[track("ger", Uploaded, &["srt"])], &["de"]), [Some("ger")]);
    }
}
