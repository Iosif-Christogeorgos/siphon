//! Managed external binaries: yt-dlp, ffmpeg/ffprobe and Deno.
//!
//! They live outside the app bundle, in `<app-local-data>/bin/<tool>/`, so
//! each can update on its own schedule:
//!
//! ```text
//! bin/
//!   manifest.json        installed versions
//!   yt-dlp/              current
//!   yt-dlp.previous/     last known-good, kept for rollback
//!   yt-dlp.staging/      download in progress
//! ```

mod install;
mod notes;
pub mod sources;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};

use crate::error::{AppError, ErrorKind, Result};
use sources::Channel;

/// Event carrying a [`ToolStatus`] whenever a tool's state or progress changes.
pub const STATUS_EVENT: &str = "binaries://status";

/// Event carrying an [`UpdateNotice`] after a tool was updated.
pub const UPDATED_EVENT: &str = "binaries://updated";

/// How often to look for new versions.
pub const CHECK_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

/// ffmpeg's rolling builds change daily but rarely matter; refresh monthly.
const FFMPEG_REFRESH_AFTER: Duration = Duration::from_secs(30 * 24 * 60 * 60);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Tool {
    YtDlp,
    Ffmpeg,
    Deno,
}

impl Tool {
    pub const ALL: [Tool; 3] = [Tool::YtDlp, Tool::Ffmpeg, Tool::Deno];

    pub fn label(self) -> &'static str {
        match self {
            Tool::YtDlp => "yt-dlp",
            Tool::Ffmpeg => "FFmpeg",
            Tool::Deno => "Deno",
        }
    }

    /// Folder name under `bin/`.
    fn dir(self) -> &'static str {
        match self {
            Tool::YtDlp => "yt-dlp",
            Tool::Ffmpeg => "ffmpeg",
            Tool::Deno => "deno",
        }
    }

    /// Executables (without extension) that must exist for the tool to work.
    fn executables(self) -> &'static [&'static str] {
        match self {
            Tool::YtDlp => &["yt-dlp"],
            Tool::Ffmpeg => &["ffmpeg", "ffprobe"],
            Tool::Deno => &["deno"],
        }
    }

    fn version_flag(self) -> &'static str {
        match self {
            Tool::Ffmpeg => "-version",
            Tool::YtDlp | Tool::Deno => "--version",
        }
    }
}

/// Adds `.exe` on Windows.
fn exe(name: &str) -> String {
    format!("{name}{}", std::env::consts::EXE_SUFFIX)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolState {
    Missing,
    Checking,
    Downloading,
    Verifying,
    Installing,
    Ready,
    Failed,
}

#[derive(Debug, Clone, Serialize)]
pub struct ToolStatus {
    pub tool: Tool,
    pub state: ToolState,
    /// Installed version, if any.
    pub version: Option<String>,
    /// Download progress, 0–1, while `state` is `downloading`.
    pub progress: Option<f64>,
    /// Plain-language reason when `state` is `failed`.
    pub error: Option<String>,
    /// A newer version that is waiting for the user's go-ahead ("ask first").
    pub available: Option<Available>,
}

/// A newer release of a tool.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Available {
    pub version: String,
    /// One line about what it changes, from the release notes.
    pub summary: String,
}

/// Sent to the frontend after an update, for the toast.
#[derive(Debug, Clone, Serialize)]
pub struct UpdateNotice {
    pub tool: Tool,
    /// `None` on a first install.
    pub from: Option<String>,
    pub to: String,
    pub summary: String,
}

/// What to do about newer versions. Missing tools are always installed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdatePolicy {
    Install,
    /// Only report them (see [`ToolStatus::available`]).
    Ask,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Installed {
    version: String,
    /// Unix seconds.
    installed_at: u64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Manifest {
    #[serde(default)]
    tools: HashMap<Tool, Installed>,
    /// When versions were last compared with GitHub, in Unix seconds.
    #[serde(default)]
    last_check: u64,
}

pub struct BinaryManager {
    app: AppHandle,
    root: PathBuf,
    http: reqwest::Client,
    manifest: Mutex<Manifest>,
    statuses: Mutex<HashMap<Tool, ToolStatus>>,
    /// Serialises syncs so two checks can never swap the same folder at once.
    sync_lock: tokio::sync::Mutex<()>,
    /// Held shared by everything that runs a tool, and exclusively for the
    /// moment a tool's folder is swapped: a download never has its yt-dlp
    /// pulled out from under it, and never starts one mid-swap.
    use_gate: Arc<tokio::sync::RwLock<()>>,
}

impl BinaryManager {
    pub fn new(app: &AppHandle) -> Result<Self> {
        let root = app
            .path()
            .app_local_data_dir()
            .map_err(|err| AppError::setup("Couldn't locate the app data folder.").with_details(err.to_string()))?
            .join("bin");
        std::fs::create_dir_all(&root)?;

        let manifest: Manifest = std::fs::read(root.join("manifest.json"))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();

        let http = reqwest::Client::builder()
            .user_agent(concat!("Siphon/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(Duration::from_secs(15))
            .read_timeout(Duration::from_secs(30))
            .build()?;

        let manager = Self {
            app: app.clone(),
            root,
            http,
            manifest: Mutex::new(manifest),
            statuses: Mutex::new(HashMap::new()),
            sync_lock: tokio::sync::Mutex::new(()),
            use_gate: Arc::default(),
        };
        for tool in Tool::ALL {
            let status = manager.initial_status(tool);
            manager.statuses.lock().unwrap().insert(tool, status);
        }
        Ok(manager)
    }

    // ── Paths ───────────────────────────────────────────────────────────

    fn tool_dir(&self, tool: Tool) -> PathBuf {
        self.root.join(tool.dir())
    }

    pub fn yt_dlp(&self) -> PathBuf {
        self.tool_dir(Tool::YtDlp).join(exe("yt-dlp"))
    }

    pub fn deno(&self) -> PathBuf {
        self.tool_dir(Tool::Deno).join(exe("deno"))
    }

    /// Folder holding ffmpeg and ffprobe, for yt-dlp's `--ffmpeg-location`.
    pub fn ffmpeg_dir(&self) -> PathBuf {
        self.tool_dir(Tool::Ffmpeg)
    }

    /// The ffmpeg executable itself, for Siphon's own conversions.
    pub fn ffmpeg(&self) -> PathBuf {
        self.ffmpeg_dir().join(exe("ffmpeg"))
    }

    // ── Status ──────────────────────────────────────────────────────────

    pub fn statuses(&self) -> Vec<ToolStatus> {
        let statuses = self.statuses.lock().unwrap();
        Tool::ALL
            .iter()
            .filter_map(|tool| statuses.get(tool).cloned())
            .collect()
    }

    /// True when every tool can be launched.
    pub fn is_ready(&self) -> bool {
        Tool::ALL.iter().all(|&tool| self.installed(tool).is_some())
    }

    fn initial_status(&self, tool: Tool) -> ToolStatus {
        let mut status = ToolStatus {
            tool,
            state: ToolState::Missing,
            version: None,
            progress: None,
            error: None,
            available: None,
        };
        if let Some(installed) = self.installed(tool) {
            status.state = ToolState::Ready;
            status.version = Some(installed.version);
        }
        status
    }

    /// The manifest entry for `tool`, but only if its files are really on disk.
    fn installed(&self, tool: Tool) -> Option<Installed> {
        let dir = self.tool_dir(tool);
        let complete = tool.executables().iter().all(|name| dir.join(exe(name)).is_file());
        if !complete {
            return None;
        }
        self.manifest.lock().unwrap().tools.get(&tool).cloned()
    }

    fn update_status(&self, tool: Tool, change: impl FnOnce(&mut ToolStatus)) {
        let status = {
            let mut statuses = self.statuses.lock().unwrap();
            let Some(status) = statuses.get_mut(&tool) else {
                return;
            };
            change(status);
            status.clone()
        };
        let _ = self.app.emit(STATUS_EVENT, status);
    }

    fn set_state(&self, tool: Tool, state: ToolState) {
        self.update_status(tool, |status| {
            status.state = state;
            status.progress = None;
            status.error = None;
        });
    }

    /// Keep this alive while a yt-dlp, ffmpeg or Deno process runs.
    pub async fn in_use(&self) -> tokio::sync::OwnedRwLockReadGuard<()> {
        Arc::clone(&self.use_gate).read_owned().await
    }

    /// True when it has been a day since versions were last compared.
    pub fn check_is_due(&self) -> bool {
        let last = UNIX_EPOCH + Duration::from_secs(self.manifest.lock().unwrap().last_check);
        SystemTime::now()
            .duration_since(last)
            .map_or(true, |since| since >= CHECK_INTERVAL)
    }

    fn save_manifest(&self, change: impl FnOnce(&mut Manifest)) -> Result<()> {
        let json = {
            let mut manifest = self.manifest.lock().unwrap();
            change(&mut manifest);
            serde_json::to_vec_pretty(&*manifest)?
        };
        // Write-then-rename so a crash can't leave a half-written manifest.
        let tmp = self.root.join("manifest.json.tmp");
        std::fs::write(&tmp, json)?;
        std::fs::rename(&tmp, self.root.join("manifest.json"))?;
        Ok(())
    }

    fn record_installed(&self, tool: Tool, version: &str) -> Result<()> {
        let installed_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let entry = Installed {
            version: version.to_string(),
            installed_at,
        };
        self.save_manifest(|manifest| {
            manifest.tools.insert(tool, entry);
        })
    }

    // ── Install & update ────────────────────────────────────────────────

    /// Installs anything missing, and installs or reports newer versions
    /// according to `policy`. Per-tool failures are reported through
    /// statuses, not as an error.
    pub async fn sync_all(&self, channel: Channel, policy: UpdatePolicy) -> Vec<ToolStatus> {
        let _guard = self.sync_lock.lock().await;
        let outcomes =
            futures_util::future::join_all(Tool::ALL.map(|tool| self.sync_tool(tool, channel, policy))).await;
        // A swap put off because a download was running should be retried
        // soon, so only a clean round counts as "checked".
        if outcomes.iter().all(|settled| *settled) {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let _ = self.save_manifest(|manifest| manifest.last_check = now);
        }
        self.statuses()
    }

    /// Updates one tool to the newest release right away, whatever the
    /// policy. Returns what changed, or `None` if it was already current.
    pub async fn update_now(&self, tool: Tool, channel: Channel) -> Result<Option<UpdateNotice>> {
        let _guard = self.sync_lock.lock().await;
        let installed = self.installed(tool);
        let release = sources::latest(&self.http, tool, channel).await?;
        if installed
            .as_ref()
            .is_some_and(|installed| installed.version == release.version)
        {
            return Ok(None);
        }
        let result = self.install_release(tool, &release, installed.as_ref()).await;
        self.settle_status(tool, result.as_ref().err());
        result.map(Some)
    }

    /// Returns false if this tool should be looked at again soon.
    async fn sync_tool(&self, tool: Tool, channel: Channel, policy: UpdatePolicy) -> bool {
        let installed = self.installed(tool);
        self.set_state(
            tool,
            if installed.is_some() {
                ToolState::Checking
            } else {
                ToolState::Missing
            },
        );

        let outcome = self.sync_release(tool, channel, policy, installed.as_ref()).await;
        self.settle_status(tool, outcome.as_ref().err());
        !matches!(&outcome, Err(error) if error.kind == ErrorKind::Busy)
    }

    /// Puts a tool's status back to rest after a check or install.
    fn settle_status(&self, tool: Tool, error: Option<&AppError>) {
        // Whatever happened, a working install (old or new) means "ready".
        // Being offline or a failed update must never block downloads.
        let now_installed = self.installed(tool);
        self.update_status(tool, |status| {
            status.progress = None;
            status.version = now_installed.as_ref().map(|installed| installed.version.clone());
            match (&now_installed, error) {
                (Some(_), _) => {
                    status.state = ToolState::Ready;
                    status.error = None;
                }
                (None, Some(error)) => {
                    status.state = ToolState::Failed;
                    status.error = Some(error.message.clone());
                }
                (None, None) => status.state = ToolState::Missing,
            }
        });
    }

    async fn sync_release(
        &self,
        tool: Tool,
        channel: Channel,
        policy: UpdatePolicy,
        installed: Option<&Installed>,
    ) -> Result<()> {
        let release = sources::latest(&self.http, tool, channel).await?;

        if let Some(installed) = installed {
            let mut current = installed.version == release.version;
            if tool == Tool::Ffmpeg {
                let age = SystemTime::now()
                    .duration_since(UNIX_EPOCH + Duration::from_secs(installed.installed_at))
                    .unwrap_or_default();
                current |= age < FFMPEG_REFRESH_AFTER;
            }
            if current {
                self.update_status(tool, |status| status.available = None);
                return Ok(());
            }
            if policy == UpdatePolicy::Ask {
                let available = Available {
                    version: release.version.clone(),
                    summary: notes::summarize(tool, release.notes.as_deref()),
                };
                self.update_status(tool, |status| status.available = Some(available));
                return Ok(());
            }
        }

        self.install_release(tool, &release, installed).await.map(|_| ())
    }

    /// Installs `release`, records it, and announces it if it replaced an older version.
    async fn install_release(
        &self,
        tool: Tool,
        release: &sources::Release,
        installed: Option<&Installed>,
    ) -> Result<UpdateNotice> {
        install::install(self, tool, release).await?;
        self.record_installed(tool, &release.version)?;
        self.update_status(tool, |status| status.available = None);

        let notice = UpdateNotice {
            tool,
            from: installed.map(|installed| installed.version.clone()),
            to: release.version.clone(),
            summary: notes::summarize(tool, release.notes.as_deref()),
        };
        // First installs are shown by the setup screen; only updates get a toast.
        if notice.from.is_some() {
            let _ = self.app.emit(UPDATED_EVENT, notice.clone());
        }
        Ok(notice)
    }
}
