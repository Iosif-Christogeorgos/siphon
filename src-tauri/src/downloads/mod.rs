//! Runs downloads and streams their progress to the frontend.

mod tracker;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};
use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};

use crate::binaries::BinaryManager;
use crate::error::{AppError, Result};
use crate::process;
use crate::ytdlp::{self, errors, progress};
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
    pid: Option<u32>,
    cancelled: bool,
}

#[derive(Default)]
pub struct DownloadManager {
    jobs: Mutex<HashMap<String, Job>>,
}

impl DownloadManager {
    /// Starts a download in the background. Progress arrives as events.
    pub fn start(
        self: &Arc<Self>,
        app: AppHandle,
        bins: Arc<BinaryManager>,
        request: DownloadRequest,
    ) -> Result<()> {
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
            let outcome = manager.run(&app, &bins, &id, &url, &output_dir).await;
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

    /// Runs yt-dlp to completion and returns the path of the finished file.
    async fn run(
        &self,
        app: &AppHandle,
        bins: &BinaryManager,
        id: &str,
        url: &str,
        output_dir: &Path,
    ) -> Result<PathBuf> {
        let emit = |progress: Progress| {
            let _ = app.emit(
                EVENT,
                DownloadEvent::Progress {
                    id: id.to_string(),
                    progress,
                },
            );
        };
        let mut tracker = Tracker::new();
        emit(tracker.current());

        let mut cmd = ytdlp::command(bins);
        cmd.args(progress::template_args())
            .args([
                "--no-playlist",
                // Stamp the file with the time it was downloaded, not uploaded.
                "--no-mtime",
                // Best video + best audio, or the best single file if the site doesn't split them.
                "--format",
                "bv*+ba/b",
            ])
            .arg("--paths")
            .arg(output_dir)
            // Cap the title in bytes so the path stays within OS limits.
            .args(["--output", "%(title).180B [%(id)s].%(ext)s", "--"])
            .arg(url)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let mut child = cmd.spawn().map_err(|err| {
            AppError::setup("Couldn't start the download engine.").with_details(err.to_string())
        })?;
        if let Some(job) = self.jobs.lock().unwrap().get_mut(id) {
            job.pid = child.id();
        }

        let stderr = child.stderr.take().expect("stderr is piped");
        let stderr_log = tokio::spawn(collect_lines(stderr));

        let mut stdout = BufReader::new(child.stdout.take().expect("stdout is piped"));
        let mut output_file: Option<PathBuf> = None;
        let mut stream_files: Vec<PathBuf> = Vec::new();
        let mut buffer = Vec::new();
        loop {
            buffer.clear();
            if stdout.read_until(b'\n', &mut buffer).await? == 0 {
                break;
            }
            // Lossy on purpose: a stray byte in a title must not end the download.
            match progress::parse(&String::from_utf8_lossy(&buffer)) {
                Some(progress::Line::Plan(plan)) => tracker.set_plan(plan),
                Some(progress::Line::Download(tick)) => {
                    if let Some(name) = tick.filename.as_deref().map(PathBuf::from) {
                        if !stream_files.contains(&name) {
                            stream_files.push(name);
                        }
                    }
                    emit(tracker.on_tick(&tick));
                }
                Some(progress::Line::PostProcess {
                    started: true,
                    name,
                }) => emit(tracker.on_postprocess(&name)),
                Some(progress::Line::File(path)) => output_file = Some(PathBuf::from(path)),
                Some(progress::Line::PostProcess { started: false, .. }) | None => {}
            }
        }

        let status = child.wait().await?;
        let log = stderr_log.await.unwrap_or_default();

        let cancelled = self
            .jobs
            .lock()
            .unwrap()
            .get(id)
            .is_some_and(|job| job.cancelled);
        if cancelled {
            remove_partial_files(&stream_files, tracker.is_multi_stream()).await;
            return Err(AppError::cancelled());
        }
        if !status.success() {
            return Err(errors::classify(&log));
        }
        output_file.ok_or_else(|| {
            AppError::internal("The download finished but the file couldn't be located.")
                .with_details(log)
        })
    }
}

fn default_output_dir(app: &AppHandle) -> Result<PathBuf> {
    app.path().download_dir().map_err(|err| {
        AppError::internal("Couldn't find your Downloads folder.").with_details(err.to_string())
    })
}

/// Reads a stream to the end, keeping the last [`MAX_LOG_LINES`] lines.
async fn collect_lines(stream: impl AsyncRead + Unpin) -> String {
    let mut reader = BufReader::new(stream);
    let mut lines: std::collections::VecDeque<String> = std::collections::VecDeque::new();
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
        let (Some(dir), Some(name)) = (
            file.parent(),
            file.file_name().and_then(|name| name.to_str()),
        ) else {
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
