// Hide the console window in release builds on Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod binaries;
mod commands;
mod error;
mod process;
mod ytdlp;

use std::sync::Arc;

use tauri::Manager;

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let binaries = binaries::BinaryManager::new(app.handle())?;
            app.manage(Arc::new(binaries));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::binaries_status,
            commands::binaries_sync,
            commands::fetch_media,
        ])
        .run(tauri::generate_context!())
        .expect("failed to start Siphon");
}
