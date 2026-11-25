//! The MKV editor's reading of a file (docs/mkv-editor.md): every track, with
//! its language and how certain that is, its flags, the chapters and what a
//! copy would flatten of them, and the files attached. Nothing here writes:
//! the file is an [`Original`], read where it is.
//!
//! A track's language is only taken as known when nothing casts doubt on it
//! (see `doubts`): what is uncertain is kept, never guessed at.

mod chapters;
mod doubts;
mod probe;

use std::path::Path;

use serde::Serialize;
use tauri::{AppHandle, Manager};

use super::{gone, lost_track, Grants, Original, READS, READ_TIMEOUT};
use crate::binaries::BinaryManager;
use crate::error::{AppError, ErrorKind, Result};
use crate::process;
pub use chapters::Shape;

/// What an MKV holds.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Contents {
    pub tracks: Vec<Track>,
    pub chapters: Chapters,
    /// Files carried in it: the fonts its ASS subtitles are drawn with, a cover.
    pub attachments: Vec<Attachment>,
    /// What casts doubt on the file as a whole: "Its name says Greek (EN-GR), but no track is Greek."
    pub doubts: Vec<String>,
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
    /// What casts doubt on its language: "Its name says Greek, but it is tagged English."
    pub doubts: Vec<String>,
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
    /// "Arial.ttf"
    pub name: String,
    /// "font/ttf", "image/jpeg"
    pub mime: Option<String>,
}

/// Reads what the MKV the user gave holds, track by track. An error for a
/// file that isn't an MKV.
pub async fn read(app: &AppHandle, bins: &BinaryManager, key: &str) -> Result<Contents> {
    let original = app.state::<Grants>().get(key).ok_or_else(lost_track)?;
    if !original.path().is_file() {
        return Err(gone());
    }
    let _permit = READS
        .acquire()
        .await
        .map_err(|err| AppError::internal("Couldn't read the file.").with_details(err.to_string()))?;
    let mut cmd = process::command(bins.ffprobe());
    cmd.args(probe::args(original.path()));
    let output = {
        let _in_use = bins.in_use().await;
        process::output_within(cmd, READ_TIMEOUT)
            .await?
            .ok_or_else(|| AppError::new(ErrorKind::Unavailable, "Reading this file took too long."))?
    };
    let json = String::from_utf8_lossy(&output.stdout);
    let mut contents = probe::parse(&json)
        .filter(|_| output.status.success())
        .ok_or_else(|| not_mkv(&original).with_details(String::from_utf8_lossy(&output.stderr).trim().to_string()))?;
    contents.chapters.shape = chapter_shape(original.path()).await;
    doubts::cast(&mut contents, &original.name());
    settle(&mut contents);
    Ok(contents)
}

/// Says, once all is read, which tracks' languages are certain, and what a
/// copy would flatten of the chapters.
fn settle(contents: &mut Contents) {
    for track in &mut contents.tracks {
        track.certain = track.kind == Kind::Video || (track.language.is_some() && track.doubts.is_empty());
    }
    contents.chapters.note = contents.chapters.note();
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
