//! Files on this PC that the user gives Siphon to convert: dropped on the
//! window, or picked with Browse.
//!
//! The webview never names a file. The paths reach the backend from the
//! window's own drop event, which carries them, or from the file dialog the
//! backend opens itself. The backend keeps them ([`Grants`]) and gives the
//! webview a key for each (`siphon-file:…`), which it uses where it would
//! use a link. A path typed or pasted into the window is never taken.
//!
//! A file is only ever read ([`Original`]): what Siphon makes from it goes
//! beside it under a name of its own.

pub mod mkv;
mod output;
mod probe;

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use serde::Serialize;
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::Semaphore;

use crate::binaries::BinaryManager;
use crate::error::{AppError, ErrorKind, Result};
use crate::plan::tracks::FileTracks;
use crate::process;
use crate::ytdlp::metadata::VideoInfo;
pub use output::{nothing_to_do, output_name, partial_name, writable};
pub use probe::Probe;

/// What the webview calls a file by, in place of a link: `siphon-file:` and its id.
pub const SCHEME: &str = "siphon-file:";

/// Event carrying an [`Intake`]: files were dropped on the window.
pub const DROPPED: &str = "files://dropped";

/// The most files Siphon takes in at once. Folders of thousands of files
/// would make a list nobody can go through, and a queue that runs for days.
const MAX_FILES: usize = 500;

/// At most this many files are read at once: dropping a few hundred must not
/// start as many ffprobes.
static READS: Semaphore = Semaphore::const_new(3);

/// How long reading a file may take: a slow network share, not a local disk.
const READ_TIMEOUT: Duration = Duration::from_secs(60);

/// Cards' frames older than this are let go at launch.
const STALE_FRAMES: Duration = Duration::from_secs(30 * 24 * 60 * 60);

/// What a folder's files are taken in by: video and audio files, by their
/// extension. A file dropped on its own is taken whatever it is called, and
/// reading it says whether it is one.
#[rustfmt::skip]
pub const MEDIA_EXTENSIONS: &[&str] = &[
    // Video
    "mp4", "m4v", "mov", "mkv", "webm", "avi", "wmv", "asf", "flv", "f4v", "ts", "mts", "m2ts",
    "mpg", "mpeg", "m2v", "vob", "3gp", "3g2", "ogv", "mxf", "dv", "divx", "rm", "rmvb",
    // Audio
    "mp3", "m4a", "m4b", "aac", "opus", "ogg", "oga", "flac", "wav", "wma", "aif", "aiff",
    "alac", "ape", "wv", "mka", "ac3", "eac3", "dts", "amr", "caf", "mp2",
];

/// A file the user gave Siphon to convert. It is read where it is and never
/// written to, moved, renamed or deleted: what is made from it goes beside it
/// under a name of its own, and the steps that move, rename or delete a file
/// take one Siphon made (`downloads::Made`), which this never becomes.
#[derive(Debug)]
pub struct Original {
    path: PathBuf,
    /// Stable for the same file (see [`file_id`]): the same file dropped twice is one.
    id: String,
}

impl Original {
    fn new(path: PathBuf) -> Self {
        let id = file_id(&path);
        Self { path, id }
    }

    /// Where it is, to read it.
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    /// What the webview calls it by.
    pub fn key(&self) -> String {
        format!("{SCHEME}{}", self.id)
    }

    /// "Clip.mp4"
    pub fn name(&self) -> String {
        self.path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    /// "Clip", for "Clip.mp4": what its video is called, and what is made from it.
    pub fn stem(&self) -> String {
        self.path
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .filter(|stem| !stem.is_empty())
            .unwrap_or_else(|| self.name())
    }

    /// The folder it is in: where what is made from it goes.
    pub fn folder(&self) -> Option<&Path> {
        self.path.parent()
    }

    /// What the webview is told of it, as it is now (`stamp`).
    fn describe(&self, stamp: Stamp) -> LocalFile {
        LocalFile {
            key: self.key(),
            name: self.name(),
            folder: self
                .folder()
                .map(|folder| folder.to_string_lossy().into_owned())
                .unwrap_or_default(),
            size: stamp.size,
            version: stamp.version(),
            matroska: false,
            editable: false,
            overview: TrackOverview::default(),
            start: 0.0,
            cover: false,
            tracks: FileTracks::default(),
        }
    }
}

/// The same id for the same file however its path was spelled (Windows
/// paths don't tell capitals apart), and for it again after a restart: the
/// first 16 hex digits of the SHA-256 of its path.
fn file_id(path: &Path) -> String {
    let spelled = path.to_string_lossy().to_lowercase();
    hex::encode(&Sha256::digest(spelled.as_bytes())[..8])
}

/// A file as the webview knows it: its key, and what to show of it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LocalFile {
    /// What it is called by, in place of a link: `siphon-file:…`.
    pub key: String,
    /// "Clip.mp4"
    pub name: String,
    /// The folder it is in, where what is made from it goes.
    pub folder: String,
    /// Bytes.
    pub size: u64,
    /// Its size and when it last changed, in a word: a file that changed
    /// since it was read is read and planned again, by the window too, which
    /// keeps what it was told by key and version.
    pub version: String,
    /// It is a Matroska file inside, whatever its name says: its tracks can
    /// be chosen (docs/mkv-editor.md). Known once it is read.
    pub matroska: bool,
    /// Its tracks can be chosen: an MKV, an MP4 or MOV, an MPEG-TS (docs/mkv-editor.md).
    pub editable: bool,
    /// How many tracks of each kind it holds, and in which languages: its
    /// tracks are offered to be chosen when there is more than one picture
    /// and one sound.
    pub overview: TrackOverview,
    /// Where its timestamps start, in seconds: an MPEG-TS from OBS starts a
    /// second or so in. A part to cut out is chosen from its start, and cut
    /// by its timestamps.
    #[serde(skip)]
    pub start: f64,
    /// It holds a cover picture (an audio file's art), kept where what is made can hold one.
    #[serde(skip)]
    pub cover: bool,
    /// Its sound and subtitle tracks, and which sound is its main one (see `plan::tracks`).
    #[serde(skip)]
    pub tracks: FileTracks,
}

/// What a file holds, track by track, as its tags say: how many video
/// (covers aside), sound and subtitle tracks, and their languages. Enough to
/// say what is in it before its tracks are read for what they are.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct TrackOverview {
    pub video: usize,
    pub sound: usize,
    pub subtitles: usize,
    /// The sound tracks' languages as tagged, each once, in the file's order ("el").
    pub sound_languages: Vec<String>,
    /// The subtitle tracks' languages, the same way.
    pub subtitle_languages: Vec<String>,
}

/// What came of files dropped on the window or picked with Browse.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Intake {
    pub files: Vec<LocalFile>,
    /// Files in dropped folders that aren't video or audio, left out.
    pub left_out: usize,
    /// Files past the most Siphon takes at once ([`MAX_FILES`]), left out.
    pub over: usize,
}

/// The files the user gave Siphon this session, by id, and what reading
/// each found. Only the backend adds to it, from a drop or the file dialog.
#[derive(Default)]
pub struct Grants(Mutex<HashMap<String, Granted>>);

struct Granted {
    original: Arc<Original>,
    read: Option<(Arc<VideoInfo>, Stamp)>,
}

/// A file's size and when it last changed: if either moves, it is read again.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Stamp {
    size: u64,
    modified: Option<SystemTime>,
}

impl Stamp {
    /// "6a3f2e-19cb5d8a2c1": its size and when it last changed (milliseconds), in hex.
    fn version(&self) -> String {
        let changed = self
            .modified
            .and_then(|time| time.duration_since(SystemTime::UNIX_EPOCH).ok())
            .map_or(0, |since| since.as_millis());
        format!("{:x}-{changed:x}", self.size)
    }

    fn of(path: &Path) -> Option<Stamp> {
        let meta = std::fs::metadata(path).ok().filter(std::fs::Metadata::is_file)?;
        Some(Stamp {
            size: meta.len(),
            modified: meta.modified().ok(),
        })
    }
}

impl Grants {
    /// Keeps `path`, a file the user gave, and returns it. None if it isn't
    /// a file. Only a drop, the file dialog, or a conversion the user started
    /// on the file before Siphon was restarted, add one.
    pub(crate) fn grant(&self, path: &Path) -> Option<Arc<Original>> {
        let path = std::path::absolute(path).ok()?;
        if !path.is_file() {
            return None;
        }
        let original = Original::new(path);
        let mut grants = self.0.lock().unwrap();
        let granted = grants.entry(original.id.clone()).or_insert_with(|| Granted {
            original: Arc::new(original),
            read: None,
        });
        Some(Arc::clone(&granted.original))
    }

    /// The file a key stands for, if the user gave it.
    pub fn get(&self, key: &str) -> Option<Arc<Original>> {
        let id = key.strip_prefix(SCHEME)?;
        self.0
            .lock()
            .unwrap()
            .get(id)
            .map(|granted| Arc::clone(&granted.original))
    }

    /// What reading the file last found, if it hasn't changed since.
    fn read_before(&self, original: &Original, stamp: Stamp) -> Option<Arc<VideoInfo>> {
        let grants = self.0.lock().unwrap();
        let (info, then) = grants.get(&original.id)?.read.as_ref()?;
        (*then == stamp).then(|| Arc::clone(info))
    }

    fn keep(&self, original: &Original, info: Arc<VideoInfo>, stamp: Stamp) {
        if let Some(granted) = self.0.lock().unwrap().get_mut(&original.id) {
            granted.read = Some((info, stamp));
        }
    }
}

/// Whether a link the webview sent is a file's key instead.
pub fn is_key(url: &str) -> bool {
    url.starts_with(SCHEME)
}

/// Takes in what the user gave: files as they are, and the video and audio
/// files directly inside folders (not in their subfolders), in the order
/// Explorer lists them. Each file once, and at most [`MAX_FILES`].
pub fn take_in(grants: &Grants, paths: Vec<PathBuf>) -> Intake {
    let mut intake = Intake::default();
    let mut seen = HashSet::new();
    let mut add = |path: &Path, intake: &mut Intake| {
        let Some(original) = grants.grant(path) else { return };
        if !seen.insert(original.id.clone()) {
            return;
        }
        if intake.files.len() >= MAX_FILES {
            intake.over += 1;
            return;
        }
        let Some(stamp) = Stamp::of(original.path()) else {
            return;
        };
        intake.files.push(original.describe(stamp));
    };
    for path in paths {
        if path.is_dir() {
            let Ok(entries) = std::fs::read_dir(&path) else {
                continue;
            };
            let mut inside: Vec<PathBuf> = entries
                .flatten()
                .map(|entry| entry.path())
                .filter(|path| path.is_file())
                .collect();
            inside.sort_by(|a, b| natural_order(&a.to_string_lossy(), &b.to_string_lossy()));
            for file in inside {
                if is_media(&file) {
                    add(&file, &mut intake);
                } else {
                    intake.left_out += 1;
                }
            }
        } else {
            add(&path, &mut intake);
        }
    }
    intake
}

/// Whether a file's extension is one of a video or audio file.
fn is_media(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| MEDIA_EXTENSIONS.contains(&ext.to_ascii_lowercase().as_str()))
}

/// Explorer's order: numbers by their value ("Clip 2" before "Clip 10"),
/// letters without regard to case.
fn natural_order(a: &str, b: &str) -> Ordering {
    let (mut a, mut b) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (a.peek().copied(), b.peek().copied()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let number = |chars: &mut std::iter::Peekable<std::str::Chars>| {
                    let mut digits = String::new();
                    while let Some(c) = chars.peek().copied().filter(char::is_ascii_digit) {
                        digits.push(c);
                        chars.next();
                    }
                    digits
                };
                let (x, y) = (number(&mut a), number(&mut b));
                let (x_trimmed, y_trimmed) = (x.trim_start_matches('0'), y.trim_start_matches('0'));
                let order = x_trimmed
                    .len()
                    .cmp(&y_trimmed.len())
                    .then_with(|| x_trimmed.cmp(y_trimmed))
                    .then_with(|| x.len().cmp(&y.len()));
                if order != Ordering::Equal {
                    return order;
                }
            }
            (Some(x), Some(y)) => {
                let order = x.to_lowercase().cmp(y.to_lowercase());
                if order != Ordering::Equal {
                    return order;
                }
                a.next();
                b.next();
            }
        }
    }
}

/// Files were dropped on the window: takes them in, and tells the webview
/// which (see [`DROPPED`]). Off the window's thread: a folder is listed.
pub fn dropped(app: &AppHandle, paths: Vec<PathBuf>) {
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let intake = take_in(&app.state::<Grants>(), paths);
        let _ = app.emit(DROPPED, intake);
    });
}

/// Reads what a file the user gave holds, for planning and converting it,
/// with a frame of it for its card. What was read is kept until the file changes.
pub async fn read(app: &AppHandle, bins: &BinaryManager, key: &str) -> Result<Arc<VideoInfo>> {
    let grants = app.state::<Grants>();
    let original = grants.get(key).ok_or_else(lost_track)?;
    let stamp = Stamp::of(original.path()).ok_or_else(gone)?;
    if let Some(info) = grants.read_before(&original, stamp) {
        return Ok(info);
    }
    let _permit = READS
        .acquire()
        .await
        .map_err(|err| AppError::internal("Couldn't read the file.").with_details(err.to_string()))?;
    let probe = run_probe(bins, original.path()).await?;
    let mut info = probe.video_info(&original, stamp)?;
    info.thumbnail = frame(app, bins, &original, &probe).await;
    let info = Arc::new(info);
    grants.keep(&original, Arc::clone(&info), stamp);
    Ok(info)
}

pub(crate) fn lost_track() -> AppError {
    AppError::internal("Siphon lost track of this file. Drop it on the window again.")
}

pub(crate) fn gone() -> AppError {
    AppError::new(
        ErrorKind::Unavailable,
        "This file isn't there any more. It may have been moved or deleted.",
    )
}

/// What ffprobe says the file holds.
async fn run_probe(bins: &BinaryManager, file: &Path) -> Result<Probe> {
    let mut cmd = process::command(bins.ffprobe());
    cmd.args(probe::probe_args(file));
    let _in_use = bins.in_use().await;
    let output = process::output_within(cmd, READ_TIMEOUT)
        .await?
        .ok_or_else(|| AppError::new(ErrorKind::Unavailable, "Reading this file took too long."))?;
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let probe = output
        .status
        .success()
        .then(|| Probe::parse(&String::from_utf8_lossy(&output.stdout)))
        .flatten();
    probe.ok_or_else(|| probe::not_media().with_details(stderr))
}

/// A frame of the file for its card, at a tenth of the way in (where a video
/// has usually got going), or an audio file's cover: a small JPEG in the
/// cache, as a URL the webview can show. None if it has neither.
async fn frame(app: &AppHandle, bins: &BinaryManager, original: &Original, probe: &Probe) -> Option<String> {
    let dir = frames_dir(app)?;
    tokio::fs::create_dir_all(&dir).await.ok()?;
    let out = dir.join(format!("{}.jpg", original.id));
    let changed = std::fs::metadata(original.path()).and_then(|meta| meta.modified()).ok();
    let made = std::fs::metadata(&out).and_then(|meta| meta.modified()).ok();
    if made.is_none() || made < changed {
        let args = probe::frame_args(original.path(), probe, &out)?;
        let mut cmd = process::command(bins.ffmpeg());
        cmd.args(args);
        let _in_use = bins.in_use().await;
        let made = process::output_within(cmd, READ_TIMEOUT).await.ok().flatten()?;
        if !made.status.success() {
            return None;
        }
    }
    out.is_file().then(|| asset_url(&out))
}

fn frames_dir(app: &AppHandle) -> Option<PathBuf> {
    app.path().app_cache_dir().ok().map(|dir| dir.join("frames"))
}

/// Lets go of cards' frames no file has asked for in a month. Run at launch.
pub fn sweep_frames(app: &AppHandle) {
    let Some(Ok(entries)) = frames_dir(app).map(std::fs::read_dir) else {
        return;
    };
    for entry in entries.flatten() {
        let stale = entry
            .metadata()
            .and_then(|meta| meta.modified())
            .ok()
            .and_then(|modified| modified.elapsed().ok())
            .is_some_and(|age| age > STALE_FRAMES);
        if stale {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// The URL the webview loads a file in the asset protocol's scope from, as
/// `convertFileSrc` makes it on Windows: the path, encoded as by
/// `encodeURIComponent`.
fn asset_url(path: &Path) -> String {
    let mut url = String::from("http://asset.localhost/");
    for byte in path.to_string_lossy().bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'!' | b'~' | b'*' | b'\'' | b'(' | b')' => {
                url.push(char::from(byte));
            }
            _ => url.push_str(&format!("%{byte:02X}")),
        }
    }
    url
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("siphon-local-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_file_is_known_by_a_key_and_only_if_it_was_given() {
        let dir = scratch("grants");
        let clip = dir.join("Clip.mp4");
        std::fs::write(&clip, b"not really a video").unwrap();
        let grants = Grants::default();

        let original = grants.grant(&clip).unwrap();
        assert_eq!(original.name(), "Clip.mp4");
        assert_eq!(original.stem(), "Clip");
        assert_eq!(original.folder(), Some(dir.as_path()));
        assert!(is_key(&original.key()));
        assert_eq!(grants.get(&original.key()).unwrap().path(), clip);

        // The same file, spelled another way, is the same file.
        let shouted = PathBuf::from(clip.to_string_lossy().to_uppercase());
        assert_eq!(grants.grant(&shouted).unwrap().key(), original.key());

        // A key nobody was given, a link, a folder and a file that isn't there are nothing.
        assert!(grants.get(&format!("{SCHEME}0123456789abcdef")).is_none());
        assert!(grants.get("https://example.com/video").is_none());
        assert!(grants.grant(&dir).is_none());
        assert!(grants.grant(&dir.join("Missing.mp4")).is_none());

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_folder_gives_its_own_video_and_audio_files_in_explorers_order() {
        let dir = scratch("intake");
        for name in [
            "Clip 10.mp4",
            "clip 2.MKV",
            "Clip 1.mp4",
            "song.mp3",
            "notes.txt",
            "cover.jpg",
        ] {
            std::fs::write(dir.join(name), b"x").unwrap();
        }
        std::fs::create_dir_all(dir.join("Sub")).unwrap();
        std::fs::write(dir.join("Sub").join("deeper.mp4"), b"x").unwrap();
        let loose = scratch("intake-loose").join("Loose.dat");
        std::fs::write(&loose, b"x").unwrap();

        let grants = Grants::default();
        let intake = take_in(&grants, vec![dir.clone(), loose.clone(), dir.join("Clip 1.mp4")]);
        let names: Vec<&str> = intake.files.iter().map(|file| file.name.as_str()).collect();
        // Numbers by value, capitals ignored; a file dropped on its own is
        // taken whatever it is called, and a file given twice is one.
        assert_eq!(
            names,
            ["Clip 1.mp4", "clip 2.MKV", "Clip 10.mp4", "song.mp3", "Loose.dat"]
        );
        assert_eq!(intake.left_out, 2, "notes.txt and cover.jpg");
        assert_eq!(intake.over, 0);
        assert!(intake.files.iter().all(|file| grants.get(&file.key).is_some()));
        assert_eq!(intake.files[0].folder, dir.to_string_lossy());
        assert_eq!(intake.files[0].size, 1);

        std::fs::remove_dir_all(&dir).unwrap();
        std::fs::remove_dir_all(loose.parent().unwrap()).unwrap();
    }

    #[test]
    fn a_huge_folder_is_taken_in_up_to_the_limit() {
        let dir = scratch("huge");
        for index in 0..(MAX_FILES + 7) {
            std::fs::write(dir.join(format!("{index}.mp3")), b"x").unwrap();
        }
        let intake = take_in(&Grants::default(), vec![dir.clone()]);
        assert_eq!(intake.files.len(), MAX_FILES);
        assert_eq!(intake.over, 7);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn explorers_order_counts_numbers_by_value() {
        let mut names = vec!["b", "Clip 10", "clip 9", "Clip 009", "a", "Clip 1"];
        names.sort_by(|a, b| natural_order(a, b));
        assert_eq!(names, ["a", "b", "Clip 1", "clip 9", "Clip 009", "Clip 10"]);
    }

    #[test]
    fn a_frame_is_shown_through_the_asset_protocol() {
        assert_eq!(
            asset_url(Path::new(r"C:\Users\Ana Lucía\cache\frames\0a1b.jpg")),
            "http://asset.localhost/C%3A%5CUsers%5CAna%20Luc%C3%ADa%5Ccache%5Cframes%5C0a1b.jpg"
        );
    }
}
