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
            commands::open_file,
            commands::reveal_file,
        ])
        .run(tauri::generate_context!())
        .expect("failed to start Siphon");
}
