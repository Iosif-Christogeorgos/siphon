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
pub mod sources;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};

use crate::error::{AppError, Result};
use sources::Channel;

/// Event carrying a [`ToolStatus`] whenever a tool's state or progress changes.
pub const STATUS_EVENT: &str = "binaries://status";

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
}

pub struct BinaryManager {
    app: AppHandle,
    root: PathBuf,
    http: reqwest::Client,
    manifest: Mutex<Manifest>,
    statuses: Mutex<HashMap<Tool, ToolStatus>>,
    /// Serialises syncs so two checks can never swap the same folder at once.
    sync_lock: tokio::sync::Mutex<()>,
}

impl BinaryManager {
    pub fn new(app: &AppHandle) -> Result<Self> {
        let root = app
            .path()
            .app_local_data_dir()
            .map_err(|err| {
                AppError::setup("Couldn't locate the app data folder.")
                    .with_details(err.to_string())
            })?
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
    /// `None` means "not managed here, let yt-dlp find it on PATH".
    pub fn ffmpeg_dir(&self) -> Option<PathBuf> {
        if sources::is_managed(Tool::Ffmpeg) {
            Some(self.tool_dir(Tool::Ffmpeg))
        } else {
            find_system_ffmpeg()
        }
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
        Tool::ALL
            .iter()
            .all(|&tool| match sources::is_managed(tool) {
                true => self.installed(tool).is_some(),
                false => find_system_ffmpeg().is_some(),
            })
    }

    fn initial_status(&self, tool: Tool) -> ToolStatus {
        let mut status = ToolStatus {
            tool,
            state: ToolState::Missing,
            version: None,
            progress: None,
            error: None,
        };
        if !sources::is_managed(tool) {
            if find_system_ffmpeg().is_some() {
                status.state = ToolState::Ready;
            } else {
                status.state = ToolState::Failed;
                status.error = Some(
                    "FFmpeg wasn't found. Install it with Homebrew: brew install ffmpeg".into(),
                );
            }
        } else if let Some(installed) = self.installed(tool) {
            status.state = ToolState::Ready;
            status.version = Some(installed.version);
        }
        status
    }

    /// The manifest entry for `tool`, but only if its files are really on disk.
    fn installed(&self, tool: Tool) -> Option<Installed> {
        let dir = self.tool_dir(tool);
        let complete = tool
            .executables()
            .iter()
            .all(|name| dir.join(exe(name)).is_file());
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

    fn record_installed(&self, tool: Tool, version: &str) -> Result<()> {
        let json = {
            let mut manifest = self.manifest.lock().unwrap();
            let installed_at = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            manifest.tools.insert(
                tool,
                Installed {
                    version: version.to_string(),
                    installed_at,
                },
            );
            serde_json::to_vec_pretty(&*manifest)?
        };
        // Write-then-rename so a crash can't leave a half-written manifest.
        let tmp = self.root.join("manifest.json.tmp");
        std::fs::write(&tmp, json)?;
        std::fs::rename(&tmp, self.root.join("manifest.json"))?;
        Ok(())
    }

    // ── Install & update ────────────────────────────────────────────────

    /// Installs anything missing and updates anything outdated.
    /// Per-tool failures are reported through statuses, not as an error.
    pub async fn sync_all(&self, channel: Channel) -> Vec<ToolStatus> {
        let _guard = self.sync_lock.lock().await;
        let managed = Tool::ALL
            .into_iter()
            .filter(|&tool| sources::is_managed(tool));
        futures_util::future::join_all(managed.map(|tool| self.sync_tool(tool, channel))).await;
        self.statuses()
    }

    async fn sync_tool(&self, tool: Tool, channel: Channel) {
        let installed = self.installed(tool);
        self.set_state(
            tool,
            if installed.is_some() {
                ToolState::Checking
            } else {
                ToolState::Missing
            },
        );

        let outcome = self
            .install_if_newer(tool, channel, installed.as_ref())
            .await;

        // Whatever happened, a working install (old or new) means "ready".
        // Being offline or a failed update must never block downloads.
        let now_installed = self.installed(tool);
        self.update_status(tool, |status| {
            status.progress = None;
            status.version = now_installed.as_ref().map(|i| i.version.clone());
            match (&now_installed, &outcome) {
                (Some(_), _) => {
                    status.state = ToolState::Ready;
                    status.error = None;
                }
                (None, Err(err)) => {
                    status.state = ToolState::Failed;
                    status.error = Some(err.message.clone());
                }
                (None, Ok(())) => status.state = ToolState::Missing,
            }
        });
    }

    async fn install_if_newer(
        &self,
        tool: Tool,
        channel: Channel,
        installed: Option<&Installed>,
    ) -> Result<()> {
        let release = sources::latest(&self.http, tool, channel).await?;

        if let Some(installed) = installed {
            if installed.version == release.version {
                return Ok(());
            }
            if tool == Tool::Ffmpeg {
                let age = SystemTime::now()
                    .duration_since(UNIX_EPOCH + Duration::from_secs(installed.installed_at))
                    .unwrap_or_default();
                if age < FFMPEG_REFRESH_AFTER {
                    return Ok(());
                }
            }
        }

        install::install(self, tool, &release).await?;
        self.record_installed(tool, &release.version)
    }
}

/// Looks for an ffmpeg the user installed themselves (macOS only in practice).
/// GUI apps don't inherit the shell's PATH there, so check Homebrew's folders too.
fn find_system_ffmpeg() -> Option<PathBuf> {
    let path = std::env::var_os("PATH").unwrap_or_default();
    std::env::split_paths(&path)
        .chain([
            PathBuf::from("/opt/homebrew/bin"),
            PathBuf::from("/usr/local/bin"),
        ])
        .find(|dir| is_executable_in(dir, "ffmpeg") && is_executable_in(dir, "ffprobe"))
}

fn is_executable_in(dir: &Path, name: &str) -> bool {
    dir.join(exe(name)).is_file()
}
