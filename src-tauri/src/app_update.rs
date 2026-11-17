//! Updating Siphon itself from its signed GitHub releases.
//!
//! The updater plugin fetches `latest.json` from the newest published
//! release, and only installs a build whose signature matches the public key
//! in `tauri.conf.json`.

use std::sync::Arc;

use tauri::{AppHandle, State};
use tauri_plugin_updater::{Error as UpdaterError, Update, UpdaterExt};

use crate::downloads::DownloadManager;
use crate::error::{AppError, ErrorKind, Result};

async fn newer_release(app: &AppHandle) -> Result<Option<Update>> {
    let failed = |err: UpdaterError| {
        AppError::new(ErrorKind::Network, "Couldn't check for a new version of Siphon.").with_details(err.to_string())
    };
    match app.updater().map_err(failed)?.check().await {
        Ok(update) => Ok(update),
        // Nothing has been published yet: that is "no update", not a failure.
        Err(UpdaterError::ReleaseNotFound) => Ok(None),
        Err(err) => Err(failed(err)),
    }
}

/// The version of a newer Siphon release, if there is one.
#[tauri::command]
pub async fn app_update_check(app: AppHandle) -> Result<Option<String>> {
    Ok(newer_release(&app).await?.map(|update| update.version))
}

/// Downloads the newest release, installs it and restarts Siphon.
#[tauri::command]
pub async fn app_update_install(app: AppHandle, downloads: State<'_, Arc<DownloadManager>>) -> Result<()> {
    // Restarting would cut off whatever is downloading.
    if !downloads.is_idle() {
        return Err(AppError::new(
            ErrorKind::Busy,
            "Siphon has to restart to update. Let your downloads finish, or pause them (they carry on after the restart), and try again.",
        ));
    }
    let Some(update) = newer_release(&app).await? else {
        return Ok(());
    };
    update.download_and_install(|_, _| {}, || {}).await.map_err(|err| {
        AppError::new(
            ErrorKind::Network,
            "The update couldn't be installed. The current version is untouched.",
        )
        .with_details(err.to_string())
    })?;
    // On Windows the installer has already closed the app by now.
    app.restart()
}
