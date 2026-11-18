//! Runs downloads and streams their progress to the frontend.
//!
//! A job waits in the queue for a free slot, then has up to three steps:
//! look the video up (unless it is cached), let yt-dlp fetch and merge the
//! streams the recipe names, then, only if the recipe converts anything,
//! run Siphon's own ffmpeg pass.

mod tracker;

use std::collections::{HashMap, HashSet, VecDeque};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};
use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};
use tokio::process::Command;
use tokio::sync::Notify;

use crate::binaries::{BinaryManager, Tool};
use crate::error::{AppError, ErrorKind, Result, Retry};
use crate::ffmpeg::smart_cut::{self, AudioPiece, Fallback};
use crate::ffmpeg::{self, encoders::Hardware, Pass};
use crate::log;
use crate::plan::codecs::VideoCodec;
use crate::plan::{self, AudioAction, Choice, Recipe, Trim};
use crate::process;
use crate::settings::{Settings, SettingsStore};
use crate::state::{CapabilityCache, MediaCache};
use crate::throttle::{self, Throttle};
use crate::ytdlp::metadata::{self, Media, VideoInfo};
use crate::ytdlp::{self, args, errors, progress};
use tracker::{Progress, Tracker};

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
        .map(|c| if c.is_control() || r#"<>:"/\|?*"#.contains(c) { ' ' } else { c })
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

/// Why the user stopped a job.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stop {
    Cancel,
    Pause,
}

#[derive(Default)]
struct Job {
    /// The child process currently working for this job, if any.
    pid: Option<u32>,
    stop: Option<Stop>,
    /// The job's way through the speed limit, while it runs. It goes with the
    /// job (see `end_job`), whether that finished, failed, was paused or was
    /// cancelled, which closes its proxy listener and connections.
    flow: Option<throttle::Flow>,
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
}

/// Everything one running job needs.
struct JobContext<'a> {
    app: &'a AppHandle,
    bins: &'a BinaryManager,
    id: &'a str,
    settings: Settings,
    /// A playlist video's folder and number; empty for anything else.
    place: Place,
}

impl JobContext<'_> {
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
            let view = serde_json::json!({ "leftovers": &*leftovers, "finished": &*finished });
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

    /// The download ids whose files are kept for a later try.
    pub fn kept_ids(&self) -> HashSet<String> {
        self.leftovers.lock().unwrap().keys().cloned().collect()
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
    pub fn start(self: &Arc<Self>, app: AppHandle, request: DownloadRequest) -> Result<()> {
        let url = ytdlp::validate_url(&request.url)?.to_string();
        let id = request.id;
        self.register(&id);

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

            let outcome = if manager.wait_for_slot(&app, &id).await {
                let outcome = match app.state::<Arc<Throttle>>().flow().await {
                    Ok(flow) => {
                        manager.attach_flow(&id, flow);
                        let job = JobContext {
                            app: &app,
                            bins: &bins,
                            id: &id,
                            // Read once the job starts, so changes made while it queued apply.
                            settings: app.state::<SettingsStore>().get(),
                            place: request.place.clone().unwrap_or_default(),
                        };
                        manager.run(&job, &url, &request.choice, request.trim).await
                    }
                    Err(error) => Err(error),
                };
                manager.gate.lock().unwrap().running -= 1;
                manager.gate_changed.notify_waiters();
                outcome
            } else {
                Err(AppError::cancelled())
            };

            let stop = manager.end_job(&id);
            if stop == Some(Stop::Cancel) {
                // Including what an earlier try left, if it was cancelled between tries.
                manager.discard_leftovers(&id).await;
            }
            let event = match (stop, outcome) {
                (Some(Stop::Cancel), _) => DownloadEvent::Cancelled { id },
                (Some(Stop::Pause), _) => DownloadEvent::Paused { id },
                (None, Ok(path)) => {
                    manager.finished.lock().unwrap().insert(path.clone());
                    manager.save();
                    DownloadEvent::Finished {
                        size: std::fs::metadata(&path).ok().map(|meta| meta.len()),
                        path: path.to_string_lossy().into_owned(),
                        id,
                    }
                }
                (None, Err(error)) => DownloadEvent::Failed { id, error },
            };
            let _ = app.emit(EVENT, event);
        });
        Ok(())
    }

    /// Adds a job to the list and to the back of the queue.
    fn register(&self, id: &str) {
        self.jobs.lock().unwrap().insert(id.to_string(), Job::default());
        self.gate.lock().unwrap().waiting.push_back(id.to_string());
    }

    /// Gives a job that is about to run its way through the speed limit.
    fn attach_flow(&self, id: &str, flow: throttle::Flow) {
        if let Some(job) = self.jobs.lock().unwrap().get_mut(id) {
            job.flow = Some(flow);
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

    async fn discard_leftovers(&self, id: &str) {
        let leftovers = self.leftovers.lock().unwrap().remove(id);
        if let Some(leftovers) = leftovers {
            self.save();
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
        // A job that hasn't ended by now still loses its proxy: no listener
        // is left open, whatever the job is stuck on.
        for job in self.jobs.lock().unwrap().values_mut() {
            job.flow = None;
        }
        self.save();
    }

    /// Returns false if there is no such job.
    async fn stop(&self, id: &str, stop: Stop) -> bool {
        let pid = {
            let mut jobs = self.jobs.lock().unwrap();
            let Some(job) = jobs.get_mut(id) else { return false };
            job.stop = Some(stop);
            job.pid
        };
        // Wakes the job if it is still waiting in the queue …
        self.gate_changed.notify_waiters();
        // … and ends its process if it is running.
        if let Some(pid) = pid {
            process::kill_tree(pid).await;
        }
        true
    }

    fn stop_of(&self, id: &str) -> Option<Stop> {
        self.jobs.lock().unwrap().get(id).and_then(|job| job.stop)
    }

    /// Runs the whole job and returns the path of the finished file.
    async fn run(&self, job: &JobContext<'_>, url: &str, choice: &Choice, trim: Option<Trim>) -> Result<PathBuf> {
        job.emit(Tracker::new(false).current());

        let output_dir = match &job.settings.output_dir {
            Some(dir) => dir.clone(),
            None => default_output_dir(job.app)?,
        };
        let output_dir = match job.place.folder.as_deref().and_then(folder_name) {
            Some(folder) => output_dir.join(folder),
            None => output_dir,
        };
        tokio::fs::create_dir_all(&output_dir).await.map_err(|err| {
            AppError::internal("Couldn't create the download folder. Check the folder in Settings.")
                .with_details(format!("{}: {err}", output_dir.display()))
        })?;

        let mut updated = false;
        let mut retries = 0;
        // Look the video up again instead of trusting what this session remembers.
        let mut refresh = false;
        loop {
            let error = match self.attempt(job, url, choice, trim, &output_dir, refresh).await {
                Ok(path) => return Ok(path),
                Err(error) => error,
            };
            if self.stop_of(job.id).is_some() {
                return Err(error);
            }

            // Sites change, and yt-dlp follows within days. If the failure
            // looks like that, get the newest yt-dlp and try again with it.
            if error.kind == ErrorKind::Outdated {
                // The formats looked up earlier may be the problem, whether
                // the site has changed since or an older yt-dlp read them.
                refresh = true;
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
            let delays = match error.retry {
                Retry::No => return Err(error),
                Retry::Soon => RETRY_SOON,
                Retry::Later => RETRY_LATER,
            };
            let Some(delay) = delays.get(retries) else {
                return Err(error);
            };
            retries += 1;
            job.emit(Tracker::retrying());
            if !self.wait_unless_stopped(job.id, *delay).await {
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

    /// One pass through the job's steps: look up, download, convert.
    async fn attempt(
        &self,
        job: &JobContext<'_>,
        url: &str,
        choice: &Choice,
        trim: Option<Trim>,
        output_dir: &Path,
        refresh: bool,
    ) -> Result<PathBuf> {
        let video = self.video_info(job, url, refresh).await?;
        let caps = job.app.state::<CapabilityCache>().get(job.bins).await;
        let recipe = plan::plan(&video, choice, trim, &caps)?;
        if let Some(reason) = &recipe.blocked {
            return Err(AppError::new(ErrorKind::TooLong, reason.clone()));
        }
        let mut tracker = Tracker::new(recipe.needs_ffmpeg_pass());

        // A source that is converted afterwards goes to the download's own work
        // folder, and stays there until the finished file is written and checked.
        let work = work_dir(job.app, job.id)?;
        let fetch_dir = if recipe.needs_ffmpeg_pass() { &work } else { output_dir };
        tokio::fs::create_dir_all(fetch_dir).await?;
        let downloaded = self
            .fetch_streams(job, &video.url, &recipe, fetch_dir, &mut tracker)
            .await?;

        let (recipe, source) = if recipe.needs_ffmpeg_pass() {
            (recipe, downloaded)
        } else {
            match (recipe.fallback.as_deref(), recipe.size_limit) {
                // Share: an original that was expected to fit but came out over
                // the limit is converted after all.
                (Some(fallback), Some(limit)) if file_size(&downloaded) > limit => {
                    let source = work.join(downloaded.file_name().unwrap_or_default());
                    tokio::fs::create_dir_all(&work).await?;
                    move_file(&downloaded, &source).await?;
                    (fallback.clone(), source)
                }
                _ => {
                    let _ = tokio::fs::remove_dir_all(&work).await;
                    return self.publish(&downloaded, job.id).await;
                }
            }
        };

        let output = self.unique_output(converted_path(&source, &recipe, output_dir), job.id);
        let result = self
            .convert_checked(job, &recipe, &source, &output, &work, &mut tracker)
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
        result.map(|()| output)
    }

    /// Step 3, checked: converts (on the CPU if the hardware encoder refuses),
    /// then makes sure a Share file is under its limit, encoding again a
    /// little smaller if it isn't. The file only gets its final name once it
    /// passes; until then it is written beside it under a temporary one.
    async fn convert_checked(
        &self,
        job: &JobContext<'_>,
        recipe: &Recipe,
        source: &Path,
        output: &Path,
        work: &Path,
        tracker: &mut Tracker,
    ) -> Result<()> {
        let partial = partial_path(output);
        let mut recipe = recipe.clone();
        let name = output.file_name().unwrap_or_default().to_string_lossy().into_owned();
        // A copied stream that is cut: re-encode only the frames at the cuts.
        // Anything the smart cut isn't sure of is re-encoded in full instead.
        if recipe.smart_cut() {
            let started = Instant::now();
            match self.smart_cut(job, &recipe, source, &partial, work, tracker).await? {
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
            let mut result = self.convert(job, &recipe, source, &partial, work, tracker).await;
            // A hardware encoder can refuse a job it passed the probe for (driver
            // limits, too many sessions). The CPU encoders always work.
            let on_hardware = recipe
                .video_encode()
                .is_some_and(|encode| encode.hardware != Hardware::Software);
            if result.is_err() && on_hardware && self.stop_of(job.id).is_none() {
                recipe = recipe.on_software();
                result = self.convert(job, &recipe, source, &partial, work, tracker).await;
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

    /// Cuts a copied stream on the exact frames, re-encoding only the frames
    /// at the cuts (see `ffmpeg::smart_cut`), and checks the result. The
    /// inner error says why it couldn't be used; the outer one is for a
    /// stopped download.
    async fn smart_cut(
        &self,
        job: &JobContext<'_>,
        recipe: &Recipe,
        source: &Path,
        output: &Path,
        work: &Path,
        tracker: &mut Tracker,
    ) -> Result<std::result::Result<String, Fallback>> {
        let pieces = work.join("cut");
        let _ = tokio::fs::remove_dir_all(&pieces).await;
        tokio::fs::create_dir_all(&pieces).await?;
        let result = self.smart_cut_in(job, recipe, source, output, &pieces, tracker).await;
        // The pieces are as large as the part itself: don't keep them.
        let _ = tokio::fs::remove_dir_all(&pieces).await;
        result
    }

    async fn smart_cut_in(
        &self,
        job: &JobContext<'_>,
        recipe: &Recipe,
        source: &Path,
        output: &Path,
        pieces: &Path,
        tracker: &mut Tracker,
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

        // Read the source and decide the pieces.
        let source_video = step!(self.packets(job, source, "v:0", pieces).await?);
        let params = smart_cut::parse_params(
            &step!(self.tool(job, &ffprobe, smart_cut::params_args(source), None).await?).join("\n"),
        );
        let encoder = step!(smart_cut::matching_args(&params, encode.hardware));
        let codec = if params.codec == "hevc" {
            VideoCodec::Hevc
        } else {
            video.source
        };
        let restarts = step!(self.restarts(job, source, codec, params.time_base, pieces).await?);
        let plan = step!(smart_cut::plan(&source_video, trim, &restarts));
        progress(tracker);

        let audio = match (&recipe.audio, recipe.strip_audio) {
            (Some(step), false) => Some(match &step.action {
                AudioAction::Copy => AudioPiece::Copy,
                AudioAction::Encode {
                    codec,
                    bitrate_kbps,
                    sample_rate,
                    stereo,
                } => AudioPiece::Encode(ffmpeg::audio_encode_args(*codec, *bitrate_kbps, *sample_rate, *stereo)),
            }),
            _ => None,
        };

        // The re-encoded start, the copied middle, the re-encoded end, the audio.
        let mut names: Vec<&str> = Vec::new();
        if plan.has_head() {
            let args = smart_cut::edge_args(
                source,
                plan.before(plan.first()),
                plan.before(plan.copy_from),
                &encoder,
                plan.delay,
                &pieces.join("head.ts"),
            );
            step!(self.tool(job, &ffmpeg, args, None).await?, "re-encoding the start");
            names.push("head.ts");
        }
        progress(tracker);
        let args = smart_cut::copy_args(
            source,
            plan.before(plan.copy_from),
            Some(plan.before(plan.copy_until)),
            codec,
            &pieces.join("middle.ts"),
        );
        step!(self.tool(job, &ffmpeg, args, None).await?, "copying the middle");
        names.push("middle.ts");
        progress(tracker);
        // The end is re-encoded from a copy of just its packets: decoding the
        // source from the start to reach them would take as long as the part.
        let rest = pieces.join("rest.ts");
        let args = smart_cut::copy_args(source, plan.before(plan.copy_until), None, codec, &rest);
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
        if let Some(piece) = &audio {
            let args = smart_cut::audio_args(source, plan.first(), plan.end(), piece, &pieces.join("audio.mka"));
            step!(self.tool(job, &ffmpeg, args, None).await?, "cutting the audio");
        }
        progress(tracker);

        // Join them, on the video's own timestamps, starting at zero.
        // Audio packets are at most a hundred a second (AAC at 48 kHz is 47).
        let audio_packets = audio
            .as_ref()
            .map_or(0, |_| ((plan.end() - plan.first()) * 100.0).ceil() as usize);
        let mut joined = Err(Fallback::NoFrames);
        // Room for the index at the front first; if it turns out too small,
        // write it at the end and move it.
        for index in [
            smart_cut::Index::room_for(plan.frames.len(), audio_packets),
            smart_cut::Index::Rewrite,
        ] {
            let args = smart_cut::join_args(
                &names,
                audio.as_ref().map(|_| "audio.mka"),
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
        let clip_video = step!(self.packets(job, output, "v:0", pieces).await?);
        step!(smart_cut::check_video(&plan, &clip_video));
        if let Some(piece) = &audio {
            let clip_audio = step!(self.packets(job, output, "a:0", pieces).await?);
            let source_audio = match piece {
                AudioPiece::Copy => Some(step!(self.packets(job, source, "a:0", pieces).await?)),
                AudioPiece::Encode(_) => None,
            };
            step!(smart_cut::check_audio(&plan, source_audio.as_deref(), &clip_audio));
        }
        progress(tracker);
        let stats = "head.psnr";
        step!(
            self.tool(
                job,
                &ffmpeg,
                smart_cut::head_check_args(output, source, &plan, stats),
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
        if recipe.cut().is_none() || recipe.audio_codec().is_none() {
            return Ok(None);
        }
        let mut times = Vec::new();
        for stream in ["v:0", "a:0"] {
            let args = smart_cut::first_times_args(input, stream);
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
        let shift = smart_cut::clock_shift(&times[0], &times[1]);
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
    /// `smart_cut::parse_restarts`).
    async fn restarts(
        &self,
        job: &JobContext<'_>,
        source: &Path,
        codec: VideoCodec,
        time_base: f64,
        pieces: &Path,
    ) -> Result<std::result::Result<Vec<f64>, Fallback>> {
        let mut cmd = process::command(job.bins.ffmpeg());
        cmd.args(smart_cut::key_frames_trace_args(source))
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

    /// A stream's packets, in decode order, as ffprobe lists them.
    async fn packets(
        &self,
        job: &JobContext<'_>,
        file: &Path,
        stream: &str,
        pieces: &Path,
    ) -> Result<std::result::Result<Vec<smart_cut::Packet>, Fallback>> {
        let list = pieces.join("packets.csv");
        let args = smart_cut::packets_args(file, stream, &list);
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
    /// `refresh` looks it up even if it is cached; the cached copy stays in
    /// place if that fails, so the open video keeps working.
    async fn video_info(&self, job: &JobContext<'_>, url: &str, refresh: bool) -> Result<Arc<VideoInfo>> {
        let cache = job.app.state::<MediaCache>();
        if let Some(video) = cache.get(url).filter(|_| !refresh) {
            return Ok(video);
        }
        match metadata::fetch(job.bins, url).await? {
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

    /// Step 2: yt-dlp downloads the recipe's streams and merges or re-wraps them.
    async fn fetch_streams(
        &self,
        job: &JobContext<'_>,
        url: &str,
        recipe: &Recipe,
        output_dir: &Path,
        tracker: &mut Tracker,
    ) -> Result<PathBuf> {
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
        if !continues {
            // A new download ignores partial files of the same name; a
            // later try leaves this out and carries on from them instead.
            cmd.arg("--no-continue");
        }
        // Every file carries this download's own marker until it is finished
        // (see `publish`), so nothing already in the folder is ever touched.
        cmd.arg("--paths")
            .arg(output_dir)
            .args([
                "--output",
                &args::output_template(
                    recipe,
                    // "07 - " in front for a numbered playlist video: digits and a dash, nothing yt-dlp reads as a field.
                    &format!("{}{}", job.place.prefix(), job.settings.filename_template()),
                    &args::own_marker(job.id),
                ),
                "--",
            ])
            .arg(url);

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
                self.check_section(recipe, &file, &log).await?;
                return Ok(file);
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
            Failure::Exited(log) => Err(errors::classify(&log)),
            Failure::Spawn(error) => Err(error),
        }
    }

    /// A trimmed part is fetched by ffmpeg, which gives up on a connection
    /// that stops sending (`-rw_timeout`, see `ytdlp::args`) but then ends as
    /// if the part were complete, and yt-dlp reports success. ffmpeg's own
    /// messages in the log tell: a part cut short is removed, and the
    /// download is tried again like any dropped connection.
    async fn check_section(&self, recipe: &Recipe, file: &Path, log: &str) -> Result<()> {
        if recipe.trim.is_none() || !errors::section_cut_short(log) {
            return Ok(());
        }
        let _ = tokio::fs::remove_file(file).await;
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
    async fn publish(&self, downloaded: &Path, id: &str) -> Result<PathBuf> {
        let stem = downloaded.file_stem().and_then(|stem| stem.to_str()).unwrap_or("video");
        let ext = downloaded.extension().and_then(|ext| ext.to_str()).unwrap_or("mp4");
        let wanted = downloaded.with_file_name(format!("{}.{ext}", args::finished_stem(stem)));
        let output = self.unique_output(wanted, id);
        tokio::fs::rename(downloaded, &output).await.map_err(|err| {
            AppError::internal("The download finished, but the file couldn't be given its name.")
                .with_details(format!("{} → {}: {err}", downloaded.display(), output.display()))
        })?;
        Ok(output)
    }

    /// Step 3: Siphon's ffmpeg pass, for recipes that convert video or audio.
    /// A size-targeted encode on the CPU runs twice; its first pass writes
    /// what it measured to the work folder.
    async fn convert(
        &self,
        job: &JobContext<'_>,
        recipe: &Recipe,
        input: &Path,
        output: &Path,
        work: &Path,
        tracker: &mut Tracker,
    ) -> Result<()> {
        let ffmpeg = job.bins.ffmpeg();
        let log = work.join("passlog");
        let passes = if recipe.video_encode().is_some_and(|encode| encode.two_pass()) {
            vec![Pass::First { log: &log }, Pass::Second { log: &log }]
        } else {
            vec![Pass::Only]
        };
        let share = 1.0 / passes.len() as f64;

        job.emit(tracker.on_convert(0.0));
        let audio_shift = self.audio_shift(job, recipe, input).await?;
        for (index, pass) in passes.into_iter().enumerate() {
            let mut cmd = process::command(&ffmpeg);
            cmd.args(ffmpeg::convert_args_shifted(recipe, input, output, pass, audio_shift));
            let result = self
                .run_child(job.bins, job.id, cmd, |event| {
                    if let ChildEvent::Line(line) = event {
                        if let (Some(done), Some(total)) = (ffmpeg::parse_progress(line), recipe.duration) {
                            job.emit(tracker.on_convert((index as f64 + done / total) * share));
                        }
                    }
                })
                .await;
            match result {
                Ok(_) => {}
                Err(Failure::Stopped(_)) => return Err(AppError::cancelled()),
                Err(Failure::Exited(log)) => {
                    return Err(AppError::internal("The video downloaded, but converting it failed.").with_details(log))
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
        let mut child = cmd
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|err| {
                Failure::Spawn(AppError::setup("Couldn't start the download engine.").with_details(err.to_string()))
            })?;
        // Registering the process and reading the stop share one lock with `stop`, so a
        // stop that came just before the process existed is caught here instead of lost.
        let stopped_meanwhile = match self.jobs.lock().unwrap().get_mut(id) {
            Some(job) => {
                job.pid = child.id();
                job.stop.is_some()
            }
            None => false,
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
        if let Some(job) = self.jobs.lock().unwrap().get_mut(id) {
            job.pid = None;
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
/// last day are kept, because they may belong to another copy of Siphon
/// (the dev and installed builds share this folder), and so are those of
/// paused or failed downloads (`kept`), which a later try carries on from.
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

/// Moves a file, copying it when the destination is on another drive.
async fn move_file(from: &Path, to: &Path) -> Result<()> {
    if tokio::fs::rename(from, to).await.is_err() {
        tokio::fs::copy(from, to).await?;
        tokio::fs::remove_file(from).await?;
    }
    Ok(())
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
/// companions. They all carry the download's own marker, so none of them
/// can be a file that was in the folder before.
async fn remove_partial_files(stream_files: &[PathBuf]) {
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
                .is_some_and(|rest| rest.starts_with(".part") || rest == ".ytdl");
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
        let number = |index, count| Place { folder: None, number: Some((index, count)) }.prefix();
        assert_eq!(number(7, 9), "07 - ");
        assert_eq!(number(7, 95), "07 - ");
        assert_eq!(number(7, 182), "007 - ");
        assert_eq!(number(1218, 1218), "1218 - ");
        assert_eq!(Place::default().prefix(), "");
    }

    #[test]
    fn a_playlist_title_becomes_a_folder_windows_accepts() {
        assert_eq!(folder_name("Blender Conference 2026").as_deref(), Some("Blender Conference 2026"));
        assert_eq!(folder_name("AC/DC: Live? <Best>").as_deref(), Some("AC DC Live Best"));
        assert_eq!(folder_name("  Ends with dots... ").as_deref(), Some("Ends with dots"));
        assert_eq!(folder_name("..").as_deref(), None);
        assert_eq!(folder_name(r"..\..\Windows").as_deref(), Some("Windows"));
        assert_eq!(folder_name("con").as_deref(), Some("con_"));
        assert_eq!(folder_name("COM1.mix").as_deref(), Some("COM1.mix_"));
        assert_eq!(folder_name("Comedy").as_deref(), Some("Comedy"));
        assert_eq!(folder_name(&"x".repeat(300)).map(|name| name.chars().count()), Some(100));
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
        manager.register(id);
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
