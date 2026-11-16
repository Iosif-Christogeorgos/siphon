//! The IPC surface: every function the frontend can call.

use std::path::PathBuf;
use std::sync::Arc;

use tauri::{AppHandle, State};
use tauri_plugin_opener::OpenerExt;

use crate::binaries::sources::Channel;
use crate::binaries::{BinaryManager, ToolStatus};
use crate::downloads::{DownloadManager, DownloadRequest};
use crate::error::{AppError, ErrorKind, Result};
use crate::ytdlp::metadata;

type Binaries<'a> = State<'a, Arc<BinaryManager>>;
type Downloads<'a> = State<'a, Arc<DownloadManager>>;

/// Current state of the managed binaries, without touching the network.
#[tauri::command]
pub fn binaries_status(bins: Binaries<'_>) -> Vec<ToolStatus> {
    bins.statuses()
}

/// Installs missing binaries and updates outdated ones.
/// Progress arrives as `binaries://status` events.
#[tauri::command]
pub async fn binaries_sync(bins: Binaries<'_>) -> Result<Vec<ToolStatus>> {
    Ok(bins.sync_all(Channel::default()).await)
}

#[tauri::command]
pub async fn fetch_media(bins: Binaries<'_>, url: String) -> Result<metadata::Media> {
    ensure_ready(&bins)?;
    metadata::fetch(&bins, &url).await
}

#[tauri::command]
pub fn start_download(
    app: AppHandle,
    bins: Binaries<'_>,
    downloads: Downloads<'_>,
    request: DownloadRequest,
) -> Result<()> {
    ensure_ready(&bins)?;
    downloads.start(app, Arc::clone(&bins), request)
}

#[tauri::command]
pub async fn cancel_download(downloads: Downloads<'_>, id: String) -> Result<()> {
    downloads.cancel(&id).await;
    Ok(())
}

/// Opens a downloaded file in its default app.
#[tauri::command]
pub fn open_file(app: AppHandle, path: String) -> Result<()> {
    let path = existing(path)?;
    app.opener()
        .open_path(path.to_string_lossy(), None::<&str>)
        .map_err(|err| AppError::internal("Couldn't open the file.").with_details(err.to_string()))
}

/// Shows a downloaded file in Explorer / Finder.
#[tauri::command]
pub fn reveal_file(app: AppHandle, path: String) -> Result<()> {
    let path = existing(path)?;
    app.opener().reveal_item_in_dir(path).map_err(|err| {
        AppError::internal("Couldn't open the folder.").with_details(err.to_string())
    })
}

fn ensure_ready(bins: &BinaryManager) -> Result<()> {
    if bins.is_ready() {
        Ok(())
    } else {
        Err(AppError::setup(
            "Siphon is still setting up its download engine.",
        ))
    }
}

fn existing(path: String) -> Result<PathBuf> {
    let path = PathBuf::from(path);
    if path.exists() {
        Ok(path)
    } else {
        Err(AppError::new(
            ErrorKind::Unavailable,
            "That file isn't there any more. It may have been moved or deleted.",
        ))
    }
}
