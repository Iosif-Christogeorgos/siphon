//! Runs downloads and streams their progress to the frontend.
//!
//! A job has up to three steps: look the video up (unless it is cached),
//! let yt-dlp fetch and merge the streams the recipe names, then, only if
//! the recipe converts anything, run Siphon's own ffmpeg pass.

mod tracker;

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};
use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};
use tokio::process::Command;

use crate::binaries::BinaryManager;
use crate::error::{AppError, ErrorKind, Result};
use crate::ffmpeg::{self, encoders::Encoders};
use crate::plan::{self, DownloadOptions, Recipe};
use crate::process;
use crate::state::{EncoderCache, MediaCache};
use crate::ytdlp::metadata::{self, Media, VideoInfo};
use crate::ytdlp::{self, args, errors, progress};
use tracker::{Progress, Tracker};

/// Event carrying a [`DownloadEvent`].
pub const EVENT: &str = "download://event";

/// Keep the tail of the log for the "details" toggle without growing forever.
const MAX_LOG_LINES: usize = 400;

#[derive(Debug, Deserialize)]
pub struct DownloadRequest {
    /// Chosen by the frontend so it can match events to its own records.
    pub id: String,
    pub url: String,
    pub options: DownloadOptions,
    /// Defaults to the user's Downloads folder.
    pub output_dir: Option<String>,
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
    Cancelled {
        id: String,
    },
}

#[derive(Default)]
struct Job {
    /// The child process currently working for this job, if any.
    pid: Option<u32>,
    cancelled: bool,
}

#[derive(Default)]
pub struct DownloadManager {
    jobs: Mutex<HashMap<String, Job>>,
}

/// Everything one running job needs.
struct JobContext<'a> {
    app: &'a AppHandle,
    bins: &'a BinaryManager,
    id: &'a str,
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
    /// Starts a download in the background. Progress arrives as events.
    pub fn start(self: &Arc<Self>, app: AppHandle, request: DownloadRequest) -> Result<()> {
        let url = ytdlp::validate_url(&request.url)?.to_string();
        let output_dir = match request.output_dir {
            Some(dir) => PathBuf::from(dir),
            None => default_output_dir(&app)?,
        };
        std::fs::create_dir_all(&output_dir)?;

        let id = request.id;
        self.jobs.lock().unwrap().insert(id.clone(), Job::default());

        let manager = Arc::clone(self);
        tauri::async_runtime::spawn(async move {
            let bins = Arc::clone(&app.state::<Arc<BinaryManager>>());
            let job = JobContext {
                app: &app,
                bins: &bins,
                id: &id,
            };
            let outcome = manager.run(&job, &url, &request.options, &output_dir).await;
            let cancelled = manager
                .jobs
                .lock()
                .unwrap()
                .remove(&id)
                .is_some_and(|job| job.cancelled);

            let event = match outcome {
                _ if cancelled => DownloadEvent::Cancelled { id },
                Ok(path) => DownloadEvent::Finished {
                    size: std::fs::metadata(&path).ok().map(|meta| meta.len()),
                    path: path.to_string_lossy().into_owned(),
                    id,
                },
                Err(error) => DownloadEvent::Failed { id, error },
            };
            let _ = app.emit(EVENT, event);
        });
        Ok(())
    }

    /// Stops a running download and removes its partial files.
    pub async fn cancel(&self, id: &str) {
        let pid = {
            let mut jobs = self.jobs.lock().unwrap();
            let Some(job) = jobs.get_mut(id) else { return };
            job.cancelled = true;
            job.pid
        };
        if let Some(pid) = pid {
            process::kill_tree(pid).await;
        }
    }

    fn is_cancelled(&self, id: &str) -> bool {
        self.jobs.lock().unwrap().get(id).is_some_and(|job| job.cancelled)
    }

    /// Runs the whole job and returns the path of the finished file.
    async fn run(
        &self,
        job: &JobContext<'_>,
        url: &str,
        options: &DownloadOptions,
        output_dir: &Path,
    ) -> Result<PathBuf> {
        job.emit(Tracker::new(false).current());

        let video = self.video_info(job, url).await?;
        let recipe = plan::plan(&video, options)?;
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
        if result.is_err() && encoders != Encoders::SOFTWARE && !self.is_cancelled(job.id) {
            result = self
                .convert(job, &recipe, &Encoders::SOFTWARE, &downloaded, &output, &mut tracker)
                .await;
        }

        let _ = tokio::fs::remove_file(&downloaded).await;
        if result.is_err() {
            let _ = tokio::fs::remove_file(&output).await;
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
                // The same video may be fetched again with other settings; the newest request wins.
                "--force-overwrites",
            ])
            .arg("--paths")
            .arg(output_dir)
            .args(["--output", &args::output_template(recipe), "--"])
            .arg(url);

        let mut output_file: Option<PathBuf> = None;
        let mut stream_files: Vec<PathBuf> = Vec::new();
        let result = self
            .run_child(job.id, cmd, |line| match progress::parse(line) {
                Some(progress::Line::Plan(plan)) => tracker.set_plan(plan),
                Some(progress::Line::Download(tick)) => {
                    if let Some(name) = tick.filename.as_deref().map(PathBuf::from) {
                        if !stream_files.contains(&name) {
                            stream_files.push(name);
                        }
                    }
                    job.emit(tracker.on_tick(&tick));
                }
                Some(progress::Line::PostProcess { started: true, name }) => job.emit(tracker.on_postprocess(&name)),
                Some(progress::Line::File(path)) => output_file = Some(PathBuf::from(path)),
                Some(progress::Line::PostProcess { started: false, .. }) | None => {}
            })
            .await;

        match result {
            Ok(log) => output_file.ok_or_else(|| {
                AppError::internal("The download finished but the file couldn't be located.").with_details(log)
            }),
            Err(Failure::Cancelled) => {
                remove_partial_files(&stream_files, tracker.is_multi_stream()).await;
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
            .run_child(job.id, cmd, |line| {
                if let (Some(done), Some(total)) = (ffmpeg::parse_progress(line), recipe.duration) {
                    job.emit(tracker.on_convert(done / total));
                }
            })
            .await;

        match result {
            Ok(_) => Ok(()),
            Err(Failure::Cancelled) => Err(AppError::cancelled()),
            Err(Failure::Exited(log)) => {
                Err(AppError::internal("The video downloaded, but converting it failed.").with_details(log))
            }
            Err(Failure::Spawn(error)) => Err(error),
        }
    }

    /// Runs a child process for a job: registers it for cancellation, feeds
    /// each stdout line to `on_line`, and returns the stderr log on success.
    async fn run_child(
        &self,
        id: &str,
        mut cmd: Command,
        mut on_line: impl FnMut(&str),
    ) -> std::result::Result<String, Failure> {
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
        let mut buffer = Vec::new();
        loop {
            buffer.clear();
            match stdout.read_until(b'\n', &mut buffer).await {
                Ok(0) | Err(_) => break,
                // Lossy on purpose: a stray byte in a title must not end the download.
                Ok(_) => on_line(&String::from_utf8_lossy(&buffer)),
            }
        }

        let status = child.wait().await;
        let log = stderr_log.await.unwrap_or_default();
        if let Some(job) = self.jobs.lock().unwrap().get_mut(id) {
            job.pid = None;
        }

        if self.is_cancelled(id) {
            Err(Failure::Cancelled)
        } else if status.is_ok_and(|status| status.success()) {
            Ok(log)
        } else {
            Err(Failure::Exited(log))
        }
    }
}

/// How a child process run can go wrong.
enum Failure {
    Spawn(AppError),
    Cancelled,
    /// Non-zero exit; carries the stderr log.
    Exited(String),
}

fn default_output_dir(app: &AppHandle) -> Result<PathBuf> {
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

/// Cleans up after a cancelled download.
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
        };
        let path = converted_path(Path::new("/videos/A clip [abc].source.mkv"), &recipe);
        assert_eq!(path, Path::new("/videos/A clip [abc].mp4"));
    }
}
