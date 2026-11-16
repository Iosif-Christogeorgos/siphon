// Hide the console window in release builds on Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app_update;
mod binaries;
mod commands;
mod downloads;
mod error;
mod ffmpeg;
mod plan;
mod preview;
mod process;
mod settings;
mod state;
mod ytdlp;

use std::sync::Arc;

use tauri::Manager;

/// Looks for new versions of the managed tools once a day while the app is
/// open. (The frontend triggers the check at launch.) Waking hourly keeps
/// the loop simple and lets a check that was put off, because a download
/// was running, happen soon after.
async fn keep_tools_updated(app: tauri::AppHandle) {
    let mut hourly = tokio::time::interval(std::time::Duration::from_secs(60 * 60));
    hourly.tick().await; // The first tick fires at once; launch is already covered.
    loop {
        hourly.tick().await;
        let bins = Arc::clone(&app.state::<Arc<binaries::BinaryManager>>());
        if bins.is_ready() && bins.check_is_due() {
            let settings = app.state::<settings::SettingsStore>().get();
            bins.sync_all(settings.update_channel, settings.update_policy()).await;
        }
    }
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .setup(|app| {
            preview::clear_cache(app.handle());
            let binaries = binaries::BinaryManager::new(app.handle())?;
            app.manage(Arc::new(binaries));
            app.manage(Arc::new(downloads::DownloadManager::default()));
            app.manage(settings::SettingsStore::load(app.handle())?);
            app.manage(state::MediaCache::default());
            app.manage(state::EncoderCache::default());
            app.manage(state::ChunkedRequests::default());
            tauri::async_runtime::spawn(keep_tools_updated(app.handle().clone()));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::binaries_status,
            commands::binaries_sync,
            commands::fetch_media,
            commands::plan_download,
            commands::preview_source,
            commands::start_download,
            commands::cancel_download,
            commands::pause_download,
            commands::get_settings,
            commands::set_settings,
            commands::open_file,
            commands::reveal_file,
            app_update::app_update_check,
            app_update::app_update_install,
        ])
        .build(tauri::generate_context!())
        .expect("failed to start Siphon")
        .run(|app, event| {
            // Closing the app must not leave yt-dlp or ffmpeg running behind it.
            if let tauri::RunEvent::Exit = event {
                let downloads = Arc::clone(&app.state::<Arc<downloads::DownloadManager>>());
                tauri::async_runtime::block_on(downloads.pause_all());
            }
        });
}
