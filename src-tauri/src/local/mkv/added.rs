//! Tracks from other files, added to an MKV's copy (docs/mkv-editor.md, step
//! 6): a subtitle file, a sound file, or another video's tracks. Each is read
//! as an MKV's own tracks are, its language taken from its name where it says
//! none ("Show S01E03.el.srt"), and its words read to bear that out. A text
//! subtitle file that isn't in UTF-8, as old Greek ones in Windows-1253 often
//! aren't, is read in the encoding its language uses, and written into the
//! copy as UTF-8: Matroska keeps text in nothing else.

use std::ffi::OsString;
use std::path::Path;

use serde::Serialize;
use tauri::{AppHandle, Manager};

use super::{probe, settle, words, Contents, Kind};
use crate::binaries::BinaryManager;
use crate::error::{AppError, ErrorKind, Result};
use crate::local::{gone, lost_track, Grants, Original, READS, READ_TIMEOUT};
use crate::process;
use crate::ytdlp::subtitles::{iso639_2, language_named, language_tag, primary};

/// The files tracks are added from, by their extension: subtitles, sound,
/// and videos whose tracks can be taken.
#[rustfmt::skip]
pub const EXTENSIONS: &[&str] = &[
    // Subtitles
    "srt", "ass", "ssa", "vtt", "sup", "idx", "mks",
    // Sound
    "mka", "ac3", "eac3", "ec3", "dts", "dtshd", "thd", "truehd", "aac", "m4a", "flac", "mp3",
    "opus", "ogg", "oga", "wav",
    // Videos, for their tracks
    "mkv", "mp4", "m4v", "mov", "webm",
    // Chapters (see `chapter_files`), and fonts to attach
    "xml", "txt", "ttf", "otf", "ttc",
];

/// Text subtitles written again as SRT when read in another encoding (ASS stays ASS).
pub const TEXT_CODECS: &[&str] = &["subrip", "srt", "webvtt", "text", "mov_text"];

/// The encodings a text subtitle file may be read in, besides UTF-8: Windows'
/// for each script, and the East Asian ones. What the user may pick from too.
pub const CHARSETS: &[&str] = &[
    "CP1253",
    "CP1251",
    "CP1250",
    "CP1252",
    "CP1254",
    "CP1255",
    "CP1256",
    "CP1257",
    "CP1258",
    "CP874",
    "SHIFT_JIS",
    "GBK",
    "BIG5",
    "CP949",
    "ISO-8859-7",
];

/// A file whose tracks may be added, as read.
#[derive(Debug, Clone, Serialize)]
pub struct Added {
    pub key: String,
    /// "Show S01E03.el.srt"
    pub name: String,
    pub contents: Contents,
    /// For a text subtitle file not in UTF-8: the encoding it is read in ("CP1253").
    pub charset: Option<String>,
    /// A few of its subtitles' first lines, to see that they read right.
    pub sample: Vec<String>,
    /// Bytes.
    pub size: u64,
}

/// Reads a file the user gave to add tracks from. `charset` reads a text
/// subtitle file in that encoding, as the user chose; else it is found.
pub async fn read(app: &AppHandle, bins: &BinaryManager, key: &str, charset: Option<&str>) -> Result<Added> {
    let original = app.state::<Grants>().get(key).ok_or_else(lost_track)?;
    if !original.path().is_file() {
        return Err(gone());
    }
    let charset = match charset {
        Some(chosen) if !CHARSETS.contains(&chosen) => {
            return Err(AppError::internal("Siphon doesn't know that encoding."));
        }
        chosen => chosen,
    };
    let _permit = READS
        .acquire()
        .await
        .map_err(|err| AppError::internal("Couldn't read the file.").with_details(err.to_string()))?;
    let path = original.path();
    let mut contents = inspect(bins, path)
        .await
        .map_err(|details| no_tracks(&original).with_details(details))?;
    if contents.tracks.is_empty() {
        return Err(no_tracks(&original));
    }
    let name = original.name();
    let named = Named::of(&name);
    for track in &mut contents.tracks {
        if track.language.is_none() {
            track.language = named.language.clone();
        }
        if track.kind == Kind::Subtitles {
            track.forced |= named.forced;
            track.sdh |= named.sdh;
        }
    }
    let text = text_file(&contents);
    let languages = app
        .state::<crate::settings::SettingsStore>()
        .get()
        .my_languages(app.state::<crate::choices::ChoiceStore>().get().subtitles.as_deref());
    let charset = match (text, charset) {
        (false, _) => None,
        (true, Some(chosen)) => Some(chosen.to_string()),
        (true, None) => find_charset(bins, path, named.language.as_deref(), &languages).await,
    };
    let mut sample = Vec::new();
    let windows = words::windows(contents.duration, contents.size);
    for track in &mut contents.tracks {
        if track.kind != Kind::Subtitles || !words::TEXT.contains(&track.codec.as_str()) {
            continue;
        }
        let lines = match &charset {
            Some(charset) => recoded_lines(bins, path, track.index, charset).await,
            None => match super::probe_output(bins, words::args(path, track.index, windows.as_deref())).await {
                Some(output) => words::lines(&output, &track.codec),
                None => continue,
            },
        };
        if sample.is_empty() {
            sample = lines
                .iter()
                .filter(|line| line.chars().count() > 12)
                .take(3)
                .cloned()
                .collect();
        }
        words::apply(track, &words::Reading::of(&lines));
    }
    super::read_pictures(bins, path, &mut contents.tracks, windows.as_deref()).await;
    settle(&mut contents);
    Ok(Added {
        key: original.key(),
        name,
        size: std::fs::metadata(path).map(|meta| meta.len()).unwrap_or(0),
        contents,
        charset,
        sample,
    })
}

/// What ffprobe lists of a file tracks are added from.
pub async fn inspect(bins: &BinaryManager, path: &Path) -> std::result::Result<Contents, String> {
    let mut cmd = process::command(bins.ffprobe());
    cmd.args(probe::args(path));
    let output = {
        let _in_use = bins.in_use().await;
        process::output_within(cmd, READ_TIMEOUT).await
    };
    let output = match output {
        Ok(Some(output)) => output,
        Ok(None) => return Err("ffprobe took too long".into()),
        Err(error) => return Err(error.to_string()),
    };
    probe::parse_any(&String::from_utf8_lossy(&output.stdout))
        .filter(|_| output.status.success())
        .ok_or_else(|| String::from_utf8_lossy(&output.stderr).trim().to_string())
}

/// A file of text subtitles alone: one whose encoding matters.
pub fn text_file(contents: &Contents) -> bool {
    contents.tracks.len() == 1
        && contents.tracks[0].kind == Kind::Subtitles
        && words::TEXT.contains(&contents.tracks[0].codec.as_str())
}

fn no_tracks(original: &Original) -> AppError {
    AppError::new(
        ErrorKind::Unsupported,
        format!("{} holds no subtitles or sound Siphon can add.", original.name()),
    )
}

/// What a file's name says of the track in it, the way subtitle files are
/// named beside a film: "Film.el.srt", "Film.Greek.forced.srt", "Film.en.sdh.srt".
#[derive(Debug, Default, PartialEq)]
pub struct Named {
    pub language: Option<String>,
    pub forced: bool,
    pub sdh: bool,
}

impl Named {
    pub fn of(file_name: &str) -> Named {
        let stem = file_name.rsplit_once('.').map_or(file_name, |(stem, _)| stem);
        let mut named = Named::default();
        // The last few parts of its name, after the dots: what such names add.
        for part in stem.rsplit('.').take(3) {
            let lower = part.trim().to_lowercase();
            match lower.as_str() {
                "forced" | "foreign" => named.forced = true,
                "sdh" | "cc" | "hi" => named.sdh = true,
                _ if named.language.is_none() => named.language = language_in(&lower),
                _ => {}
            }
        }
        // Or a language's name at the end, after a space: "Film Greek.srt".
        if named.language.is_none() {
            named.language = stem
                .rsplit([' ', '_', '-'])
                .next()
                .and_then(|word| language_named(word.trim()))
                .map(str::to_string);
        }
        named
    }
}

/// A part of a name that is a language: "el", "ell", "gre", "greek", "pt-br".
fn language_in(part: &str) -> Option<String> {
    if let Some(tag) = language_named(part) {
        return Some(tag.to_string());
    }
    // A tag is letters, and a region or script after a hyphen: not "gre-1253".
    if !part.chars().all(|c| c.is_ascii_alphabetic() || c == '-') {
        return None;
    }
    let code = primary(part);
    let known = (2..=3).contains(&code.len())
        && code.chars().all(|c| c.is_ascii_lowercase())
        && iso639_2(code, true).is_some()
        && crate::ytdlp::subtitles::language_name(code).is_some();
    known.then(|| language_tag(part))
}

/// The encoding a language's text was written in before UTF-8, on Windows.
pub fn charset_of(language: &str) -> &'static str {
    match primary(language) {
        "el" => "CP1253",
        "ru" | "uk" | "bg" | "be" | "mk" | "sr" => "CP1251",
        "pl" | "cs" | "sk" | "hu" | "ro" | "hr" | "sl" | "bs" | "sq" => "CP1250",
        "tr" | "az" => "CP1254",
        "he" | "yi" => "CP1255",
        "ar" | "fa" | "ur" => "CP1256",
        "lt" | "lv" | "et" => "CP1257",
        "vi" => "CP1258",
        "th" => "CP874",
        "ja" => "SHIFT_JIS",
        "zh" => "GBK",
        "ko" => "CP949",
        _ => "CP1252",
    }
}

/// The encoding a text subtitle file is in, if not UTF-8 (or UTF-16, which
/// FFmpeg reads by its mark). Tried in the encoding of the language its name
/// says, then of the user's languages, then of the common scripts: one is
/// taken when every line reads in it and its words are in a language written
/// in it. Failing all, the name's language decides, else Western.
async fn find_charset(bins: &BinaryManager, path: &Path, named: Option<&str>, mine: &[String]) -> Option<String> {
    let bytes = tokio::fs::read(path).await.ok()?;
    if bytes.len() > 64 * 1024 * 1024
        || bytes.starts_with(&[0xEF, 0xBB, 0xBF])
        || bytes.starts_with(&[0xFF, 0xFE])
        || bytes.starts_with(&[0xFE, 0xFF])
        || std::str::from_utf8(&bytes).is_ok()
    {
        return None;
    }
    // Every byte reads in Latin-1: how many lines there are.
    let all = recoded_lines(bins, path, 0, "ISO-8859-1").await.len();
    let mut candidates: Vec<&str> = named.into_iter().map(charset_of).collect();
    candidates.extend(mine.iter().map(|language| charset_of(language)));
    candidates.extend(["CP1253", "CP1251", "CP1255", "CP1256", "CP1252", "CP1250"]);
    let mut tried = Vec::new();
    for charset in candidates {
        if tried.contains(&charset) {
            continue;
        }
        tried.push(charset);
        let lines = recoded_lines(bins, path, 0, charset).await;
        // A byte the encoding has no letter for drops its line.
        if lines.len() * 100 < all * 95 {
            continue;
        }
        let Some(language) = words::Reading::of(&lines).language else {
            continue;
        };
        if charset_of(&language) == charset && reads_right(&language, &lines) {
            return Some(charset.to_string());
        }
    }
    Some(named.map_or("CP1252", charset_of).to_string())
}

/// Whether text read as `language` looks written in it, beyond its alphabet:
/// Greek read from another script's bytes (Hebrew's, say) is still all Greek
/// letters, but puts the final sigma inside words.
fn reads_right(language: &str, lines: &[String]) -> bool {
    if primary(language) != "el" {
        return true;
    }
    let words: Vec<&str> = lines.iter().flat_map(|line| line.split_whitespace()).collect();
    let misplaced = words
        .iter()
        .filter(|word| {
            let letters: Vec<char> = word.chars().filter(|c| c.is_alphabetic()).collect();
            letters.iter().rev().skip(1).any(|c| *c == 'ς')
        })
        .count();
    misplaced * 50 <= words.len()
}

/// The arguments that make FFmpeg print a text subtitle track as SRT, read in `charset`.
pub fn recode_args(path: &Path, index: usize, charset: &str) -> Vec<OsString> {
    let mut args: Vec<OsString> = ["-hide_banner", "-nostdin", "-v", "quiet", "-sub_charenc", charset, "-i"]
        .map(OsString::from)
        .to_vec();
    args.push(path.into());
    args.extend(["-map", &format!("0:{index}"), "-c:s", "srt", "-f", "srt", "-"].map(OsString::from));
    args
}

/// A text subtitle track's lines, read in `charset`, without their numbers, times and formatting.
async fn recoded_lines(bins: &BinaryManager, path: &Path, index: usize, charset: &str) -> Vec<String> {
    let mut cmd = process::command(bins.ffmpeg());
    cmd.args(recode_args(path, index, charset));
    let output = {
        let _in_use = bins.in_use().await;
        process::output_within(cmd, READ_TIMEOUT).await.ok().flatten()
    };
    output
        .map(|output| srt_lines(&String::from_utf8_lossy(&output.stdout)))
        .unwrap_or_default()
}

/// The cues of an SRT file, as plain text, one each.
fn srt_lines(srt: &str) -> Vec<String> {
    srt.replace("\r\n", "\n")
        .split("\n\n")
        .map(|cue| {
            let text: Vec<&str> = cue
                .lines()
                .skip_while(|line| line.trim().chars().all(|c| c.is_ascii_digit()) || line.contains("-->"))
                .collect();
            words::plain(&text.join("\n"))
        })
        .filter(|text| !text.is_empty())
        .collect()
}

/// Matches files to add to a season's episodes by their names: the same name
/// with more after it ("Show S01E03.el.srt" to "Show S01E03.mkv"), else the
/// same season and episode ("S01E03", "1x03"), else the same episode number
/// when no episode names a season. For each file, the episode's place, if one alone fits.
pub fn match_episodes(episodes: &[String], files: &[String]) -> Vec<Option<usize>> {
    let stems: Vec<String> = episodes.iter().map(|name| stem(name).to_lowercase()).collect();
    let numbers: Vec<Option<(Option<u32>, u32)>> = episodes.iter().map(|name| episode_of(name)).collect();
    files
        .iter()
        .map(|file| {
            let lower = file.to_lowercase();
            let by_name: Vec<usize> = (0..episodes.len())
                .filter(|&place| {
                    let stem = &stems[place];
                    lower.starts_with(stem.as_str()) && lower[stem.len()..].starts_with(['.', ' ', '_', '-'])
                })
                .collect();
            if let [only] = by_name[..] {
                return Some(only);
            }
            let wanted = episode_of(file)?;
            let by_number: Vec<usize> = (0..episodes.len())
                .filter(|&place| match (numbers[place], wanted) {
                    (Some((Some(season), episode)), (Some(s), e)) => season == s && episode == e,
                    (Some((_, episode)), (_, e)) => episode == e,
                    _ => false,
                })
                .collect();
            match by_number[..] {
                [only] => Some(only),
                _ => None,
            }
        })
        .collect()
}

fn stem(name: &str) -> &str {
    name.rsplit_once('.').map_or(name, |(stem, _)| stem)
}

/// The season and episode a name says: "S01E03" → (1, 3), "1x03" → (1, 3),
/// "E03" or "Episode 3" → (none, 3).
fn episode_of(name: &str) -> Option<(Option<u32>, u32)> {
    let lower = name.to_lowercase();
    let bytes = lower.as_bytes();
    let number_at = |from: usize| -> Option<(u32, usize)> {
        let digits: String = lower[from..].chars().take_while(char::is_ascii_digit).take(4).collect();
        (!digits.is_empty()).then(|| (digits.parse().unwrap_or(0), from + digits.len()))
    };
    let boundary = |at: usize| at == 0 || !bytes[at - 1].is_ascii_alphanumeric();
    for at in 0..bytes.len() {
        // "s01e03"
        if bytes[at] == b's' && boundary(at) {
            if let Some((season, end)) = number_at(at + 1) {
                if bytes.get(end) == Some(&b'e') {
                    if let Some((episode, _)) = number_at(end + 1) {
                        return Some((Some(season), episode));
                    }
                }
            }
        }
        // "1x03"
        if bytes[at].is_ascii_digit() && boundary(at) {
            if let Some((season, end)) = number_at(at) {
                if bytes.get(end) == Some(&b'x') && season < 100 {
                    if let Some((episode, after)) = number_at(end + 1) {
                        if after >= bytes.len() || !bytes[after].is_ascii_alphanumeric() {
                            return Some((Some(season), episode));
                        }
                    }
                }
            }
        }
    }
    for (word, skip) in [("episode", 7), ("ep", 2), ("e", 1)] {
        let mut from = 0;
        while let Some(found) = lower[from..].find(word) {
            let at = from + found;
            if boundary(at) {
                let after = at + skip;
                let after = after + lower[after..].chars().take_while(|c| *c == ' ' || *c == '.').count();
                if let Some((episode, _)) = number_at(after) {
                    return Some((None, episode));
                }
            }
            from = at + word.len();
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::local::mkv::Track;

    #[test]
    fn a_name_says_the_language_and_kind_of_subtitles_in_it() {
        let el = Some("el".to_string());
        assert_eq!(Named::of("Show S01E03.el.srt").language, el);
        assert_eq!(Named::of("Show S01E03.gre.srt").language, el);
        assert_eq!(Named::of("Show S01E03.ell.srt").language, el);
        assert_eq!(Named::of("Show S01E03.Greek.srt").language, el);
        assert_eq!(Named::of("Show S01E03 Greek.srt").language, el);
        let forced = Named::of("Film (2010).el.forced.srt");
        assert_eq!(
            (forced.language.as_deref(), forced.forced, forced.sdh),
            (Some("el"), true, false)
        );
        let sdh = Named::of("Film.en.sdh.srt");
        assert_eq!((sdh.language.as_deref(), sdh.sdh), (Some("en"), true));
        assert_eq!(Named::of("Film.pt-br.srt").language.as_deref(), Some("pt-br"));
        // Names that say no language.
        assert_eq!(Named::of("Show S01E03.srt").language, None);
        assert_eq!(
            Named::of("The.Legend.Of.Korra.S01E03.The.Revelation.srt").language,
            None
        );
        assert_eq!(Named::of("Film.2010.1080p.srt").language, None);
        assert_eq!(Named::of("Film.gre-1253.srt").language, None);
    }

    #[test]
    fn a_language_names_the_encoding_its_old_text_files_used() {
        assert_eq!(charset_of("el"), "CP1253");
        assert_eq!(charset_of("ru"), "CP1251");
        assert_eq!(charset_of("pl"), "CP1250");
        assert_eq!(charset_of("en"), "CP1252");
        assert_eq!(charset_of("pt-br"), "CP1252");
        assert!(CHARSETS.contains(&charset_of("ja")));
    }

    #[test]
    fn greek_read_from_another_scripts_bytes_is_found_out() {
        let real = vec![
            "Όταν ήμουν μικρός, ο πατέρας μου".to_string(),
            "Δεν έχω λεφτά.".to_string(),
        ];
        assert!(reads_right("el", &real));
        // Hebrew bytes read as Windows-1253: the final sigma lands inside words.
        let wrong = vec!["ςιμος αςλς ςφιρ ρςγ".to_string(), "λςαμ ςπιρ".to_string()];
        assert!(!reads_right("el", &wrong));
    }

    #[test]
    fn an_srt_reads_as_its_cues_text() {
        let srt = "1\r\n00:00:02,030 --> 00:00:03,780\r\n<i>Γη</i>\r\n\r\n2\r\n00:00:04,910 --> 00:00:06,220\r\nΦωτιά\r\nκαι νερό\r\n\r\n";
        assert_eq!(srt_lines(srt), ["Γη", "Φωτιά\nκαι νερό"]);
    }

    #[test]
    fn files_are_matched_to_episodes_by_name_then_number() {
        let episodes: Vec<String> = (1..=12).map(|n| format!("Korra S01E{n:02}.mkv")).collect();
        let files = [
            "Korra S01E03.el.srt",
            "korra s01e04.Greek.forced.srt",
            "The.Legend.of.Korra.S01E05.720p.GR.srt",
            "Korra 1x06.srt",
            "Korra S02E06.srt",
            "Notes.txt",
        ]
        .map(String::from);
        assert_eq!(
            match_episodes(&episodes, &files),
            [Some(2), Some(3), Some(4), Some(5), None, None]
        );
        // Episodes named by number alone: a file's episode number places it.
        let plain: Vec<String> = ["Episode 1.mkv", "Episode 2.mkv"].map(String::from).to_vec();
        assert_eq!(match_episodes(&plain, &["Show.S01E02.el.srt".to_string()]), [Some(1)]);
        // Two episodes it could be: none is chosen.
        let twice: Vec<String> = ["A S01E01.mkv", "B S02E01.mkv"].map(String::from).to_vec();
        assert_eq!(match_episodes(&twice, &["E01.srt".to_string()]), [None]);
    }

    #[test]
    fn a_name_says_its_season_and_episode() {
        assert_eq!(episode_of("Korra S01E03.mkv"), Some((Some(1), 3)));
        assert_eq!(episode_of("korra.s1e12.srt"), Some((Some(1), 12)));
        assert_eq!(episode_of("Korra 2x05 Greek.srt"), Some((Some(2), 5)));
        assert_eq!(episode_of("Episode 7.mkv"), Some((None, 7)));
        assert_eq!(episode_of("Show - E08.srt"), Some((None, 8)));
        assert_eq!(episode_of("Film (2010) 1920x1080.mkv"), None);
        assert_eq!(episode_of("Film.mkv"), None);
    }

    #[test]
    fn tracks_kept_track_of_their_kind() {
        let track = Track {
            index: 0,
            kind: Kind::Subtitles,
            codec: "subrip".into(),
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
            bytes: None,
            doubts: Vec::new(),
            notes: Vec::new(),
            read: false,
            certain: false,
        };
        let mut contents = Contents {
            container: Default::default(),
            tracks: vec![track.clone()],
            chapters: Default::default(),
            attachments: Vec::new(),
            doubts: Vec::new(),
            suggestion: None,
            duration: None,
            size: None,
        };
        assert!(text_file(&contents));
        contents.tracks[0].codec = "hdmv_pgs_subtitle".into();
        assert!(!text_file(&contents));
    }
}
