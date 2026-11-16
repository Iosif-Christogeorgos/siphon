//! The IPC surface: every function the frontend can call.

use std::sync::Arc;

use tauri::State;

use crate::binaries::sources::Channel;
use crate::binaries::{BinaryManager, ToolStatus};
use crate::error::Result;

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
