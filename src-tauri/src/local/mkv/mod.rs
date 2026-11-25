//! The MKV editor's reading of a file (docs/mkv-editor.md): every track, with
//! its language and how certain that is, its flags, the chapters and what a
//! copy would flatten of them, and the files attached. Nothing here writes:
//! the file is an [`Original`], read where it is.
//!
//! A track's language is only taken as known when nothing casts doubt on it
//! (see `doubts`): what is uncertain is kept, never guessed at.

pub mod added;
mod chapters;
mod doubts;
pub mod edit;
pub mod pick;
mod pictures;
mod probe;
mod words;

use std::path::Path;

use futures_util::StreamExt;
use serde::Serialize;
use tauri::{AppHandle, Manager};

use super::{gone, lost_track, Grants, Original, READS, READ_TIMEOUT};
use crate::binaries::BinaryManager;
use crate::choices::ChoiceStore;
use crate::error::{AppError, ErrorKind, Result};
use crate::process;
use crate::settings::SettingsStore;
pub use chapters::Shape;
pub(crate) use probe::language;

/// What an MKV holds.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Contents {
    pub tracks: Vec<Track>,
    pub chapters: Chapters,
    /// Files carried in it: the fonts its ASS subtitles are drawn with, a cover.
    pub attachments: Vec<Attachment>,
    /// What casts doubt on the file as a whole: "Its name says Greek (EN-GR), but no track is Greek."
    pub doubts: Vec<String>,
    /// What it keeps when the user's languages choose (see `pick`): applied
    /// as soon as it opens.
    pub suggestion: Option<pick::Suggestion>,
    /// Seconds.
    #[serde(skip)]
    pub duration: Option<f64>,
    /// Bytes.
    #[serde(skip)]
    pub size: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Video,
    Sound,
    Subtitles,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Track {
    /// Its place among the file's streams, as FFmpeg counts them (`0:N`).
    pub index: usize,
    pub kind: Kind,
    /// ffprobe's name for its codec: "hevc", "truehd", "subrip", "hdmv_pgs_subtitle".
    pub codec: String,
    /// The name the file gives it: "Surround", "Stereo-Commentary".
    pub name: Option<String>,
    /// Its language as a tag ("el", "en"); None when the file doesn't say
    /// (no tag, `und`, `mul`, `zxx`).
    pub language: Option<String>,
    /// Marked to play first.
    pub default: bool,
    /// Subtitles shown even with the sound in their language: signs, foreign lines.
    pub forced: bool,
    /// Subtitles for the deaf and hard of hearing (SDH).
    pub sdh: bool,
    pub commentary: bool,
    /// Sound describing the picture, for the blind.
    pub described: bool,
    /// The film's own sound, not a dub.
    pub original: bool,
    /// Sound: how many channels (2, 6, 8).
    pub channels: Option<u32>,
    /// Video: its size.
    pub width: Option<u32>,
    pub height: Option<u32>,
    /// How many frames, sound packets or subtitle lines it holds, where the
    /// file says (MKVToolNix writes it): a forced subtitle track has far
    /// fewer lines than a full one.
    pub entries: Option<u64>,
    /// How many bytes it takes, where the file says (MKVToolNix writes it).
    pub bytes: Option<u64>,
    /// What casts doubt on its language: "Its name says Greek, but it is tagged English."
    pub doubts: Vec<String>,
    /// What reading it found, besides: "Tagged English, but its words are Greek: the copy says Greek."
    pub notes: Vec<String>,
    /// Its language was found, or its tag borne out, by reading its words.
    pub read: bool,
    /// Sound or subtitles whose language is known, with nothing casting
    /// doubt on it: only then may it be left out for its language. Video
    /// always is: no language decides it (see [`settle`]).
    pub certain: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Chapters {
    /// The chapters ffprobe lists: the first edition's, one level deep.
    pub count: usize,
    /// What they are beyond that, read from the file itself.
    pub shape: Option<Shape>,
    /// Said before a copy flattens chapters that are more than a plain list.
    pub note: Option<String>,
}

impl Chapters {
    fn note(&self) -> Option<String> {
        let shape = self.shape.as_ref().filter(|shape| !shape.plain())?;
        let mut has = Vec::new();
        if shape.editions > 1 {
            has.push(format!("{} versions of them", shape.editions));
        }
        if shape.nested {
            has.push("chapters within chapters".to_string());
        }
        if shape.hidden {
            has.push("hidden chapters".to_string());
        }
        if shape.names {
            has.push("names in several languages".to_string());
        }
        if shape.ordered {
            has.push("an edition that plays its parts in another order".to_string());
        }
        Some(format!(
            "Its chapters have {}. The copy keeps them as one plain list.",
            join(&has)
        ))
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Attachment {
    /// Its place among the file's streams (`0:N`), as FFmpeg counts them.
    pub index: usize,
    /// "Arial.ttf"
    pub name: String,
    /// "font/ttf", "image/jpeg"
    pub mime: Option<String>,
}

/// Reads what the MKV the user gave holds, track by track, its subtitles
/// read for their language. An error for a file that isn't an MKV.
pub async fn read(app: &AppHandle, bins: &BinaryManager, key: &str) -> Result<Contents> {
    let original = app.state::<Grants>().get(key).ok_or_else(lost_track)?;
    if !original.path().is_file() {
        return Err(gone());
    }
    let _permit = READS
        .acquire()
        .await
        .map_err(|err| AppError::internal("Couldn't read the file.").with_details(err.to_string()))?;
    let mut contents = inspect(bins, &original).await?;
    contents.chapters.shape = chapter_shape(original.path()).await;
    let windows = words::windows(contents.duration, contents.size);
    let mut lines = read_words(bins, original.path(), &mut contents.tracks, windows.as_deref()).await;
    lines.extend(read_pictures(bins, original.path(), &mut contents.tracks, windows.as_deref()).await);
    mark_forced(&mut contents.tracks, &lines);
    doubts::cast(&mut contents, &original.name());
    settle(&mut contents);
    let subtitles = app.state::<ChoiceStore>().get().subtitles;
    let languages = app.state::<SettingsStore>().get().my_languages(subtitles.as_deref());
    contents.suggestion = Some(pick::suggest(&contents, &languages));
    Ok(contents)
}

/// What an MKV holds as ffprobe lists it, without reading its subtitles:
/// what a copy is made from, and checked against.
pub async fn inspect(bins: &BinaryManager, original: &Original) -> Result<Contents> {
    inspect_path(bins, original.path())
        .await
        .map_err(|details| not_mkv(original).with_details(details))
}

/// As [`inspect`], for a file Siphon made: the copy, checked.
pub async fn inspect_path(bins: &BinaryManager, path: &Path) -> std::result::Result<Contents, String> {
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
    probe::parse(&String::from_utf8_lossy(&output.stdout))
        .filter(|_| output.status.success())
        .ok_or_else(|| String::from_utf8_lossy(&output.stderr).trim().to_string())
}

/// Says, once all is read, which tracks' languages are certain, and what a
/// copy would flatten of the chapters.
fn settle(contents: &mut Contents) {
    for track in &mut contents.tracks {
        track.certain = track.kind == Kind::Video || (track.language.is_some() && track.doubts.is_empty());
    }
    contents.chapters.note = contents.chapters.note();
}

/// Reads each text subtitle track's words, for its language and SDH (see
/// `words`), in `windows` of the file or all of it. Says how many lines each
/// had, for telling forced subtitles. A track that can't be read is left as
/// tagged.
async fn read_words(
    bins: &BinaryManager,
    path: &Path,
    tracks: &mut [Track],
    windows: Option<&[(f64, f64)]>,
) -> Vec<(usize, usize)> {
    let mut lines = Vec::new();
    for track in tracks {
        if track.kind != Kind::Subtitles || !words::TEXT.contains(&track.codec.as_str()) {
            continue;
        }
        let Some(output) = probe_output(bins, words::args(path, track.index, windows)).await else {
            continue;
        };
        let reading = words::Reading::of(&words::lines(&output, &track.codec));
        words::apply(track, &reading);
        lines.push((track.index, reading.lines));
    }
    lines
}

/// Reads each picture subtitle track with Windows' OCR (see `pictures`), in
/// the same stretches as [`read_words`]. Says how many pictures showing a
/// line each had. Without OCR, or for a track that can't be drawn, tags stand.
async fn read_pictures(
    bins: &BinaryManager,
    path: &Path,
    tracks: &mut [Track],
    windows: Option<&[(f64, f64)]>,
) -> Vec<(usize, usize)> {
    let mut shown = Vec::new();
    for track in tracks {
        if track.kind != Kind::Subtitles || !pictures::PICTURES.contains(&track.codec.as_str()) {
            continue;
        }
        let Some(output) = probe_output(bins, pictures::packet_args(path, track.index, windows)).await else {
            continue;
        };
        let (moments, count) = pictures::moments(&output);
        shown.push((track.index, count));
        let (width, height) = (track.width.unwrap_or(1920), track.height.unwrap_or(1080));
        let index = track.index;
        let drawn: Vec<Vec<u8>> = futures_util::stream::iter(moments)
            .map(|at| async move {
                let mut cmd = process::command(bins.ffmpeg());
                cmd.args(pictures::draw_args(path, index, at, width, height));
                let _in_use = bins.in_use().await;
                let output = process::output_within(cmd, READ_TIMEOUT).await.ok().flatten()?;
                (output.status.success() && output.stdout.len() == (width * height) as usize).then_some(output.stdout)
            })
            .buffered(4)
            .filter_map(|picture| async move { picture })
            .collect()
            .await;
        if drawn.is_empty() {
            continue;
        }
        let read = tokio::task::spawn_blocking(move || {
            let readers = pictures::Readers::new()?;
            let mut lines = vec![Vec::new(); readers.len()];
            for picture in &drawn {
                for (reader, found) in readers.read(picture, width, height).into_iter().enumerate() {
                    lines[reader].extend(found);
                }
            }
            Some(lines)
        })
        .await;
        if let Ok(Some(lines)) = read {
            let readings: Vec<words::Reading> = lines.iter().map(|lines| words::Reading::of(lines)).collect();
            pictures::apply(track, &readings);
        }
    }
    shown
}

/// What ffprobe prints when run with `args`; None if it fails or takes too long.
async fn probe_output(bins: &BinaryManager, args: Vec<std::ffi::OsString>) -> Option<String> {
    let mut cmd = process::command(bins.ffprobe());
    cmd.args(args);
    let _in_use = bins.in_use().await;
    let output = process::output_within(cmd, READ_TIMEOUT).await.ok().flatten()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Marks as forced a subtitle track with a quarter of the lines, or fewer,
/// of the fullest in its language: forced subtitles carry only signs and
/// foreign lines. `lines` is what each track read had (lines, or pictures
/// that show one), over the same stretches of the file.
fn mark_forced(tracks: &mut [Track], lines: &[(usize, usize)]) {
    const FULL: usize = 40;
    let language = |tracks: &[Track], index: usize| {
        tracks
            .iter()
            .find(|t| t.index == index)
            .and_then(|t| t.language.clone())
    };
    for &(index, count) in lines {
        let Some(own) = language(tracks, index) else { continue };
        let fullest = lines
            .iter()
            .filter(|(other, _)| *other != index && language(tracks, *other).as_deref() == Some(own.as_str()))
            .map(|(_, count)| *count)
            .max()
            .unwrap_or(0);
        if fullest < FULL || count * 4 > fullest {
            continue;
        }
        let Some(track) = tracks.iter_mut().find(|t| t.index == index) else {
            continue;
        };
        if !track.forced {
            track.forced = true;
            track.notes.push(format!(
                "It has {count} lines where the other {} subtitles have {fullest}: forced subtitles, for signs and foreign lines.",
                crate::ytdlp::subtitles::language_name(&own).unwrap_or(&own)
            ));
        }
    }
}

async fn chapter_shape(path: &Path) -> Option<Shape> {
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || {
        let mut file = std::io::BufReader::new(std::fs::File::open(path).ok()?);
        chapters::shape(&mut file)
    })
    .await
    .ok()
    .flatten()
}

fn not_mkv(original: &Original) -> AppError {
    AppError::new(
        ErrorKind::Unsupported,
        format!("Only an MKV's tracks can be chosen, and {} isn't one.", original.name()),
    )
}

/// "a", "a and b", "a, b and c"
fn join(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn says_what_a_copy_flattens_of_the_chapters() {
        let mut chapters = Chapters {
            count: 4,
            shape: Some(Shape {
                editions: 1,
                ..Shape::default()
            }),
            note: None,
        };
        assert_eq!(chapters.note(), None);
        chapters.shape = Some(Shape {
            editions: 2,
            nested: false,
            hidden: true,
            names: true,
            ordered: false,
        });
        assert_eq!(
            chapters.note().as_deref(),
            Some("Its chapters have 2 versions of them, hidden chapters and names in several languages. The copy keeps them as one plain list.")
        );
        // Chapters it couldn't read the shape of are said nothing about.
        chapters.shape = None;
        assert_eq!(chapters.note(), None);
    }
}
