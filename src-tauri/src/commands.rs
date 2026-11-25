//! The IPC surface: every function the frontend can call.

use std::path::PathBuf;

use serde::Serialize;
use serde_json::Value;
use std::sync::Arc;

use tauri::{AppHandle, Manager, State};
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_notification::NotificationExt;
use tauri_plugin_opener::OpenerExt;
use tokio::sync::{oneshot, Semaphore};

use crate::binaries::{BinaryManager, ToolStatus, UpdatePolicy};
use crate::chat::preview::ChatPreviews;
use crate::chat::Placement;
use crate::choices::{self, ChoiceStore, Choices};
use crate::downloads::{self, DownloadManager, DownloadRequest};
use crate::error::{AppError, ErrorKind, Result};
use crate::local::{self, Grants, Intake};
use crate::plan::explain::{self, PlanSummary};
use crate::plan::registry::{self, Registry};
use crate::plan::{self, Choice, Trim};
use crate::preview::{self, Preview};
use crate::sessions::browsers::{self, Browser};
use crate::sessions::sign_in::{self, Outcome};
use crate::sessions::{Account, Method, Mode, Sessions, SiteSession};
use crate::settings::{Settings, SettingsStore};
use crate::sizing::{Measure, Sizing};
use crate::state::{CapabilityCache, MediaCache};
use crate::throttle::Throttle;
use crate::ytdlp::metadata::{self, LiveStatus, Media, VideoInfo};

type Binaries<'a> = State<'a, Arc<BinaryManager>>;
type Downloads<'a> = State<'a, Arc<DownloadManager>>;

/// Current state of the managed binaries, without touching the network.
#[tauri::command]
pub fn binaries_status(bins: Binaries<'_>) -> Vec<ToolStatus> {
    bins.statuses()
}

/// Installs missing binaries and looks for newer versions. Those are
/// installed or only reported, as the settings say, unless `install`
/// forces it (the user pressed "Update now").
/// Progress arrives as `binaries://status` events.
#[tauri::command]
pub async fn binaries_sync(
    bins: Binaries<'_>,
    settings: State<'_, SettingsStore>,
    install: bool,
) -> Result<Vec<ToolStatus>> {
    let settings = settings.get();
    let policy = if install {
        UpdatePolicy::Install
    } else {
        settings.update_policy()
    };
    Ok(bins.sync_all(settings.update_channel, policy).await)
}

/// Looks a link up and remembers the result for planning and downloading.
/// Without an account unless it's needed; `account` uses the site's sign-in
/// for this one (the user's "Try with your sign-in").
#[tauri::command]
pub async fn fetch_media(
    bins: Binaries<'_>,
    sessions: State<'_, Arc<Sessions>>,
    cache: State<'_, MediaCache>,
    url: String,
    account: Option<bool>,
) -> Result<Media> {
    ensure_ready(&bins)?;
    let account = if account == Some(true) {
        Account::Use
    } else {
        Account::Auto
    };
    let media = sessions.fetch_media(&bins, &url, account).await?;
    if let Media::Video(video) = &media {
        cache.insert(url.trim(), Arc::new((**video).clone()));
    }
    Ok(media)
}

/// Browse: Windows' file dialog for video and audio files to convert, opened
/// here so the webview never names a file. The files picked are kept and
/// given back by key (see `crate::local`); None if the dialog was closed.
#[tauri::command]
pub async fn pick_files(app: AppHandle, grants: State<'_, Grants>) -> Result<Option<Intake>> {
    let (chosen, picked) = oneshot::channel();
    let mut dialog = app
        .dialog()
        .file()
        .set_title("Choose videos or sounds to convert")
        .add_filter("Video and audio", local::MEDIA_EXTENSIONS)
        .add_filter("All files", &["*"]);
    if let Some(window) = app.get_webview_window("main") {
        dialog = dialog.set_parent(&window);
    }
    dialog.pick_files(move |paths| {
        let _ = chosen.send(paths);
    });
    let Some(paths) = picked.await.ok().flatten() else {
        return Ok(None);
    };
    let paths = paths.into_iter().filter_map(|path| path.into_path().ok()).collect();
    Ok(Some(local::take_in(&grants, paths)))
}

/// Reads what a file the user gave holds (by its key, from a drop or
/// Browse); it is remembered until the file changes (see `local::read`).
#[tauri::command]
pub async fn read_file(app: AppHandle, bins: Binaries<'_>, key: String) -> Result<VideoInfo> {
    ensure_ready(&bins)?;
    Ok((*local::read(&app, &bins, &key).await?).clone())
}

/// Every track of an MKV the user gave, with how certain its language is,
/// for choosing which it keeps (docs/mkv-editor.md).
#[tauri::command]
pub async fn read_tracks(app: AppHandle, bins: Binaries<'_>, key: String) -> Result<local::mkv::Contents> {
    ensure_ready(&bins)?;
    local::mkv::read(&app, &bins, &key).await
}

/// A file picked to add tracks from: its key, and its name to show and match by.
#[derive(serde::Serialize)]
pub struct PickedFile {
    pub key: String,
    pub name: String,
}

/// Windows' file dialog for subtitle and sound files (and videos) to add to
/// MKVs' copies (docs/mkv-editor.md, step 6), opened here so the webview
/// never names a file. None if the dialog was closed.
#[tauri::command]
pub async fn pick_track_files(app: AppHandle, grants: State<'_, Grants>) -> Result<Option<Vec<PickedFile>>> {
    let (chosen, picked) = oneshot::channel();
    let mut dialog = app
        .dialog()
        .file()
        .set_title("Choose subtitles or sound to add")
        .add_filter("Subtitles and sound", local::mkv::added::EXTENSIONS)
        .add_filter("All files", &["*"]);
    if let Some(window) = app.get_webview_window("main") {
        dialog = dialog.set_parent(&window);
    }
    dialog.pick_files(move |paths| {
        let _ = chosen.send(paths);
    });
    let Some(paths) = picked.await.ok().flatten() else {
        return Ok(None);
    };
    Ok(Some(
        paths
            .into_iter()
            .filter_map(|path| path.into_path().ok())
            .filter_map(|path| grants.grant(&path))
            .map(|file| PickedFile {
                key: file.key(),
                name: file.name(),
            })
            .collect(),
    ))
}

/// Reads a file the user gave to add tracks from (see `local::mkv::added`),
/// a text subtitle file in `charset` if the user chose one.
#[tauri::command]
pub async fn read_added(
    app: AppHandle,
    bins: Binaries<'_>,
    key: String,
    charset: Option<String>,
) -> Result<local::mkv::added::Added> {
    ensure_ready(&bins)?;
    local::mkv::added::read(&app, &bins, &key, charset.as_deref()).await
}

/// Which of a season's episodes each file to add goes with, by their names.
#[tauri::command]
pub fn match_episodes(episodes: Vec<String>, files: Vec<String>) -> Vec<Option<usize>> {
    local::mkv::added::match_episodes(&episodes, &files)
}

/// Every preset group, target and tier: the frontend defines none itself.
#[tauri::command]
pub fn get_presets() -> Registry {
    registry::registry()
}

/// What was chosen last time, and the user's saved presets.
#[tauri::command]
pub fn get_choices(store: State<'_, ChoiceStore>) -> Choices {
    store.get()
}

#[tauri::command]
pub fn set_choices(store: State<'_, ChoiceStore>, choices: Choices) -> Result<Choices> {
    store.set(choices)
}

/// Takes over the choice an older version kept in the webview's storage
/// (`siphon.options`). Only the first time: once Siphon has saved choices of
/// its own, those win.
#[tauri::command]
pub fn import_legacy_choices(store: State<'_, ChoiceStore>, old: Value) -> Result<Choices> {
    if !store.is_new() {
        return Ok(store.get());
    }
    store.set(choices::from_legacy(&old))
}

/// Describes what downloading `url` with `choice` would do, without doing it.
/// For a live stream, what recording it would do (`from_start`: from the
/// start), sized per hour, with what the speed limit and the disk allow.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn plan_download(
    app: AppHandle,
    bins: Binaries<'_>,
    cache: State<'_, MediaCache>,
    caps: State<'_, CapabilityCache>,
    settings: State<'_, SettingsStore>,
    url: String,
    choice: Choice,
    trim: Option<Trim>,
    from_start: Option<bool>,
) -> Result<PlanSummary> {
    let video = remembered(&app, &bins, &cache, &url).await?;
    let caps = caps.get(&bins).await;
    if video.live.is_none() {
        return explain::summarize(&video, &choice, trim, &caps);
    }
    let from_start = from_start.unwrap_or(false);
    let mut summary = plan::live::summarize(&video, &choice, from_start, &caps)?;
    let recipe = plan::live::plan(&video, &choice, from_start, &caps)?;
    let settings = settings.get();
    let folder = settings
        .output_dir
        .clone()
        .or_else(|| downloads::default_output_dir(&app).ok());
    let free = folder.as_deref().and_then(downloads::free_space);
    summary.hints.extend(plan::live::hints(
        &recipe,
        video.extractor.as_deref(),
        settings.speed_limit,
        free,
    ));
    Ok(summary)
}

/// Whether a live stream can be recorded from its start: the site keeps what
/// already aired. Looks it up (a few seconds), and keeps those formats for
/// planning and recording.
#[tauri::command]
pub async fn live_from_start(
    bins: Binaries<'_>,
    sessions: State<'_, Arc<Sessions>>,
    cache: State<'_, MediaCache>,
    url: String,
) -> Result<bool> {
    let video = cache
        .get(&url)
        .ok_or_else(|| AppError::internal("Siphon lost track of this video. Paste the link again."))?;
    if !video.live.as_ref().is_some_and(|live| live.status == LiveStatus::Live) {
        return Ok(false);
    }
    let cookies = if video.with_account {
        sessions.cookie_file(&video.url)
    } else {
        None
    };
    // Not offered is the answer to anything that goes wrong here.
    let formats = metadata::fetch_from_start(&bins, &video.url, cookies.as_ref().map(|file| file.path()))
        .await
        .unwrap_or_default();
    let available = !formats.is_empty();
    let mut with = (*video).clone();
    if let Some(live) = with.live.as_mut() {
        live.from_start = Some(formats);
    }
    cache.insert(&url, Arc::new(with));
    Ok(available)
}

/// At most this many playlist videos are looked up at once for size estimates.
static ESTIMATES: Semaphore = Semaphore::const_new(3);

/// The estimated size of downloading `url` with `choice`, looking the video
/// up first if this session hasn't. For the total under a playlist.
#[tauri::command]
pub async fn estimate_download(
    bins: Binaries<'_>,
    sessions: State<'_, Arc<Sessions>>,
    cache: State<'_, MediaCache>,
    caps: State<'_, CapabilityCache>,
    url: String,
    choice: Choice,
) -> Result<Option<u64>> {
    ensure_ready(&bins)?;
    let video = match cache.get(url.trim()) {
        Some(video) => video,
        None => {
            let _permit = ESTIMATES
                .acquire()
                .await
                .map_err(|err| AppError::internal("Couldn't look the video up.").with_details(err.to_string()))?;
            match sessions.fetch_media(&bins, &url, Account::Auto).await? {
                Media::Video(video) => {
                    let video = Arc::new(*video);
                    cache.insert(url.trim(), Arc::clone(&video));
                    video
                }
                Media::Playlist(_) => return Ok(None),
            }
        }
    };
    let recipe = plan::plan(&video, &choice, None, &caps.get(&bins).await)?;
    Ok(recipe.estimated_size())
}

/// Where the trim preview should play from. `level` starts at 0; ask again
/// with the returned level + 1 if that source fails to play.
#[tauri::command]
pub async fn preview_source(
    app: AppHandle,
    bins: Binaries<'_>,
    cache: State<'_, MediaCache>,
    url: String,
    level: u8,
) -> Result<Preview> {
    let video = remembered(&app, &bins, &cache, &url).await?;
    preview::source(&app, &bins, &video, level).await
}

#[tauri::command]
pub fn start_download(
    app: AppHandle,
    bins: Binaries<'_>,
    downloads: Downloads<'_>,
    sizing: State<'_, Sizing>,
    request: DownloadRequest,
) -> Result<()> {
    ensure_ready(&bins)?;
    // The download goes first: measuring its size is no longer worth the connection.
    sizing.stop();
    downloads.start(app, request)
}

/// Measures in the background what the plan for `choice` could only estimate
/// (see `crate::sizing`). A new measurement, a download starting, or
/// `stop_measuring` ends the one before; what it found by then is kept.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn measure_size(
    app: AppHandle,
    bins: Binaries<'_>,
    cache: State<'_, MediaCache>,
    caps: State<'_, CapabilityCache>,
    throttle: State<'_, Arc<Throttle>>,
    sizing: State<'_, Sizing>,
    url: String,
    choice: Choice,
    trim: Option<Trim>,
) -> Result<Measure> {
    ensure_ready(&bins)?;
    let video = remembered(&app, &bins, &cache, &url).await?;
    let caps = caps.get(&bins).await;
    sizing
        .measure(
            &bins,
            &throttle,
            &video,
            &choice,
            trim,
            &caps,
            |facts| cache.add_sizes(&url, facts),
            |text| crate::log::line(&app, text),
        )
        .await
}

/// Ends the size measurement running now, if any (the video was left).
#[tauri::command]
pub fn stop_measuring(sizing: State<'_, Sizing>) {
    sizing.stop();
}

/// The chat box over the preview: the chat of the video at `url` at `at`
/// seconds, placed by `placement` on a frame of `width` × `height` pixels.
/// Answers the box's left, top, width and height (four little-endian u32s),
/// then its pixels as straight RGBA.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn chat_preview(
    app: AppHandle,
    cache: State<'_, MediaCache>,
    previews: State<'_, ChatPreviews>,
    url: String,
    at: f64,
    placement: Placement,
    width: u32,
    height: u32,
) -> Result<tauri::ipc::Response> {
    let video = cache
        .get(&url)
        .ok_or_else(|| AppError::internal("Siphon lost track of this video. Paste the link again."))?;
    let source = video
        .chat
        .clone()
        .ok_or_else(|| AppError::new(ErrorKind::Unsupported, "Only a Twitch video's chat can be drawn in."))?;
    let frame = previews
        .frame(
            &app,
            &source,
            downloads::chat_image_folder(&app)?,
            at,
            placement,
            width,
            height,
        )
        .await?;
    let geometry = frame.geometry;
    let mut bytes = Vec::with_capacity(16 + frame.pixels.len());
    for value in [geometry.x, geometry.y, geometry.width, geometry.height] {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    bytes.extend_from_slice(&frame.pixels);
    Ok(tauri::ipc::Response::new(bytes))
}

/// Stops a download for good and deletes its partial files.
#[tauri::command]
pub async fn cancel_download(downloads: Downloads<'_>, id: String) -> Result<()> {
    downloads.cancel(&id).await;
    Ok(())
}

/// Stops a download but keeps its partial files; start it again with `resume: true`.
#[tauri::command]
pub async fn pause_download(downloads: Downloads<'_>, id: String) -> Result<()> {
    downloads.pause(&id).await;
    Ok(())
}

/// Ends a live recording there and makes it into its file. Its stream
/// keeps going; only the recording stops.
#[tauri::command]
pub async fn stop_recording(downloads: Downloads<'_>, id: String) -> Result<()> {
    downloads.finish(&id).await;
    Ok(())
}

/// Finishes the recordings that Siphon's closing or a crash cut short, and
/// says which, for the download list. Called once at launch, after the list
/// is restored.
#[tauri::command]
pub fn recover_recordings(app: AppHandle, downloads: Downloads<'_>) -> Vec<downloads::Recovered> {
    downloads.recover(&app)
}

/// Removes the files of paused or failed downloads the frontend no longer
/// lists. Called once it has restored its download list.
#[tauri::command]
pub async fn forget_other_downloads(downloads: Downloads<'_>, ids: Vec<String>) -> Result<()> {
    downloads.forget_all_but(&ids.into_iter().collect()).await;
    Ok(())
}

/// The settings, plus the facts the settings screen needs to show them.
#[derive(Serialize)]
pub struct SettingsView {
    #[serde(flatten)]
    settings: Settings,
    /// Where downloads go when no folder is chosen.
    default_output_dir: Option<String>,
    /// The languages taken as the user's until they choose some.
    default_languages: Vec<String>,
}

fn settings_view(app: &AppHandle, settings: Settings) -> SettingsView {
    let default_output_dir = downloads::default_output_dir(app)
        .ok()
        .map(|dir| dir.to_string_lossy().into_owned());
    let subtitles = app.state::<ChoiceStore>().get().subtitles;
    SettingsView {
        settings,
        default_output_dir,
        default_languages: crate::settings::default_languages(subtitles.as_deref()),
    }
}

#[tauri::command]
pub fn get_settings(app: AppHandle, store: State<'_, SettingsStore>) -> SettingsView {
    settings_view(&app, store.get())
}

#[tauri::command]
pub fn set_settings(
    app: AppHandle,
    store: State<'_, SettingsStore>,
    downloads: Downloads<'_>,
    throttle: State<'_, Arc<Throttle>>,
    settings: Settings,
) -> Result<SettingsView> {
    let saved = store.set(settings)?;
    // The number of simultaneous downloads may have changed …
    downloads.refresh_queue();
    // … and the speed limit, which running downloads follow at once.
    throttle.set_limit(saved.speed_limit);
    Ok(settings_view(&app, saved))
}

/// Opens a downloaded file in its default app.
#[tauri::command]
pub fn open_file(app: AppHandle, downloads: Downloads<'_>, path: String) -> Result<()> {
    let path = existing(&downloads, path)?;
    app.opener()
        .open_path(path.to_string_lossy(), None::<&str>)
        .map_err(|err| AppError::internal("Couldn't open the file.").with_details(err.to_string()))
}

/// Shows a downloaded file in Explorer / Finder.
#[tauri::command]
pub fn reveal_file(app: AppHandle, downloads: Downloads<'_>, path: String) -> Result<()> {
    let path = existing(&downloads, path)?;
    app.opener()
        .reveal_item_in_dir(path)
        .map_err(|err| AppError::internal("Couldn't open the folder.").with_details(err.to_string()))
}

/// Which of these downloaded files are still where Siphon put them, in the same
/// order. A path Siphon didn't download counts as gone, so this can't be used
/// to look around the disk.
#[tauri::command]
pub fn files_present(downloads: Downloads<'_>, paths: Vec<String>) -> Vec<bool> {
    paths
        .into_iter()
        .map(|path| existing(&downloads, path).is_ok())
        .collect()
}

/// Shows a Windows notification, but only while Siphon isn't the active
/// window: in front of it, the app shows the news itself.
#[tauri::command]
pub fn notify(app: AppHandle, title: String, body: String) {
    let in_front = app
        .get_webview_window("main")
        .and_then(|window| window.is_focused().ok())
        .unwrap_or(false);
    if in_front {
        return;
    }
    let short = |text: String| text.chars().take(300).collect::<String>();
    let _ = app
        .notification()
        .builder()
        .title(short(title))
        .body(short(body))
        .show();
}

/// The browsers on this computer a sign-in can be taken over from.
#[tauri::command]
pub fn sign_in_browsers() -> Vec<Browser> {
    browsers::installed()
}

/// Signs in to the site of `url` (a video that needed it) with `method`, and
/// checks the sign-in works on that video before keeping it.
#[tauri::command]
pub async fn sign_in(app: AppHandle, url: String, method: Method) -> Result<Outcome> {
    sign_in::sign_in(&app, &url, method).await
}

/// For the sign-in window's bar: the site it's for.
#[tauri::command]
pub fn sign_in_site(app: AppHandle) -> Option<String> {
    sign_in::current_site(&app)
}

/// The sign-in window's Done.
#[tauri::command]
pub async fn sign_in_done(app: AppHandle) {
    sign_in::done(&app, false).await;
}

/// The sign-in window's Cancel (and its close button).
#[tauri::command]
pub fn sign_in_cancel(app: AppHandle) {
    sign_in::finish(&app, Outcome::Cancelled);
}

/// The sites Siphon is signed in to, for Settings. Never any cookie.
#[tauri::command]
pub fn signed_in_sites(sessions: State<'_, Arc<Sessions>>) -> Vec<SiteSession> {
    sessions.list()
}

/// Settings' "When needed" or "Always" for a signed-in site.
#[tauri::command]
pub fn set_sign_in_mode(sessions: State<'_, Arc<Sessions>>, site: String, mode: Mode) {
    sessions.set_mode(&site, mode);
}

/// Forgets a site's sign-in: its cookies and its sign-in window's profile.
#[tauri::command]
pub fn sign_out(app: AppHandle, sessions: State<'_, Arc<Sessions>>, site: String) {
    // Its sign-in window can't stay open on a profile that is being deleted.
    if sign_in::current_site(&app).as_deref() == Some(site.as_str()) {
        sign_in::finish(&app, Outcome::Cancelled);
    }
    sessions.sign_out(&site);
}

/// The video a link or a file's key stands for: a link as this session
/// looked it up; a file as it is now, read again if it changed since it was
/// read (see `local::read`), so it is never planned from what it used to hold.
async fn remembered(app: &AppHandle, bins: &BinaryManager, cache: &MediaCache, url: &str) -> Result<Arc<VideoInfo>> {
    if local::is_key(url) {
        return local::read(app, bins, url).await;
    }
    cache
        .get(url)
        .ok_or_else(|| AppError::internal("Siphon lost track of this video. Paste the link again."))
}

fn ensure_ready(bins: &BinaryManager) -> Result<()> {
    if bins.is_ready() {
        Ok(())
    } else {
        Err(AppError::setup("Siphon is still setting up its download engine."))
    }
}

/// Only files Siphon downloaded, this session or one it remembers in
/// `downloads.json`, can be opened: the webview can't use these commands to
/// launch arbitrary paths.
fn existing(downloads: &DownloadManager, path: String) -> Result<PathBuf> {
    let path = PathBuf::from(path);
    if !downloads.produced(&path) {
        return Err(AppError::internal("Siphon can only open files it downloaded."));
    }
    if path.exists() {
        Ok(path)
    } else {
        Err(AppError::new(
            ErrorKind::Unavailable,
            "That file isn't there any more. It may have been moved or deleted.",
        ))
    }
}
