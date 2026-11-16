//! The IPC surface: every function the frontend can call.

use std::path::PathBuf;

use serde::Serialize;
use std::sync::Arc;

use tauri::{AppHandle, State};
use tauri_plugin_opener::OpenerExt;

use crate::binaries::{BinaryManager, ToolStatus, UpdatePolicy};
use crate::downloads::{self, DownloadManager, DownloadRequest};
use crate::error::{AppError, ErrorKind, Result};
use crate::plan::explain::{self, PlanSummary};
use crate::plan::{self, DownloadOptions, Trim};
use crate::preview::{self, Preview};
use crate::settings::{Settings, SettingsStore};
use crate::state::{EncoderCache, MediaCache};
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

/// Describes what downloading `url` with `options` would do, without doing it.
#[tauri::command]
pub async fn plan_download(
    bins: Binaries<'_>,
    cache: State<'_, MediaCache>,
    encoders: State<'_, EncoderCache>,
    url: String,
    options: DownloadOptions,
    trim: Option<Trim>,
) -> Result<PlanSummary> {
    let video = cache
        .get(&url)
        .ok_or_else(|| AppError::internal("Siphon lost track of this video. Paste the link again."))?;
    let recipe = plan::plan(&video, &options, trim)?;
    Ok(explain::summarize(&recipe, &encoders.get(&bins).await))
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
    settings: Settings,
) -> Result<SettingsView> {
    let saved = store.set(settings)?;
    // The number of simultaneous downloads may have changed.
    downloads.refresh_queue();
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
