//! What casts doubt on a track's language tag. A doubt never leaves a track
//! out: it keeps it, marked, for the user to look at (docs/mkv-editor.md).
//!
//! - A name that says another language than the tag ("Greek", tagged English).
//! - Two sound tracks alike in language, codec and channels, neither of them
//!   commentary or description: one may be a dub tagged as the original.
//! - A file named for a language (`EN-GR`, `GREEK`) with no track tagged so.

use super::{Contents, Kind, Track};
use crate::ytdlp::subtitles::{language_name, language_named, language_tag, primary};

/// Languages' own names for themselves, which track names often use.
const NATIVE: &[(&str, &str)] = &[
    ("ελληνικά", "el"),
    ("ελληνικα", "el"),
    ("deutsch", "de"),
    ("français", "fr"),
    ("francais", "fr"),
    ("español", "es"),
    ("espanol", "es"),
    ("castellano", "es"),
    ("italiano", "it"),
    ("português", "pt"),
    ("portugues", "pt"),
    ("nederlands", "nl"),
    ("svenska", "sv"),
    ("dansk", "da"),
    ("norsk", "no"),
    ("suomi", "fi"),
    ("polski", "pl"),
    ("čeština", "cs"),
    ("magyar", "hu"),
    ("türkçe", "tr"),
    ("română", "ro"),
    ("русский", "ru"),
    ("українська", "uk"),
    ("עברית", "he"),
    ("العربية", "ar"),
    ("日本語", "ja"),
    ("中文", "zh"),
    ("한국어", "ko"),
];

/// Countries' codes that release names use for their language: "EN-GR".
const COUNTRIES: &[(&str, &str)] = &[
    ("gr", "el"),
    ("jp", "ja"),
    ("kr", "ko"),
    ("cn", "zh"),
    ("dk", "da"),
    ("se", "sv"),
    ("cz", "cs"),
    ("ua", "uk"),
];

/// Adds to `contents` what casts doubt on its tracks' languages, and on the
/// file's, by its name.
pub fn cast(contents: &mut Contents, file_name: &str) {
    for track in &mut contents.tracks {
        if let (Some(tagged), Some(named)) = (&track.language, track.name.as_deref().and_then(named_language)) {
            if primary(tagged) != named {
                track.doubts.push(format!(
                    "Its name says {}, but it is tagged {}.",
                    name(named),
                    name(tagged)
                ));
            }
        }
    }
    let alike: Vec<(usize, usize)> = pairs(&contents.tracks).collect();
    for (a, b) in alike {
        for (this, other) in [(a, b), (b, a)] {
            let other = &contents.tracks[other];
            let doubt = format!(
                "Another sound track is {} too, in the same codec and channels: one of them may be another language.",
                name(other.language.as_deref().unwrap_or_default()),
            );
            contents.tracks[this].doubts.push(doubt);
        }
    }
    for (word, language) in file_languages(file_name) {
        let found = contents
            .tracks
            .iter()
            .any(|track| track.language.as_deref().map(primary) == Some(language));
        if !found {
            contents.doubts.push(format!(
                "Its name says {} ({word}), but no track is {}.",
                name(language),
                name(language)
            ));
        }
    }
}

/// Sound tracks alike in language, codec and channels, neither commentary
/// nor description, by their places in `tracks`.
fn pairs(tracks: &[Track]) -> impl Iterator<Item = (usize, usize)> + '_ {
    let sound = |t: &Track| t.kind == Kind::Sound && t.language.is_some() && !t.commentary && !t.described;
    (0..tracks.len()).flat_map(move |a| {
        (a + 1..tracks.len()).filter_map(move |b| {
            let (x, y) = (&tracks[a], &tracks[b]);
            let alike =
                sound(x) && sound(y) && x.language == y.language && x.codec == y.codec && x.channels == y.channels;
            alike.then_some((a, b))
        })
    })
}

/// The one language a track's name says, in English or in its own words:
/// "English SDH" → "en", "Ελληνικά" → "el". None if it says none, or several
/// ("Spanish (Latin America)" says Spanish and Latin).
fn named_language(name: &str) -> Option<&'static str> {
    let mut found = None;
    for word in words(name) {
        let lower = word.to_lowercase();
        let language =
            language_named(&lower).or_else(|| NATIVE.iter().find(|(native, _)| *native == lower).map(|(_, tag)| *tag));
        match (found, language) {
            (_, None) => {}
            (None, Some(language)) => found = Some(language),
            (Some(before), Some(language)) if before == language => {}
            (Some(_), Some(_)) => return None,
        }
    }
    found
}

/// The languages a file's name says: in pairs and more ("EN-GR",
/// "ENG-GRE"), and in capitals ("GREEK", not "The.English.Patient").
fn file_languages(file_name: &str) -> Vec<(String, &'static str)> {
    let stem = file_name.rsplit_once('.').map_or(file_name, |(stem, _)| stem);
    let mut found = Vec::new();
    for token in stem.split(|c: char| c.is_whitespace() || ".,_[]()".contains(c)) {
        let languages: Vec<&'static str> = if token.contains('-') {
            let parts: Option<Vec<_>> = token.split('-').map(code).collect();
            parts.filter(|parts| parts.len() > 1).unwrap_or_default()
        } else if token.len() > 3 && token.chars().all(|c| c.is_ascii_uppercase()) {
            language_named(token).into_iter().collect()
        } else {
            Vec::new()
        };
        for language in languages {
            if !found.iter().any(|(_, known)| *known == language) {
                found.push((token.to_string(), language));
            }
        }
    }
    found
}

/// A language's code in a release name: "EN", "GR", "ENG", "GRE".
fn code(part: &str) -> Option<&'static str> {
    if !(2..=3).contains(&part.len()) || !part.chars().all(|c| c.is_ascii_alphabetic()) {
        return None;
    }
    let lower = part.to_ascii_lowercase();
    if let Some((_, tag)) = COUNTRIES.iter().find(|(country, _)| *country == lower) {
        return Some(tag);
    }
    // Only a code the table knows: "DL" in "WEB-DL" is none.
    language_name(&lower)
        .and_then(language_named)
        .filter(|tag| language_tag(&lower) == *tag)
}

fn words(text: &str) -> impl Iterator<Item = &str> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
}

fn name(tag: &str) -> String {
    language_name(tag).map_or_else(|| tag.to_string(), str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::local::mkv::Chapters;

    fn track(
        index: usize,
        kind: Kind,
        codec: &str,
        language: Option<&str>,
        name: Option<&str>,
        channels: u32,
    ) -> Track {
        Track {
            index,
            kind,
            codec: codec.into(),
            name: name.map(str::to_string),
            language: language.map(str::to_string),
            default: false,
            forced: false,
            sdh: false,
            commentary: name.is_some_and(|name| name.contains("Commentary")),
            described: false,
            original: false,
            channels: Some(channels),
            width: None,
            height: None,
            entries: None,
            bytes: None,
            doubts: Vec::new(),
            notes: Vec::new(),
            read: false,
            certain: false,
        }
    }

    /// Its language known, and nothing casting doubt on it.
    fn certain(track: &Track) -> bool {
        track.language.is_some() && track.doubts.is_empty()
    }

    fn contents(tracks: Vec<Track>) -> Contents {
        Contents {
            tracks,
            chapters: Chapters::default(),
            attachments: Vec::new(),
            doubts: Vec::new(),
            duration: None,
            size: None,
        }
    }

    #[test]
    fn the_korra_episode_raises_no_doubt() {
        let mut korra = contents(vec![
            track(1, Kind::Sound, "aac", Some("el"), None, 2),
            track(2, Kind::Sound, "aac", Some("en"), Some("Surround"), 6),
            track(3, Kind::Sound, "aac", Some("en"), Some("Stereo-Commentary"), 2),
            track(4, Kind::Subtitles, "subrip", Some("el"), None, 0),
        ]);
        cast(&mut korra, "The.Legend.Of.Korra.2012.S01E01.Welcome.To.The.Republic.City.EN-GR.BluRay.1080p.x265-ByteShare[UTR]-evstav.mkv");
        assert!(korra.doubts.is_empty());
        assert!(korra.tracks.iter().all(certain));
    }

    #[test]
    fn a_name_that_says_another_language_than_the_tag() {
        let mut film = contents(vec![
            track(1, Kind::Sound, "ac3", Some("en"), Some("Ελληνικά"), 6),
            track(2, Kind::Subtitles, "subrip", Some("en"), Some("English SDH"), 0),
            track(
                3,
                Kind::Subtitles,
                "subrip",
                Some("es"),
                Some("Spanish (Latin America)"),
                0,
            ),
        ]);
        cast(&mut film, "Film.mkv");
        assert_eq!(
            film.tracks[0].doubts,
            ["Its name says Greek, but it is tagged English."]
        );
        assert!(certain(&film.tracks[1]));
        // Two languages in the name: no say either way.
        assert!(certain(&film.tracks[2]));
    }

    #[test]
    fn two_sound_tracks_alike_cast_doubt_on_each_other() {
        let mut film = contents(vec![
            track(1, Kind::Sound, "ac3", Some("en"), None, 6),
            track(2, Kind::Sound, "ac3", Some("en"), None, 6),
            // A compatibility track in another codec, and commentary, are expected.
            track(3, Kind::Sound, "truehd", Some("en"), None, 6),
            track(4, Kind::Sound, "ac3", Some("en"), Some("Director's Commentary"), 6),
        ]);
        cast(&mut film, "Film.mkv");
        assert_eq!(
            film.tracks[0].doubts,
            ["Another sound track is English too, in the same codec and channels: one of them may be another language."]
        );
        assert_eq!(film.tracks[1].doubts.len(), 1);
        assert!(certain(&film.tracks[2]) && certain(&film.tracks[3]));
    }

    #[test]
    fn a_file_named_for_a_language_none_of_its_tracks_is_in() {
        let mut film = contents(vec![
            track(1, Kind::Sound, "aac", Some("en"), None, 2),
            track(2, Kind::Sound, "aac", Some("en"), None, 6),
        ]);
        cast(&mut film, "Show.S01E03.EN-GR.1080p.WEB-DL.mkv");
        assert_eq!(film.doubts, ["Its name says Greek (EN-GR), but no track is Greek."]);
    }

    #[test]
    fn reads_languages_from_release_names_but_not_from_titles() {
        let languages = |name: &str| -> Vec<&str> { file_languages(name).into_iter().map(|(_, l)| l).collect() };
        assert_eq!(languages("Korra.S01E01.EN-GR.BluRay.x265-ByteShare.mkv"), ["en", "el"]);
        assert_eq!(languages("Film.2010.GREEK.1080p.mkv"), ["el"]);
        assert_eq!(languages("Film.ENG-GRE.mkv"), ["en", "el"]);
        assert!(languages("The.English.Patient.1996.mkv").is_empty());
        assert!(languages("Film.WEB-DL.DTS-HD.MA.5.1.mkv").is_empty());
        assert!(languages("Film.2010.x265-GROUP.mkv").is_empty());
    }
}
