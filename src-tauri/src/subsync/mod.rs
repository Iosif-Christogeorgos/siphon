//! Subtitle sync (docs/subtitle-sync.md): gives every line of a subtitle file the
//! moment its words are spoken in the film, with speech recognition on the
//! graphics card. An optional download, made only when the user asks for it
//! in Settings, and removable there:
//!
//! ```text
//! subsync/
//!   uv/uv.exe          Astral's uv, which installs the rest (checksum checked)
//!   python/            a private Python 3.14: not registered with Windows, not on the PATH
//!   env/               its packages: PyTorch with CUDA, Qwen3-ASR, sentence-transformers, Silero VAD
//!   models/            Qwen3-ASR 1.7B, Qwen3-ForcedAligner 0.6B, BGE-M3 (half size), at pinned revisions
//!   siphon_sync.py     the sync itself (src-tauri/sync), written out from the app
//!   work/              synced copies, kept only while a choice of tracks or an unfinished copy uses them
//!   ready.json         written last: the install is whole
//! ```

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Emitter, Manager};
use tokio::io::{AsyncBufReadExt, BufReader};

use crate::binaries::sources::find_sha256;
use crate::binaries::BinaryManager;
use crate::error::{AppError, ErrorKind, Result};
use crate::process;

/// Carries a [`SubSyncStatus`] whenever the download's state or progress changes.
pub const STATUS_EVENT: &str = "subsync://status";
/// Carries a [`SyncProgress`] while a file is synced.
pub const PROGRESS_EVENT: &str = "subsync://progress";

const SCRIPT: &str = include_str!("../../sync/siphon_sync.py");
/// Every package but PyTorch, at the versions tested (`uv pip freeze`, 2026-11-26).
const REQUIREMENTS: &str = include_str!("../../sync/requirements.txt");
const UV_VERSION: &str = "0.13.10";
const UV_ASSET: &str = "uv-x86_64-pc-windows-msvc.zip";
const PYTHON: &str = "3.14";
const TORCH: [&str; 2] = ["torch==2.11.0+cu128", "torchaudio==2.11.0+cu128"];
const TORCH_INDEX: &str = "https://download.pytorch.org/whl/cu128";
/// Raised when the packages or models change: an older install is made again.
const VERSION: u32 = 1;
/// What is downloaded, measured 2026-11-26: the packages (PyTorch alone is 2.6 GB) and the models.
const LIBRARIES_BYTES: u64 = 3_300_000_000;
const MODELS_BYTES: u64 = 8_836_384_618;
/// What it takes on disk once installed: the packages 4.9 GB, the models 7.3 GB.
const DISK_BYTES: u64 = 12_300_000_000;
/// The models and their work need this much of the graphics card's memory.
const MIN_VRAM_MB: u64 = 8000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    /// No NVIDIA graphics card with memory enough: the download isn't offered.
    NoGpu,
    Absent,
    Installing,
    Ready,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Step {
    Tools,
    Python,
    Libraries,
    Models,
}

#[derive(Debug, Clone, Serialize)]
pub struct SubSyncStatus {
    pub state: State,
    pub step: Option<Step>,
    /// 0–1 over the whole download, while installing.
    pub progress: Option<f64>,
    pub error: Option<String>,
    /// "NVIDIA GeForce RTX 5080 (16 GB)", or why none can be used.
    pub gpu: Option<String>,
    pub download_bytes: u64,
    pub disk_bytes: u64,
}

/// How far a sync has got: which file, at what stage, how far through it.
#[derive(Debug, Clone, Serialize)]
pub struct SyncProgress {
    pub key: String,
    pub stage: String,
    pub fraction: f64,
}

/// What a sync found, as the script reports it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncSummary {
    /// Lines in the file.
    pub lines: usize,
    /// Lines given the time of their own words.
    pub by_words: usize,
    /// Lines no speech could be found for, placed by the lines around them.
    pub placed: usize,
    /// Where the first of those now start, in seconds.
    #[serde(default)]
    pub placed_at: Vec<f64>,
    /// In ASS: signs, songs and drawings, moved with the lines spoken around them.
    #[serde(default)]
    pub moved: usize,
    pub seconds: f64,
}

/// The subtitles a sync gives new times to.
pub enum Subtitles<'a> {
    /// A subtitle file, read in `charset` when it isn't UTF-8.
    File { path: &'a Path, charset: Option<&'a str> },
    /// A text subtitle track of the film itself, by its place among its streams.
    Track(usize),
}

pub struct SubSync {
    app: AppHandle,
    root: PathBuf,
    http: reqwest::Client,
    status: Mutex<SubSyncStatus>,
    /// The graphics card, looked for once: (name, memory in MB), or None.
    gpu: tokio::sync::OnceCell<Option<(String, u64)>>,
    installing: tokio::sync::Mutex<()>,
    /// One sync at a time: the models fill most of the graphics card.
    syncing: tokio::sync::Mutex<()>,
    /// The folder the sync running now works in.
    syncing_in: Mutex<Option<PathBuf>>,
    running: AtomicU32,
    cancelled: AtomicBool,
}

impl SubSync {
    pub fn new(app: &AppHandle) -> Result<Self> {
        let root = app
            .path()
            .app_local_data_dir()
            .map_err(|err| AppError::setup("Couldn't locate the app data folder.").with_details(err.to_string()))?
            .join("subsync");
        let http = crate::throttle::follow_system_proxy(reqwest::Client::builder())
            .user_agent(concat!("Siphon/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(Duration::from_secs(15))
            .read_timeout(Duration::from_secs(60))
            .build()?;
        let sync = Self {
            app: app.clone(),
            status: Mutex::new(SubSyncStatus {
                state: State::Absent,
                step: None,
                progress: None,
                error: None,
                gpu: None,
                download_bytes: LIBRARIES_BYTES + MODELS_BYTES,
                disk_bytes: DISK_BYTES,
            }),
            root,
            http,
            gpu: tokio::sync::OnceCell::new(),
            installing: tokio::sync::Mutex::new(()),
            syncing: tokio::sync::Mutex::new(()),
            syncing_in: Mutex::new(None),
            running: AtomicU32::new(0),
            cancelled: AtomicBool::new(false),
        };
        if sync.is_installed() {
            sync.status.lock().unwrap().state = State::Ready;
        }
        // Gone at launch as well as at closing, in case the last session never closed.
        sync.clear_work();
        Ok(sync)
    }

    fn python(&self) -> PathBuf {
        self.root.join("env").join("Scripts").join("python.exe")
    }

    fn models(&self) -> PathBuf {
        self.root.join("models")
    }

    fn is_installed(&self) -> bool {
        #[derive(Deserialize)]
        struct Ready {
            version: u32,
        }
        let ready = std::fs::read(self.root.join("ready.json"))
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Ready>(&bytes).ok());
        ready.is_some_and(|ready| ready.version == VERSION)
            && self.python().is_file()
            && self.models().join("ready.json").is_file()
    }

    /// The current state, with the graphics card looked for the first time.
    pub async fn status(&self) -> SubSyncStatus {
        let gpu = self.gpu().await;
        let mut status = self.status.lock().unwrap();
        status.gpu = Some(match &gpu {
            Some((name, mb)) => format!("{name} ({} GB)", (mb + 512) / 1024),
            None => "No NVIDIA graphics card was found.".to_string(),
        });
        let usable = gpu.as_ref().is_some_and(|(_, mb)| *mb >= MIN_VRAM_MB);
        if !usable && status.state == State::Absent {
            status.state = State::NoGpu;
            if let Some((name, mb)) = &gpu {
                status.gpu = Some(format!(
                    "{name} has {} GB of memory; subtitle sync needs 8 GB.",
                    (mb + 512) / 1024
                ));
            }
        }
        status.clone()
    }

    async fn gpu(&self) -> Option<(String, u64)> {
        self.gpu
            .get_or_init(|| async {
                let mut cmd = process::command("nvidia-smi");
                cmd.args(["--query-gpu=name,memory.total", "--format=csv,noheader,nounits"]);
                let output = process::output_within(cmd, Duration::from_secs(10))
                    .await
                    .ok()
                    .flatten()?;
                parse_gpu(&String::from_utf8_lossy(&output.stdout))
            })
            .await
            .clone()
    }

    fn update(&self, change: impl FnOnce(&mut SubSyncStatus)) {
        let status = {
            let mut status = self.status.lock().unwrap();
            change(&mut status);
            status.clone()
        };
        let _ = self.app.emit(STATUS_EVENT, status);
    }

    fn at(&self, step: Step, progress: f64) {
        self.update(|status| {
            status.state = State::Installing;
            status.step = Some(step);
            status.progress = Some(progress.clamp(0.0, 1.0));
            status.error = None;
        });
    }

    // ── The download ────────────────────────────────────────────────────

    /// Downloads and installs everything, carrying on from what an earlier
    /// attempt finished. Progress comes as [`STATUS_EVENT`]s.
    pub async fn install(&self) -> Result<()> {
        let Ok(_installing) = self.installing.try_lock() else {
            return Ok(());
        };
        let status = self.status().await;
        if status.state == State::NoGpu {
            return Err(AppError::setup(status.gpu.unwrap_or_default()));
        }
        let result = self.install_steps().await;
        match &result {
            Ok(()) => self.update(|status| {
                status.state = State::Ready;
                status.step = None;
                status.progress = None;
                status.error = None;
            }),
            Err(error) => {
                crate::log::line(
                    &self.app,
                    &format!("subtitle sync install failed: {} {:?}", error.message, error.details),
                );
                self.update(|status| {
                    status.state = State::Failed;
                    status.progress = None;
                    status.error = Some(error.message.clone());
                });
            }
        }
        result
    }

    async fn install_steps(&self) -> Result<()> {
        let total = (LIBRARIES_BYTES + MODELS_BYTES) as f64;
        let share_libraries = LIBRARIES_BYTES as f64 / total;
        tokio::fs::create_dir_all(&self.root).await?;
        let _ = tokio::fs::remove_file(self.root.join("ready.json")).await;

        self.at(Step::Tools, 0.0);
        let uv = self.root.join("uv").join("uv.exe");
        if !uv.is_file() {
            self.fetch_uv(&uv).await?;
        }

        self.at(Step::Python, 0.005);
        if !self.python().is_file() {
            self.uv(
                &uv,
                &["python", "install", PYTHON, "--no-bin", "--no-registry"],
                "Python couldn't be installed.",
            )
            .await?;
            let env = self.root.join("env");
            let env = env.to_string_lossy();
            self.uv(&uv, &["venv", &env, "--python", PYTHON], "Python couldn't be set up.")
                .await?;
        }

        self.at(Step::Libraries, 0.01);
        let cache = self.root.join("cache");
        let watching = self.watch_folder(cache.clone(), LIBRARIES_BYTES, |done| {
            self.at(Step::Libraries, 0.01 + done * (share_libraries - 0.01));
        });
        let python = self.python();
        let python = python.to_string_lossy();
        let mut torch = vec!["pip", "install", "--python", &python, "--index-url", TORCH_INDEX];
        torch.extend(TORCH);
        let requirements = self.root.join("requirements.txt");
        tokio::fs::write(&requirements, REQUIREMENTS).await?;
        let requirements = requirements.to_string_lossy();
        let libraries = async {
            self.uv(&uv, &torch, "PyTorch couldn't be installed.").await?;
            self.uv(
                &uv,
                &["pip", "install", "--python", &python, "--no-deps", "-r", &requirements],
                "The AI libraries couldn't be installed.",
            )
            .await
        };
        let installed = tokio::select! {
            result = libraries => result,
            () = watching => unreachable!("the watch runs until stopped"),
        };
        installed?;
        // The packages are copied out of uv's cache (UV_LINK_MODE=copy), so it can go.
        let _ = tokio::fs::remove_dir_all(&cache).await;

        self.at(Step::Models, share_libraries);
        self.write_script().await?;
        let script = self.root.join("siphon_sync.py");
        let models = self.models();
        let mut cmd = self.python_command();
        cmd.arg(&script).arg("setup").arg("--models").arg(&models);
        let outcome = self
            .run_script(cmd, |line| {
                if line.get("stage").and_then(|v| v.as_str()) == Some("models") {
                    let done = line.get("done").and_then(serde_json::Value::as_f64).unwrap_or(0.0);
                    let all = line
                        .get("total")
                        .and_then(serde_json::Value::as_f64)
                        .unwrap_or(1.0)
                        .max(1.0);
                    self.at(Step::Models, share_libraries + (done / all) * (1.0 - share_libraries));
                }
            })
            .await
            .map_err(|err| AppError::setup("The models couldn't be downloaded.").with_details(err))?;
        if let Err(message) = outcome {
            return Err(AppError::setup("The models couldn't be downloaded.").with_details(message));
        }
        tokio::fs::write(self.root.join("ready.json"), format!("{{\"version\": {VERSION}}}")).await?;
        Ok(())
    }

    /// uv, from its GitHub release, checked against its published checksum.
    async fn fetch_uv(&self, dest: &Path) -> Result<()> {
        let base = format!("https://github.com/astral-sh/uv/releases/download/{UV_VERSION}/{UV_ASSET}");
        let checksum = self
            .http
            .get(format!("{base}.sha256"))
            .send()
            .await?
            .error_for_status()?
            .text()
            .await?;
        let expected = find_sha256(&checksum, UV_ASSET)
            .ok_or_else(|| AppError::setup("uv has no published checksum, so it wasn't installed."))?;
        let response = self.http.get(&base).send().await?.error_for_status()?;
        let mut stream = response.bytes_stream();
        let mut bytes = Vec::new();
        let mut hasher = Sha256::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            hasher.update(&chunk);
            bytes.extend_from_slice(&chunk);
        }
        if hex::encode(hasher.finalize()) != expected {
            return Err(AppError::setup(
                "The uv download didn't match its published checksum, so it wasn't installed.",
            ));
        }
        let dir = dest.parent().map(Path::to_path_buf).unwrap_or_default();
        let dest = dest.to_path_buf();
        tokio::task::spawn_blocking(move || -> Result<()> {
            std::fs::create_dir_all(&dir)?;
            let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes))
                .map_err(|err| AppError::setup("Couldn't unpack uv.").with_details(err.to_string()))?;
            let mut entry = zip
                .by_name("uv.exe")
                .map_err(|err| AppError::setup("The uv download didn't hold uv.exe.").with_details(err.to_string()))?;
            let mut out = std::fs::File::create(&dest)?;
            std::io::copy(&mut entry, &mut out)?;
            Ok(())
        })
        .await
        .map_err(|err| AppError::internal("Unpacking uv was interrupted.").with_details(err.to_string()))?
    }

    /// Runs uv with everything it makes kept inside Siphon's folder.
    async fn uv(&self, uv: &Path, args: &[&str], failed: &str) -> Result<()> {
        let mut cmd = process::command(uv);
        cmd.args(args)
            .current_dir(&self.root)
            .env("UV_PYTHON_INSTALL_DIR", self.root.join("python"))
            .env("UV_CACHE_DIR", self.root.join("cache"))
            .env("UV_LINK_MODE", "copy")
            // Only the Python installed here, never one on the PC.
            .env("UV_PYTHON_PREFERENCE", "only-managed")
            .env("UV_NO_CONFIG", "1")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let output = cmd.output().await?;
        if output.status.success() {
            return Ok(());
        }
        let details = String::from_utf8_lossy(&output.stderr);
        let tail: Vec<&str> = details.lines().rev().take(20).collect();
        Err(AppError::setup(failed).with_details(tail.into_iter().rev().collect::<Vec<_>>().join("\n")))
    }

    /// Reports how full `folder` is against `expected` bytes, twice a second, until dropped.
    async fn watch_folder(&self, folder: PathBuf, expected: u64, report: impl Fn(f64) + Send) {
        loop {
            tokio::time::sleep(Duration::from_millis(500)).await;
            let folder = folder.clone();
            let size = tokio::task::spawn_blocking(move || folder_size(&folder))
                .await
                .unwrap_or(0);
            report((size as f64 / expected as f64).min(0.99));
        }
    }

    async fn write_script(&self) -> Result<()> {
        let path = self.root.join("siphon_sync.py");
        if tokio::fs::read_to_string(&path).await.ok().as_deref() != Some(SCRIPT) {
            tokio::fs::write(&path, SCRIPT).await?;
        }
        Ok(())
    }

    fn python_command(&self) -> tokio::process::Command {
        let mut cmd = process::command(self.python());
        let bins = self.app.state::<Arc<BinaryManager>>();
        let path = std::env::var_os("PATH").unwrap_or_default();
        let mut paths = vec![bins.ffmpeg_dir()];
        paths.extend(std::env::split_paths(&path));
        cmd.current_dir(&self.root)
            .env("PATH", std::env::join_paths(paths).unwrap_or(path))
            .env("PYTHONUTF8", "1")
            .env("PYTHONIOENCODING", "utf-8")
            .env("PYTHONNOUSERSITE", "1")
            .env("HF_HOME", self.root.join("hf"))
            .env("HF_HUB_DISABLE_TELEMETRY", "1")
            .env("HF_HUB_DISABLE_SYMLINKS_WARNING", "1")
            .env("TRANSFORMERS_VERBOSITY", "error")
            .env("TOKENIZERS_PARALLELISM", "false")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        cmd
    }

    /// Runs the script, handing each JSON line to `on_line`. Ok(Ok(done line)) when it
    /// finished, Ok(Err(message)) when it reported a failure, Err(details) when it died.
    async fn run_script(
        &self,
        mut cmd: tokio::process::Command,
        on_line: impl Fn(&serde_json::Value),
    ) -> std::result::Result<std::result::Result<serde_json::Value, String>, String> {
        process::below_others(&mut cmd);
        let mut child = cmd.spawn().map_err(|err| err.to_string())?;
        process::end_with_siphon(&child);
        self.running.store(child.id().unwrap_or(0), Ordering::SeqCst);
        let stdout = child.stdout.take().ok_or("no output")?;
        let stderr = child.stderr.take().ok_or("no output")?;
        let errors = tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            let mut tail = std::collections::VecDeque::new();
            while let Ok(Some(line)) = lines.next_line().await {
                if tail.len() == 40 {
                    tail.pop_front();
                }
                tail.push_back(line);
            }
            Vec::from(tail).join("\n")
        });
        let mut ending = None;
        let mut lines = BufReader::new(stdout).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) else {
                continue;
            };
            match value.get("event").and_then(|v| v.as_str()) {
                Some("progress") => on_line(&value),
                Some("done") => ending = Some(Ok(value)),
                Some("error") => {
                    ending = Some(Err(value
                        .get("message")
                        .and_then(|v| v.as_str())
                        .unwrap_or("The sync failed.")
                        .to_string()));
                }
                _ => {}
            }
        }
        let status = child.wait().await.map_err(|err| err.to_string())?;
        self.running.store(0, Ordering::SeqCst);
        let errors = errors.await.unwrap_or_default();
        match ending {
            Some(ending) => Ok(ending),
            None => Err(format!("{status}\n{errors}")),
        }
    }

    /// Takes the download away again, all of it.
    pub async fn remove(&self) -> Result<()> {
        let Ok(_installing) = self.installing.try_lock() else {
            return Err(AppError::new(ErrorKind::Busy, "Subtitle sync is being downloaded."));
        };
        let Ok(_syncing) = self.syncing.try_lock() else {
            return Err(AppError::new(ErrorKind::Busy, "A subtitle file is being synced."));
        };
        let root = self.root.clone();
        tokio::task::spawn_blocking(move || {
            std::fs::remove_dir_all(&root).or_else(|err| match err.kind() {
                std::io::ErrorKind::NotFound => Ok(()),
                _ => Err(err),
            })
        })
        .await
        .map_err(|err| AppError::internal("Removing was interrupted.").with_details(err.to_string()))??;
        self.update(|status| {
            status.state = State::Absent;
            status.step = None;
            status.progress = None;
            status.error = None;
        });
        Ok(())
    }

    // ── A sync ──────────────────────────────────────────────────────────

    /// Syncs `subs` to the speech in stream `stream` of `video`, and returns
    /// the synced copy's path and what was found. `key` names the sync in the
    /// progress events.
    pub async fn sync(
        &self,
        key: &str,
        video: &Path,
        stream: usize,
        language: Option<&str>,
        subs: Subtitles<'_>,
    ) -> Result<(PathBuf, SyncSummary)> {
        if !self.is_installed() {
            return Err(AppError::setup("Subtitle sync isn't downloaded. It's in Settings."));
        }
        let Ok(_syncing) = self.syncing.try_lock() else {
            return Err(AppError::new(
                ErrorKind::Busy,
                "Another subtitle file is being synced. Try again when it's done.",
            ));
        };
        // FFmpeg reads the film all through the sync: it isn't swapped for a new version meanwhile.
        let bins = self.app.state::<Arc<BinaryManager>>().inner().clone();
        let _tools = bins.in_use().await;
        self.cancelled.store(false, Ordering::SeqCst);
        self.write_script().await?;
        let work = self.root.join("work").join(work_id());
        tokio::fs::create_dir_all(&work).await?;
        *self.syncing_in.lock().unwrap() = Some(work.clone());
        let made = self.sync_in(&work, key, &bins, video, stream, language, subs).await;
        *self.syncing_in.lock().unwrap() = None;
        if made.is_err() {
            let _ = tokio::fs::remove_dir_all(&work).await;
        }
        made
    }

    /// The sync itself, in a folder of its own, `work`, which holds only the
    /// synced copy once it has finished; [`Self::sync`] takes the folder away
    /// if it fails.
    #[allow(clippy::too_many_arguments)]
    async fn sync_in(
        &self,
        work: &Path,
        key: &str,
        bins: &BinaryManager,
        video: &Path,
        stream: usize,
        language: Option<&str>,
        subs: Subtitles<'_>,
    ) -> Result<(PathBuf, SyncSummary)> {
        // A track of the film's own is taken out to a file, needed only while the sync reads it.
        // ASS and SSA stay so (their styles, signs and songs are kept); anything else is SRT.
        let (subs, taken_out, charset, stem, kind) = match subs {
            Subtitles::File { path, charset } => {
                let stem = path
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "subtitles".into());
                let ext = path
                    .extension()
                    .map(|e| e.to_string_lossy().to_ascii_lowercase())
                    .unwrap_or_default();
                let kind = if ext == "ass" || ext == "ssa" {
                    ext
                } else {
                    "srt".into()
                };
                (path.to_path_buf(), false, charset, stem, kind)
            }
            Subtitles::Track(index) => {
                let film = video
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "film".into());
                let stem = format!("{film} (track {index})");
                let _ = self.app.emit(
                    PROGRESS_EVENT,
                    SyncProgress {
                        key: key.to_string(),
                        stage: "subtitles".into(),
                        fraction: 0.0,
                    },
                );
                let kind = if self.styled(&bins.ffprobe(), video, index).await {
                    "ass"
                } else {
                    "srt"
                };
                let path = work.join(format!("{stem}.{kind}"));
                self.take_out(&bins.ffmpeg(), video, index, kind, &path).await?;
                (path, true, None, stem, kind.to_string())
            }
        };
        let out = work.join(format!("{stem}.synced.{kind}"));

        let mut cmd = self.python_command();
        cmd.env("HF_HUB_OFFLINE", "1")
            .arg(self.root.join("siphon_sync.py"))
            .arg("run")
            .arg("--models")
            .arg(self.models())
            .arg("--video")
            .arg(video)
            .arg("--stream")
            .arg(stream.to_string())
            .arg("--subs")
            .arg(&subs)
            .arg("--out")
            .arg(&out);
        if let Some(language) = language {
            cmd.arg("--language").arg(language);
        }
        if let Some(charset) = charset {
            cmd.arg("--charset").arg(charset);
        }
        let app = self.app.clone();
        let outcome = self
            .run_script(cmd, |line| {
                let progress = SyncProgress {
                    key: key.to_string(),
                    stage: line.get("stage").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                    fraction: line.get("fraction").and_then(serde_json::Value::as_f64).unwrap_or(0.0),
                };
                let _ = app.emit(PROGRESS_EVENT, progress);
            })
            .await;
        if taken_out {
            let _ = tokio::fs::remove_file(&subs).await;
        }
        if self.cancelled.swap(false, Ordering::SeqCst) {
            return Err(AppError::cancelled());
        }
        let done = match outcome {
            Ok(Ok(done)) => done,
            Ok(Err(message)) => return Err(AppError::new(ErrorKind::Internal, message)),
            Err(details) => {
                crate::log::line(&self.app, &format!("subtitle sync failed: {details}"));
                return Err(AppError::internal("The sync stopped unexpectedly.").with_details(details));
            }
        };
        let summary: SyncSummary = serde_json::from_value(done)
            .map_err(|err| AppError::internal("The sync's report couldn't be read.").with_details(err.to_string()))?;
        crate::log::line(
            &self.app,
            &format!(
                "subtitle sync: {} of {} lines by their words, {} placed by their neighbours, {:.0} s",
                summary.by_words, summary.lines, summary.placed, summary.seconds
            ),
        );
        Ok((out, summary))
    }

    /// Takes away every synced copy but those at `keep`, and the folder of a
    /// sync running now: a synced copy is kept only while a choice of tracks
    /// or an unfinished copy uses it (`src/stores/subsync.ts` says which).
    pub async fn keep_only(&self, keep: &[PathBuf]) {
        let Ok(mut folders) = tokio::fs::read_dir(self.root.join("work")).await else {
            return;
        };
        let running = self.syncing_in.lock().unwrap().clone();
        while let Ok(Some(entry)) = folders.next_entry().await {
            let path = entry.path();
            let used = keep.iter().any(|kept| kept.parent() == Some(path.as_path()));
            if used || running.as_ref() == Some(&path) {
                continue;
            }
            if path.is_dir() {
                let _ = tokio::fs::remove_dir_all(&path).await;
            } else {
                let _ = tokio::fs::remove_file(&path).await;
            }
        }
    }

    /// Takes away every synced copy. None is of use once Siphon closes: files
    /// added to a film, synced copies among them, aren't kept across a restart.
    pub fn clear_work(&self) {
        let _ = std::fs::remove_dir_all(self.root.join("work"));
    }

    /// Whether subtitle track `index` of `video` is ASS or SSA, which a sync keeps as it is.
    async fn styled(&self, ffprobe: &Path, video: &Path, index: usize) -> bool {
        let mut cmd = process::command(ffprobe);
        cmd.args(["-v", "error", "-select_streams", &index.to_string()])
            .args(["-show_entries", "stream=codec_name", "-of", "csv=p=0"])
            .arg(video);
        cmd.output()
            .await
            .is_ok_and(|out| matches!(String::from_utf8_lossy(&out.stdout).trim(), "ass" | "ssa"))
    }

    /// Writes text subtitle track `index` of `video` out to a file, for a sync
    /// of subtitles the film already has: as ASS when it is (`kind`), copied as
    /// it is, else as SRT. FFmpeg reads the whole film for it (a track's packets
    /// are spread all through it): seconds on an SSD.
    async fn take_out(&self, ffmpeg: &Path, video: &Path, index: usize, kind: &str, file: &Path) -> Result<()> {
        let codec = if kind == "ass" { "copy" } else { "srt" };
        let mut cmd = process::command(ffmpeg);
        cmd.args(["-v", "error", "-nostdin", "-y", "-i"])
            .arg(video)
            .args(["-map", &format!("0:{index}"), "-c:s", codec, "-f", kind])
            .arg(file)
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        process::below_others(&mut cmd);
        let child = cmd.spawn()?;
        process::end_with_siphon(&child);
        self.running.store(child.id().unwrap_or(0), Ordering::SeqCst);
        let output = child.wait_with_output().await;
        self.running.store(0, Ordering::SeqCst);
        if self.cancelled.load(Ordering::SeqCst) {
            return Err(AppError::cancelled());
        }
        let output = output?;
        if !output.status.success() {
            let details = String::from_utf8_lossy(&output.stderr).trim().to_string();
            crate::log::line(
                &self.app,
                &format!("subtitle sync: taking out track {index} failed: {details}"),
            );
            return Err(AppError::internal("Couldn't take the subtitles out of the film.").with_details(details));
        }
        Ok(())
    }

    /// Stops the sync running now, if any.
    pub async fn cancel(&self) {
        let pid = self.running.load(Ordering::SeqCst);
        if pid != 0 {
            self.cancelled.store(true, Ordering::SeqCst);
            process::kill_tree(pid).await;
        }
    }
}

/// "NVIDIA GeForce RTX 5080, 16303" → the first card's name and memory in MB.
fn parse_gpu(output: &str) -> Option<(String, u64)> {
    let line = output.lines().find(|line| !line.trim().is_empty())?;
    let (name, mb) = line.rsplit_once(',')?;
    Some((name.trim().to_string(), mb.trim().parse().ok()?))
}

fn folder_size(path: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(path) else { return 0 };
    entries
        .filter_map(std::result::Result::ok)
        .map(|entry| match entry.file_type() {
            Ok(kind) if kind.is_dir() => folder_size(&entry.path()),
            _ => entry.metadata().map(|meta| meta.len()).unwrap_or(0),
        })
        .sum()
}

/// A folder name no other sync has: the time and a counter.
fn work_id() -> String {
    static COUNT: AtomicU32 = AtomicU32::new(0);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_millis());
    format!("{now:x}-{}", COUNT.fetch_add(1, Ordering::SeqCst))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_graphics_card_nvidia_smi_lists() {
        assert_eq!(
            parse_gpu("NVIDIA GeForce RTX 5080, 16303\n"),
            Some(("NVIDIA GeForce RTX 5080".to_string(), 16303))
        );
        // The first card of two; a name with a comma in it.
        assert_eq!(
            parse_gpu("Quadro RTX 4000, Laptop, 8192\nNVIDIA GeForce GTX 1050, 2048\n"),
            Some(("Quadro RTX 4000, Laptop".to_string(), 8192))
        );
        assert_eq!(parse_gpu(""), None);
        assert_eq!(parse_gpu("No devices were found"), None);
    }

    #[test]
    fn the_requirements_pin_every_package_and_leave_pytorch_to_its_own_index() {
        for line in REQUIREMENTS.lines().filter(|line| !line.trim().is_empty()) {
            assert!(line.contains("=="), "not pinned: {line}");
            assert!(
                !line.starts_with("torch=="),
                "PyTorch comes from its CUDA index: {line}"
            );
        }
        assert!(SCRIPT.contains("def run(args)"));
    }
}
