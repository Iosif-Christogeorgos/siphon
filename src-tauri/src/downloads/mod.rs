//! Runs downloads and streams their progress to the frontend.
//!
//! A job waits in the queue for a free slot, then has up to three steps:
//! look the video up (unless it is cached), let yt-dlp fetch and merge the
//! streams the recipe names, then, only if the recipe converts anything,
//! run Siphon's own ffmpeg pass.

mod tracker;

use std::collections::{HashMap, HashSet, VecDeque};
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
use crate::ffmpeg::{self, encoders::Hardware, Pass};
use crate::plan::{self, Choice, Recipe, Trim};
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

/// A file one stream is downloaded to.
#[derive(Debug, Clone, PartialEq)]
struct StreamFile {
    path: PathBuf,
    /// One of several streams waiting to be merged, so the file itself can
    /// go. Otherwise it carries the finished file's own name, which may
    /// belong to an older download: only its `.part` companions are ours.
    intermediate: bool,
}

/// What a paused or failed download left on disk. The next try at the same
/// download carries on from these files; cancelling it removes them.
#[derive(Default)]
struct Leftovers {
    stream_files: Vec<StreamFile>,
    /// Anything else: a section's `.part` file, a downloaded source awaiting conversion.
    others: Vec<PathBuf>,
    /// The " (2)" this download added to its name because the plain name was
    /// taken; the next try keeps it, so it carries on from its own files.
    copy: Option<u32>,
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
}

/// Everything one running job needs.
struct JobContext<'a> {
    app: &'a AppHandle,
    bins: &'a BinaryManager,
    id: &'a str,
    settings: Settings,
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

    /// Whether `path` is a file a download produced this session.
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
            leftovers.remove().await;
        }
    }

    fn keep_leftovers(&self, id: &str, leftovers: Leftovers) {
        self.leftovers.lock().unwrap().insert(id.to_string(), leftovers);
    }

    /// Stops a download but keeps what it has fetched, so it can be resumed.
    pub async fn pause(&self, id: &str) {
        self.stop(id, Stop::Pause).await;
    }

    /// Stops every download and removes what they left on disk. For when the
    /// app closes: no yt-dlp or ffmpeg process may outlive it, and since the
    /// list is not kept between sessions and a new download starts afresh,
    /// nothing would ever pick the partial files up again.
    pub async fn shut_down(&self) {
        let ids: Vec<String> = self.jobs.lock().unwrap().keys().cloned().collect();
        for id in ids {
            self.stop(&id, Stop::Cancel).await;
        }
        // Each job removes its own files once its process has ended.
        let deadline = Instant::now() + SHUTDOWN_GRACE;
        while !self.is_idle() && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        // A job that hasn't ended by now still loses its proxy: no listener
        // is left open, whatever the job is stuck on.
        for job in self.jobs.lock().unwrap().values_mut() {
            job.flow = None;
        }
        // What is left belongs to downloads that were paused or had failed.
        let leftovers: Vec<Leftovers> = self.leftovers.lock().unwrap().drain().map(|(_, left)| left).collect();
        for left in leftovers {
            left.remove().await;
        }
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
                    return Ok(downloaded);
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
        let previous = self.leftovers.lock().unwrap().remove(job.id);
        let continues = previous.is_some();
        // (Its work folder is the one this try uses again; it is removed with the download.)
        let Leftovers {
            mut stream_files,
            others: earlier_files,
            mut copy,
            work: _,
        } = previous.unwrap_or_default();
        // The name of the finished file, as yt-dlp checks it.
        let finished_ext = if recipe.needs_ffmpeg_pass() {
            "mkv"
        } else {
            recipe.container.ext()
        };
        // A converted file gets its own name when it is written (see `attempt`).
        let check_name = !continues && !recipe.needs_ffmpeg_pass();

        let mut output_file: Option<PathBuf> = None;
        // Section downloads go through ffmpeg, which stays silent until it is
        // done. For those, progress is read off the growing ".part" file.
        let mut part_file: Option<PathBuf> = None;
        let expected_bytes = recipe.trim.and(recipe.estimated_source_bytes());
        let result = loop {
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
                // (Not --force-overwrites: that deletes a finished file of the
                // same name before the name is announced, so it couldn't be
                // given its own.)
                cmd.arg("--no-continue");
            }
            cmd.arg("--paths")
                .arg(output_dir)
                .args([
                    "--output",
                    &args::output_template(recipe, &job.settings.filename_template(), copy),
                    "--",
                ])
                .arg(url);

            // Set when the name yt-dlp is about to write is already taken, with
            // whether a file on disk has it (rather than another running download).
            let mut taken: Option<(PathBuf, bool)> = None;
            let result = self
                .run_child(job.bins, job.id, cmd, |event| match event {
                    ChildEvent::Tick => {
                        if let (Some(part), Some(expected)) = (&part_file, expected_bytes) {
                            if let Ok(meta) = std::fs::metadata(part) {
                                job.emit(tracker.on_bytes(meta.len(), expected));
                            }
                        }
                        Flow::Continue
                    }
                    ChildEvent::Line(line) => match progress::parse(line) {
                        Some(progress::Line::Target(path)) => {
                            let target = PathBuf::from(&path);
                            let finished = target.with_extension(finished_ext);
                            // Never replace a file: the same video downloaded
                            // again gets its own name. Stopped before any data moves.
                            if check_name && !self.claim(&finished, job.id, target.exists()) {
                                let on_disk = target.exists() || finished.exists();
                                taken = Some((target, on_disk));
                                return Flow::Stop;
                            }
                            part_file = Some(PathBuf::from(format!("{path}.part")));
                            Flow::Continue
                        }
                        Some(progress::Line::Plan(plan)) => {
                            tracker.set_plan(plan);
                            Flow::Continue
                        }
                        Some(progress::Line::Download(tick)) => {
                            if let Some(path) = tick.filename.as_deref().map(PathBuf::from) {
                                if !stream_files.iter().any(|file| file.path == path) {
                                    stream_files.push(StreamFile {
                                        path,
                                        // A trimmed part arrives as one file, already under its final name.
                                        intermediate: tracker.is_multi_stream() && recipe.trim.is_none(),
                                    });
                                }
                            }
                            job.emit(tracker.on_tick(&tick));
                            Flow::Continue
                        }
                        Some(progress::Line::PostProcess { started: true, name }) => {
                            job.emit(tracker.on_postprocess(&name));
                            Flow::Continue
                        }
                        Some(progress::Line::File(path)) => {
                            output_file = Some(PathBuf::from(path));
                            Flow::Continue
                        }
                        Some(progress::Line::PostProcess { started: false, .. }) | None => Flow::Continue,
                    },
                })
                .await;
            match (taken, &result) {
                (Some((target, on_disk)), Err(Failure::Abandoned)) => {
                    // yt-dlp starts writing as soon as it names the file, so a
                    // moment of it may be there. When the name belongs to a
                    // finished file, that ".part" can only be this run's; when
                    // another download holds it, the ".part" is that one's.
                    if on_disk {
                        let _ = tokio::fs::remove_file(format!("{}.part", target.display())).await;
                    }
                    copy = Some(self.free_copy(&target, finished_ext, copy));
                }
                _ => break result,
            }
        };

        let failure = match result {
            Ok(log) => {
                // An earlier try may have left files this one had no use for
                // (the streams picked can differ from one try to the next).
                stream_files.retain(|file| Some(&file.path) != output_file.as_ref());
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
            copy,
            work: recipe.needs_ffmpeg_pass().then(|| output_dir.to_path_buf()),
        };
        if let Failure::Stopped(Stop::Cancel) = failure {
            leftovers.remove().await;
            return Err(AppError::cancelled());
        }
        // Kept for the next try, automatic or by hand, to carry on from.
        self.keep_leftovers(job.id, leftovers);
        match failure {
            Failure::Stopped(_) | Failure::Abandoned => Err(AppError::cancelled()),
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

    /// Claims `finished` for this download. False when it is taken: the file
    /// exists (or `exists` says the download's own target does), or another
    /// running download is writing to it.
    fn claim(&self, finished: &Path, id: &str, exists: bool) -> bool {
        let mut claimed = self.claimed.lock().unwrap();
        let other = claimed.get(finished).is_some_and(|owner| owner != id);
        if exists || other || finished.exists() {
            return false;
        }
        claimed.insert(finished.to_path_buf(), id.to_string());
        true
    }

    /// The first copy number after `after` whose name is free:
    /// `Title [id].mp4` → `Title [id] (2).mp4`, `(3)`, …
    fn free_copy(&self, target: &Path, ext: &str, after: Option<u32>) -> u32 {
        let stem = target.file_stem().and_then(|stem| stem.to_str()).unwrap_or("video");
        // The target of a later try already carries the number it was given.
        let numbered = after.map(|n| format!(" ({n})"));
        let base = numbered
            .as_deref()
            .and_then(|suffix| stem.strip_suffix(suffix))
            .unwrap_or(stem);
        let claimed = self.claimed.lock().unwrap();
        (after.map_or(2, |n| n + 1)..)
            .find(|n| {
                let candidate = target.with_file_name(format!("{base} ({n}).{ext}"));
                !candidate.exists() && !claimed.contains_key(&candidate)
            })
            .expect("some number is free")
    }

    /// `output`, or the first "(2)", "(3)", … variant of it that no file or
    /// running download has, claimed for this download. For converted files,
    /// whose name Siphon chooses itself.
    fn unique_output(&self, output: PathBuf, id: &str) -> PathBuf {
        if self.claim(&output, id, false) {
            return output;
        }
        let ext = output
            .extension()
            .and_then(|ext| ext.to_str())
            .unwrap_or("mp4")
            .to_string();
        let copy = self.free_copy(&output, &ext, None);
        let stem = output.file_stem().and_then(|stem| stem.to_str()).unwrap_or("video");
        let unique = output.with_file_name(format!("{stem} ({copy}).{ext}"));
        self.claim(&unique, id, false);
        unique
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
        for (index, pass) in passes.into_iter().enumerate() {
            let mut cmd = process::command(&ffmpeg);
            cmd.args(ffmpeg::convert_args(recipe, input, output, pass));
            let result = self
                .run_child(job.bins, job.id, cmd, |event| {
                    if let ChildEvent::Line(line) = event {
                        if let (Some(done), Some(total)) = (ffmpeg::parse_progress(line), recipe.duration) {
                            job.emit(tracker.on_convert((index as f64 + done / total) * share));
                        }
                    }
                    Flow::Continue
                })
                .await;
            match result {
                Ok(_) => {}
                Err(Failure::Stopped(_) | Failure::Abandoned) => return Err(AppError::cancelled()),
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
    /// `on_event`, and returns the stderr log on success. `on_event` can stop
    /// the process early, which ends the run with `Failure::Abandoned`.
    async fn run_child(
        &self,
        bins: &BinaryManager,
        id: &str,
        mut cmd: Command,
        mut on_event: impl FnMut(ChildEvent<'_>) -> Flow,
    ) -> std::result::Result<String, Failure> {
        // Stopped between steps: don't start the next one.
        if let Some(stop) = self.stop_of(id) {
            return Err(Failure::Stopped(stop));
        }
        // Keeps a tool update from swapping the binaries while this runs.
        let _in_use = bins.in_use().await;
        let mut child = cmd
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|err| {
                Failure::Spawn(AppError::setup("Couldn't start the download engine.").with_details(err.to_string()))
            })?;
        if let Some(job) = self.jobs.lock().unwrap().get_mut(id) {
            job.pid = child.id();
        }

        let stderr_log = tokio::spawn(collect_lines(child.stderr.take().expect("stderr is piped")));
        let mut stdout = BufReader::new(child.stdout.take().expect("stdout is piped"));
        let mut ticker = tokio::time::interval(Duration::from_millis(250));
        let mut buffer = Vec::new();
        let mut abandoned = false;
        loop {
            let flow = tokio::select! {
                // If the tick wins, the bytes read so far stay in `buffer` and
                // the next read carries on from there, so no output is lost.
                read = stdout.read_until(b'\n', &mut buffer) => match read {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {
                        // Lossy on purpose: a stray byte in a title must not end the download.
                        let flow = on_event(ChildEvent::Line(&String::from_utf8_lossy(&buffer)));
                        buffer.clear();
                        flow
                    }
                },
                _ = ticker.tick() => on_event(ChildEvent::Tick),
            };
            if flow == Flow::Stop {
                if let Some(pid) = child.id() {
                    process::kill_tree(pid).await;
                }
                abandoned = true;
                break;
            }
        }

        let status = child.wait().await;
        let log = stderr_log.await.unwrap_or_default();
        if let Some(job) = self.jobs.lock().unwrap().get_mut(id) {
            job.pid = None;
        }

        if let Some(stop) = self.stop_of(id) {
            Err(Failure::Stopped(stop))
        } else if abandoned {
            Err(Failure::Abandoned)
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

/// Whether `run_child` lets the process carry on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Flow {
    Continue,
    Stop,
}

/// How a child process run can go wrong.
enum Failure {
    /// `on_event` asked for the process to be stopped.
    Abandoned,
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
/// (the dev and installed builds share this folder).
pub fn sweep_work(app: &AppHandle) {
    let Ok(Ok(entries)) = work_root(app).map(std::fs::read_dir) else {
        return;
    };
    for entry in entries.flatten() {
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
    let stem = stem.strip_suffix(args::SOURCE_MARKER).unwrap_or(stem);
    output_dir.join(format!("{stem}.{}", recipe.container.ext()))
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

/// Removes what a stopped download left on disk.
///
/// `stream_files` are the per-stream targets yt-dlp reported. Their `.part`,
/// `.ytdl` and fragment companions are always safe to delete. The targets
/// themselves are only deleted when they are intermediates awaiting a merge;
/// otherwise the target is the user's final file and may predate this run.
async fn remove_partial_files(stream_files: &[StreamFile]) {
    for file in stream_files {
        let (Some(dir), Some(name)) = (file.path.parent(), file.path.file_name().and_then(|name| name.to_str())) else {
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
            if is_companion || (file.intermediate && entry_name == name) {
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
    fn a_taken_name_gets_the_next_free_number() {
        let dir = std::env::temp_dir().join(format!("siphon-names-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = |name: &str| dir.join(name);
        std::fs::write(file("Clip [x].mp4"), b"").unwrap();
        std::fs::write(file("Clip [x] (2).mp4"), b"").unwrap();
        let manager = DownloadManager::default();

        // Taken on disk.
        assert!(!manager.claim(&file("Clip [x].mp4"), "a", false));
        assert_eq!(manager.free_copy(&file("Clip [x].mp4"), "mp4", None), 3);
        // A later try's target carries its number; the next one comes after it.
        assert_eq!(manager.free_copy(&file("Clip [x] (3).mp4"), "mp4", Some(3)), 4);
        // Taken by another running download, but free for the one holding it.
        assert!(manager.claim(&file("Other.mp4"), "a", false));
        assert!(!manager.claim(&file("Other.mp4"), "b", false));
        assert!(manager.claim(&file("Other.mp4"), "a", false));
        assert_eq!(manager.unique_output(file("Other.mp4"), "b"), file("Other (2).mp4"));

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
