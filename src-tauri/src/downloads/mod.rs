//! Runs downloads and streams their progress to the frontend.
//!
//! A job waits in the queue for a free slot, then has up to three steps:
//! look the video up (unless it is cached), let yt-dlp fetch and merge the
//! streams the recipe names, then, only if the recipe converts anything,
//! run Siphon's own ffmpeg pass.

mod chat;
mod files;
mod live;
mod live_chat;
mod live_draw;
mod tracker;
mod twitch_feed;

pub use chat::image_folder as chat_image_folder;
pub use live::Recovered;

use std::collections::{HashMap, HashSet, VecDeque};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};
use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};
use tokio::process::Command;
use tokio::sync::Notify;

use crate::binaries::{BinaryManager, Tool};
use crate::error::{AppError, Chance, ErrorKind, Result, Retry};
use crate::ffmpeg::smart_cut::{self, Fallback};
use crate::ffmpeg::{self, encoders::Hardware, Pass};
use crate::local::{self, Grants, Original};
use crate::log;
use crate::plan::codecs::{Container, VideoCodec};
use crate::plan::subtitles::SubtitleStep;
use crate::plan::{self, AudioAction, Choice, ReadFrom, Recipe, Trim};
use crate::process;
use crate::sessions::{Account, CookieFile, Sessions};
use crate::settings::{Settings, SettingsStore};
use crate::srt;
use crate::state::{CapabilityCache, MediaCache};
use crate::throttle::{self, Throttle};
use crate::ytdlp::metadata::{Media, VideoInfo};
use crate::ytdlp::subtitles::language_name;
use crate::ytdlp::{self, args, errors, progress};
use tracker::{Progress, Stage, Tracker};

/// Event carrying a [`DownloadEvent`].
pub const EVENT: &str = "download://event";

/// Keep the tail of the log for the "details" toggle without growing forever.
const MAX_LOG_LINES: usize = 400;

/// How long to wait before each further try at a download that failed in a
/// way that may pass. A dropped or refused connection gets two more tries,
/// which also covers Wi-Fi reconnecting; a site that is limiting requests
/// gets one, after a longer wait. Every try carries on from the files the
/// last one left, so trying again costs little.
const RETRY_SOON: &[Duration] = &[Duration::from_secs(2), Duration::from_secs(10)];
const RETRY_LATER: &[Duration] = &[Duration::from_secs(30)];

/// How many further tries a download has had on each schedule. Each counts
/// apart, so a refused request followed by a site that limits requests
/// still gets the longer wait.
#[derive(Debug, Default)]
struct Retries {
    soon: usize,
    later: usize,
}

impl Retries {
    /// How long to wait before trying again after an error that may pass,
    /// or None once its schedule has had all its tries.
    fn next(&mut self, retry: Retry) -> Option<Duration> {
        let (schedule, used) = match retry {
            Retry::No => return None,
            Retry::Soon => (RETRY_SOON, &mut self.soon),
            Retry::Later => (RETRY_LATER, &mut self.later),
        };
        let delay = schedule.get(*used).copied()?;
        *used += 1;
        Some(delay)
    }
}

/// How long closing the app waits for running downloads to remove their files.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(3);

/// Share: how many more times a file that came out over the limit is encoded again.
const MAX_SHRINKS: usize = 2;

/// Work folders older than this are left over from a crash and are removed at launch.
/// (Newer ones may belong to another copy of Siphon that is running.)
const STALE_WORK: Duration = Duration::from_secs(24 * 60 * 60);

#[derive(Debug, Deserialize)]
pub struct DownloadRequest {
    /// Chosen by the frontend so it can match events to its own records.
    pub id: String,
    pub url: String,
    pub choice: Choice,
    /// Download only this part of the video.
    #[serde(default)]
    pub trim: Option<Trim>,
    /// Where a video from a playlist goes: its own folder, a number in front of its name.
    #[serde(default)]
    pub place: Option<Place>,
    /// Use the site's sign-in from the start: the user asked for it ("Try with
    /// your sign-in"). Otherwise the download goes without an account unless
    /// its video needed one (see `sessions::Sessions::lookup`).
    #[serde(default)]
    pub account: bool,
    /// Record a live stream instead (see [`live`]): waits for it to start if
    /// it hasn't, and records until it ends or is stopped.
    #[serde(default)]
    pub record: Option<live::RecordRequest>,
}

/// How a job ended well: the finished file, and anything worth saying about it.
pub struct Done {
    pub path: PathBuf,
    /// Something worth saying about the file: why it was kept as recorded,
    /// or that the disk filled up.
    pub note: Option<String>,
    /// A recording kept as it was recorded, rather than made into the preset's file.
    pub as_recorded: bool,
    /// What the download list says of `note`, in a word or two ("No subtitles").
    /// None for a recording's notes, which the list names from `as_recorded`.
    pub note_label: Option<&'static str>,
}

impl Done {
    /// A finished download, and anything worth saying about it.
    fn file(path: PathBuf, note: Option<Note>) -> Self {
        Done {
            path,
            note_label: note.as_ref().map(|note| note.label),
            note: note.map(|note| note.text),
            as_recorded: false,
        }
    }
}

/// What a job makes its file from.
enum Source {
    /// A link, downloaded.
    Link(String),
    /// A file on this PC the user gave, converted (see [`files`]).
    File(Arc<Original>),
}

/// A file Siphon made for a job: what yt-dlp downloaded (under the job's own
/// marker, or in its work folder), or what a recording recorded. Only such a
/// file is moved, renamed or deleted by the steps that finish a job
/// (`move_file`, `publish`, `check_section`), so none of them can be handed
/// a file that was already on the PC: a file the user gave Siphon to convert
/// is read where it is and never becomes one of these.
#[derive(Debug)]
struct Made(PathBuf);

impl Made {
    fn path(&self) -> &Path {
        &self.0
    }
}

/// Something worth saying about a finished download.
struct Note {
    text: String,
    /// The same, in a word or two, for the download list.
    label: &'static str,
}

impl Note {
    fn subtitles(text: String) -> Self {
        Note {
            text,
            label: "No subtitles",
        }
    }

    fn chat(text: String) -> Self {
        Note { text, label: "No chat" }
    }
}

/// Subtitles ready to go into the file, or into the picture.
struct Prepared {
    /// The file for the ffmpeg pass, in the work folder, on the video's own clock.
    track: PathBuf,
    /// How many cues it has: they take room in a smart cut's index.
    cues: usize,
}

/// A playlist video's folder and number. Sent again, unchanged, on every
/// resume and retry, so the download carries on in the same folder.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Place {
    /// A folder inside the download folder, usually the playlist's title. Cleaned up before use.
    #[serde(default)]
    pub folder: Option<String>,
    /// Its position in the playlist (from 1) and the playlist's length, for "07 - Title".
    #[serde(default)]
    pub number: Option<(u32, u32)>,
}

impl Place {
    /// "07 - " for the 7th of 95; at least two digits, more for longer playlists.
    fn prefix(&self) -> String {
        match self.number {
            Some((index, count)) => {
                let width = count.max(index).to_string().len().max(2);
                format!("{index:0width$} - ")
            }
            None => String::new(),
        }
    }
}

/// A playlist's title as a folder name Windows accepts: no characters it
/// forbids, no dots or spaces at the end, not a reserved device name, and not
/// too long. None if nothing usable is left.
fn folder_name(title: &str) -> Option<String> {
    let cleaned: String = title
        .chars()
        .map(|c| {
            if c.is_control() || r#"<>:"/\|?*"#.contains(c) {
                ' '
            } else {
                c
            }
        })
        .collect();
    let mut name = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    if name.chars().count() > 100 {
        name = name.chars().take(100).collect();
    }
    let name = name.trim_matches(|c: char| c == '.' || c.is_whitespace()).to_string();
    if name.is_empty() {
        return None;
    }
    let stem = name.split('.').next().unwrap_or_default().to_ascii_uppercase();
    let reserved = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (stem.len() == 4
            && (stem.starts_with("COM") || stem.starts_with("LPT"))
            && stem.as_bytes()[3].is_ascii_digit());
    Some(if reserved { format!("{name}_") } else { name })
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DownloadEvent {
    Progress {
        id: String,
        #[serde(flatten)]
        progress: Progress,
    },
    Finished {
        id: String,
        path: String,
        /// Bytes.
        size: Option<u64>,
        /// Anything worth saying about the file (a recording kept as it was recorded, and why).
        #[serde(skip_serializing_if = "Option::is_none")]
        note: Option<String>,
        /// A recording kept as it was recorded (see `note`), not made into the preset's file.
        #[serde(skip_serializing_if = "std::ops::Not::not")]
        as_recorded: bool,
        /// `note` in a word or two, for the download list.
        #[serde(skip_serializing_if = "Option::is_none")]
        note_label: Option<&'static str>,
        /// A recording that was cut short by Siphon closing or crashing, finished after it started again.
        #[serde(skip_serializing_if = "std::ops::Not::not")]
        recovered: bool,
    },
    Failed {
        id: String,
        error: AppError,
    },
    /// Stopped for now; its partial files are kept so it can be resumed.
    Paused {
        id: String,
    },
    Cancelled {
        id: String,
    },
}

/// How one try at a job looks its video up.
#[derive(Debug, Clone, Copy)]
struct Lookup {
    /// Again, instead of trusting what this session remembers.
    refresh: bool,
    /// With the site's sign-in, or as the site's setting says.
    account: Account,
}

/// Why the user stopped a job.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stop {
    Cancel,
    Pause,
    /// A live recording's Stop: end it there and make it into its file.
    Finish,
}

#[derive(Default)]
struct Job {
    /// The child processes working for this job: the download, and beside it
    /// the subtitles' (see `attempt`). Stopping the job ends them all.
    pids: Vec<u32>,
    stop: Option<Stop>,
    /// The job's way through the speed limit, while it runs. It goes with the
    /// job (see `end_job`), whether that finished, failed, was paused or was
    /// cancelled, which closes its proxy listener and connections.
    flow: Option<throttle::Flow>,
    /// The site's sign-in for yt-dlp, while the job runs: a temporary
    /// `cookies.txt` that goes with the job the same way as `flow`, and is
    /// deleted with it (see `sessions::CookieFile`).
    cookies: Option<CookieFile>,
    /// Its process is asked to stop (Ctrl+Break) rather than killed, so the
    /// file it writes is closed properly: a live recording.
    graceful: bool,
    /// That request, while it is under way: it ends once every process it
    /// asked has ended (see `process::interrupt_tree`).
    interrupting: Option<tokio::task::JoinHandle<()>>,
    /// A recording read from Twitch's VOD, behind live, while it is: Stop
    /// lets it record on to where Stop was pressed (see `twitch_feed`).
    catch_up: Option<twitch_feed::CatchUp>,
    /// What it runs keeps below other programs: a conversion of files on
    /// this PC, which a batch of keeps the processor busy for long.
    gentle: bool,
}

/// Who may run: jobs take a slot in the order they were added.
#[derive(Default)]
struct Gate {
    waiting: VecDeque<String>,
    running: usize,
}

/// What a paused or failed download left on disk. The next try at the same
/// download carries on from these files, even after Siphon was closed and
/// opened again (see [`SavedState`]); cancelling it removes them.
#[derive(Default, Serialize, Deserialize)]
struct Leftovers {
    /// The files yt-dlp downloaded streams to. Each carries the download's
    /// own marker (see `args::own_marker`), so they, and their `.part`
    /// companions, are always this download's to remove.
    stream_files: Vec<PathBuf>,
    /// Anything else: a section's `.part` file, a downloaded source awaiting conversion.
    others: Vec<PathBuf>,
    /// The download's work folder, holding a source that awaits converting.
    work: Option<PathBuf>,
}

impl Leftovers {
    async fn remove(self) {
        for file in &self.others {
            let _ = tokio::fs::remove_file(file).await;
        }
        remove_partial_files(&self.stream_files).await;
        if let Some(work) = &self.work {
            let _ = tokio::fs::remove_dir_all(work).await;
        }
    }
}

#[derive(Default)]
pub struct DownloadManager {
    jobs: Mutex<HashMap<String, Job>>,
    gate: Mutex<Gate>,
    /// Signalled whenever the gate may let someone through: a slot was
    /// freed, a waiting job was stopped, or the limit changed.
    gate_changed: Notify,
    /// By download id, for the downloads that are paused or have failed.
    leftovers: Mutex<HashMap<String, Leftovers>>,
    /// Files finished this session: the only ones the UI may open or reveal.
    finished: Mutex<HashSet<PathBuf>>,
    /// The finished names running downloads are writing to, and by which
    /// download, so two downloads of one video never pick the same name.
    claimed: Mutex<HashMap<PathBuf, String>>,
    /// Live recordings that haven't been made into their file yet, by id:
    /// what they recorded so far and what to make of it. Kept on disk from
    /// the first byte, so one that Siphon's closing or a crash cut short is
    /// finished when it starts again (see [`live`]).
    recordings: Mutex<HashMap<String, live::Recording>>,
    /// The file on this PC each conversion of one was started on, by id,
    /// until it has finished or is cancelled: a paused or failed one can be
    /// resumed after Siphon was restarted (see [`files`]). Never written to.
    originals: Mutex<HashMap<String, PathBuf>>,
    /// Who may convert a file on this PC now, by what it takes (see [`files`]).
    files_gate: Mutex<files::FilesGate>,
    /// Where `leftovers` and `finished` are kept between sessions. None in tests.
    state_file: Option<PathBuf>,
}

/// What the download list needs from one session to the next: the files of
/// paused and failed downloads, so they carry on where they stopped, and the
/// finished files, so they can still be opened from the list.
#[derive(Default, Serialize, Deserialize)]
struct SavedState {
    #[serde(default)]
    leftovers: HashMap<String, Leftovers>,
    #[serde(default)]
    finished: Vec<PathBuf>,
    #[serde(default)]
    recordings: HashMap<String, live::Recording>,
    #[serde(default)]
    originals: HashMap<String, PathBuf>,
}

/// Everything one running job needs.
struct JobContext<'a> {
    app: &'a AppHandle,
    bins: &'a BinaryManager,
    id: &'a str,
    settings: Settings,
    /// A playlist video's folder and number; empty for anything else.
    place: Place,
    /// It converts a file on this PC, rather than a download: what went
    /// wrong is said so.
    converts_file: bool,
}

impl JobContext<'_> {
    /// Where the job's file goes: the download folder, or the playlist's folder in it. Made if missing.
    async fn output_dir(&self) -> Result<PathBuf> {
        let output_dir = match &self.settings.output_dir {
            Some(dir) => dir.clone(),
            None => default_output_dir(self.app)?,
        };
        let output_dir = match self.place.folder.as_deref().and_then(folder_name) {
            Some(folder) => output_dir.join(folder),
            None => output_dir,
        };
        tokio::fs::create_dir_all(&output_dir).await.map_err(|err| {
            AppError::internal("Couldn't create the download folder. Check the folder in Settings.")
                .with_details(format!("{}: {err}", output_dir.display()))
        })?;
        Ok(output_dir)
    }

    fn emit(&self, progress: Progress) {
        let _ = self.app.emit(
            EVENT,
            DownloadEvent::Progress {
                id: self.id.to_string(),
                progress,
            },
        );
    }
}

impl DownloadManager {
    /// The manager, with what the last session left: paused and failed
    /// downloads' files (those still on disk), and finished files that still exist.
    pub fn load(app: &AppHandle) -> Self {
        match app.path().app_config_dir() {
            Ok(dir) => Self::from_file(dir.join("downloads.json")),
            Err(_) => Self::default(),
        }
    }

    fn from_file(state_file: PathBuf) -> Self {
        let saved: SavedState = std::fs::read(&state_file)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        Self {
            leftovers: Mutex::new(saved.leftovers),
            finished: Mutex::new(saved.finished.into_iter().filter(|path| path.exists()).collect()),
            recordings: Mutex::new(saved.recordings),
            originals: Mutex::new(saved.originals),
            state_file: Some(state_file),
            ..Self::default()
        }
    }

    /// Writes what the next session needs (see [`SavedState`]). Write-then-rename,
    /// so a crash can't leave half a file; a failure only costs resuming after a restart.
    fn save(&self) {
        let Some(file) = &self.state_file else { return };
        let bytes = {
            let leftovers = self.leftovers.lock().unwrap();
            let finished = self.finished.lock().unwrap();
            let recordings = self.recordings.lock().unwrap();
            let originals = self.originals.lock().unwrap();
            let view = serde_json::json!({
                "leftovers": &*leftovers,
                "finished": &*finished,
                "recordings": &*recordings,
                "originals": &*originals,
            });
            serde_json::to_vec(&view)
        };
        let Ok(bytes) = bytes else { return };
        let tmp = file.with_extension("json.tmp");
        if std::fs::write(&tmp, bytes).is_ok() {
            let _ = std::fs::rename(&tmp, file);
        }
    }

    /// Takes what an earlier try at this download left, for this try to carry on from.
    fn take_leftovers(&self, id: &str) -> Option<Leftovers> {
        let taken = self.leftovers.lock().unwrap().remove(id);
        if taken.is_some() {
            self.save();
        }
        taken
    }

    /// The download ids whose files are kept for a later try, a recording
    /// still to be finished, or a conversion of a file that may be resumed.
    pub fn kept_ids(&self) -> HashSet<String> {
        let mut kept: HashSet<String> = self.leftovers.lock().unwrap().keys().cloned().collect();
        kept.extend(self.recordings.lock().unwrap().keys().cloned());
        kept.extend(self.originals.lock().unwrap().keys().cloned());
        kept
    }

    /// Removes the files of paused or failed downloads that the list no longer
    /// has (it was cleared, or lost): nothing would ever carry on from them.
    pub async fn forget_all_but(&self, known: &HashSet<String>) {
        let orphans: Vec<String> = self.kept_ids().into_iter().filter(|id| !known.contains(id)).collect();
        for id in orphans {
            self.discard_leftovers(&id).await;
        }
    }

    /// Queues a download. It starts when a slot is free; progress arrives as events.
    /// A live recording starts at once: the stream doesn't wait either.
    /// A file on this PC (its key in place of a link) is converted when its
    /// turn comes (see [`files`]).
    pub fn start(self: &Arc<Self>, app: AppHandle, request: DownloadRequest) -> Result<()> {
        let source = self.source_of(&app, &request)?;
        let id = request.id;
        let recording = request.record;
        match &source {
            Source::Link(_) => self.register(&id, recording.is_none()),
            Source::File(_) => self.register_file(&id),
        }

        let manager = Arc::clone(self);
        tauri::async_runtime::spawn(async move {
            let bins = Arc::clone(&app.state::<Arc<BinaryManager>>());
            let _ = app.emit(
                EVENT,
                DownloadEvent::Progress {
                    id: id.clone(),
                    progress: Tracker::queued(),
                },
            );
            let account = if request.account { Account::Use } else { Account::Auto };
            let job = || JobContext {
                app: &app,
                bins: &bins,
                id: &id,
                // Read once the job starts, so changes made while it queued apply.
                settings: app.state::<SettingsStore>().get(),
                place: request.place.clone().unwrap_or_default(),
                converts_file: matches!(source, Source::File(_)),
            };

            let outcome = match (&source, recording) {
                (Source::File(original), _) => {
                    let outcome = manager.run_file(&job(), original, &request.choice, request.trim).await;
                    if let Err(error) = &outcome {
                        if error.kind != ErrorKind::Cancelled {
                            let name = original.name();
                            log::line(&app, &format!("convert {name} failed: {}", failure_summary(error)));
                        }
                    }
                    outcome
                }
                (Source::Link(url), Some(recording)) => match app.state::<Arc<Throttle>>().flow().await {
                    Ok(flow) => {
                        manager.attach_flow(&id, flow);
                        manager.record(&job(), url, &request.choice, recording, account).await
                    }
                    Err(error) => Err(error),
                },
                (Source::Link(url), None) => {
                    if manager.wait_for_slot(&app, &id).await {
                        let outcome = match app.state::<Arc<Throttle>>().flow().await {
                            Ok(flow) => {
                                manager.attach_flow(&id, flow);
                                manager.run(&job(), url, &request.choice, request.trim, account).await
                            }
                            Err(error) => Err(error),
                        };
                        manager.gate.lock().unwrap().running -= 1;
                        manager.gate_changed.notify_waiters();
                        outcome
                    } else {
                        Err(AppError::cancelled())
                    }
                }
            };

            let stop = manager.end_job(&id);
            if stop == Some(Stop::Cancel) {
                // Including what an earlier try left, if it was cancelled between tries.
                manager.discard_leftovers(&id).await;
            }
            // A conversion that is done no longer needs its file remembered;
            // a paused or failed one does, to be resumed after a restart.
            if matches!(source, Source::File(_)) && stop.is_none() && outcome.is_ok() {
                manager.forget_original(&id);
            }
            let event = match (stop, outcome) {
                (Some(Stop::Cancel), _) => DownloadEvent::Cancelled { id },
                (Some(Stop::Pause), _) => DownloadEvent::Paused { id },
                // A stopped recording is finished like one whose stream ended.
                (None | Some(Stop::Finish), Ok(done)) => manager.finished_event(id, done, false),
                (None | Some(Stop::Finish), Err(error)) => DownloadEvent::Failed { id, error },
            };
            let _ = app.emit(EVENT, event);
        });
        Ok(())
    }

    /// What to tell the frontend about a finished job; its file may be opened from now on.
    fn finished_event(&self, id: String, done: Done, recovered: bool) -> DownloadEvent {
        self.finished.lock().unwrap().insert(done.path.clone());
        self.save();
        DownloadEvent::Finished {
            size: std::fs::metadata(&done.path).ok().map(|meta| meta.len()),
            path: done.path.to_string_lossy().into_owned(),
            note: done.note,
            as_recorded: done.as_recorded,
            note_label: done.note_label,
            recovered,
            id,
        }
    }

    /// Adds a job to the list, and a download to the back of the queue.
    fn register(&self, id: &str, queued: bool) {
        self.jobs.lock().unwrap().insert(id.to_string(), Job::default());
        if queued {
            self.gate.lock().unwrap().waiting.push_back(id.to_string());
        }
    }

    /// Adds a conversion of a file on this PC to the list. It waits for its
    /// turn by what it takes (see [`files`]), not in the downloads' queue,
    /// and what it runs keeps below other programs, so the PC stays usable.
    fn register_file(&self, id: &str) {
        let job = Job {
            gentle: true,
            ..Job::default()
        };
        self.jobs.lock().unwrap().insert(id.to_string(), job);
    }

    /// What a request makes its file from: its link, or the file on this PC
    /// its key stands for (see `crate::local`). The webview names files only
    /// by such keys, and only the backend gives them out; after a restart, a
    /// conversion is resumed on the file it was started on.
    fn source_of(&self, app: &AppHandle, request: &DownloadRequest) -> Result<Source> {
        if !local::is_key(&request.url) {
            return Ok(Source::Link(ytdlp::validate_url(&request.url)?.to_string()));
        }
        if request.record.is_some() {
            return Err(AppError::new(ErrorKind::Unsupported, "A file can't be recorded."));
        }
        let grants = app.state::<Grants>();
        let original = match grants.get(&request.url) {
            Some(original) => original,
            None => {
                let saved = self.originals.lock().unwrap().get(&request.id).cloned();
                let Some(saved) = saved else {
                    return Err(AppError::internal(
                        "Siphon lost track of this file. Drop it on the window again.",
                    ));
                };
                grants
                    .grant(&saved)
                    .filter(|original| original.key() == request.url)
                    .ok_or_else(|| {
                        AppError::new(
                            ErrorKind::Unavailable,
                            "This file isn't there any more. It may have been moved or deleted.",
                        )
                    })?
            }
        };
        self.originals
            .lock()
            .unwrap()
            .insert(request.id.clone(), original.path().to_path_buf());
        self.save();
        Ok(Source::File(original))
    }

    /// A conversion of a file is done: its file needn't be remembered.
    fn forget_original(&self, id: &str) {
        if self.originals.lock().unwrap().remove(id).is_some() {
            self.save();
        }
    }

    /// Gives a job that is about to run its way through the speed limit.
    fn attach_flow(&self, id: &str, flow: throttle::Flow) {
        if let Some(job) = self.jobs.lock().unwrap().get_mut(id) {
            job.flow = Some(flow);
        }
    }

    /// Gives a job's yt-dlp run its site's sign-in, replacing an earlier try's.
    fn attach_cookies(&self, id: &str, cookies: Option<CookieFile>) -> bool {
        let signed_in = cookies.is_some();
        if let Some(job) = self.jobs.lock().unwrap().get_mut(id) {
            job.cookies = cookies;
        }
        signed_in
    }

    /// Adds the job's cookie file, if it has one, to a yt-dlp command.
    fn add_cookies(&self, id: &str, cmd: &mut Command) {
        if let Some(file) = self.jobs.lock().unwrap().get(id).and_then(|job| job.cookies.as_ref()) {
            cmd.arg("--cookies").arg(file.path());
        }
    }

    /// Sends a yt-dlp command's downloads through the job's flow.
    fn route(&self, id: &str, cmd: &mut Command) -> Result<()> {
        let jobs = self.jobs.lock().unwrap();
        // Never download around the limit.
        let flow = jobs
            .get(id)
            .and_then(|job| job.flow.as_ref())
            .ok_or_else(|| AppError::internal("Couldn't prepare the download."))?;
        flow.route(cmd);
        Ok(())
    }

    /// The one way out for every job, however it ended: takes it off the
    /// list, which closes its proxy listener and connections, and releases
    /// the names it claimed. Returns why it was stopped, if it was.
    fn end_job(&self, id: &str) -> Option<Stop> {
        let job = self.jobs.lock().unwrap().remove(id);
        self.claimed.lock().unwrap().retain(|_, owner| owner != id);
        job.and_then(|job| job.stop)
    }

    /// Waits until this job is first in line and a slot is free.
    /// Returns false if the job was stopped while it waited.
    async fn wait_for_slot(&self, app: &AppHandle, id: &str) -> bool {
        loop {
            // Created before checking, so a signal between check and await isn't missed.
            let changed = self.gate_changed.notified();
            {
                let mut gate = self.gate.lock().unwrap();
                if self.stop_of(id).is_some() {
                    gate.waiting.retain(|waiting| waiting != id);
                    drop(gate);
                    // The job behind may be first in line now.
                    self.gate_changed.notify_waiters();
                    return false;
                }
                let limit = app.state::<SettingsStore>().get().max_concurrent;
                if gate.running < limit && gate.waiting.front().is_some_and(|first| first == id) {
                    gate.waiting.pop_front();
                    gate.running += 1;
                    drop(gate);
                    // More than one slot may be free: let the next in line look too.
                    self.gate_changed.notify_waiters();
                    return true;
                }
            }
            changed.await;
        }
    }

    /// Whether `path` is a file a download produced, this session or one
    /// remembered in `downloads.json`.
    pub fn produced(&self, path: &Path) -> bool {
        self.finished.lock().unwrap().contains(path)
    }

    /// True when nothing is running or waiting (paused downloads don't count).
    pub fn is_idle(&self) -> bool {
        self.jobs.lock().unwrap().is_empty()
    }

    /// Re-evaluates the queue, after the concurrency limit changed.
    pub fn refresh_queue(&self) {
        self.gate_changed.notify_waiters();
    }

    /// Stops a download for good and removes its partial files.
    /// Also discards the files of a download that was paused or failed earlier.
    pub async fn cancel(&self, id: &str) {
        if !self.stop(id, Stop::Cancel).await {
            self.discard_leftovers(id).await;
        }
    }

    /// Removes what a paused or failed download left on disk, and forgets
    /// which file a conversion was on (the file itself is never touched).
    async fn discard_leftovers(&self, id: &str) {
        let leftovers = self.leftovers.lock().unwrap().remove(id);
        let original = self.originals.lock().unwrap().remove(id);
        if leftovers.is_some() || original.is_some() {
            self.save();
        }
        if let Some(leftovers) = leftovers {
            leftovers.remove().await;
        }
    }

    fn keep_leftovers(&self, id: &str, leftovers: Leftovers) {
        self.leftovers.lock().unwrap().insert(id.to_string(), leftovers);
        self.save();
    }

    /// Stops a download but keeps what it has fetched, so it can be resumed.
    pub async fn pause(&self, id: &str) {
        self.stop(id, Stop::Pause).await;
    }

    /// Pauses every download, for when the app closes: no yt-dlp or ffmpeg
    /// process may outlive it, but what they fetched is kept, and the next
    /// session carries on from it (see [`SavedState`]).
    pub async fn shut_down(&self) {
        let ids: Vec<String> = self.jobs.lock().unwrap().keys().cloned().collect();
        for id in ids {
            self.stop(&id, Stop::Pause).await;
        }
        // Each job records its own files once its process has ended.
        let deadline = Instant::now() + SHUTDOWN_GRACE;
        while !self.is_idle() && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        // A job that hasn't ended by now still loses its proxy and its
        // cookies: no listener is left open and no cookie file on disk,
        // whatever the job is stuck on.
        for job in self.jobs.lock().unwrap().values_mut() {
            job.flow = None;
            job.cookies = None;
        }
        self.save();
    }

    /// Ends a live recording there, and makes it into its file.
    pub async fn finish(&self, id: &str) {
        self.stop(id, Stop::Finish).await;
    }

    /// Returns false if there is no such job.
    async fn stop(&self, id: &str, stop: Stop) -> bool {
        let pid = {
            let mut jobs = self.jobs.lock().unwrap();
            let Some(job) = jobs.get_mut(id) else { return false };
            job.stop = Some(stop);
            if job.graceful && stop != Stop::Cancel {
                // Behind live, it records on to where Stop was pressed and
                // ends there by itself; pressed again, Stop stops it at once.
                let ends_itself = stop == Stop::Finish && job.catch_up.as_ref().is_some_and(twitch_feed::CatchUp::ask);
                // Asked to stop, so it closes its file properly; whoever runs it
                // waits for that (see `run_child`).
                if let (false, Some(&pid)) = (ends_itself, job.pids.first()) {
                    job.interrupting = Some(interrupt(pid));
                }
                Vec::new()
            } else {
                job.pids.clone()
            }
        };
        // Wakes the job if it is still waiting in the queue …
        self.gate_changed.notify_waiters();
        // … and ends its processes if it is running.
        for pid in pid {
            process::kill_tree(pid).await;
        }
        true
    }

    fn stop_of(&self, id: &str) -> Option<Stop> {
        self.jobs.lock().unwrap().get(id).and_then(|job| job.stop)
    }

    /// Runs the whole job and returns the finished file.
    async fn run(
        &self,
        job: &JobContext<'_>,
        url: &str,
        choice: &Choice,
        trim: Option<Trim>,
        account: Account,
    ) -> Result<Done> {
        job.emit(Tracker::new(false).current());
        let output_dir = job.output_dir().await?;

        let mut updated = false;
        let mut retries = Retries::default();
        let mut lookup = Lookup {
            refresh: false,
            account,
        };
        loop {
            let error = match self.attempt(job, url, choice, trim, &output_dir, lookup).await {
                Ok(done) => return Ok(done),
                Err(error) => error,
            };
            if self.stop_of(job.id).is_some() {
                return Err(error);
            }
            // Why a try failed, retried or not: the user sees only the last error.
            log::line(job.app, &format!("try failed {url}: {}", failure_summary(&error)));

            // The site refused without an account, in a way the sign-in Siphon
            // has could fix: look the video up again and download it with that,
            // once. (The bot check and unrecognised errors are the user's call.)
            let account_helps = error.sign_in.as_ref().is_some_and(|need| {
                need.signed_in && !need.refused && matches!(need.chance, Chance::Needed | Chance::Maybe)
            });
            if lookup.account == Account::Auto && account_helps {
                lookup = Lookup {
                    refresh: true,
                    account: Account::Use,
                };
                job.emit(Tracker::new(false).current());
                continue;
            }

            // Sites change, and yt-dlp follows within days. If the failure
            // looks like that, get the newest yt-dlp and try again with it.
            if error.kind == ErrorKind::Outdated {
                // The formats looked up earlier may be the problem, whether
                // the site has changed since or an older yt-dlp read them.
                lookup.refresh = true;
            }
            if error.kind == ErrorKind::Outdated && !updated {
                updated = true;
                job.emit(Tracker::updating());
                if let Ok(Some(_)) = job.bins.update_now(Tool::YtDlp, job.settings.update_channel).await {
                    job.emit(Tracker::new(false).current());
                    continue;
                }
                // Already current, or the update has to wait for other
                // downloads: fall through to an ordinary retry, if any.
            }

            // Sites refuse or drop a request now and then, and the same
            // request works a little later. Wait, then carry on from the
            // files this try left.
            let Some(delay) = retries.next(error.retry) else {
                return Err(error);
            };
            // The next try looks the video up again, as every try did when the
            // download looked it up itself: the stream links the last try had
            // may be what failed.
            lookup.refresh = true;
            job.emit(Tracker::retrying());
            if !self.wait_unless_stopped(job.id, delay).await {
                return Err(error);
            }
            job.emit(Tracker::new(false).current());
        }
    }

    /// Waits for `delay`, or until the job is stopped. Returns false if it was stopped.
    async fn wait_unless_stopped(&self, id: &str, delay: Duration) -> bool {
        let delay = tokio::time::sleep(delay);
        tokio::pin!(delay);
        loop {
            // Stopping a job signals the gate (see `stop`). Created before
            // checking, so a signal between check and await isn't missed.
            let changed = self.gate_changed.notified();
            if self.stop_of(id).is_some() {
                return false;
            }
            tokio::select! {
                _ = &mut delay => return self.stop_of(id).is_none(),
                _ = changed => {}
            }
        }
    }

    /// One pass through the job's steps: look up, download, add subtitles, convert.
    async fn attempt(
        &self,
        job: &JobContext<'_>,
        url: &str,
        choice: &Choice,
        trim: Option<Trim>,
        output_dir: &Path,
        lookup: Lookup,
    ) -> Result<Done> {
        let video = self.video_info(job, url, lookup).await?;
        let caps = job.app.state::<CapabilityCache>().get(job.bins).await;
        let recipe = plan::plan(&video, choice, trim, &caps)?;
        if let Some(reason) = &recipe.blocked {
            return Err(AppError::new(ErrorKind::TooLong, reason.clone()));
        }
        // A pass that only adds a subtitle track to copied streams is quick:
        // the download is nearly all of the work, and the bar says so.
        let only_subtitles = recipe.adds_only_subtitles();
        let mut tracker = Tracker::new(recipe.needs_ffmpeg_pass() && !only_subtitles);
        if only_subtitles {
            tracker.set_pass_stage(Stage::Subtitles);
        }

        // A source that is converted afterwards goes to the download's own work
        // folder, and stays there until the finished file is written and checked.
        let work = work_dir(job.app, job.id)?;
        let fetch_dir = if recipe.needs_ffmpeg_pass() { &work } else { output_dir };
        tokio::fs::create_dir_all(fetch_dir).await?;
        // Without the work folder there is nowhere for the lookup or subtitles
        // to go; the video can still be downloaded, and the subtitles say why.
        let work_ready = tokio::fs::create_dir_all(&work).await.is_ok();
        let info_json = if work_ready {
            saved_lookup(&video, &recipe, &work).await
        } else {
            None
        };

        // The subtitles are fetched beside the video, by a yt-dlp of their own:
        // subtitles that can't be had are said, never a reason for the video to
        // fail, and with the video in the same run they would be (see
        // `subtitles`). Side by side rather than after it, they cost the
        // download no time of its own.
        let (downloaded, fetched, chat) = {
            let subtitles_fetch = async {
                let step = recipe.subtitles.as_ref()?;
                if !work_ready {
                    return Some(Ok(Err("Siphon couldn't make a folder for them.".to_string())));
                }
                Some(
                    self.fetch_subtitles(job, &video.url, step, &work, info_json.as_deref())
                        .await,
                )
            };
            // The chat too, the same way: a chat that can't be had never costs the video.
            let chat_fetch = async {
                let step = recipe.chat.as_ref()?;
                if !work_ready {
                    return Some(Ok(Err("Siphon couldn't make a folder for it.".to_string())));
                }
                Some(self.fetch_chat(job, &recipe, step, &work).await)
            };
            tokio::pin!(subtitles_fetch);
            tokio::pin!(chat_fetch);
            let (downloaded, fetched, chat) = {
                let streams = self.fetch_streams(
                    job,
                    &video.url,
                    video.with_account,
                    &recipe,
                    fetch_dir,
                    info_json.as_deref(),
                    &mut tracker,
                );
                tokio::pin!(streams);
                let mut fetched = None;
                let mut chat = None;
                let downloaded = loop {
                    tokio::select! {
                        result = &mut streams => break result,
                        result = &mut subtitles_fetch, if fetched.is_none() => fetched = Some(result),
                        result = &mut chat_fetch, if chat.is_none() => chat = Some(result),
                    }
                };
                (downloaded, fetched, chat)
            };
            // A video that failed takes its subtitles and chat with it: dropping
            // their fetches ends them.
            let fetched = match (&downloaded, fetched) {
                (Err(_), _) => None,
                (Ok(_), Some(fetched)) => fetched,
                (Ok(_), None) => {
                    job.emit(tracker.on_subtitles());
                    subtitles_fetch.await
                }
            };
            let chat = match (&downloaded, chat) {
                (Err(_), _) => None,
                (Ok(_), Some(chat)) => chat,
                (Ok(_), None) => {
                    job.emit(tracker.on_chat());
                    chat_fetch.await
                }
            };
            (downloaded, fetched, chat)
        };
        // However the fetch ended: the next try writes it again.
        if let Some(info_json) = &info_json {
            let _ = tokio::fs::remove_file(info_json).await;
        }
        let downloaded = downloaded?;
        let (subtitles, mut note) = self
            .subtitles(&video, choice, &recipe, &work, fetched.transpose()?)
            .await;
        // No chat to draw: the video as it would have been without it, so a
        // copy stays a copy.
        let (recipe, downloaded) = match chat.transpose()? {
            Some(Err(why)) => {
                let text = format!("Saved without the chat. {why}");
                // Missing subtitles are said too, after it.
                note = Some(Note::chat(match note {
                    Some(subtitles) => format!("{text} {}", subtitles.text),
                    None => text,
                }));
                let mut without = choice.clone();
                if let Choice::Preset(preset) = &mut without {
                    preset.chat = None;
                }
                let plain = plan::plan(&video, &without, trim, &caps)?;
                let downloaded = if plain.needs_ffmpeg_pass() || fetch_dir == output_dir {
                    downloaded
                } else {
                    // Fetched into the work folder for the conversion that isn't happening.
                    let moved = output_dir.join(downloaded.path().file_name().unwrap_or_default());
                    move_file(downloaded, &moved).await?
                };
                (plain, downloaded)
            }
            _ => (recipe, downloaded),
        };

        let (recipe, source) = if recipe.needs_ffmpeg_pass() {
            (recipe, downloaded)
        } else {
            match (recipe.fallback.as_deref(), recipe.size_limit) {
                // Share: an original that was expected to fit but came out over
                // the limit is converted after all.
                (Some(fallback), Some(limit)) if file_size(downloaded.path()) > limit => {
                    let source = work.join(downloaded.path().file_name().unwrap_or_default());
                    tokio::fs::create_dir_all(&work).await?;
                    (fallback.clone(), move_file(downloaded, &source).await?)
                }
                _ => {
                    let path = self.publish(downloaded, job.id).await?;
                    let _ = tokio::fs::remove_dir_all(&work).await;
                    return Ok(Done::file(path, note));
                }
            }
        };

        let output = self.unique_output(converted_path(source.path(), &recipe, output_dir), job.id);
        let result = self
            .convert_checked(
                job,
                &recipe,
                source.path(),
                &output,
                &partial_path(&output),
                &work,
                &mut tracker,
                subtitles.as_ref(),
            )
            .await;
        match (&result, self.stop_of(job.id)) {
            (Ok(()), _) | (Err(_), Some(Stop::Cancel)) => {
                let _ = tokio::fs::remove_dir_all(&work).await;
            }
            // Paused, or the conversion failed: keep the source, so the next try
            // goes straight back to converting instead of downloading it again.
            (Err(_), _) => self.keep_leftovers(
                job.id,
                Leftovers {
                    work: Some(work),
                    ..Leftovers::default()
                },
            ),
        }
        result?;
        Ok(Done::file(output, note))
    }

    /// The recipe's subtitles, fetched and made ready to add, or why there
    /// are none: the track couldn't be had, or the video has none in the
    /// language asked for (a playlist's videos differ). The note says so.
    /// `fetched` is where `fetch_subtitles` put the track, or why it couldn't.
    async fn subtitles(
        &self,
        video: &VideoInfo,
        choice: &Choice,
        recipe: &Recipe,
        work: &Path,
        fetched: Option<std::result::Result<PathBuf, String>>,
    ) -> (Option<Prepared>, Option<Note>) {
        let (Some(step), Some(fetched)) = (&recipe.subtitles, fetched) else {
            let missing = choice
                .subtitles()
                .filter(|_| video.live.is_none() && plan::subtitles::delivery(choice, recipe).is_some());
            let note = missing.map(|wanted| {
                let language = language_name(wanted).unwrap_or(wanted);
                Note::subtitles(format!("This video has no {language} subtitles."))
            });
            return (None, note);
        };
        let without = |why: String| {
            Some(Note::subtitles(format!(
                "Saved without {} subtitles. {why}",
                step.title
            )))
        };
        let raw = match fetched {
            Ok(raw) => raw,
            Err(why) => return (None, without(why)),
        };
        let text = match tokio::fs::read(&raw).await {
            Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
            Err(err) => return (None, without(format!("Couldn't read them: {err}."))),
        };
        if step.styled {
            // SubStation Alpha goes into Matroska, or into the picture, as it came, styling and all.
            let cues = text.lines().filter(|line| line.starts_with("Dialogue:")).count();
            return (Some(Prepared { track: raw, cues }), None);
        }
        let span = recipe.trim.map(|trim| srt::Span {
            start: trim.start,
            end: trim.end,
        });
        let cues = srt::prepare(srt::parse(&text), span);
        if cues.is_empty() {
            let why = if recipe.trim.is_some() {
                "They have nothing in this part."
            } else {
                "The site's file was empty."
            };
            return (None, without(why.into()));
        }
        let track = work.join("subtitles.srt");
        if let Err(err) = tokio::fs::write(&track, srt::write(&cues)).await {
            return (None, without(format!("Couldn't write them: {err}.")));
        }
        let prepared = Prepared {
            track,
            cues: cues.len(),
        };
        (Some(prepared), None)
    }

    /// Fetches one subtitle track into `work`, and says where it is, or why
    /// it isn't there. A refusal that may pass (HTTP 429, a dropped
    /// connection) is tried once more after a pause.
    /// What an earlier try at the download fetched is used again.
    async fn fetch_subtitles(
        &self,
        job: &JobContext<'_>,
        url: &str,
        step: &SubtitleStep,
        work: &Path,
        info_json: Option<&Path>,
    ) -> Result<std::result::Result<PathBuf, String>> {
        if let Some(found) = fetched_subtitles(work).await {
            return Ok(Ok(found));
        }
        let Some(subtitle_args) = args::subtitle_args(step) else {
            return Ok(Err("The site names them in a way Siphon can't ask for.".into()));
        };
        for try_number in 0..2 {
            let mut cmd = ytdlp::command(job.bins);
            cmd.args(&subtitle_args);
            self.route(job.id, &mut cmd)?;
            // The way the video was fetched: with the site's sign-in if it had one.
            self.add_cookies(job.id, &mut cmd);
            cmd.arg("--paths")
                .arg(work)
                .args(["--output", &format!("{}.%(ext)s", args::SUBTITLE_SOURCE)]);
            from_lookup(&mut cmd, info_json, url);
            match self.run_child(job.bins, job.id, cmd, |_| {}).await {
                Ok(_) => {
                    return Ok(fetched_subtitles(work)
                        .await
                        .ok_or_else(|| "The site didn't send them.".to_string()))
                }
                Err(Failure::Stopped(_)) => return Err(AppError::cancelled()),
                Err(Failure::Spawn(error)) => return Ok(Err(error.message)),
                Err(Failure::Exited(log)) => {
                    let error = errors::classify(&log);
                    if try_number == 0 && error.retry != Retry::No {
                        if !self.wait_unless_stopped(job.id, SUBTITLE_RETRY).await {
                            return Err(AppError::cancelled());
                        }
                        continue;
                    }
                    return Ok(Err(error.message));
                }
            }
        }
        Ok(Err("The site didn't send them.".into()))
    }

    /// Step 3, checked: converts (on the CPU if the hardware encoder refuses),
    /// then makes sure a Share file is under its limit, encoding again a
    /// little smaller if it isn't. The file only gets its final name once it
    /// passes; until then it is written beside it as `partial`.
    #[allow(clippy::too_many_arguments)]
    async fn convert_checked(
        &self,
        job: &JobContext<'_>,
        recipe: &Recipe,
        source: &Path,
        output: &Path,
        partial: &Path,
        work: &Path,
        tracker: &mut Tracker,
        subtitles: Option<&Prepared>,
    ) -> Result<()> {
        let partial = partial.to_path_buf();
        let mut recipe = recipe.clone();
        let name = output.file_name().unwrap_or_default().to_string_lossy().into_owned();
        // A copied stream that is cut: re-encode only the frames at the cuts.
        // Anything the smart cut isn't sure of is re-encoded in full instead.
        if recipe.smart_cut() {
            let started = Instant::now();
            // Joined and checked as MP4 (the checks read decode times, which
            // Matroska doesn't keep), then copied into the file type asked for,
            // a file's own subtitle tracks with it (an MP4 can't hold every kind).
            let file_subtitles = !recipe.tracks.subtitles.is_empty();
            let rewrap = !matches!(recipe.container, Container::Mp4 | Container::Mov) || file_subtitles;
            let (cut_recipe, cut_file) = if rewrap {
                let mut as_mp4 = recipe.clone();
                as_mp4.container = Container::Mp4;
                (as_mp4, work.join(SMART_CUT_FILE))
            } else {
                (recipe.clone(), partial.clone())
            };
            let mut result = self
                .smart_cut(job, &cut_recipe, source, &cut_file, work, tracker, subtitles)
                .await?;
            if rewrap && result.is_ok() {
                let piece = work.join(SUBTITLES_PIECE);
                let kept = file_subtitles.then_some((piece.as_path(), recipe.tracks.subtitles.as_slice()));
                let args = ffmpeg::rewrap_args(&cut_file, &partial, recipe.subtitles.as_ref(), recipe.container, kept);
                if let Err(fallback) = self.tool(job, &job.bins.ffmpeg(), args, None).await? {
                    result = Err(fallback);
                }
                let _ = tokio::fs::remove_file(&cut_file).await;
                let _ = tokio::fs::remove_file(&piece).await;
            }
            match result {
                Ok(note) => {
                    log::line(
                        job.app,
                        &format!(
                            "cut {name}: smart cut, {note}, {:.1} s",
                            started.elapsed().as_secs_f64()
                        ),
                    );
                    match (recipe.size_limit, recipe.fallback.as_deref()) {
                        // Share: a cut original that came out over the limit is converted after all.
                        (Some(limit), Some(fallback)) if file_size(&partial) > limit => {
                            let _ = tokio::fs::remove_file(&partial).await;
                            recipe = fallback.clone();
                        }
                        _ => {
                            tokio::fs::rename(&partial, output).await?;
                            return Ok(());
                        }
                    }
                }
                Err(fallback) => {
                    log::line(
                        job.app,
                        &format!("cut {name}: re-encoding the whole part, because {fallback}"),
                    );
                    let _ = tokio::fs::remove_file(&partial).await;
                    recipe = recipe.with_full_cut();
                }
            }
        } else if recipe.cut().is_some() {
            let why = recipe.reason.as_deref().unwrap_or("the video is converted anyway");
            log::line(job.app, &format!("cut {name}: during the conversion, because {why}"));
        }
        let mut shrinks = 0;
        loop {
            let track = subtitles.map(|prepared| prepared.track.as_path());
            let mut result = self.convert(job, &recipe, source, &partial, work, tracker, track).await;
            // A hardware encoder can refuse a job it passed the probe for (driver
            // limits, too many sessions). The CPU encoders always work.
            let on_hardware = recipe
                .video_encode()
                .is_some_and(|encode| encode.hardware != Hardware::Software);
            if result.is_err() && on_hardware && self.stop_of(job.id).is_none() {
                recipe = recipe.on_software();
                result = self.convert(job, &recipe, source, &partial, work, tracker, track).await;
            }
            if let Ok(()) = result {
                result = self.check_converted(job, &recipe, &partial).await;
            }
            if let Err(error) = result {
                let _ = tokio::fs::remove_file(&partial).await;
                return Err(error);
            }
            let size = file_size(&partial);
            let Some(smaller) = recipe
                .size_limit
                .filter(|limit| size > *limit)
                .map(|_| recipe.shrunk(size))
            else {
                break;
            };
            match smaller {
                Some(smaller) if shrinks < MAX_SHRINKS => {
                    shrinks += 1;
                    recipe = smaller;
                }
                _ => {
                    let _ = tokio::fs::remove_file(&partial).await;
                    return Err(AppError::new(
                        ErrorKind::TooLong,
                        "The file kept coming out over the size limit. Trim the video, or pick a larger limit.",
                    ));
                }
            }
        }
        tokio::fs::rename(&partial, output).await?;
        Ok(())
    }

    /// A converted file must hold what it was made for: ffmpeg finishes
    /// without an error when a cut finds nothing of its selection, and
    /// leaves a file with no tracks. Found 2026-11-23: a part of a Twitch VOD
    /// still being recorded, not on the video's clock, came out as a 262-byte
    /// MP4 and was called finished.
    async fn check_converted(&self, job: &JobContext<'_>, recipe: &Recipe, file: &Path) -> Result<()> {
        let verdict = match self.probe(job, file).await {
            Some(probed) => probed.has_content(recipe.video.is_some()),
            None => Err("ffprobe couldn't read it".into()),
        };
        verdict.map_err(|why| {
            AppError::internal(if job.converts_file {
                "The converted file came out empty."
            } else {
                "The video downloaded, but the converted file came out empty."
            })
            .with_details(format!("{}: {why}", file.display()))
        })
    }

    /// Cuts a copied stream on the exact frames, re-encoding only the frames
    /// at the cuts (see `ffmpeg::smart_cut`), and checks the result. The
    /// inner error says why it couldn't be used; the outer one is for a
    /// stopped download.
    #[allow(clippy::too_many_arguments)]
    async fn smart_cut(
        &self,
        job: &JobContext<'_>,
        recipe: &Recipe,
        source: &Path,
        output: &Path,
        work: &Path,
        tracker: &mut Tracker,
        subtitles: Option<&Prepared>,
    ) -> Result<std::result::Result<String, Fallback>> {
        let pieces = work.join("cut");
        let _ = tokio::fs::remove_dir_all(&pieces).await;
        tokio::fs::create_dir_all(&pieces).await?;
        let result = self
            .smart_cut_in(job, recipe, source, output, &pieces, tracker, subtitles)
            .await;
        // The pieces are as large as the part itself: don't keep them.
        let _ = tokio::fs::remove_dir_all(&pieces).await;
        result
    }

    #[allow(clippy::too_many_arguments)]
    async fn smart_cut_in(
        &self,
        job: &JobContext<'_>,
        recipe: &Recipe,
        source: &Path,
        output: &Path,
        pieces: &Path,
        tracker: &mut Tracker,
        subtitles: Option<&Prepared>,
    ) -> Result<std::result::Result<String, Fallback>> {
        macro_rules! step {
            ($result:expr) => {
                match $result {
                    Ok(value) => value,
                    Err(fallback) => return Ok(Err(fallback)),
                }
            };
            // A failed tool, with what it was doing.
            ($result:expr, $what:expr) => {
                match $result {
                    Ok(value) => value,
                    Err(Fallback::Failed(error)) => return Ok(Err(Fallback::Failed(format!("{}: {error}", $what)))),
                    Err(fallback) => return Ok(Err(fallback)),
                }
            };
        }
        let (Some(trim), Some(encode), Some(video)) = (recipe.cut(), recipe.cut_encode.as_ref(), recipe.video.as_ref())
        else {
            return Ok(Err(Fallback::Failed("not a smart cut".into())));
        };
        let ffmpeg = job.bins.ffmpeg();
        let ffprobe = job.bins.ffprobe();
        const STEPS: f64 = 9.0;
        let mut done = 0.0;
        // How long each step took, for the log.
        let mut laps: Vec<f64> = Vec::new();
        let mut lap = Instant::now();
        let mut progress = |tracker: &mut Tracker| {
            done += 1.0;
            laps.push(lap.elapsed().as_secs_f64());
            lap = Instant::now();
            job.emit(tracker.on_convert(done / STEPS));
        };
        job.emit(tracker.on_convert(0.0));

        // A whole file on this PC is read from a little before the part, not from its start.
        let from = recipe.read_from;
        let read = |args: Vec<OsString>, to: f64| smart_cut::reading_from(args, source, from, to);

        // Read the source and decide the pieces.
        let source_video = step!(self.packets(job, source, "v:0", pieces, from, trim.end).await?);
        let params = smart_cut::parse_params(
            &step!(self.tool(job, &ffprobe, smart_cut::params_args(source), None).await?).join("\n"),
        );
        let encoder = step!(smart_cut::matching_args(&params, encode.hardware));
        let codec = if params.codec == "hevc" {
            VideoCodec::Hevc
        } else {
            video.source
        };
        let restarts = step!(
            self.restarts(job, source, codec, params.time_base, pieces, from, trim.end)
                .await?
        );
        let plan = step!(smart_cut::plan(&source_video, trim, &restarts));
        progress(tracker);

        // The main sound, then a file's other sound tracks (see `plan::tracks`).
        let sounds: Vec<(usize, &AudioAction)> = match (&recipe.audio, recipe.strip_audio) {
            (Some(step), false) => std::iter::once((recipe.tracks.main, &step.action))
                .chain(recipe.tracks.sounds.iter().map(|sound| (sound.track, &sound.action)))
                .collect(),
            _ => Vec::new(),
        };

        // The re-encoded start, the copied middle, the re-encoded end, the audio.
        let mut names: Vec<&str> = Vec::new();
        if plan.has_head() {
            let args = read(
                smart_cut::edge_args(
                    source,
                    plan.before(plan.first()),
                    plan.before(plan.copy_from),
                    &encoder,
                    plan.delay,
                    &pieces.join("head.ts"),
                ),
                plan.before(plan.copy_from),
            );
            step!(self.tool(job, &ffmpeg, args, None).await?, "re-encoding the start");
            names.push("head.ts");
        }
        progress(tracker);
        let args = read(
            smart_cut::copy_args(
                source,
                plan.before(plan.copy_from),
                Some(plan.before(plan.copy_until)),
                codec,
                &pieces.join("middle.ts"),
            ),
            plan.before(plan.copy_until),
        );
        step!(self.tool(job, &ffmpeg, args, None).await?, "copying the middle");
        names.push("middle.ts");
        progress(tracker);
        // The end is re-encoded from a copy of just its packets: decoding the
        // source from the start to reach them would take as long as the part.
        let rest = pieces.join("rest.ts");
        // A downloaded part ends a second past the cut; a whole file goes on: only what the end needs.
        let rest_until = from.map(|_| plan.end() + 2.0);
        let args = read(
            smart_cut::copy_args(source, plan.before(plan.copy_until), rest_until, codec, &rest),
            plan.end(),
        );
        step!(self.tool(job, &ffmpeg, args, None).await?, "copying the end's packets");
        let args = smart_cut::edge_args(
            &rest,
            plan.before(plan.copy_until),
            plan.before(plan.end()),
            &encoder,
            plan.delay,
            &pieces.join("tail.ts"),
        );
        step!(self.tool(job, &ffmpeg, args, None).await?, "re-encoding the end");
        names.push("tail.ts");
        progress(tracker);
        if !sounds.is_empty() {
            let args = read(
                smart_cut::audio_args(source, plan.first(), plan.end(), &sounds, &pieces.join("audio.mka")),
                plan.end(),
            );
            step!(self.tool(job, &ffmpeg, args, None).await?, "cutting the audio");
        }
        // A file's own subtitle tracks, kept apart until the clip is put in its file type.
        if !recipe.tracks.subtitles.is_empty() {
            let args = read(
                smart_cut::subtitles_args(
                    source,
                    plan.first(),
                    plan.end(),
                    &recipe.tracks.subtitles,
                    &pieces.with_file_name(SUBTITLES_PIECE),
                ),
                plan.end(),
            );
            step!(self.tool(job, &ffmpeg, args, None).await?, "cutting the subtitles");
        }
        progress(tracker);

        // Join them, on the video's own timestamps, starting at zero.
        // Audio packets are at most a hundred a second (AAC at 48 kHz is 47).
        let audio_packets =
            ((plan.end() - plan.first()) * smart_cut::AUDIO_PACKETS_KEPT_FOR).ceil() as usize * sounds.len();
        // The subtitle track, shifted with the pieces; each cue is a packet in the index.
        let track = subtitles
            .map(|prepared| prepared.track.as_path())
            .zip(recipe.subtitles.as_ref());
        let other_packets = audio_packets + subtitles.map_or(0, |prepared| prepared.cues * 2);
        let mut joined = Err(Fallback::NoFrames);
        // Room for the index at the front first; if it turns out too small,
        // write it at the end and move it.
        for index in [
            smart_cut::Index::room_for(plan.frames.len(), other_packets),
            smart_cut::Index::Rewrite,
        ] {
            let args = smart_cut::join_args(
                &names,
                (!sounds.is_empty()).then_some(("audio.mka", sounds.len())),
                track,
                plan.first(),
                codec,
                recipe.container,
                index,
                output,
            );
            joined = self.tool(job, &ffmpeg, args, Some(pieces)).await?;
            if joined.is_ok() {
                break;
            }
        }
        step!(joined, "joining");
        progress(tracker);

        // Check the result: every frame at its time, the audio in sync, and
        // the joins decoding cleanly, the first copied frames exactly as in the source.
        let clip_video = step!(self.packets(job, output, "v:0", pieces, None, 0.0).await?);
        step!(smart_cut::check_video(&plan, &clip_video));
        if let Some((main, action)) = sounds.first() {
            let clip_audio = step!(self.packets(job, output, "a:0", pieces, None, 0.0).await?);
            let source_audio = match action {
                AudioAction::Copy => Some(step!(
                    self.packets(job, source, &format!("a:{main}"), pieces, from, trim.end)
                        .await?
                )),
                AudioAction::Encode { .. } => None,
            };
            step!(smart_cut::check_audio(&plan, source_audio.as_deref(), &clip_audio));
        }
        progress(tracker);
        let stats = "head.psnr";
        step!(
            self.tool(
                job,
                &ffmpeg,
                read(smart_cut::head_check_args(output, source, &plan, stats), plan.end()),
                Some(pieces)
            )
            .await?,
            "decoding the first join"
        );
        let compared = tokio::fs::read_to_string(pieces.join(stats)).await.unwrap_or_default();
        step!(smart_cut::check_head(&plan, &compared));
        progress(tracker);
        step!(
            self.tool(job, &ffmpeg, smart_cut::tail_check_args(output, &plan), None)
                .await?,
            "decoding the last join"
        );
        progress(tracker);

        let copied = plan.frames.len()
            - plan.head_frames()
            - plan
                .frames
                .iter()
                .filter(|pts| **pts >= plan.before(plan.copy_until))
                .count();
        let steps = [
            "read",
            "start",
            "middle",
            "end",
            "audio",
            "join",
            "frames",
            "first join",
            "last join",
        ];
        let laps = steps
            .iter()
            .zip(&laps)
            .map(|(step, seconds)| format!("{step} {seconds:.1}"))
            .collect::<Vec<_>>()
            .join(", ");
        Ok(Ok(format!(
            "{} frames, {} copied, {} re-encoded, checked (seconds: {laps})",
            plan.frames.len(),
            copied,
            plan.frames.len() - copied
        )))
    }

    /// For a cut: how far the downloaded part's audio has to move onto its
    /// video's clock, when the two disagree (see `smart_cut::clock_shift`).
    async fn audio_shift(&self, job: &JobContext<'_>, recipe: &Recipe, input: &Path) -> Result<Option<f64>> {
        let (Some(cut), Some(_)) = (recipe.cut(), recipe.audio_codec()) else {
            return Ok(None);
        };
        let mut times = Vec::new();
        for stream in ["v:0".to_string(), format!("a:{}", recipe.tracks.main)] {
            let args = smart_cut::first_times_args(input, &stream);
            let lines = self
                .tool(job, &job.bins.ffprobe(), args, None)
                .await?
                .unwrap_or_default();
            times.push(
                lines
                    .iter()
                    .filter_map(|line| line.trim().parse::<f64>().ok())
                    .collect::<Vec<_>>(),
            );
        }
        let shift = smart_cut::clock_shift(&times[0], &times[1], cut.start);
        if let Some(shift) = shift {
            let name = input.file_name().unwrap_or_default().to_string_lossy();
            log::line(
                job.app,
                &format!("cut {name}: its audio is on another clock; moved by {shift:.3} s"),
            );
        }
        Ok(shift)
    }

    /// The times of the source's key frames that decoding restarts at (see
    /// `smart_cut::parse_restarts`); of a whole file, those from `from`
    /// before the part up to `to` (see `plan::ReadFrom`).
    #[allow(clippy::too_many_arguments)]
    async fn restarts(
        &self,
        job: &JobContext<'_>,
        source: &Path,
        codec: VideoCodec,
        time_base: f64,
        pieces: &Path,
        from: Option<ReadFrom>,
        to: f64,
    ) -> Result<std::result::Result<Vec<f64>, Fallback>> {
        let mut cmd = process::command(job.bins.ffmpeg());
        cmd.args(smart_cut::reading_from(
            smart_cut::key_frames_trace_args(source),
            source,
            from,
            to,
        ))
        .current_dir(pieces)
        // The trace goes to a report file: there is too much of it to keep in memory.
        .env("FFREPORT", "file=keys.log:level=32");
        match self.run_child(job.bins, job.id, cmd, |_| {}).await {
            Ok(_) => {}
            Err(Failure::Stopped(_)) => return Err(AppError::cancelled()),
            Err(Failure::Exited(log)) => {
                return Ok(Err(Fallback::Failed(format!(
                    "reading the key frames: {}",
                    log.lines().last().unwrap_or("no message").trim()
                ))))
            }
            Err(Failure::Spawn(error)) => return Ok(Err(Fallback::Failed(error.message))),
        }
        let trace = tokio::fs::read_to_string(pieces.join("keys.log"))
            .await
            .unwrap_or_default();
        let _ = tokio::fs::remove_file(pieces.join("keys.log")).await;
        if time_base <= 0.0 {
            return Ok(Err(Fallback::Timing));
        }
        Ok(Ok(smart_cut::parse_restarts(&trace, codec, time_base)))
    }

    /// A stream's packets, in decode order, as ffprobe lists them; of a
    /// whole file, those from `from` before the part up to `to` (see `plan::ReadFrom`).
    #[allow(clippy::too_many_arguments)]
    async fn packets(
        &self,
        job: &JobContext<'_>,
        file: &Path,
        stream: &str,
        pieces: &Path,
        from: Option<ReadFrom>,
        to: f64,
    ) -> Result<std::result::Result<Vec<smart_cut::Packet>, Fallback>> {
        let list = pieces.join("packets.csv");
        let args = smart_cut::probe_reading(smart_cut::packets_args(file, stream, &list), from, to);
        if let Err(fallback) = self.tool(job, &job.bins.ffprobe(), args, None).await? {
            return Ok(Err(fallback));
        }
        let text = tokio::fs::read_to_string(&list).await.unwrap_or_default();
        let _ = tokio::fs::remove_file(&list).await;
        Ok(Ok(text.lines().filter_map(smart_cut::parse_packet).collect()))
    }

    /// Runs ffmpeg or ffprobe for a job and returns what it printed. A failure
    /// is a reason to fall back; a stopped job is an error.
    async fn tool(
        &self,
        job: &JobContext<'_>,
        program: &Path,
        args: Vec<OsString>,
        dir: Option<&Path>,
    ) -> Result<std::result::Result<Vec<String>, Fallback>> {
        let mut cmd = process::command(program);
        cmd.args(args);
        if let Some(dir) = dir {
            cmd.current_dir(dir);
        }
        let mut lines = Vec::new();
        let result = self
            .run_child(job.bins, job.id, cmd, |event| {
                if let ChildEvent::Line(line) = event {
                    lines.push(line.trim_end().to_string());
                }
            })
            .await;
        Ok(match result {
            Ok(_) => Ok(lines),
            Err(Failure::Stopped(_)) => return Err(AppError::cancelled()),
            Err(Failure::Exited(log)) => Err(Fallback::Failed(
                log.lines().last().unwrap_or("no message").trim().to_string(),
            )),
            Err(Failure::Spawn(error)) => Err(Fallback::Failed(error.message)),
        })
    }

    /// The video's metadata: from this session's cache, or looked up now
    /// (playlist items are queued without having been looked up first).
    /// A refreshing lookup looks it up even if it is cached; the cached copy
    /// stays in place if that fails, so the open video keeps working. One that
    /// is to use the sign-in doesn't take a copy looked up without it.
    async fn video_info(&self, job: &JobContext<'_>, url: &str, lookup: Lookup) -> Result<Arc<VideoInfo>> {
        let cache = job.app.state::<MediaCache>();
        // A lookup old enough for its stream links to have run out is made again.
        let young = |video: &Arc<VideoInfo>| video.saved.as_ref().is_none_or(|saved| saved.fresh());
        let usable = |video: &Arc<VideoInfo>| {
            !lookup.refresh && (lookup.account == Account::Auto || video.with_account) && young(video)
        };
        if let Some(video) = cache.get(url).filter(usable) {
            return Ok(video);
        }
        let sessions = Arc::clone(&job.app.state::<Arc<Sessions>>());
        match sessions.fetch_media(job.bins, url, lookup.account).await? {
            Media::Video(video) => {
                let video = Arc::new(*video);
                cache.insert(url, Arc::clone(&video));
                Ok(video)
            }
            Media::Playlist(_) => Err(AppError::new(
                ErrorKind::Unsupported,
                "This link is a playlist. Choose the videos to download from it first.",
            )),
        }
    }

    /// Step 2: yt-dlp downloads the recipe's streams and merges or re-wraps
    /// them, from the saved lookup `info_json` if there is one.
    #[allow(clippy::too_many_arguments)]
    async fn fetch_streams(
        &self,
        job: &JobContext<'_>,
        url: &str,
        with_account: bool,
        recipe: &Recipe,
        output_dir: &Path,
        info_json: Option<&Path>,
        tracker: &mut Tracker,
    ) -> Result<Made> {
        // What an earlier try at this download left: it was paused, or it failed.
        let previous = self.take_leftovers(job.id);
        let continues = previous.is_some();
        // (Its work folder is the one this try uses again; it is removed with the download.)
        let Leftovers {
            mut stream_files,
            others: earlier_files,
            work: _,
        } = previous.unwrap_or_default();

        let mut output_file: Option<PathBuf> = None;
        // Section downloads go through ffmpeg, which stays silent until it is
        // done. For those, progress is read off the growing ".part" file.
        let mut part_file: Option<PathBuf> = None;
        let expected_bytes = recipe.trim.and(recipe.estimated_source_bytes());
        let mut cmd = ytdlp::command(job.bins);
        cmd.args(progress::template_args())
            .args(args::TRANSFER_ARGS)
            .args(args::recipe_args(recipe))
            .args([
                "--no-playlist",
                // Stamp the file with the time it was downloaded, not uploaded.
                "--no-mtime",
            ]);
        self.route(job.id, &mut cmd)?;
        // The way the video was looked up: with the site's sign-in (its cookies,
        // and only its own) if it needed one, otherwise without.
        let sessions = Arc::clone(&job.app.state::<Arc<Sessions>>());
        let cookies = if with_account { sessions.cookie_file(url) } else { None };
        let signed_in = self.attach_cookies(job.id, cookies);
        self.add_cookies(job.id, &mut cmd);
        if !continues {
            // A new download ignores partial files of the same name; a
            // later try leaves this out and carries on from them instead.
            cmd.arg("--no-continue");
        }
        // Every file carries this download's own marker until it is finished
        // (see `publish`), so nothing already in the folder is ever touched.
        cmd.arg("--paths").arg(output_dir).args([
            "--output",
            &args::output_template(
                recipe,
                // "07 - " in front for a numbered playlist video: digits and a dash, nothing yt-dlp reads as a field.
                &format!("{}{}", job.place.prefix(), job.settings.filename_template()),
                &args::own_marker(job.id),
            ),
        ]);
        from_lookup(&mut cmd, info_json, url);

        let result = self
            .run_child(job.bins, job.id, cmd, |event| match event {
                ChildEvent::Tick => {
                    if let (Some(part), Some(expected)) = (&part_file, expected_bytes) {
                        if let Ok(meta) = std::fs::metadata(part) {
                            job.emit(tracker.on_bytes(meta.len(), expected));
                        }
                    }
                }
                ChildEvent::Line(line) => match progress::parse(line) {
                    Some(progress::Line::Target(path)) => {
                        part_file = Some(PathBuf::from(format!("{path}.part")));
                    }
                    Some(progress::Line::Plan(plan)) => tracker.set_plan(plan),
                    Some(progress::Line::Download(tick)) => {
                        if let Some(path) = tick.filename.as_deref().map(PathBuf::from) {
                            if !stream_files.contains(&path) {
                                stream_files.push(path);
                            }
                        }
                        job.emit(tracker.on_tick(&tick));
                    }
                    Some(progress::Line::PostProcess { started: true, name }) => {
                        job.emit(tracker.on_postprocess(&name));
                    }
                    Some(progress::Line::File(path)) => output_file = Some(PathBuf::from(path)),
                    Some(progress::Line::PostProcess { started: false, .. }) | None => {}
                },
            })
            .await;

        let failure = match result {
            Ok(log) => {
                // An earlier try may have left files this one had no use for
                // (the streams picked can differ from one try to the next).
                stream_files.retain(|file| Some(file) != output_file.as_ref());
                remove_partial_files(&stream_files).await;
                let file = output_file.ok_or_else(|| {
                    AppError::internal("The download finished but the file couldn't be located.")
                        .with_details(log.clone())
                })?;
                // yt-dlp wrote it under this download's own marker, or into its work folder.
                return self.check_section(recipe, Made(file), &log).await;
            }
            Err(failure) => failure,
        };

        let leftovers = Leftovers {
            stream_files,
            others: earlier_files.into_iter().chain(part_file).collect(),
            work: recipe.needs_ffmpeg_pass().then(|| output_dir.to_path_buf()),
        };
        if let Failure::Stopped(Stop::Cancel) = failure {
            leftovers.remove().await;
            return Err(AppError::cancelled());
        }
        // Kept for the next try, automatic or by hand, to carry on from.
        self.keep_leftovers(job.id, leftovers);
        match failure {
            Failure::Stopped(_) => Err(AppError::cancelled()),
            Failure::Exited(log) => Err(sessions.explain(errors::classify(&log), url, signed_in)),
            Failure::Spawn(error) => Err(error),
        }
    }

    /// A trimmed part is fetched by ffmpeg, which gives up on a connection
    /// that stops sending (`-rw_timeout`, see `ytdlp::args`) but then ends as
    /// if the part were complete, and yt-dlp reports success. ffmpeg's own
    /// messages in the log tell: a part cut short is removed, and the
    /// download is tried again like any dropped connection.
    async fn check_section(&self, recipe: &Recipe, file: Made, log: &str) -> Result<Made> {
        if recipe.trim.is_none() || !errors::section_cut_short(log) {
            return Ok(file);
        }
        let _ = tokio::fs::remove_file(file.path()).await;
        Err(AppError::new(
            ErrorKind::Network,
            "The site stopped sending the video partway. Try again; it usually works the second time.",
        )
        .with_details(log))
    }

    /// Claims `finished` for this download. False when it is taken: a file
    /// has the name, or another running download is about to write it.
    fn claim(&self, finished: &Path, id: &str) -> bool {
        let mut claimed = self.claimed.lock().unwrap();
        let other = claimed.get(finished).is_some_and(|owner| owner != id);
        if other || finished.exists() {
            return false;
        }
        claimed.insert(finished.to_path_buf(), id.to_string());
        true
    }

    /// The first copy number whose name is free:
    /// `Title [id].mp4` → `Title [id] (2).mp4`, `(3)`, …
    fn free_copy(&self, target: &Path, ext: &str) -> u32 {
        let stem = target.file_stem().and_then(|stem| stem.to_str()).unwrap_or("video");
        let claimed = self.claimed.lock().unwrap();
        (2..)
            .find(|n| {
                let candidate = target.with_file_name(format!("{stem} ({n}).{ext}"));
                !candidate.exists() && !claimed.contains_key(&candidate)
            })
            .expect("some number is free")
    }

    /// `output`, or the first "(2)", "(3)", … variant of it that no file or
    /// running download has, claimed for this download. Siphon names every
    /// finished file itself, so nothing already in the folder is replaced.
    fn unique_output(&self, output: PathBuf, id: &str) -> PathBuf {
        if self.claim(&output, id) {
            return output;
        }
        let ext = output
            .extension()
            .and_then(|ext| ext.to_str())
            .unwrap_or("mp4")
            .to_string();
        let copy = self.free_copy(&output, &ext);
        let stem = output.file_stem().and_then(|stem| stem.to_str()).unwrap_or("video");
        let unique = output.with_file_name(format!("{stem} ({copy}).{ext}"));
        self.claim(&unique, id);
        unique
    }

    /// Gives a file yt-dlp finished its final name: the download's marker comes
    /// off, and a name that is taken gets the next free number. Same folder,
    /// so it is a rename, however large the file.
    async fn publish(&self, downloaded: Made, id: &str) -> Result<PathBuf> {
        let downloaded = downloaded.path();
        let stem = downloaded.file_stem().and_then(|stem| stem.to_str()).unwrap_or("video");
        let ext = downloaded.extension().and_then(|ext| ext.to_str()).unwrap_or("mp4");
        let wanted = downloaded.with_file_name(format!("{}.{ext}", args::finished_stem(stem)));
        let output = self.unique_output(wanted, id);
        tokio::fs::rename(downloaded, &output).await.map_err(|err| {
            AppError::internal("The download finished, but the file couldn't be given its name.").with_details(format!(
                "{} → {}: {err}",
                downloaded.display(),
                output.display()
            ))
        })?;
        Ok(output)
    }

    /// Step 3: Siphon's ffmpeg pass, for recipes that convert video or audio
    /// or add a subtitle track (`subtitles`, the prepared file).
    /// A size-targeted encode on the CPU runs twice; its first pass writes
    /// what it measured to the work folder.
    #[allow(clippy::too_many_arguments)]
    async fn convert(
        &self,
        job: &JobContext<'_>,
        recipe: &Recipe,
        input: &Path,
        output: &Path,
        work: &Path,
        tracker: &mut Tracker,
        subtitles: Option<&Path>,
    ) -> Result<()> {
        let ffmpeg = job.bins.ffmpeg();
        let _encoding = recipe.encodes_video().then(Encoding::begin);
        let log = work.join("passlog");
        let passes = if recipe.video_encode().is_some_and(|encode| encode.two_pass()) {
            vec![Pass::First { log: &log }, Pass::Second { log: &log }]
        } else {
            vec![Pass::Only]
        };
        let share = 1.0 / passes.len() as f64;

        if recipe.chat.is_some() {
            tracker.set_pass_stage(Stage::DrawingChat);
        }
        job.emit(tracker.on_convert(0.0));
        let audio_shift = self.audio_shift(job, recipe, input).await?;
        if let Some(step) = &recipe.chat {
            return self
                .draw_chat(
                    job,
                    recipe,
                    step,
                    input,
                    output,
                    work,
                    audio_shift,
                    subtitles,
                    |done, written| {
                        if let Some(written) = written {
                            tracker.heading_for(written, done);
                        }
                        job.emit(tracker.on_convert(done))
                    },
                )
                .await;
        }
        for (index, pass) in passes.into_iter().enumerate() {
            // The first of two passes writes no file.
            let writes = !matches!(pass, Pass::First { .. });
            let mut written = None;
            let mut cmd = process::command(&ffmpeg);
            // Subtitles drawn into the picture are read from here, by name.
            cmd.current_dir(work);
            let args = ffmpeg::convert_args_shifted(recipe, input, output, pass, audio_shift, subtitles, None);
            // A part of a whole file is read from a little before it (see `plan::ReadFrom`).
            let to = recipe.cut().map_or(0.0, |cut| cut.end);
            cmd.args(smart_cut::reading_from(args, input, recipe.read_from, to));
            let result = self
                .run_child(job.bins, job.id, cmd, |event| {
                    if let ChildEvent::Line(line) = event {
                        if let Some(bytes) = ffmpeg::parse_written(line) {
                            written = Some(bytes);
                        }
                        if let (Some(done), Some(total)) = (ffmpeg::parse_progress(line), recipe.duration) {
                            if let (true, Some(bytes)) = (writes, written) {
                                tracker.heading_for(bytes, done / total);
                            }
                            job.emit(tracker.on_convert((index as f64 + done / total) * share));
                        }
                    }
                })
                .await;
            match result {
                Ok(_) => {}
                Err(Failure::Stopped(_)) => return Err(AppError::cancelled()),
                Err(Failure::Exited(log)) => {
                    let what = if job.converts_file {
                        "Converting the file failed."
                    } else {
                        "The video downloaded, but converting it failed."
                    };
                    return Err(AppError::internal(what).with_details(log));
                }
                Err(Failure::Spawn(error)) => return Err(error),
            }
        }
        Ok(())
    }

    /// Runs a child process for a job: registers it so the job can be
    /// stopped, feeds each stdout line (and a tick four times a second) to
    /// `on_event`, and returns the stderr log on success.
    async fn run_child(
        &self,
        bins: &BinaryManager,
        id: &str,
        mut cmd: Command,
        mut on_event: impl FnMut(ChildEvent<'_>),
    ) -> std::result::Result<String, Failure> {
        // Stopped between steps: don't start the next one.
        if let Some(stop) = self.stop_of(id) {
            return Err(Failure::Stopped(stop));
        }
        // Keeps a tool update from swapping the binaries while this runs.
        let _in_use = bins.in_use().await;
        // Waiting for an update can take seconds: the job may have been stopped meanwhile.
        if let Some(stop) = self.stop_of(id) {
            return Err(Failure::Stopped(stop));
        }
        if self.jobs.lock().unwrap().get(id).is_some_and(|job| job.gentle) {
            process::below_others(&mut cmd);
        }
        let mut child = cmd
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|err| {
                Failure::Spawn(AppError::setup("Couldn't start the download engine.").with_details(err.to_string()))
            })?;
        // Whatever the command was given as its input (a pipe Siphon writes
        // into) is now the process's alone: when it ends, the writer hears so.
        drop(cmd);
        // Ends with Siphon even if Siphon crashes, rather than writing on alone.
        process::end_with_siphon(&child);
        // Registering the process and reading the stop share one lock with `stop`, so a
        // stop that came just before the process existed is caught here instead of lost.
        let stopped_meanwhile = match self.jobs.lock().unwrap().get_mut(id) {
            Some(job) => {
                job.pids.extend(child.id());
                job.stop.is_some()
            }
            None => false,
        };
        // Forgets the process however this ends, also if the caller stops
        // waiting for it (which ends it: see `process::command`).
        let _registered = Registered {
            manager: self,
            id,
            pid: child.id(),
        };
        if stopped_meanwhile {
            if let Some(pid) = child.id() {
                process::kill_tree(pid).await;
            }
        }

        let stderr_log = tokio::spawn(collect_lines(child.stderr.take().expect("stderr is piped")));
        let mut stdout = BufReader::new(child.stdout.take().expect("stdout is piped"));
        let mut ticker = tokio::time::interval(Duration::from_millis(250));
        let mut buffer = Vec::new();
        loop {
            tokio::select! {
                // If the tick wins, the bytes read so far stay in `buffer` and
                // the next read carries on from there, so no output is lost.
                read = stdout.read_until(b'\n', &mut buffer) => match read {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {
                        // Lossy on purpose: a stray byte in a title must not end the download.
                        on_event(ChildEvent::Line(&String::from_utf8_lossy(&buffer)));
                        buffer.clear();
                    }
                },
                _ = ticker.tick() => on_event(ChildEvent::Tick),
            }
        }

        let status = child.wait().await;
        let log = stderr_log.await.unwrap_or_default();
        let interrupting = self
            .jobs
            .lock()
            .unwrap()
            .get_mut(id)
            .and_then(|job| job.interrupting.take());
        // Asked to stop gently: what it started (ffmpeg, writing the file) may
        // outlive it by a moment. Nothing is done with the file until it has gone.
        if let Some(interrupting) = interrupting {
            let _ = interrupting.await;
        }

        if let Some(stop) = self.stop_of(id) {
            Err(Failure::Stopped(stop))
        } else if status.is_ok_and(|status| status.success()) {
            Ok(log)
        } else {
            Err(Failure::Exited(log))
        }
    }
}

/// A child process on its job's list, taken off when this goes.
struct Registered<'a> {
    manager: &'a DownloadManager,
    id: &'a str,
    pid: Option<u32>,
}

impl Drop for Registered<'_> {
    fn drop(&mut self) {
        let Some(pid) = self.pid else { return };
        if let Some(job) = self.manager.jobs.lock().unwrap().get_mut(self.id) {
            job.pids.retain(|other| *other != pid);
        }
    }
}

/// What `run_child` reports while the process runs.
enum ChildEvent<'a> {
    /// One line of stdout.
    Line(&'a str),
    /// A regular heartbeat, for progress that has to be polled.
    Tick,
}

/// How a child process run can go wrong.
enum Failure {
    Spawn(AppError),
    /// The user cancelled or paused the job.
    Stopped(Stop),
    /// Non-zero exit; carries the stderr log.
    Exited(String),
}

/// Asks the process group `pid` heads to stop, with Siphon's own executable
/// as the helper that does it (see `process::interrupt_tree`).
fn interrupt(pid: u32) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        match std::env::current_exe() {
            Ok(helper) => process::interrupt_tree(&helper, pid, live::STOP_GRACE).await,
            Err(_) => process::kill_tree(pid).await,
        }
    })
}

/// Where a smart cut is joined as MP4 before it is put in another file type.
const SMART_CUT_FILE: &str = "smart-cut.mp4";

/// A file's own subtitle tracks for a smart cut, cut and starting at zero
/// like the clip, in the work folder until they go in with it (see
/// `smart_cut::subtitles_args`).
const SUBTITLES_PIECE: &str = "subtitles.mks";

/// How long to wait before asking for a subtitle track a second time.
const SUBTITLE_RETRY: Duration = Duration::from_secs(5);

/// The subtitle file yt-dlp fetched into `work`, if there is one:
/// `subtitles-source.<key>.srt`, or `.ass` kept styled.
/// An error in one line for the log: its message and yt-dlp's own last error.
fn failure_summary(error: &AppError) -> String {
    let said = error
        .details
        .as_deref()
        .unwrap_or_default()
        .lines()
        .rev()
        .find(|line| line.contains("ERROR"))
        .map(|line| format!(" ({})", line.trim()))
        .unwrap_or_default();
    format!("{:?}: {}{said}", error.kind, error.message)
}

/// Where a download's saved lookup is written for yt-dlp, in its work folder.
const INFO_JSON: &str = "lookup.info.json";

/// Writes the video's saved lookup into `work` for yt-dlp to start from, if
/// it is young enough (see `ytdlp::saved`), with a whole YouTube video's
/// streams in pieces, and without the cookies of a signed-in lookup. `None`:
/// yt-dlp looks the video up itself.
async fn saved_lookup(video: &VideoInfo, recipe: &Recipe, work: &Path) -> Option<PathBuf> {
    let saved = video.saved.as_ref()?;
    // A trimmed part is fetched by ffmpeg from the stream's own link, as a plain file.
    let pieces: Vec<&str> = if recipe.trim.is_none() {
        let video = recipe.video.as_ref().map(|step| step.format_id.as_str());
        let audio = recipe.audio.as_ref().and_then(|step| step.format_id.as_deref());
        video.into_iter().chain(audio).collect()
    } else {
        Vec::new()
    };
    let json = saved.for_download(&pieces, video.with_account)?;
    let path = work.join(INFO_JSON);
    tokio::fs::write(&path, json).await.ok()?;
    Some(path)
}

/// Points yt-dlp at what to download: the saved lookup if there is one,
/// otherwise the link, after `--` so it can't be read as an option.
fn from_lookup(cmd: &mut Command, info_json: Option<&Path>, url: &str) {
    match info_json {
        Some(path) => cmd.arg("--load-info-json").arg(path),
        None => cmd.arg("--").arg(url),
    };
}

async fn fetched_subtitles(work: &Path) -> Option<PathBuf> {
    let mut entries = tokio::fs::read_dir(work).await.ok()?;
    while let Ok(Some(entry)) = entries.next_entry().await {
        let name = entry.file_name().to_string_lossy().into_owned();
        let ours = name.starts_with(&format!("{}.", args::SUBTITLE_SOURCE));
        if ours && [".srt", ".ass", ".ssa"].iter().any(|ext| name.ends_with(ext)) {
            return Some(entry.path());
        }
    }
    None
}

/// How many of Siphon's own encodes are running: conversions, and chat
/// being drawn in. Measuring a video's size (see `crate::sizing`) waits
/// until none is, rather than slow one down on the same encoder.
static ENCODING: AtomicUsize = AtomicUsize::new(0);

/// Counts an encode for as long as it is kept.
pub(crate) struct Encoding;

impl Encoding {
    pub(crate) fn begin() -> Self {
        ENCODING.fetch_add(1, Ordering::SeqCst);
        Encoding
    }
}

impl Drop for Encoding {
    fn drop(&mut self) {
        ENCODING.fetch_sub(1, Ordering::SeqCst);
    }
}

/// True while any of Siphon's own encodes runs.
pub fn encoding_now() -> bool {
    ENCODING.load(Ordering::SeqCst) > 0
}

/// Bytes free for the current user on the disk `dir` is on.
pub fn free_space(dir: &Path) -> Option<u64> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    let wide: Vec<u16> = dir.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
    let mut free = 0u64;
    // SAFETY: a null-terminated path and pointers to a local; the other two outputs may be null.
    let ok = unsafe { GetDiskFreeSpaceExW(wide.as_ptr(), &mut free, std::ptr::null_mut(), std::ptr::null_mut()) };
    (ok != 0).then_some(free)
}

pub fn default_output_dir(app: &AppHandle) -> Result<PathBuf> {
    app.path()
        .download_dir()
        .map_err(|err| AppError::internal("Couldn't find your Downloads folder.").with_details(err.to_string()))
}

/// Where a download keeps a source that is converted afterwards:
/// `<app cache>/work/<download id>/`.
fn work_dir(app: &AppHandle, id: &str) -> Result<PathBuf> {
    work_root(app).map(|root| root.join(id))
}

fn work_root(app: &AppHandle) -> Result<PathBuf> {
    app.path()
        .app_cache_dir()
        .map(|dir| dir.join("work"))
        .map_err(|err| AppError::internal("Couldn't locate the cache folder.").with_details(err.to_string()))
}

/// Removes work folders a crash left behind. Run at launch; folders from the
/// last day are kept as a margin, and so are those of paused or failed
/// downloads (`kept`), which a later try carries on from.
pub fn sweep_work(app: &AppHandle, kept: &HashSet<String>) {
    let Ok(Ok(entries)) = work_root(app).map(std::fs::read_dir) else {
        return;
    };
    for entry in entries.flatten() {
        if entry.file_name().to_str().is_some_and(|name| kept.contains(name)) {
            continue;
        }
        let stale = entry
            .metadata()
            .and_then(|meta| meta.modified())
            .ok()
            .and_then(|modified| modified.elapsed().ok())
            .is_some_and(|age| age > STALE_WORK);
        if stale {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

/// `work/…/Title [id].source.mkv` → `<output_dir>/Title [id].mp4`
///
/// The one place a converted file's name is decided; giving a second download
/// of the same video its own name would start here.
fn converted_path(source: &Path, recipe: &Recipe, output_dir: &Path) -> PathBuf {
    let stem = source.file_stem().and_then(|stem| stem.to_str()).unwrap_or("video");
    output_dir.join(format!("{}.{}", args::finished_stem(stem), recipe.container.ext()))
}

/// `Title [id].mp4` → `Title [id].converting.mp4`: where a conversion writes
/// until the file is finished and checked. The extension stays, because
/// ffmpeg picks the format from it.
fn partial_path(output: &Path) -> PathBuf {
    let stem = output.file_stem().and_then(|stem| stem.to_str()).unwrap_or("video");
    let ext = output.extension().and_then(|ext| ext.to_str()).unwrap_or("mp4");
    output.with_file_name(format!("{stem}.converting.{ext}"))
}

fn file_size(path: &Path) -> u64 {
    std::fs::metadata(path).map_or(0, |meta| meta.len())
}

/// Moves a file Siphon made, copying it when the destination is on another
/// drive, and says where it is now.
async fn move_file(from: Made, to: &Path) -> Result<Made> {
    if tokio::fs::rename(from.path(), to).await.is_err() {
        tokio::fs::copy(from.path(), to).await?;
        tokio::fs::remove_file(from.path()).await?;
    }
    Ok(Made(to.to_path_buf()))
}

/// Reads a stream to the end, keeping the last [`MAX_LOG_LINES`] lines.
async fn collect_lines(stream: impl AsyncRead + Unpin) -> String {
    let mut reader = BufReader::new(stream);
    let mut lines: VecDeque<String> = VecDeque::new();
    let mut buffer = Vec::new();
    loop {
        buffer.clear();
        match reader.read_until(b'\n', &mut buffer).await {
            Ok(0) | Err(_) => break,
            Ok(_) => {
                if lines.len() == MAX_LOG_LINES {
                    lines.pop_front();
                }
                lines.push_back(String::from_utf8_lossy(&buffer).trim_end().to_string());
            }
        }
    }
    Vec::from(lines).join("\n")
}

/// Removes what a stopped download left on disk: the files yt-dlp
/// downloaded streams to, with their `.part`, `.ytdl` and fragment
/// (`-Frag12`) companions. They all carry the download's own marker, so none
/// of them can be a file that was in the folder before.
pub(super) async fn remove_partial_files(stream_files: &[PathBuf]) {
    for file in stream_files {
        let (Some(dir), Some(name)) = (file.parent(), file.file_name().and_then(|name| name.to_str())) else {
            continue;
        };
        let Ok(mut entries) = tokio::fs::read_dir(dir).await else {
            continue;
        };
        while let Ok(Some(entry)) = entries.next_entry().await {
            let entry_name = entry.file_name();
            let Some(entry_name) = entry_name.to_str() else {
                continue;
            };
            let is_companion = entry_name
                .strip_prefix(name)
                .is_some_and(|rest| rest.starts_with(".part") || rest.starts_with("-Frag") || rest == ".ytdl");
            if is_companion || entry_name == name {
                let _ = tokio::fs::remove_file(entry.path()).await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::codecs::Container;
    use crate::plan::{Badge, Recipe};

    fn recipe(container: Container) -> Recipe {
        Recipe {
            video: None,
            audio: None,
            container,
            strip_audio: false,
            reason: None,
            promise: String::new(),
            hints: Vec::new(),
            duration: None,
            trim: None,
            source_fraction: 1.0,
            badge: Badge::Lossless,
            size_limit: None,
            fits: None,
            blocked: None,
            section_request_size: None,
            fallback: None,
            cut_encode: None,
            subtitles: None,
            chat: None,
            measured: Default::default(),
            read_from: None,
            tracks: Default::default(),
        }
    }

    #[test]
    fn a_converted_file_lands_in_the_download_folder_without_the_source_marker() {
        let path = converted_path(
            Path::new("/cache/work/1/A clip [abc].source.mkv"),
            &recipe(Container::Mp4),
            Path::new("/videos"),
        );
        assert_eq!(path, Path::new("/videos/A clip [abc].mp4"));
        assert_eq!(partial_path(&path), Path::new("/videos/A clip [abc].converting.mp4"));
    }
}

#[cfg(test)]
mod naming_tests {
    use super::*;

    #[test]
    fn playlist_numbers_have_as_many_digits_as_the_playlist_needs() {
        let number = |index, count| {
            Place {
                folder: None,
                number: Some((index, count)),
            }
            .prefix()
        };
        assert_eq!(number(7, 9), "07 - ");
        assert_eq!(number(7, 95), "07 - ");
        assert_eq!(number(7, 182), "007 - ");
        assert_eq!(number(1218, 1218), "1218 - ");
        assert_eq!(Place::default().prefix(), "");
    }

    #[test]
    fn a_playlist_title_becomes_a_folder_windows_accepts() {
        assert_eq!(
            folder_name("Blender Conference 2026").as_deref(),
            Some("Blender Conference 2026")
        );
        assert_eq!(folder_name("AC/DC: Live? <Best>").as_deref(), Some("AC DC Live Best"));
        assert_eq!(folder_name("  Ends with dots... ").as_deref(), Some("Ends with dots"));
        assert_eq!(folder_name("..").as_deref(), None);
        assert_eq!(folder_name(r"..\..\Windows").as_deref(), Some("Windows"));
        assert_eq!(folder_name("con").as_deref(), Some("con_"));
        assert_eq!(folder_name("COM1.mix").as_deref(), Some("COM1.mix_"));
        assert_eq!(folder_name("Comedy").as_deref(), Some("Comedy"));
        assert_eq!(
            folder_name(&"x".repeat(300)).map(|name| name.chars().count()),
            Some(100)
        );
    }

    #[test]
    fn each_kind_of_passing_failure_gets_its_own_tries() {
        let mut retries = Retries::default();
        // A refused request (403), then a site limiting requests (429): the
        // second still gets its longer wait.
        assert_eq!(retries.next(Retry::Soon), Some(Duration::from_secs(2)));
        assert_eq!(retries.next(Retry::Later), Some(Duration::from_secs(30)));
        assert_eq!(retries.next(Retry::Soon), Some(Duration::from_secs(10)));
        // Each schedule ends on its own, and what won't pass is never tried again.
        assert_eq!(retries.next(Retry::Soon), None);
        assert_eq!(retries.next(Retry::Later), None);
        assert_eq!(Retries::default().next(Retry::No), None);
    }

    #[test]
    fn a_taken_name_gets_the_next_free_number() {
        let dir = std::env::temp_dir().join(format!("siphon-names-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = |name: &str| dir.join(name);
        std::fs::write(file("Clip [x].mp4"), b"").unwrap();
        std::fs::write(file("Clip [x] (2).mp4"), b"").unwrap();
        let manager = DownloadManager::default();

        // Taken on disk.
        assert!(!manager.claim(&file("Clip [x].mp4"), "a"));
        assert_eq!(manager.free_copy(&file("Clip [x].mp4"), "mp4"), 3);
        // Taken by another running download, but free for the one holding it.
        assert!(manager.claim(&file("Other.mp4"), "a"));
        assert!(!manager.claim(&file("Other.mp4"), "b"));
        assert!(manager.claim(&file("Other.mp4"), "a"));
        assert_eq!(manager.unique_output(file("Other.mp4"), "b"), file("Other (2).mp4"));

        std::fs::remove_dir_all(&dir).unwrap();
    }
}

#[cfg(test)]
mod saved_state_tests {
    use super::*;

    #[tokio::test]
    async fn paused_downloads_and_finished_files_outlive_a_restart() {
        let dir = std::env::temp_dir().join(format!("siphon-state-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let state = dir.join("downloads.json");
        let finished = dir.join("Clip.mp4");
        let partial = dir.join("Other.siphon-abc.mp4");
        std::fs::write(&finished, b"done").unwrap();
        std::fs::write(&partial, b"half").unwrap();

        let session = DownloadManager::from_file(state.clone());
        session.keep_leftovers(
            "paused",
            Leftovers {
                stream_files: vec![partial.clone()],
                ..Leftovers::default()
            },
        );
        session.finished.lock().unwrap().insert(finished.clone());
        session.save();

        // The next session carries on from the paused download's files, and can open the finished one.
        let next = DownloadManager::from_file(state.clone());
        assert!(next.kept_ids().contains("paused"));
        assert!(next.produced(&finished));

        // A download the list no longer has loses its files; a finished file that was deleted is forgotten.
        next.forget_all_but(&HashSet::new()).await;
        assert!(!partial.exists());
        std::fs::remove_file(&finished).unwrap();
        let after = DownloadManager::from_file(state);
        assert!(after.kept_ids().is_empty());
        assert!(!after.produced(&finished));

        std::fs::remove_dir_all(&dir).unwrap();
    }
}

#[cfg(test)]
mod proxy_tests {
    use std::net::SocketAddr;

    use tokio::net::TcpStream;

    use super::*;

    /// A job as `start` sets it up once it has a slot, with its proxy listening.
    async fn running_job(manager: &DownloadManager, throttle: &Throttle, id: &str) -> SocketAddr {
        manager.register(id, true);
        let flow = throttle.flow().await.unwrap();
        let address = flow.address();
        manager.attach_flow(id, flow);
        assert!(listening(address).await, "the proxy should listen while the job runs");
        address
    }

    async fn listening(address: SocketAddr) -> bool {
        // Let an aborted proxy task finish closing first.
        for _ in 0..10 {
            tokio::task::yield_now().await;
        }
        TcpStream::connect(address).await.is_ok()
    }

    #[tokio::test]
    async fn the_proxy_closes_however_a_download_ends() {
        let manager = DownloadManager::default();
        let throttle = Throttle::new(None);

        // Finished, or failed: `run` returned, and the job ends with no stop.
        for id in ["finished", "failed"] {
            let address = running_job(&manager, &throttle, id).await;
            assert_eq!(manager.end_job(id), None);
            assert!(!listening(address).await, "{id}: the proxy is still listening");
        }

        // Paused: its files are kept for later, but not its proxy.
        let address = running_job(&manager, &throttle, "paused").await;
        manager.pause("paused").await;
        manager.keep_leftovers("paused", Leftovers::default());
        assert_eq!(manager.end_job("paused"), Some(Stop::Pause));
        assert!(!listening(address).await, "paused: the proxy is still listening");

        // Cancelled.
        let address = running_job(&manager, &throttle, "cancelled").await;
        manager.cancel("cancelled").await;
        assert_eq!(manager.end_job("cancelled"), Some(Stop::Cancel));
        assert!(!listening(address).await, "cancelled: the proxy is still listening");

        // A job that has ended can't download any more: never around the limit.
        let mut cmd = Command::new("yt-dlp");
        assert!(manager.route("finished", &mut cmd).is_err());
    }

    #[tokio::test]
    async fn quitting_closes_every_proxy_even_of_a_download_that_wont_stop() {
        let manager = DownloadManager::default();
        let throttle = Throttle::new(None);
        // Two downloads still running when the app quits, whose jobs never get to end.
        let first = running_job(&manager, &throttle, "a").await;
        let second = running_job(&manager, &throttle, "b").await;

        manager.shut_down().await;
        assert!(!listening(first).await);
        assert!(!listening(second).await);
    }
}

#[cfg(test)]
mod cookie_tests {
    use super::*;
    use crate::sessions::cookies::Cookie;
    use crate::sessions::Method;

    /// A job of a signed-in site, as `fetch_streams` sets it up: returns its cookie file.
    fn running_job(manager: &DownloadManager, sessions: &Arc<Sessions>, id: &str) -> PathBuf {
        manager.register(id, true);
        let file = sessions.cookie_file("https://www.youtube.com/watch?v=abc").unwrap();
        let path = file.path().to_path_buf();
        assert!(manager.attach_cookies(id, Some(file)));
        let mut cmd = Command::new("yt-dlp");
        manager.add_cookies(id, &mut cmd);
        assert!(cmd.as_std().get_args().any(|arg| arg == "--cookies"));
        assert!(path.exists());
        path
    }

    #[tokio::test]
    async fn the_cookie_file_goes_however_a_download_ends() {
        let dir = std::env::temp_dir().join(format!("siphon-test-job-cookies-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let sessions = Sessions::in_dir(dir.clone());
        let fake = Cookie {
            domain: "youtube.com".into(),
            include_subdomains: true,
            path: "/".into(),
            secure: true,
            http_only: true,
            expires: 0,
            name: "SID".into(),
            value: "fake-test-value".into(),
        };
        sessions.save("youtube.com", vec![fake], Method::Window).unwrap();
        let manager = DownloadManager::default();

        for id in ["finished", "failed"] {
            let file = running_job(&manager, &sessions, id);
            manager.end_job(id);
            assert!(!file.exists(), "{id}: the cookie file is still there");
        }

        let file = running_job(&manager, &sessions, "paused");
        manager.pause("paused").await;
        manager.end_job("paused");
        assert!(!file.exists(), "paused: the cookie file is still there");

        let file = running_job(&manager, &sessions, "cancelled");
        manager.cancel("cancelled").await;
        manager.end_job("cancelled");
        assert!(!file.exists(), "cancelled: the cookie file is still there");

        // Quitting, with downloads whose jobs never get to end.
        let first = running_job(&manager, &sessions, "a");
        let second = running_job(&manager, &sessions, "b");
        manager.shut_down().await;
        assert!(
            !first.exists() && !second.exists(),
            "quit: a cookie file is still there"
        );

        // A site Siphon isn't signed in to runs without one.
        manager.register("other", true);
        assert!(!manager.attach_cookies("other", sessions.cookie_file("https://vimeo.com/1")));
        let _ = std::fs::remove_dir_all(dir);
    }
}
