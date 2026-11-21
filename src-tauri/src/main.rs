// Hide the console window in release builds on Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app_update;
mod binaries;
mod chat;
mod choices;
mod commands;
mod downloads;
mod error;
mod ffmpeg;
mod log;
mod plan;
mod preview;
mod process;
mod sessions;
mod settings;
mod srt;
mod state;
mod throttle;
mod ytdlp;

use std::sync::Arc;

use tauri::Manager;

/// Added to the identifier of debug builds, so the dev build keeps its own
/// folders (`%APPDATA%\app.siphon.desktop.dev` and the like) apart from the
/// installed Siphon's: its settings, sign-ins, paused downloads and webview
/// profile. Every folder Tauri resolves follows the identifier.
const DEV_SUFFIX: &str = ".dev";

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
    // Siphon's own executable doubles as the helper that stops a live
    // recording gently (see `process::interrupt_tree`): it does that and exits.
    if let Some(group) = process::interrupt_request() {
        std::process::exit(if process::send_break(group) { 0 } else { 1 });
    }
    let mut context = tauri::generate_context!();
    if cfg!(debug_assertions) {
        context.config_mut().identifier.push_str(DEV_SUFFIX);
    }
    tauri::Builder::default()
        // Opening Siphon again brings the open window forward instead of starting
        // a second copy, which would share its files and settings. It has to be
        // the first plugin. The dev build has its own identifier, so it still
        // runs beside the installed app.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.unminimize();
                let _ = window.show();
                let _ = window.set_focus();
            }
        }))
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        // Used from Rust only; the webview has no notification permission.
        .plugin(tauri_plugin_notification::init())
        .setup(|app| {
            preview::clear_cache(app.handle());
            // Paused and failed downloads from the last session carry on from their files.
            let downloads = downloads::DownloadManager::load(app.handle());
            downloads::sweep_work(app.handle(), &downloads.kept_ids());
            let binaries = binaries::BinaryManager::new(app.handle())?;
            app.manage(Arc::new(binaries));
            app.manage(sessions::Sessions::load(app.handle()));
            app.manage(sessions::sign_in::SignInWindow::default());
            app.manage(Arc::new(downloads));
            let settings = settings::SettingsStore::load(app.handle())?;
            // Every download goes through the speed limit, set or not.
            let throttle = throttle::Throttle::new(settings.get().speed_limit);
            throttle.start(app.handle().clone());
            app.manage(Arc::new(throttle));
            app.manage(settings);
            app.manage(choices::ChoiceStore::load(app.handle())?);
            app.manage(state::MediaCache::default());
            app.manage(state::CapabilityCache::default());
            app.manage(chat::preview::ChatPreviews::default());
            tauri::async_runtime::spawn(keep_tools_updated(app.handle().clone()));
            Ok(())
        })
        // Siphon ends with its main window: a sign-in window left open would keep it running.
        .on_window_event(|window, event| {
            if window.label() == "main" && matches!(event, tauri::WindowEvent::Destroyed) {
                if let Some(sign_in) = window.app_handle().get_window(sessions::sign_in::WINDOW) {
                    let _ = sign_in.destroy();
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::binaries_status,
            commands::binaries_sync,
            commands::fetch_media,
            commands::get_presets,
            commands::get_choices,
            commands::set_choices,
            commands::import_legacy_choices,
            commands::plan_download,
            commands::estimate_download,
            commands::preview_source,
            commands::chat_preview,
            commands::start_download,
            commands::cancel_download,
            commands::pause_download,
            commands::stop_recording,
            commands::recover_recordings,
            commands::live_from_start,
            commands::forget_other_downloads,
            commands::get_settings,
            commands::set_settings,
            commands::open_file,
            commands::reveal_file,
            commands::files_present,
            commands::notify,
            commands::sign_in_browsers,
            commands::sign_in,
            commands::sign_in_site,
            commands::sign_in_done,
            commands::sign_in_cancel,
            commands::signed_in_sites,
            commands::set_sign_in_mode,
            commands::sign_out,
            app_update::app_update_check,
            app_update::app_update_install,
        ])
        .build(context)
        .expect("failed to start Siphon")
        .run(|app, event| {
            // Closing the app must not leave yt-dlp or ffmpeg running behind
            // it. Running downloads are paused, and carry on next time.
            if let tauri::RunEvent::Exit = event {
                let downloads = Arc::clone(&app.state::<Arc<downloads::DownloadManager>>());
                tauri::async_runtime::block_on(downloads.shut_down());
                // Every download has stopped: no cookie file may stay behind.
                app.state::<Arc<sessions::Sessions>>().sweep_temp();
            }
        });
}
