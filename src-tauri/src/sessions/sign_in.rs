//! Signing in to a site, three ways, each checked before Siphon says so:
//!
//! 1. **Siphon's sign-in window.** The site's own sign-in page, in a window
//!    with a browser profile of its own (one per site, apart from Siphon's
//!    interface). The user signs in as usual, two-factor and all; Siphon
//!    reads that profile's cookies (HttpOnly ones included) when they press
//!    Done, or by itself once a site's known session cookie appears.
//! 2. **A browser on this computer** (see [`browsers`]).
//! 3. **A `cookies.txt` file** the user exported, chosen in a dialog opened
//!    from Rust, so the webview can't make Siphon read any other file.
//!
//! The check is the lookup that failed, run again with the new cookies. Only
//! if it gets through are the site's cookies kept (see [`Sessions::save`]).

use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Serialize;
use tauri::webview::{PageLoadEvent, WebviewBuilder};
use tauri::window::WindowBuilder;
use tauri::{AppHandle, Emitter, LogicalPosition, LogicalSize, Manager, Url, WebviewUrl, WindowEvent};
use tauri_plugin_dialog::DialogExt;
use tokio::sync::oneshot;

use super::cookies::{self, Cookie};
use super::{browsers, site, Method, Sessions};
use crate::binaries::BinaryManager;
use crate::error::{AppError, ErrorKind, Result};
use crate::ytdlp::metadata;

/// The sign-in window, its bar (Siphon's own page: the site's name, Done,
/// Cancel) and the site's page under it.
pub const WINDOW: &str = "sign-in";
pub const BAR: &str = "sign-in-bar";
const PAGE: &str = "sign-in-page";
const BAR_HEIGHT: f64 = 52.0;

/// The bar's state, sent to it as it changes.
pub const STATUS_EVENT: &str = "sign-in://status";

/// How long to wait between looks for a site's session cookie.
const WATCH_EVERY: Duration = Duration::from_millis(1500);

/// A cookies.txt bigger than this is not one.
const MAX_FILE: u64 = 20 * 1024 * 1024;

/// How a sign-in ended. A refusal says why in a line, and in full behind the ⓘ.
#[derive(Debug, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum Outcome {
    SignedIn {
        site: String,
        /// First sign-in to a Google site: show the tip about a separate account.
        tip: bool,
    },
    Refused {
        short: String,
        reason: String,
    },
    Cancelled,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "phase", rename_all = "snake_case")]
enum Status {
    /// Waiting for the user to sign in and press Done.
    Waiting,
    Checking,
    /// Done was pressed, but the site still wanted a sign-in.
    NotYet,
    /// The check couldn't run (offline, say).
    Failed {
        message: String,
    },
}

/// The one sign-in window that may be open, and who is waiting for it.
#[derive(Default)]
pub struct SignInWindow {
    pending: Mutex<Option<Pending>>,
}

struct Pending {
    site: String,
    url: String,
    reply: oneshot::Sender<Outcome>,
    /// A check is running: Done and the watcher don't start a second.
    checking: bool,
    /// The cookies last checked by the watcher, so it doesn't check the same ones twice.
    watched: Vec<Cookie>,
}

/// Signs in to `url`'s site with `method`, and checks it works on `url`.
pub async fn sign_in(app: &AppHandle, url: &str, method: Method) -> Result<Outcome> {
    let site =
        site::site_of(url).ok_or_else(|| AppError::internal("Siphon couldn't tell which site this link is on."))?;
    let bins = Arc::clone(&app.state::<Arc<BinaryManager>>());
    let sessions = Arc::clone(&app.state::<Arc<Sessions>>());
    match &method {
        Method::Browser { browser } => match browsers::import(&bins, &sessions, browser, url, &site).await? {
            Ok(cookies) => signed_in(&sessions, &site, cookies, method),
            Err(refusal) => Ok(Outcome::Refused {
                short: refusal.short,
                reason: refusal.reason,
            }),
        },
        Method::File => from_file(app, &bins, &sessions, url, &site).await,
        Method::Window => in_window(app, url, &site).await,
    }
}

fn signed_in(sessions: &Sessions, site: &str, cookies: Vec<Cookie>, method: Method) -> Result<Outcome> {
    let tip = sessions.save(site, cookies, method)?;
    Ok(Outcome::SignedIn {
        site: site.to_string(),
        tip,
    })
}

/// Whether a lookup that failed got past the site's sign-in all the same:
/// the site answered with an error of the video's own (removed, unreadable,
/// protected) rather than "sign in". The sign-in works then, and the video's
/// own error shows once it is looked up again. A failed connection, a bot
/// check or a refusal prove nothing either way, and neither does a problem
/// on Siphon's side (no `details`: yt-dlp never answered).
pub(super) fn past_the_wall(error: &AppError) -> bool {
    error.details.is_some()
        && matches!(
            error.kind,
            ErrorKind::Unavailable | ErrorKind::Drm | ErrorKind::Internal
        )
}

/// Runs the lookup that failed again with `cookies`. Returns the cookies as
/// yt-dlp left them (sites renew some on the way) if it got past the sign-in,
/// None if the site still wants one.
async fn check(
    bins: &BinaryManager,
    sessions: &Sessions,
    url: &str,
    site: &str,
    cookies: &[Cookie],
) -> Result<Option<Vec<Cookie>>> {
    let trial = sessions.trial_file(cookies)?;
    match metadata::fetch(bins, url, Some(trial.path())).await {
        Err(error) if error.kind == ErrorKind::LoginRequired => Ok(None),
        Err(error) if !past_the_wall(&error) => Err(error),
        // Through, or past the sign-in to an error of the video's own.
        _ => {
            let renewed = cookies::scoped(trial.read_back(), &site::cookie_domains(site), super::now() as i64);
            Ok(Some(if renewed.is_empty() { cookies.to_vec() } else { renewed }))
        }
    }
}

// ── A cookies.txt file ──────────────────────────────────────────────────

async fn from_file(
    app: &AppHandle,
    bins: &BinaryManager,
    sessions: &Sessions,
    url: &str,
    site: &str,
) -> Result<Outcome> {
    let (chosen, picked) = oneshot::channel();
    app.dialog()
        .file()
        .set_title(format!("Choose a cookies.txt with your {site} sign-in"))
        .add_filter("cookies.txt", &["txt"])
        .pick_file(move |path| {
            let _ = chosen.send(path);
        });
    let Some(path) = picked.await.ok().flatten() else {
        return Ok(Outcome::Cancelled);
    };
    let path = path
        .into_path()
        .map_err(|err| AppError::internal("Couldn't open that file.").with_details(err.to_string()))?;
    if std::fs::metadata(&path).map(|meta| meta.len()).unwrap_or(0) > MAX_FILE {
        return Ok(not_a_cookie_file());
    }
    let text = String::from_utf8_lossy(&std::fs::read(&path)?).into_owned();
    let parsed = cookies::parse(&text);
    if parsed.cookies.is_empty() {
        return Ok(not_a_cookie_file());
    }
    let mine = cookies::scoped(parsed.cookies, &site::cookie_domains(site), super::now() as i64);
    if mine.is_empty() {
        return Ok(Outcome::Refused {
            short: format!("No {site} cookies in that file"),
            reason: format!("Export the cookies while you're on {site} and signed in, then choose the new file."),
        });
    }
    match check(bins, sessions, url, site, &mine).await? {
        Some(renewed) => signed_in(sessions, site, renewed, Method::File),
        None => Ok(Outcome::Refused {
            short: format!("That file's {site} sign-in didn't work"),
            reason: "The cookies in it may have run out, or come from an account that can't watch this video. Sign in again in your browser, export the cookies again, and choose the new file.".into(),
        }),
    }
}

fn not_a_cookie_file() -> Outcome {
    Outcome::Refused {
        short: "That isn't a cookies.txt file".into(),
        reason: "A cookies.txt file is exported from your browser with an extension that saves cookies in the Netscape format: a text file with one cookie per line, its parts separated by tabs.".into(),
    }
}

// ── Siphon's sign-in window ─────────────────────────────────────────────

async fn in_window(app: &AppHandle, url: &str, site: &str) -> Result<Outcome> {
    let (reply, outcome) = oneshot::channel();
    {
        let state = app.state::<SignInWindow>();
        let mut pending = state.pending.lock().unwrap();
        if let Some(open) = pending.as_ref() {
            if let Some(window) = app.get_window(WINDOW) {
                let _ = window.set_focus();
            }
            return Err(AppError::new(
                ErrorKind::Busy,
                format!("Finish signing in to {} first, or close that window.", open.site),
            ));
        }
        *pending = Some(Pending {
            site: site.to_string(),
            url: url.to_string(),
            reply,
            checking: false,
            watched: Vec::new(),
        });
    }
    if let Err(error) = open_window(app, site, url) {
        app.state::<SignInWindow>().pending.lock().unwrap().take();
        if let Some(window) = app.get_window(WINDOW) {
            let _ = window.destroy();
        }
        return Err(error);
    }
    tauri::async_runtime::spawn(watch(app.clone()));
    Ok(outcome.await.unwrap_or(Outcome::Cancelled))
}

fn open_window(app: &AppHandle, site: &str, url: &str) -> Result<()> {
    let failed =
        |err: tauri::Error| AppError::internal("Couldn't open the sign-in window.").with_details(err.to_string());
    let page = Url::parse(&site::sign_in_page(site, url))
        .map_err(|err| AppError::internal("Couldn't open the sign-in page.").with_details(err.to_string()))?;
    let profile = app.state::<Arc<Sessions>>().profile_dir(site);

    let window = WindowBuilder::new(app, WINDOW)
        .title(format!("Sign in to {site}"))
        .inner_size(520.0, 760.0)
        .min_inner_size(420.0, 520.0)
        .decorations(false)
        .shadow(true)
        .center()
        .build()
        .map_err(failed)?;
    let size = window
        .inner_size()
        .map_err(failed)?
        .to_logical::<f64>(window.scale_factor().map_err(failed)?);

    // Siphon's own page, like the main window: it can talk to Siphon.
    window
        .add_child(
            WebviewBuilder::new(BAR, WebviewUrl::App("index.html#sign-in".into())),
            LogicalPosition::new(0.0, 0.0),
            LogicalSize::new(size.width, BAR_HEIGHT),
        )
        .map_err(failed)?;

    // The site's page, in the site's own profile. It has no way to talk to
    // Siphon: no capability names it, and it isn't Siphon's origin.
    let app_for_page = app.clone();
    let site_page = WebviewBuilder::new(PAGE, WebviewUrl::External(page))
        .data_directory(profile)
        .on_navigation(move |url| {
            if site::is_google_refusal(url) {
                let app = app_for_page.clone();
                // Not from inside the navigation callback: the window closes after it returns.
                tauri::async_runtime::spawn(async move {
                    finish(
                        &app,
                        Outcome::Refused {
                            short: "Google won't sign in here".into(),
                            reason: "Google doesn't allow signing in from a window inside another app, and Siphon doesn't try to get around that. Take the sign-in over from Firefox, or use a cookies.txt file.".into(),
                        },
                    );
                });
            }
            true
        })
        .on_page_load(|webview, payload| {
            // Each finished page is a moment the user may have signed in.
            if payload.event() == PageLoadEvent::Finished {
                let _ = webview.app_handle().emit_to(BAR, STATUS_EVENT, Status::Waiting);
            }
        });
    window
        .add_child(
            site_page,
            LogicalPosition::new(0.0, BAR_HEIGHT),
            LogicalSize::new(size.width, (size.height - BAR_HEIGHT).max(0.0)),
        )
        .map_err(failed)?;

    let app = app.clone();
    let resized = window.clone();
    window.on_window_event(move |event| match event {
        WindowEvent::Resized(_) => layout(&resized),
        // Closed some other way than Done or Cancel (Alt+F4): cancelled.
        WindowEvent::Destroyed => {
            reply(&app, Outcome::Cancelled);
        }
        _ => {}
    });
    Ok(())
}

/// The bar across the top, the site's page under it.
fn layout(window: &tauri::Window) {
    let (Ok(size), Ok(scale)) = (window.inner_size(), window.scale_factor()) else {
        return;
    };
    let size = size.to_logical::<f64>(scale);
    for webview in window.webviews() {
        let (top, height) = if webview.label() == BAR {
            (0.0, BAR_HEIGHT)
        } else {
            (BAR_HEIGHT, (size.height - BAR_HEIGHT).max(0.0))
        };
        let _ = webview.set_position(LogicalPosition::new(0.0, top));
        let _ = webview.set_size(LogicalSize::new(size.width, height));
    }
}

/// The site's cookies in the sign-in window's profile. Off the main thread:
/// reading WebView2's cookies from it deadlocks.
async fn page_cookies(app: &AppHandle, site: &str) -> Option<Vec<Cookie>> {
    let page = app.get_webview(PAGE)?;
    let all = tauri::async_runtime::spawn_blocking(move || page.cookies())
        .await
        .ok()?
        .ok()?;
    let converted = all.iter().filter_map(|cookie| {
        Some(Cookie {
            // The leading dot of a domain cookie doesn't survive the conversion,
            // so every cookie is written as one for the domain and its
            // subdomains: never wider than the site.
            domain: cookie.domain()?.trim_start_matches('.').to_ascii_lowercase(),
            include_subdomains: true,
            path: cookie.path().unwrap_or("/").to_string(),
            secure: cookie.secure().unwrap_or(false),
            http_only: cookie.http_only().unwrap_or(false),
            expires: cookie.expires_datetime().map_or(0, |at| at.unix_timestamp().max(0)),
            name: cookie.name().to_string(),
            value: cookie.value().to_string(),
        })
    });
    Some(cookies::scoped(
        converted,
        &site::cookie_domains(site),
        super::now() as i64,
    ))
}

/// Finishes by itself once the site's session cookie appears, for sites
/// where it is known. Runs until the window closes.
async fn watch(app: AppHandle) {
    let Some(site) = pending_site(&app) else { return };
    let Some(session_cookie) = site::session_cookie(&site) else {
        return;
    };
    loop {
        tokio::time::sleep(WATCH_EVERY).await;
        if pending_site(&app).as_deref() != Some(site.as_str()) {
            return;
        }
        let Some(cookies) = page_cookies(&app, &site).await else {
            continue;
        };
        if !cookies.iter().any(|cookie| cookie.name == session_cookie) {
            continue;
        }
        let fresh = {
            let state = app.state::<SignInWindow>();
            let mut pending = state.pending.lock().unwrap();
            match pending.as_mut() {
                Some(pending) if pending.watched != cookies => {
                    pending.watched = cookies.clone();
                    true
                }
                _ => false,
            }
        };
        if fresh {
            done(&app, true).await;
        }
    }
}

fn pending_site(app: &AppHandle) -> Option<String> {
    app.state::<SignInWindow>()
        .pending
        .lock()
        .unwrap()
        .as_ref()
        .map(|pending| pending.site.clone())
}

/// Done was pressed (or the watcher saw a session): checks the window's
/// cookies on the video, and keeps them if they work.
pub async fn done(app: &AppHandle, by_itself: bool) {
    let started = {
        let state = app.state::<SignInWindow>();
        let mut pending = state.pending.lock().unwrap();
        match pending.as_mut() {
            Some(pending) if !pending.checking => {
                pending.checking = true;
                Some((pending.site.clone(), pending.url.clone()))
            }
            _ => None,
        }
    };
    let Some((site, url)) = started else { return };
    let status = |status: Status| {
        let _ = app.emit_to(BAR, STATUS_EVENT, status);
    };
    status(Status::Checking);

    let bins = Arc::clone(&app.state::<Arc<BinaryManager>>());
    let sessions = Arc::clone(&app.state::<Arc<Sessions>>());
    let result = match page_cookies(app, &site).await {
        Some(cookies) if !cookies.is_empty() => check(&bins, &sessions, &url, &site, &cookies).await,
        _ => Ok(None),
    };
    let saved = match result {
        Ok(Some(cookies)) => sessions.save(&site, cookies, Method::Window).map(Some),
        Ok(None) => Ok(None),
        Err(error) => Err(error),
    };
    match saved {
        Ok(Some(tip)) => finish(app, Outcome::SignedIn { site, tip }),
        // Not signed in yet: the watcher stays quiet, Done says so.
        Ok(None) => status(if by_itself { Status::Waiting } else { Status::NotYet }),
        Err(error) => status(if by_itself {
            Status::Waiting
        } else {
            Status::Failed { message: error.message }
        }),
    }
    if let Some(pending) = app.state::<SignInWindow>().pending.lock().unwrap().as_mut() {
        pending.checking = false;
    }
}

/// The site the open sign-in window is for, for its bar.
pub fn current_site(app: &AppHandle) -> Option<String> {
    pending_site(app)
}

/// Ends the sign-in with `outcome` and closes the window.
pub fn finish(app: &AppHandle, outcome: Outcome) {
    reply(app, outcome);
    if let Some(window) = app.get_window(WINDOW) {
        let _ = window.destroy();
    }
}

fn reply(app: &AppHandle, outcome: Outcome) {
    let pending = app.state::<SignInWindow>().pending.lock().unwrap().take();
    if let Some(pending) = pending {
        let _ = pending.reply.send(outcome);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ytdlp::errors::classify;

    #[test]
    fn a_video_that_fails_for_its_own_reason_still_proves_the_sign_in() {
        // Found 2026-11-18: signed in to Instagram, the test reel then failed
        // like this, and Done could never finish.
        assert!(past_the_wall(&classify(
            "ERROR: [Instagram] CWqAgUZgCku: Video info extraction failed: HTTP Error 400: Bad Request (caused by <HTTPError 400: Bad Request>)"
        )));
        assert!(past_the_wall(&classify(
            "ERROR: [youtube] abc: Video unavailable. This video has been removed by the uploader"
        )));
        // Still behind the sign-in, or nothing learned.
        assert!(!past_the_wall(&classify(
            "ERROR: [youtube] abc: Sign in to confirm your age. Use --cookies-from-browser or --cookies for the authentication."
        )));
        assert!(!past_the_wall(&classify(
            "ERROR: Unable to download webpage: timed out"
        )));
        assert!(!past_the_wall(&classify(
            "ERROR: [youtube] abc: Sign in to confirm you’re not a bot. Use --cookies-from-browser or --cookies for the authentication."
        )));
        assert!(!past_the_wall(&classify("ERROR: HTTP Error 429: Too Many Requests")));
        // Siphon's own trouble: yt-dlp never answered.
        assert!(!past_the_wall(&AppError::internal(
            "Couldn't start the download engine."
        )));
    }
}
