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
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};
use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};
use tokio::process::Command;
use tokio::sync::Notify;

use crate::binaries::{BinaryManager, Tool};
use crate::error::{AppError, ErrorKind, Result};
use crate::ffmpeg::{self, encoders::Encoders};
use crate::plan::{self, DownloadOptions, Recipe, Trim};
use crate::process;
use crate::settings::{Settings, SettingsStore};
use crate::state::{EncoderCache, MediaCache};
use crate::ytdlp::metadata::{self, Media, VideoInfo};
use crate::ytdlp::{self, args, errors, progress};
use tracker::{Progress, Tracker};

/// Event carrying a [`DownloadEvent`].
pub const EVENT: &str = "download://event";

/// Keep the tail of the log for the "details" toggle without growing forever.
const MAX_LOG_LINES: usize = 400;

/// How long to wait before the one retry after a dropped connection.
const NETWORK_RETRY_DELAY: Duration = Duration::from_secs(2);

#[derive(Debug, Deserialize)]
pub struct DownloadRequest {
    /// Chosen by the frontend so it can match events to its own records.
    pub id: String,
    pub url: String,
    pub options: DownloadOptions,
    /// Download only this part of the video.
    #[serde(default)]
    pub trim: Option<Trim>,
    /// True when continuing a paused download: files already on disk are
    /// picked up where they stopped instead of being fetched again.
    #[serde(default)]
    pub resume: bool,
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
}

/// Who may run: jobs take a slot in the order they were added.
#[derive(Default)]
struct Gate {
    waiting: VecDeque<String>,
    running: usize,
}

/// Files a paused download leaves on disk, removed if it is cancelled later.
#[derive(Default)]
struct Leftovers {
    stream_files: Vec<PathBuf>,
    multi_stream: bool,
    /// Anything else: a section's `.part` file, a downloaded source awaiting conversion.
    others: Vec<PathBuf>,
}

impl Leftovers {
    async fn remove(self) {
        for file in &self.others {
            let _ = tokio::fs::remove_file(file).await;
        }
        remove_partial_files(&self.stream_files, self.multi_stream).await;
    }
}

#[derive(Default)]
pub struct DownloadManager {
    jobs: Mutex<HashMap<String, Job>>,
    gate: Mutex<Gate>,
    /// Signalled whenever the gate may let someone through: a slot was
    /// freed, a waiting job was stopped, or the limit changed.
    gate_changed: Notify,
    paused: Mutex<HashMap<String, Leftovers>>,
    /// Files finished this session: the only ones the UI may open or reveal.
    finished: Mutex<HashSet<PathBuf>>,
}

/// Everything one running job needs.
struct JobContext<'a> {
    app: &'a AppHandle,
    bins: &'a BinaryManager,
    id: &'a str,
    settings: Settings,
    resume: bool,
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
        self.jobs.lock().unwrap().insert(id.clone(), Job::default());
        self.gate.lock().unwrap().waiting.push_back(id.clone());

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
                let job = JobContext {
                    app: &app,
                    bins: &bins,
                    id: &id,
                    // Read once the job starts, so changes made while it queued apply.
                    settings: app.state::<SettingsStore>().get(),
                    resume: request.resume,
                };
                let outcome = manager.run(&job, &url, &request.options, request.trim).await;
                manager.gate.lock().unwrap().running -= 1;
                manager.gate_changed.notify_waiters();
                outcome
            } else {
                Err(AppError::cancelled())
            };

            let stop = manager.jobs.lock().unwrap().remove(&id).and_then(|job| job.stop);
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
    /// Also discards the files of a download that was paused earlier.
    pub async fn cancel(&self, id: &str) {
        if !self.stop(id, Stop::Cancel).await {
            let leftovers = self.paused.lock().unwrap().remove(id);
            if let Some(leftovers) = leftovers {
                leftovers.remove().await;
            }
        }
    }

    /// Stops a download but keeps what it has fetched, so it can be resumed.
    pub async fn pause(&self, id: &str) {
        self.stop(id, Stop::Pause).await;
    }

    /// Stops every running and waiting download, keeping their partial files.
    /// Used when the app closes, so no yt-dlp or ffmpeg process is left behind.
    pub async fn pause_all(&self) {
        let ids: Vec<String> = self.jobs.lock().unwrap().keys().cloned().collect();
        for id in ids {
            self.pause(&id).await;
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
    async fn run(
        &self,
        job: &JobContext<'_>,
        url: &str,
        options: &DownloadOptions,
        trim: Option<Trim>,
    ) -> Result<PathBuf> {
        job.emit(Tracker::new(false).current());
        // A resumed job starts clean: whatever it left is either reused or replaced.
        self.paused.lock().unwrap().remove(job.id);

        let output_dir = match &job.settings.output_dir {
            Some(dir) => dir.clone(),
            None => default_output_dir(job.app)?,
        };
        tokio::fs::create_dir_all(&output_dir).await.map_err(|err| {
            AppError::internal("Couldn't create the download folder. Check the folder in Settings.")
                .with_details(format!("{}: {err}", output_dir.display()))
        })?;

        match self.attempt(job, url, options, trim, &output_dir).await {
            // Sites change, and yt-dlp follows within days. If the failure
            // looks like that, get the newest yt-dlp and try once more.
            Err(error) if error.kind == ErrorKind::Outdated && self.stop_of(job.id).is_none() => {
                job.emit(Tracker::updating());
                match job.bins.update_now(Tool::YtDlp, job.settings.update_channel).await {
                    Ok(Some(_)) => {
                        // Formats looked up by the old version may be the problem.
                        job.app.state::<MediaCache>().forget(url);
                        job.emit(Tracker::new(false).current());
                        self.attempt(job, url, options, trim, &output_dir).await
                    }
                    // Already current (or the update couldn't run): the first error stands.
                    _ => Err(error),
                }
            }
            // Sites refuse or drop a request now and then, and the same
            // request works a moment later. Try once more before giving up.
            Err(error) if error.kind == ErrorKind::Network && self.stop_of(job.id).is_none() => {
                tokio::time::sleep(NETWORK_RETRY_DELAY).await;
                if self.stop_of(job.id).is_some() {
                    return Err(error);
                }
                job.emit(Tracker::new(false).current());
                self.attempt(job, url, options, trim, &output_dir).await
            }
            outcome => outcome,
        }
    }

    /// One pass through the job's steps: look up, download, convert.
    async fn attempt(
        &self,
        job: &JobContext<'_>,
        url: &str,
        options: &DownloadOptions,
        trim: Option<Trim>,
        output_dir: &Path,
    ) -> Result<PathBuf> {
        let video = self.video_info(job, url).await?;
        let recipe = plan::plan(&video, options, trim)?;
        let mut tracker = Tracker::new(recipe.needs_ffmpeg_pass());

        let downloaded = self
            .fetch_streams(job, &video.url, &recipe, output_dir, &mut tracker)
            .await?;
        if !recipe.needs_ffmpeg_pass() {
            return Ok(downloaded);
        }

        let output = converted_path(&downloaded, &recipe);
        let encoders = job.app.state::<EncoderCache>().get(job.bins).await;
        let mut result = self
            .convert(job, &recipe, &encoders, &downloaded, &output, &mut tracker)
            .await;
        // A hardware encoder can refuse a job it passed the probe for (driver
        // limits, too many sessions). The CPU encoders always work.
        if result.is_err() && encoders != Encoders::SOFTWARE && self.stop_of(job.id).is_none() {
            result = self
                .convert(job, &recipe, &Encoders::SOFTWARE, &downloaded, &output, &mut tracker)
                .await;
        }

        if result.is_err() {
            let _ = tokio::fs::remove_file(&output).await;
        }
        if self.stop_of(job.id) == Some(Stop::Pause) {
            // Keep the downloaded source: resuming goes straight back to converting.
            let leftovers = Leftovers {
                others: vec![downloaded],
                ..Leftovers::default()
            };
            self.paused.lock().unwrap().insert(job.id.to_string(), leftovers);
        } else {
            let _ = tokio::fs::remove_file(&downloaded).await;
        }
        result.map(|()| output)
    }

    /// The video's metadata: from this session's cache, or looked up now
    /// (playlist items are queued without having been looked up first).
    async fn video_info(&self, job: &JobContext<'_>, url: &str) -> Result<Arc<VideoInfo>> {
        let cache = job.app.state::<MediaCache>();
        if let Some(video) = cache.get(url) {
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
        let mut cmd = ytdlp::command(job.bins);
        cmd.args(progress::template_args())
            .args(args::recipe_args(recipe))
            .args([
                "--no-playlist",
                // Stamp the file with the time it was downloaded, not uploaded.
                "--no-mtime",
            ]);
        if !job.resume {
            // The same video may be fetched again with other settings; the
            // newest request wins. (A resumed job instead reuses its files.)
            cmd.arg("--force-overwrites");
        }
        cmd.arg("--paths")
            .arg(output_dir)
            .args([
                "--output",
                &args::output_template(recipe, &job.settings.filename_template()),
                "--",
            ])
            .arg(url);

        let mut output_file: Option<PathBuf> = None;
        let mut stream_files: Vec<PathBuf> = Vec::new();
        // Section downloads go through ffmpeg, which stays silent until it is
        // done. For those, progress is read off the growing ".part" file.
        let mut part_file: Option<PathBuf> = None;
        let expected_bytes = recipe.trim.and(recipe.estimated_source_bytes());
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
                    Some(progress::Line::Target(path)) => part_file = Some(PathBuf::from(format!("{path}.part"))),
                    Some(progress::Line::Plan(plan)) => tracker.set_plan(plan),
                    Some(progress::Line::Download(tick)) => {
                        if let Some(name) = tick.filename.as_deref().map(PathBuf::from) {
                            if !stream_files.contains(&name) {
                                stream_files.push(name);
                            }
                        }
                        job.emit(tracker.on_tick(&tick));
                    }
                    Some(progress::Line::PostProcess { started: true, name }) => {
                        job.emit(tracker.on_postprocess(&name))
                    }
                    Some(progress::Line::File(path)) => output_file = Some(PathBuf::from(path)),
                    Some(progress::Line::PostProcess { started: false, .. }) | None => {}
                },
            })
            .await;

        let leftovers = || Leftovers {
            stream_files: stream_files.clone(),
            multi_stream: tracker.is_multi_stream(),
            others: part_file.iter().cloned().collect(),
        };
        match result {
            Ok(log) => output_file.ok_or_else(|| {
                AppError::internal("The download finished but the file couldn't be located.").with_details(log)
            }),
            Err(Failure::Stopped(Stop::Cancel)) => {
                leftovers().remove().await;
                Err(AppError::cancelled())
            }
            Err(Failure::Stopped(Stop::Pause)) => {
                self.paused.lock().unwrap().insert(job.id.to_string(), leftovers());
                Err(AppError::cancelled())
            }
            Err(Failure::Exited(log)) => Err(errors::classify(&log)),
            Err(Failure::Spawn(error)) => Err(error),
        }
    }

    /// Step 3: Siphon's ffmpeg pass, for recipes that convert video or audio.
    async fn convert(
        &self,
        job: &JobContext<'_>,
        recipe: &Recipe,
        encoders: &Encoders,
        input: &Path,
        output: &Path,
        tracker: &mut Tracker,
    ) -> Result<()> {
        let ffmpeg = job
            .bins
            .ffmpeg()
            .ok_or_else(|| AppError::setup("FFmpeg isn't installed."))?;
        let mut cmd = process::command(ffmpeg);
        cmd.args(ffmpeg::convert_args(recipe, encoders, input, output));

        job.emit(tracker.on_convert(0.0));
        let result = self
            .run_child(job.bins, job.id, cmd, |event| {
                let ChildEvent::Line(line) = event else { return };
                if let (Some(done), Some(total)) = (ffmpeg::parse_progress(line), recipe.duration) {
                    job.emit(tracker.on_convert(done / total));
                }
            })
            .await;

        match result {
            Ok(_) => Ok(()),
            Err(Failure::Stopped(_)) => Err(AppError::cancelled()),
            Err(Failure::Exited(log)) => {
                Err(AppError::internal("The video downloaded, but converting it failed.").with_details(log))
            }
            Err(Failure::Spawn(error)) => Err(error),
        }
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

/// `Title [id].source.mkv` → `Title [id].mp4`
fn converted_path(downloaded: &Path, recipe: &Recipe) -> PathBuf {
    let stem = downloaded.file_stem().and_then(|stem| stem.to_str()).unwrap_or("video");
    let stem = stem.strip_suffix(args::SOURCE_MARKER).unwrap_or(stem);
    downloaded.with_file_name(format!("{stem}.{}", recipe.container.ext()))
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
/// themselves are only deleted for multi-stream downloads, where they are
/// intermediates awaiting a merge; for a single-stream download the target
/// is the user's final file and may predate this run.
async fn remove_partial_files(stream_files: &[PathBuf], multi_stream: bool) {
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
            if is_companion || (multi_stream && entry_name == name) {
                let _ = tokio::fs::remove_file(entry.path()).await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::codecs::Container;

    #[test]
    fn converted_file_drops_the_source_marker() {
        let recipe = Recipe {
            video: None,
            audio: None,
            container: Container::Mp4,
            strip_audio: false,
            reason: None,
            promise: String::new(),
            hints: Vec::new(),
            qualities: Vec::new(),
            duration: None,
            trim: None,
            source_fraction: 1.0,
        };
        let path = converted_path(Path::new("/videos/A clip [abc].source.mkv"), &recipe);
        assert_eq!(path, Path::new("/videos/A clip [abc].mp4"));
    }
}
