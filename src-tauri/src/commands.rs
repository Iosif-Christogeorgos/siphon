//! The IPC surface: every function the frontend can call.

use std::path::PathBuf;

use serde::Serialize;
use serde_json::Value;
use std::sync::Arc;

use tauri::{AppHandle, State};
use tauri_plugin_opener::OpenerExt;
use tokio::sync::Semaphore;

use crate::binaries::{BinaryManager, ToolStatus, UpdatePolicy};
use crate::choices::{self, ChoiceStore, Choices};
use crate::downloads::{self, DownloadManager, DownloadRequest};
use crate::error::{AppError, ErrorKind, Result};
use crate::plan::explain::{self, PlanSummary};
use crate::plan::registry::{self, Registry};
use crate::plan::{self, Choice, Trim};
use crate::preview::{self, Preview};
use crate::settings::{Settings, SettingsStore};
use crate::state::{CapabilityCache, MediaCache};
use crate::throttle::Throttle;
use crate::ytdlp::metadata::{self, Media};

type Binaries<'a> = State<'a, Arc<BinaryManager>>;
type Downloads<'a> = State<'a, Arc<DownloadManager>>;

/// Current state of the managed binaries, without touching the network.
#[tauri::command]
pub fn binaries_status(bins: Binaries<'_>) -> Vec<ToolStatus> {
    bins.statuses()
}

/// Installs missing binaries and looks for newer versions. Those are
/// installed or only reported, as the settings say, unless `install`
/// forces it (the user pressed "Update now").
/// Progress arrives as `binaries://status` events.
#[tauri::command]
pub async fn binaries_sync(
    bins: Binaries<'_>,
    settings: State<'_, SettingsStore>,
    install: bool,
) -> Result<Vec<ToolStatus>> {
    let settings = settings.get();
    let policy = if install {
        UpdatePolicy::Install
    } else {
        settings.update_policy()
    };
    Ok(bins.sync_all(settings.update_channel, policy).await)
}

/// Looks a link up and remembers the result for planning and downloading.
#[tauri::command]
pub async fn fetch_media(bins: Binaries<'_>, cache: State<'_, MediaCache>, url: String) -> Result<Media> {
    ensure_ready(&bins)?;
    let media = metadata::fetch(&bins, &url).await?;
    if let Media::Video(video) = &media {
        cache.insert(url.trim(), Arc::new((**video).clone()));
    }
    Ok(media)
}

/// Every preset group, target and tier: the frontend defines none itself.
#[tauri::command]
pub fn get_presets() -> Registry {
    registry::registry()
}

/// What was chosen last time, and the user's saved presets.
#[tauri::command]
pub fn get_choices(store: State<'_, ChoiceStore>) -> Choices {
    store.get()
}

#[tauri::command]
pub fn set_choices(store: State<'_, ChoiceStore>, choices: Choices) -> Result<Choices> {
    store.set(choices)
}

/// Takes over the choice an older version kept in the webview's storage
/// (`siphon.options`). Only the first time: once Siphon has saved choices of
/// its own, those win.
#[tauri::command]
pub fn import_legacy_choices(store: State<'_, ChoiceStore>, old: Value) -> Result<Choices> {
    if !store.is_new() {
        return Ok(store.get());
    }
    store.set(choices::from_legacy(&old))
}

/// Describes what downloading `url` with `choice` would do, without doing it.
#[tauri::command]
pub async fn plan_download(
    bins: Binaries<'_>,
    cache: State<'_, MediaCache>,
    caps: State<'_, CapabilityCache>,
    url: String,
    choice: Choice,
    trim: Option<Trim>,
) -> Result<PlanSummary> {
    let video = cache
        .get(&url)
        .ok_or_else(|| AppError::internal("Siphon lost track of this video. Paste the link again."))?;
    explain::summarize(&video, &choice, trim, &caps.get(&bins).await)
}

/// At most this many playlist videos are looked up at once for size estimates.
static ESTIMATES: Semaphore = Semaphore::const_new(3);

/// The estimated size of downloading `url` with `choice`, looking the video
/// up first if this session hasn't. For the total under a playlist.
#[tauri::command]
pub async fn estimate_download(
    bins: Binaries<'_>,
    cache: State<'_, MediaCache>,
    caps: State<'_, CapabilityCache>,
    url: String,
    choice: Choice,
) -> Result<Option<u64>> {
    ensure_ready(&bins)?;
    let video = match cache.get(url.trim()) {
        Some(video) => video,
        None => {
            let _permit = ESTIMATES
                .acquire()
                .await
                .map_err(|err| AppError::internal("Couldn't look the video up.").with_details(err.to_string()))?;
            match metadata::fetch(&bins, &url).await? {
                Media::Video(video) => {
                    let video = Arc::new(*video);
                    cache.insert(url.trim(), Arc::clone(&video));
                    video
                }
                Media::Playlist(_) => return Ok(None),
            }
        }
    };
    let recipe = plan::plan(&video, &choice, None, &caps.get(&bins).await)?;
    Ok(recipe.estimated_size())
}

/// Where the trim preview should play from. `level` starts at 0; ask again
/// with the returned level + 1 if that source fails to play.
#[tauri::command]
pub async fn preview_source(
    app: AppHandle,
    bins: Binaries<'_>,
    cache: State<'_, MediaCache>,
    url: String,
    level: u8,
) -> Result<Preview> {
    let video = cache
        .get(&url)
        .ok_or_else(|| AppError::internal("Siphon lost track of this video. Paste the link again."))?;
    preview::source(&app, &bins, &video, level).await
}

#[tauri::command]
pub fn start_download(
    app: AppHandle,
    bins: Binaries<'_>,
    downloads: Downloads<'_>,
    request: DownloadRequest,
) -> Result<()> {
    ensure_ready(&bins)?;
    downloads.start(app, request)
}

/// Stops a download for good and deletes its partial files.
#[tauri::command]
pub async fn cancel_download(downloads: Downloads<'_>, id: String) -> Result<()> {
    downloads.cancel(&id).await;
    Ok(())
}

/// Stops a download but keeps its partial files; start it again with `resume: true`.
#[tauri::command]
pub async fn pause_download(downloads: Downloads<'_>, id: String) -> Result<()> {
    downloads.pause(&id).await;
    Ok(())
}

/// The settings, plus the facts the settings screen needs to show them.
/// Called once the frontend has restored its download list: the files of
/// paused or failed downloads it no longer lists are removed.
#[tauri::command]
pub async fn forget_other_downloads(downloads: Downloads<'_>, ids: Vec<String>) -> Result<()> {
    downloads.forget_all_but(&ids.into_iter().collect()).await;
    Ok(())
}

#[derive(Serialize)]
pub struct SettingsView {
    #[serde(flatten)]
    settings: Settings,
    /// Where downloads go when no folder is chosen.
    default_output_dir: Option<String>,
}

fn settings_view(app: &AppHandle, settings: Settings) -> SettingsView {
    let default_output_dir = downloads::default_output_dir(app)
        .ok()
        .map(|dir| dir.to_string_lossy().into_owned());
    SettingsView {
        settings,
        default_output_dir,
    }
}

#[tauri::command]
pub fn get_settings(app: AppHandle, store: State<'_, SettingsStore>) -> SettingsView {
    settings_view(&app, store.get())
}

#[tauri::command]
pub fn set_settings(
    app: AppHandle,
    store: State<'_, SettingsStore>,
    downloads: Downloads<'_>,
    throttle: State<'_, Arc<Throttle>>,
    settings: Settings,
) -> Result<SettingsView> {
    let saved = store.set(settings)?;
    // The number of simultaneous downloads may have changed …
    downloads.refresh_queue();
    // … and the speed limit, which running downloads follow at once.
    throttle.set_limit(saved.speed_limit);
    Ok(settings_view(&app, saved))
}

/// Opens a downloaded file in its default app.
#[tauri::command]
pub fn open_file(app: AppHandle, downloads: Downloads<'_>, path: String) -> Result<()> {
    let path = existing(&downloads, path)?;
    app.opener()
        .open_path(path.to_string_lossy(), None::<&str>)
        .map_err(|err| AppError::internal("Couldn't open the file.").with_details(err.to_string()))
}

/// Shows a downloaded file in Explorer / Finder.
#[tauri::command]
pub fn reveal_file(app: AppHandle, downloads: Downloads<'_>, path: String) -> Result<()> {
    let path = existing(&downloads, path)?;
    app.opener()
        .reveal_item_in_dir(path)
        .map_err(|err| AppError::internal("Couldn't open the folder.").with_details(err.to_string()))
}

/// Which of these downloaded files are still where Siphon put them, in the same
/// order. A path Siphon didn't download counts as gone, so this can't be used
/// to look around the disk.
#[tauri::command]
pub fn files_present(downloads: Downloads<'_>, paths: Vec<String>) -> Vec<bool> {
    paths
        .into_iter()
        .map(|path| existing(&downloads, path).is_ok())
        .collect()
}

fn ensure_ready(bins: &BinaryManager) -> Result<()> {
    if bins.is_ready() {
        Ok(())
    } else {
        Err(AppError::setup("Siphon is still setting up its download engine."))
    }
}

/// Only files Siphon downloaded this session can be opened: the webview
/// can't use these commands to launch arbitrary paths.
fn existing(downloads: &DownloadManager, path: String) -> Result<PathBuf> {
    let path = PathBuf::from(path);
    if !downloads.produced(&path) {
        return Err(AppError::internal("Siphon can only open files it downloaded."));
    }
    if path.exists() {
        Ok(path)
    } else {
        Err(AppError::new(
            ErrorKind::Unavailable,
            "That file isn't there any more. It may have been moved or deleted.",
        ))
    }
}
