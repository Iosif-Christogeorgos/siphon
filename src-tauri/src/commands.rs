//! The IPC surface: every function the frontend can call.

use std::sync::Arc;

use tauri::State;

use crate::binaries::sources::Channel;
use crate::binaries::{BinaryManager, ToolStatus};
use crate::error::{AppError, Result};
use crate::ytdlp::metadata;

type Binaries<'a> = State<'a, Arc<BinaryManager>>;

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

fn ensure_ready(bins: &BinaryManager) -> Result<()> {
    if bins.is_ready() {
        Ok(())
    } else {
        Err(AppError::setup(
            "Siphon is still setting up its download engine.",
        ))
    }
}
