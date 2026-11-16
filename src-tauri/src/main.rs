// Hide the console window in release builds on Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

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

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            preview::clear_cache(app.handle());
            let binaries = binaries::BinaryManager::new(app.handle())?;
            app.manage(Arc::new(binaries));
            app.manage(Arc::new(downloads::DownloadManager::default()));
            app.manage(settings::SettingsStore::load(app.handle())?);
            app.manage(state::MediaCache::default());
            app.manage(state::EncoderCache::default());
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
