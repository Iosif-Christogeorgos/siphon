//! The IPC surface: every function the frontend can call.

use std::path::PathBuf;
use std::sync::Arc;

use tauri::{AppHandle, State};
use tauri_plugin_opener::OpenerExt;

use crate::binaries::sources::Channel;
use crate::binaries::{BinaryManager, ToolStatus};
use crate::downloads::{DownloadManager, DownloadRequest};
use crate::error::{AppError, ErrorKind, Result};
use crate::plan::explain::{self, PlanSummary};
use crate::plan::{self, DownloadOptions, Trim};
use crate::state::{EncoderCache, MediaCache};
use crate::ytdlp::metadata::{self, Media};

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
