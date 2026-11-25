//! Chapters from a file of their own, for an MKV's copy (docs/mkv-editor.md,
//! step 7): Matroska's XML, as MKVToolNix writes them; the simple text OGM
//! used ("CHAPTER01=00:00:00.000", "CHAPTER01NAME=Intro"); FFmpeg's own
//! metadata file; or another video's chapters. And the file FFmpeg is given
//! to write a copy's chapters from, as chosen.

use tauri::{AppHandle, Manager};

use super::{added, Chapter};
use crate::binaries::BinaryManager;
use crate::error::{AppError, ErrorKind, Result};
use crate::local::{gone, lost_track, Grants};

/// The files chapters are read from by their text, by extension.
pub const EXTENSIONS: &[&str] = &["xml", "txt"];

/// At most this many chapters: more is not a list of chapters.
pub const MOST: usize = 2000;

/// Reads the chapters in a file the user gave: a chapters file, or a video's.
pub async fn read(app: &AppHandle, bins: &BinaryManager, key: &str) -> Result<Vec<Chapter>> {
    let file = app.state::<Grants>().get(key).ok_or_else(lost_track)?;
    let path = file.path();
    if !path.is_file() {
        return Err(gone());
    }
    let none = || {
        AppError::new(
            ErrorKind::Unsupported,
            format!("Siphon found no chapters in {}.", file.name()),
        )
    };
    let textual = path
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| EXTENSIONS.contains(&ext.to_ascii_lowercase().as_str()));
    let chapters = if textual {
        let bytes = tokio::fs::read(path).await?;
        if bytes.len() > 8 * 1024 * 1024 {
            return Err(none());
        }
        parse(&String::from_utf8_lossy(&bytes))
    } else {
        added::inspect(bins, path)
            .await
            .ok()
            .map(|contents| contents.chapters.list)
            .filter(|list| !list.is_empty())
    };
    chapters.filter(|list| list.len() <= MOST).ok_or_else(none)
}

/// The chapters in a chapters file's text, in order; None if it holds none.
pub fn parse(text: &str) -> Option<Vec<Chapter>> {
    let text = text.trim_start_matches('\u{feff}');
    let mut chapters = if text.contains("<Chapters") || text.contains("<ChapterAtom") {
        xml(text)
    } else if text.trim_start().starts_with(";FFMETADATA") {
        ffmetadata(text)
    } else {
        ogm(text)
    };
    chapters.sort_by(|a, b| a.start.total_cmp(&b.start));
    chapters.dedup_by(|a, b| (a.start - b.start).abs() < 0.001);
    (!chapters.is_empty()).then_some(chapters)
}

/// Matroska's XML: each chapter's start and first name, of the first
/// edition, chapters within chapters taken as they come; hidden ones left out.
fn xml(text: &str) -> Vec<Chapter> {
    // The first edition alone, as a copy keeps.
    let first = match text.match_indices("<EditionEntry").nth(1) {
        Some((at, _)) => &text[..at],
        None => text,
    };
    let starts: Vec<usize> = first.match_indices("<ChapterTimeStart>").map(|(at, _)| at).collect();
    starts
        .iter()
        .enumerate()
        .filter_map(|(place, &at)| {
            let end = starts.get(place + 1).copied().unwrap_or(first.len());
            let atom = &first[at..end];
            if inner(atom, "ChapterFlagHidden").is_some_and(|hidden| hidden.trim() == "1") {
                return None;
            }
            Some(Chapter {
                start: clock(&inner(atom, "ChapterTimeStart")?)?,
                title: inner(atom, "ChapterString")
                    .map(|title| unescape(title.trim()))
                    .unwrap_or_default(),
            })
        })
        .collect()
}

/// The text inside `<tag>…</tag>`, the first in `text`.
fn inner(text: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = text.find(&open)? + open.len();
    let length = text[start..].find(&close)?;
    Some(text[start..start + length].to_string())
}

fn unescape(text: &str) -> String {
    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

/// The simple text OGM used: "CHAPTER01=00:00:00.000" and "CHAPTER01NAME=Intro".
fn ogm(text: &str) -> Vec<Chapter> {
    let mut chapters: Vec<(String, Chapter)> = Vec::new();
    let mut names: Vec<(String, String)> = Vec::new();
    for line in text.lines() {
        let Some((key, value)) = line.trim().split_once('=') else {
            continue;
        };
        let key = key.trim().to_ascii_uppercase();
        let Some(number) = key.strip_prefix("CHAPTER") else {
            continue;
        };
        if let Some(number) = number.strip_suffix("NAME") {
            names.push((number.to_string(), value.trim().to_string()));
        } else if let Some(start) = clock(value) {
            chapters.push((
                number.to_string(),
                Chapter {
                    start,
                    title: String::new(),
                },
            ));
        }
    }
    chapters
        .into_iter()
        .map(|(number, mut chapter)| {
            if let Some((_, name)) = names.iter().find(|(other, _)| *other == number) {
                chapter.title = name.clone();
            }
            chapter
        })
        .collect()
}

/// FFmpeg's metadata file: `[CHAPTER]` blocks of TIMEBASE, START and title.
fn ffmetadata(text: &str) -> Vec<Chapter> {
    let mut chapters = Vec::new();
    for block in text.split("[CHAPTER]").skip(1) {
        let block = block.split("\n[").next().unwrap_or(block);
        let value = |name: &str| {
            block.lines().find_map(|line| {
                let (key, value) = line.split_once('=')?;
                key.trim().eq_ignore_ascii_case(name).then(|| value.trim().to_string())
            })
        };
        let (numerator, denominator) = value("TIMEBASE")
            .and_then(|base| {
                let (n, d) = base.split_once('/')?;
                Some((n.trim().parse::<f64>().ok()?, d.trim().parse::<f64>().ok()?))
            })
            .unwrap_or((1.0, 1_000_000_000.0));
        let Some(start) = value("START").and_then(|start| start.parse::<f64>().ok()) else {
            continue;
        };
        chapters.push(Chapter {
            start: start * numerator / denominator,
            title: value("title")
                .map(|title| unescape_ffmetadata(&title))
                .unwrap_or_default(),
        });
    }
    chapters
}

fn unescape_ffmetadata(text: &str) -> String {
    let mut out = String::new();
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            if let Some(next) = chars.next() {
                out.push(next);
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// "00:07:13.933", "00:07:13.933000000", "7:13", "433.9": seconds.
pub fn clock(text: &str) -> Option<f64> {
    let text = text.trim();
    let mut seconds = 0.0;
    for part in text.split(':') {
        let value: f64 = part.trim().parse().ok()?;
        if !value.is_finite() || value < 0.0 {
            return None;
        }
        seconds = seconds * 60.0 + value;
    }
    // To the microsecond: what chapters are timed in, and what compares equal.
    (text.split(':').count() <= 3).then_some((seconds * 1e6).round() / 1e6)
}

/// The FFmpeg metadata file a copy's chapters are written from: each runs
/// until the next starts, the last until the file ends.
pub fn ffmetadata_of(chapters: &[Chapter], duration: Option<f64>) -> String {
    let millis = |seconds: f64| (seconds * 1000.0).round().max(0.0) as u64;
    let mut out = String::from(";FFMETADATA1\n");
    for (place, chapter) in chapters.iter().enumerate() {
        let start = millis(chapter.start);
        let end = chapters
            .get(place + 1)
            .map(|next| millis(next.start))
            .or(duration.map(millis))
            .unwrap_or(start + 1000)
            .max(start + 1);
        out.push_str(&format!(
            "[CHAPTER]\nTIMEBASE=1/1000\nSTART={start}\nEND={end}\ntitle={}\n",
            escape(&chapter.title)
        ));
    }
    out
}

/// FFmpeg's metadata file escapes these with a backslash; a line break ends a value.
fn escape(text: &str) -> String {
    let mut out = String::new();
    for c in text.chars() {
        match c {
            '=' | ';' | '#' | '\\' => {
                out.push('\\');
                out.push(c);
            }
            '\n' | '\r' => out.push(' '),
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn starts(chapters: &[Chapter]) -> Vec<(f64, &str)> {
        chapters.iter().map(|c| (c.start, c.title.as_str())).collect()
    }

    #[test]
    fn reads_mkvtoolnix_xml_of_the_first_edition_without_hidden_chapters() {
        let xml = r#"<?xml version="1.0"?>
<!DOCTYPE Chapters SYSTEM "matroskachapters.dtd">
<Chapters>
  <EditionEntry>
    <ChapterAtom>
      <ChapterTimeStart>00:00:00.000000000</ChapterTimeStart>
      <ChapterDisplay><ChapterString>Αρχή &amp; τίτλοι</ChapterString><ChapterLanguage>gre</ChapterLanguage></ChapterDisplay>
      <ChapterDisplay><ChapterString>Opening</ChapterString></ChapterDisplay>
    </ChapterAtom>
    <ChapterAtom>
      <ChapterTimeStart>00:07:13.933000000</ChapterTimeStart>
      <ChapterFlagHidden>1</ChapterFlagHidden>
      <ChapterDisplay><ChapterString>Hidden</ChapterString></ChapterDisplay>
    </ChapterAtom>
    <ChapterAtom>
      <ChapterTimeStart>00:16:37.329000000</ChapterTimeStart>
      <ChapterDisplay><ChapterString>Part 2</ChapterString></ChapterDisplay>
    </ChapterAtom>
  </EditionEntry>
  <EditionEntry>
    <ChapterAtom><ChapterTimeStart>00:01:00.000</ChapterTimeStart></ChapterAtom>
  </EditionEntry>
</Chapters>"#;
        assert_eq!(
            starts(&parse(xml).unwrap()),
            [(0.0, "Αρχή & τίτλοι"), (997.329, "Part 2")]
        );
    }

    #[test]
    fn reads_the_simple_text_chapters_and_ffmpegs_own() {
        let ogm = "\u{feff}CHAPTER01=00:00:00.000\r\nCHAPTER01NAME=Intro\r\nCHAPTER02=00:03:12.500\r\nCHAPTER02NAME=Part 1\r\n";
        assert_eq!(starts(&parse(ogm).unwrap()), [(0.0, "Intro"), (192.5, "Part 1")]);
        let ff = ";FFMETADATA1\ntitle=Film\n[CHAPTER]\nTIMEBASE=1/1000\nSTART=60000\nEND=120000\ntitle=Two \\= 2\n[CHAPTER]\nTIMEBASE=1/1000\nSTART=0\nEND=60000\ntitle=One\n";
        assert_eq!(starts(&parse(ff).unwrap()), [(0.0, "One"), (60.0, "Two = 2")]);
        assert!(parse("Not chapters at all").is_none());
    }

    #[test]
    fn writes_chapters_for_ffmpeg_each_until_the_next() {
        let chapters = [
            Chapter {
                start: 0.0,
                title: "Αρχή = start; #1".into(),
            },
            Chapter {
                start: 434.225,
                title: "Two\nlines".into(),
            },
        ];
        assert_eq!(
            ffmetadata_of(&chapters, Some(1444.8)),
            ";FFMETADATA1\n[CHAPTER]\nTIMEBASE=1/1000\nSTART=0\nEND=434225\ntitle=Αρχή \\= start\\; \\#1\n[CHAPTER]\nTIMEBASE=1/1000\nSTART=434225\nEND=1444800\ntitle=Two lines\n"
        );
        // What is written reads back the same.
        assert_eq!(
            starts(&parse(&ffmetadata_of(&chapters, None)).unwrap()),
            [(0.0, "Αρχή = start; #1"), (434.225, "Two lines")]
        );
    }

    #[test]
    fn reads_a_clock() {
        assert_eq!(clock("00:07:13.933000000"), Some(433.933));
        assert_eq!(clock("7:13"), Some(433.0));
        assert_eq!(clock("12.5"), Some(12.5));
        assert_eq!(clock("1:2:3:4"), None);
        assert_eq!(clock("-1"), None);
        assert_eq!(clock("soon"), None);
    }
}
