//! What a text subtitle track says, read from its own words: its language,
//! told by `whatlang` from a few minutes of its lines; whether its lines
//! describe sounds, as subtitles for the deaf do; how many lines it has
//! beside the file's other tracks, which tells forced subtitles.
//!
//! The lines come from ffprobe, which prints a track's packets: from the
//! whole file when it is small, and from a few windows across it when it is
//! large, so a 40 GB film is read for a gigabyte at most.

use std::ffi::OsString;
use std::path::Path;

use serde_json::Value;
use whatlang::Script;

use super::Track;
use crate::ytdlp::subtitles::{language_name, language_tag, primary};

/// Subtitles made of text, by ffprobe's codec names.
pub const TEXT: &[&str] = &["subrip", "ass", "ssa", "webvtt", "text", "mov_text"];

/// Bytes of a file read for its subtitles' words, at most.
const BUDGET: f64 = 1024.0 * 1024.0 * 1024.0;
/// Windows spread across a large file, each between these many seconds long.
const WINDOWS: usize = 4;
const SHORTEST: f64 = 30.0;
const LONGEST: f64 = 300.0;
/// Lines a track's language is told from, at the fewest, unless its
/// alphabet is a language's own (Greek, Hebrew, Korean…).
const FEWEST_LINES: usize = 8;
/// A track with at least this share of lines that describe sounds is SDH.
const SDH_SHARE: f64 = 0.08;
const SDH_LINES: usize = 20;

/// Languages too close for their words to tell them apart reliably: a tag
/// naming one of them isn't overruled by a reading of another.
const CLOSE: &[&[&str]] = &[
    &["da", "no", "nb", "nn", "sv"],
    &["sr", "hr", "bs", "sh"],
    &["id", "ms"],
    &["cs", "sk"],
    &["ru", "uk", "be"],
    &["es", "gl", "ca"],
    &["pt", "gl"],
    &["nl", "af"],
];

/// The windows of a file read for its subtitles, as (start, seconds); None
/// to read it whole.
pub fn windows(duration: Option<f64>, size: Option<u64>) -> Option<Vec<(f64, f64)>> {
    let duration = duration.filter(|seconds| *seconds > 0.0)?;
    let size = size? as f64;
    if size <= BUDGET {
        return None;
    }
    let rate = size / duration;
    let each = (BUDGET / rate / WINDOWS as f64).clamp(SHORTEST, LONGEST);
    if each * WINDOWS as f64 >= duration {
        return None;
    }
    let windows = (0..WINDOWS).map(|i| {
        let middle = duration * (2 * i + 1) as f64 / (2 * WINDOWS) as f64;
        ((middle - each / 2.0).max(0.0), each)
    });
    Some(windows.collect())
}

/// The arguments that make ffprobe print the packets of the track at
/// `index`, in `windows` or the whole file.
pub fn args(file: &Path, index: usize, windows: Option<&[(f64, f64)]>) -> Vec<OsString> {
    let mut args: Vec<OsString> = ["-v", "error"].map(OsString::from).to_vec();
    if let Some(windows) = windows {
        let intervals: Vec<String> = windows
            .iter()
            .map(|(start, seconds)| format!("{start:.3}%+{seconds:.3}"))
            .collect();
        args.extend(["-read_intervals".into(), intervals.join(",").into()]);
    }
    args.extend(
        [
            "-select_streams",
            &index.to_string(),
            "-show_entries",
            "packet=data",
            "-show_data",
            "-print_format",
            "json",
        ]
        .map(OsString::from),
    );
    args.push(file.into());
    args
}

/// The lines in what [`args`] printed for a track in `codec`, as plain text.
pub fn lines(json: &str, codec: &str) -> Vec<String> {
    let Ok(value) = serde_json::from_str::<Value>(json) else {
        return Vec::new();
    };
    let packets = value["packets"].as_array().map(Vec::as_slice).unwrap_or_default();
    packets
        .iter()
        .filter_map(|packet| packet["data"].as_str())
        .map(|dump| String::from_utf8_lossy(&unhex(dump)).trim_end_matches('\0').to_string())
        .map(|text| match codec {
            // Matroska keeps an ASS line as "ReadOrder,Layer,Style,Name,MarginL,MarginR,MarginV,Effect,Text".
            "ass" | "ssa" => text.splitn(9, ',').nth(8).unwrap_or_default().to_string(),
            _ => text,
        })
        .map(|text| plain(&text))
        .filter(|text| !text.is_empty())
        .collect()
}

/// What a track's lines say about it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Reading {
    /// The language its words are in, where they leave no doubt.
    pub language: Option<String>,
    pub lines: usize,
    /// Lines that describe sounds: "[door slams]", "(sighs)", "JOHN: Hi".
    pub sounds: usize,
    /// One of them, to show.
    pub sound: Option<String>,
}

impl Reading {
    pub fn of(lines: &[String]) -> Reading {
        // A subtitle's lines one by one: the "[door slams]" under "Hey!".
        let sounds: Vec<&str> = lines
            .iter()
            .filter_map(|cue| cue.lines().find(|line| describes_sound(line)))
            .collect();
        let text = lines
            .iter()
            .map(|line| line.trim_start_matches(['-', ' ']))
            .collect::<Vec<_>>()
            .join("\n");
        let language = whatlang::detect(&text)
            .filter(|info| info.is_reliable() && (lines.len() >= FEWEST_LINES || own_alphabet(info.script())))
            .map(|info| tag(info.lang().code()));
        Reading {
            language,
            lines: lines.len(),
            sounds: sounds.len(),
            sound: sounds.first().map(|line| line.to_string()),
        }
    }

    /// Its lines describe sounds, as SDH does.
    pub fn sdh(&self) -> bool {
        self.lines >= SDH_LINES && self.sounds as f64 >= self.lines as f64 * SDH_SHARE
    }
}

/// What a track's own words say about it, put in it: its language, found or
/// put right; SDH.
pub fn apply(track: &mut Track, reading: &Reading) {
    if let Some(read) = &reading.language {
        match track.language.clone() {
            None => {
                track.language = Some(read.clone());
                track.read = true;
            }
            Some(tagged) if primary(&tagged) == primary(read) => track.read = true,
            // Too close to tell apart by words: the tag stands.
            Some(tagged) if close(&tagged, read) => {}
            Some(tagged) => {
                track.notes.push(format!(
                    "Tagged {}, but its words are {}: the copy says {}.",
                    name(&tagged),
                    name(read),
                    name(read)
                ));
                track.language = Some(read.clone());
                track.read = true;
            }
        }
    }
    if reading.sdh() && !track.sdh {
        track.sdh = true;
        let example = reading
            .sound
            .as_deref()
            .map(|line| format!(": \"{line}\""))
            .unwrap_or_default();
        track.notes.push(format!(
            "Its lines describe sounds, as subtitles for the deaf do{example}."
        ));
    }
}

/// A line describes a sound or names its speaker, as only SDH does.
fn describes_sound(line: &str) -> bool {
    let bracketed = |open: char, close: char| {
        line.find(open).is_some_and(|start| {
            line[start + open.len_utf8()..]
                .find(close)
                .is_some_and(|length| (1..=40).contains(&length))
        })
    };
    let speaker = line.split_once(':').is_some_and(|(before, _)| {
        let before = before.trim_start_matches(['-', ' ']);
        before.chars().count() >= 2
            && before.chars().any(char::is_alphabetic)
            && before
                .chars()
                .all(|c| c.is_uppercase() || c == ' ' || c == '.' || c == '\'')
    });
    bracketed('[', ']') || bracketed('(', ')') || speaker
}

/// A line without its formatting: ASS's `{\i1}` and `\N`, SRT's `<i>`.
pub(super) fn plain(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut skipping = None;
    for c in text
        .replace("\\N", "\n")
        .replace("\\n", "\n")
        .replace("\\h", " ")
        .chars()
    {
        match (skipping, c) {
            (None, '{') => skipping = Some('}'),
            (None, '<') => skipping = Some('>'),
            (Some(end), c) if c == end => skipping = None,
            (Some(_), _) => {}
            (None, c) => out.push(c),
        }
    }
    out.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

/// The bytes in ffprobe's dump of a packet: "00000000: 2d20 cea3 …  - ..".
fn unhex(dump: &str) -> Vec<u8> {
    let mut bytes = Vec::new();
    for line in dump.lines() {
        let Some((_, rest)) = line.split_once(": ") else {
            continue;
        };
        let hex = rest.split("  ").next().unwrap_or_default();
        let digits: Vec<u8> = hex
            .bytes()
            .filter_map(|c| (c as char).to_digit(16))
            .map(|d| d as u8)
            .collect();
        let (pairs, _) = digits.as_chunks::<2>();
        bytes.extend(pairs.iter().map(|[high, low]| high << 4 | low));
    }
    bytes
}

/// A language `whatlang` names (ISO 639-3) as Siphon names it: "ell" → "el".
fn tag(code: &str) -> String {
    match code {
        "cmn" => "zh".into(),
        "pes" => "fa".into(),
        _ => language_tag(code),
    }
}

/// An alphabet only one language `whatlang` knows is written in.
fn own_alphabet(script: Script) -> bool {
    matches!(
        script,
        Script::Greek | Script::Hebrew | Script::Georgian | Script::Armenian | Script::Hangul | Script::Thai
    )
}

fn close(a: &str, b: &str) -> bool {
    let (a, b) = (primary(a), primary(b));
    CLOSE.iter().any(|group| group.contains(&a) && group.contains(&b))
}

fn name(tag: &str) -> String {
    language_name(tag).map_or_else(|| tag.to_string(), str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::local::mkv::Kind;

    /// ffprobe's dump of a packet holding `text`.
    fn dump(text: &str) -> String {
        let mut out = String::new();
        for (row, chunk) in text.as_bytes().chunks(16).enumerate() {
            let hex: Vec<String> = chunk
                .chunks(2)
                .map(|pair| pair.iter().map(|b| format!("{b:02x}")).collect())
                .collect();
            out.push_str(&format!("\n{:08x}: {:<40} {}", row * 16, hex.join(" "), "."));
        }
        out
    }

    fn json(texts: &[&str]) -> String {
        let packets: Vec<Value> = texts
            .iter()
            .map(|text| serde_json::json!({ "data": dump(text) }))
            .collect();
        serde_json::json!({ "packets": packets }).to_string()
    }

    fn track(language: Option<&str>) -> Track {
        Track {
            index: 4,
            kind: Kind::Subtitles,
            codec: "subrip".into(),
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
            certain: false,
        }
    }

    const GREEK: &[&str] = &[
        "- Σε παρακαλώ πες μου ότι φτάσαμε!\r\n- Ναι, Ίκι.",
        "Δεν έχω λεφτά.",
        "Η Ρεπούμπλικ Σίτι είναι πανέμορφη.",
    ];

    const ENGLISH: &[&str] = &[
        "Naga, go! Thatta girl!",
        "Hey, you! Stop! You can't fish here!",
        "From all over the world could live and thrive together in peace and harmony.",
        "Aang's time has passed.",
        "Can she firebend, too?",
        "Could she be the one we have been waiting for all these years?",
        "I don't know what you're talking about, but I'm going to find out.",
        "Yes, well. We will see about that tomorrow morning.",
    ];

    #[test]
    fn reads_the_lines_of_srt_and_of_ass_as_matroska_keeps_them() {
        assert_eq!(
            lines(&json(GREEK), "subrip")[0],
            "- Σε παρακαλώ πες μου ότι φτάσαμε!\n- Ναι, Ίκι."
        );
        let ass = json(&["74,0,Default,,0,0,0,,{\\i1}Is it too late{\\i0} for you\\Nto un-retire\0"]);
        assert_eq!(lines(&ass, "ass"), ["Is it too late for you\nto un-retire"]);
        assert_eq!(
            lines(&json(&["<i>Hello</i> <font color=\"red\">there</font>"]), "subrip"),
            ["Hello there"]
        );
    }

    #[test]
    fn tells_greek_from_a_few_lines_and_english_from_more() {
        let greek: Vec<String> = GREEK.iter().map(|line| plain(line)).collect();
        assert_eq!(Reading::of(&greek).language.as_deref(), Some("el"));
        let english: Vec<String> = ENGLISH.iter().map(|line| line.to_string()).collect();
        assert_eq!(Reading::of(&english).language.as_deref(), Some("en"));
        // Two English lines aren't enough to be sure of.
        assert_eq!(Reading::of(&english[..2]).language, None);
    }

    #[test]
    fn finds_or_puts_right_a_language_but_not_between_close_ones() {
        let reading = |language: &str| Reading {
            language: Some(language.into()),
            lines: 50,
            ..Reading::default()
        };

        let mut untagged = track(None);
        apply(&mut untagged, &reading("el"));
        assert_eq!((untagged.language.as_deref(), untagged.read), (Some("el"), true));

        let mut confirmed = track(Some("el"));
        apply(&mut confirmed, &reading("el"));
        assert!(confirmed.read && confirmed.notes.is_empty());

        let mut wrong = track(Some("en"));
        apply(&mut wrong, &reading("el"));
        assert_eq!(wrong.language.as_deref(), Some("el"));
        assert_eq!(
            wrong.notes,
            ["Tagged English, but its words are Greek: the copy says Greek."]
        );

        let mut close = track(Some("da"));
        apply(&mut close, &reading("nb"));
        assert_eq!((close.language.as_deref(), close.read), (Some("da"), false));
    }

    #[test]
    fn knows_sdh_by_lines_that_describe_sounds() {
        let mut lines: Vec<String> = ENGLISH.iter().cycle().take(40).map(|line| line.to_string()).collect();
        assert!(!Reading::of(&lines).sdh());
        lines[3] = "Get down!\n[door slams]".into();
        lines[9] = "(SIGHS) Fine.".into();
        lines[17] = "KORRA: Let's go!".into();
        lines[30] = "- MAKO: Wait up!".into();
        let reading = Reading::of(&lines);
        assert!(reading.sdh());
        let mut sdh = track(Some("en"));
        apply(&mut sdh, &reading);
        assert!(sdh.sdh);
        assert_eq!(
            sdh.notes,
            ["Its lines describe sounds, as subtitles for the deaf do: \"[door slams]\"."]
        );
        // Songs and a time aren't sounds described.
        assert!(!describes_sound("♪ Leaves from the vine ♪"));
        assert!(!describes_sound("Meet me at 10:30."));
        assert!(!describes_sound("I: what?"));
    }

    #[test]
    fn reads_a_long_film_in_windows_and_a_short_one_whole() {
        // A 24-minute, 290 MB episode: whole.
        assert_eq!(windows(Some(1444.0), Some(290_000_000)), None);
        // A 2-hour, 40 GB film: four windows of about 50 seconds, a gigabyte in all.
        let film = windows(Some(7200.0), Some(40_000_000_000)).unwrap();
        assert_eq!(film.len(), 4);
        let (first, seconds) = film[0];
        assert!((first - (900.0 - seconds / 2.0)).abs() < 0.01 && (30.0..60.0).contains(&seconds));
        assert_eq!(windows(None, Some(1)), None);
    }
}
