//! Signed-in sites: one cookie jar per site, encrypted for the Windows user,
//! and handed to yt-dlp one download at a time.
//!
//! Siphon doesn't log in to anything itself. The user signs in to a site
//! the usual way (see [`sign_in`]), Siphon keeps that site's cookies, and
//! yt-dlp, which knows every site's quirks, uses them. A download only ever
//! gets the cookies of its own site (see [`site`]), in a temporary
//! `cookies.txt` that lives as long as the download does (see [`CookieFile`]).
//!
//! On disk, in `%LOCALAPPDATA%\app.siphon.desktop\sessions`:
//!
//! ```text
//! index.json       which sites are signed in, how, since when, and whether the session ran out
//! jars\<site>.bin  the site's cookies, encrypted with DPAPI
//! profiles\<site>\ the sign-in window's browser profile for the site
//! temp\            cookie files of running downloads; emptied at launch and on quit
//! ```

pub mod browsers;
pub mod cookies;
mod dpapi;
pub mod sign_in;
pub mod site;

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

use crate::binaries::BinaryManager;
use crate::error::{AppError, ErrorKind, Result, SignInNeed};
use crate::ytdlp::metadata::{self, Media};
use cookies::Cookie;

/// How a site's session was made.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Method {
    /// Siphon's own sign-in window.
    Window,
    /// Imported from a browser on this computer (`firefox`, `chrome`, …).
    Browser { browser: String },
    /// A `cookies.txt` the user exported.
    File,
}

/// One signed-in site, as Settings lists it. Never holds a cookie.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SiteSession {
    pub site: String,
    pub method: Method,
    /// Unix seconds.
    pub signed_in_at: u64,
    pub last_used: Option<u64>,
    /// The site refused its cookies: signing in again is the only fix.
    pub expired: bool,
}

#[derive(Default, Serialize, Deserialize)]
struct Index {
    #[serde(default)]
    sites: BTreeMap<String, SiteSession>,
    /// Sites whose first-sign-in tip has been shown.
    #[serde(default)]
    tipped: BTreeSet<String>,
}

pub struct Sessions {
    dir: PathBuf,
    index: Mutex<Index>,
    /// Held while a jar is read and rewritten, so two downloads writing back
    /// renewed cookies at once don't lose one another's.
    jar_lock: Mutex<()>,
}

/// For unique temporary file names.
static NEXT_FILE: AtomicU64 = AtomicU64::new(0);

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

impl Sessions {
    pub fn load(app: &AppHandle) -> Arc<Self> {
        let dir = app
            .path()
            .app_local_data_dir()
            .unwrap_or_else(|_| std::env::temp_dir().join("app.siphon.desktop"))
            .join("sessions");
        Arc::new(Self::at(dir))
    }

    /// Sessions kept in `dir`, for tests elsewhere.
    #[cfg(test)]
    pub fn in_dir(dir: PathBuf) -> Arc<Self> {
        Arc::new(Self::at(dir))
    }

    fn at(dir: PathBuf) -> Self {
        let index = std::fs::read(dir.join("index.json"))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        let sessions = Self {
            dir,
            index: Mutex::new(index),
            jar_lock: Mutex::new(()),
        };
        // Whatever the last session left behind, if it couldn't clean up.
        sessions.sweep_temp();
        sessions
    }

    /// Every signed-in site, for Settings.
    pub fn list(&self) -> Vec<SiteSession> {
        self.index.lock().unwrap().sites.values().cloned().collect()
    }

    fn has(&self, site: &str) -> bool {
        self.index.lock().unwrap().sites.contains_key(site)
    }

    /// Keeps a site's cookies (only its own), replacing any it had. Returns
    /// whether this is the first time for a site whose sign-in needs a word
    /// of caution (see [`site::is_google`]).
    pub fn save(&self, site: &str, cookies: Vec<Cookie>, method: Method) -> Result<bool> {
        let cookies = cookies::scoped(cookies, &site::cookie_domains(site), now() as i64);
        if cookies.is_empty() {
            return Err(AppError::internal(format!("There were no cookies for {site}.")));
        }
        {
            let _jar = self.jar_lock.lock().unwrap();
            self.write_jar(site, &cookies)?;
        }
        let mut index = self.index.lock().unwrap();
        index.sites.insert(
            site.to_string(),
            SiteSession {
                site: site.to_string(),
                method,
                signed_in_at: now(),
                last_used: Some(now()),
                expired: false,
            },
        );
        let tip = site::is_google(site) && index.tipped.insert(site.to_string());
        self.save_index(&index);
        Ok(tip)
    }

    /// Forgets a site: its cookies, its entry and its sign-in window's profile.
    pub fn sign_out(&self, site: &str) {
        {
            let _jar = self.jar_lock.lock().unwrap();
            let _ = std::fs::remove_file(self.jar_path(site));
        }
        let mut index = self.index.lock().unwrap();
        index.sites.remove(site);
        self.save_index(&index);
        drop(index);
        let _ = std::fs::remove_dir_all(self.profile_dir(site));
    }

    /// The sign-in window's own browser profile for `site`, apart from
    /// Siphon's interface and from every other site.
    pub fn profile_dir(&self, site: &str) -> PathBuf {
        self.dir.join("profiles").join(file_name(site))
    }

    /// A `cookies.txt` with the stored cookies of `url`'s site, for one yt-dlp
    /// run, or None if Siphon isn't signed in there.
    pub fn cookie_file(self: &Arc<Self>, url: &str) -> Option<CookieFile> {
        let site = site::site_of(url)?;
        if !self.has(&site) {
            return None;
        }
        let jar = {
            let _jar = self.jar_lock.lock().unwrap();
            self.read_jar(&site)
        };
        let cookies = cookies::scoped(jar, &site::cookie_domains(&site), now() as i64);
        if cookies.is_empty() {
            return None;
        }
        let file = self.temp_file(&cookies).ok()?;
        if let Some(entry) = self.index.lock().unwrap().sites.get_mut(&site) {
            entry.last_used = Some(now());
        }
        // Saved without waiting: "last used" is only a hint.
        self.save_index(&self.index.lock().unwrap());
        Some(CookieFile {
            path: file,
            written: cookies,
            site: Some((site, Arc::clone(self))),
        })
    }

    /// A `cookies.txt` with cookies that aren't kept yet: for checking that a
    /// sign-in works before saving it.
    pub fn trial_file(&self, cookies: &[Cookie]) -> Result<CookieFile> {
        Ok(CookieFile {
            path: self.temp_file(cookies)?,
            written: cookies.to_vec(),
            site: None,
        })
    }

    /// A private temporary file for yt-dlp to read cookies from (and write
    /// renewed ones back to).
    pub fn temp_file_path(&self) -> Result<PathBuf> {
        let dir = self.dir.join("temp");
        std::fs::create_dir_all(&dir)?;
        let n = NEXT_FILE.fetch_add(1, Ordering::Relaxed);
        Ok(dir.join(format!("{}-{n}.txt", std::process::id())))
    }

    fn temp_file(&self, cookies: &[Cookie]) -> Result<PathBuf> {
        let path = self.temp_file_path()?;
        std::fs::write(&path, cookies::write(cookies))?;
        Ok(path)
    }

    /// Deletes every temporary cookie file: at launch, and when Siphon quits
    /// (after its downloads have stopped).
    pub fn sweep_temp(&self) {
        let _ = std::fs::remove_dir_all(self.dir.join("temp"));
    }

    /// Adds what signing in can do to an error from a yt-dlp run on `url`:
    /// which site to sign in to, and, when the run already had that site's
    /// cookies, that the session ran out (which is also remembered, so Settings shows it).
    pub fn explain(&self, mut error: AppError, url: &str, had_cookies: bool) -> AppError {
        if error.kind != ErrorKind::LoginRequired {
            return error;
        }
        let Some(site) = site::site_of(url) else { return error };
        let expired = had_cookies && self.mark_expired(&site);
        error.sign_in = Some(SignInNeed { site, expired });
        error
    }

    fn mark_expired(&self, site: &str) -> bool {
        let mut index = self.index.lock().unwrap();
        let Some(entry) = index.sites.get_mut(site) else {
            return false;
        };
        entry.expired = true;
        self.save_index(&index);
        true
    }

    /// Looks a link up with its site's cookies, if Siphon is signed in there.
    pub async fn fetch_media(self: &Arc<Self>, bins: &BinaryManager, url: &str) -> Result<Media> {
        let cookies = self.cookie_file(url);
        metadata::fetch(bins, url, cookies.as_ref().map(CookieFile::path))
            .await
            .map_err(|error| self.explain(error, url, cookies.is_some()))
    }

    /// Writes back what a yt-dlp run changed in `site`'s cookies: sites renew
    /// some as they are used, and the session stays fresh that way.
    fn write_back(&self, site: &str, before: &[Cookie], file: &Path) {
        let Ok(text) = std::fs::read_to_string(file) else {
            return;
        };
        let after = cookies::scoped(cookies::parse(&text).cookies, &site::cookie_domains(site), now() as i64);
        let changes = cookies::changes(before, &after);
        if changes.is_empty() || after.is_empty() {
            // Unchanged, or yt-dlp never got to write the file back.
            return;
        }
        let _jar = self.jar_lock.lock().unwrap();
        let mut jar = self.read_jar(site);
        cookies::apply(&mut jar, &changes, now() as i64);
        let _ = self.write_jar(site, &jar);
    }

    fn jar_path(&self, site: &str) -> PathBuf {
        self.dir.join("jars").join(format!("{}.bin", file_name(site)))
    }

    /// The stored cookies; none if there are none or they can't be decrypted
    /// (another Windows user's, or damaged).
    fn read_jar(&self, site: &str) -> Vec<Cookie> {
        std::fs::read(self.jar_path(site))
            .ok()
            .and_then(|sealed| dpapi::unprotect(&sealed).ok())
            .and_then(|plain| String::from_utf8(plain).ok())
            .map(|text| cookies::parse(&text).cookies)
            .unwrap_or_default()
    }

    fn write_jar(&self, site: &str, jar: &[Cookie]) -> Result<()> {
        let path = self.jar_path(site);
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let sealed = dpapi::protect(cookies::write(jar).as_bytes())?;
        let tmp = path.with_extension("bin.tmp");
        std::fs::write(&tmp, sealed)?;
        std::fs::rename(&tmp, &path)?;
        Ok(())
    }

    fn save_index(&self, index: &Index) {
        let _ = std::fs::create_dir_all(&self.dir);
        let Ok(bytes) = serde_json::to_vec_pretty(index) else {
            return;
        };
        let path = self.dir.join("index.json");
        let tmp = path.with_extension("json.tmp");
        if std::fs::write(&tmp, bytes).is_ok() {
            let _ = std::fs::rename(&tmp, path);
        }
    }
}

/// A site name made safe as a file name (it already is, coming from a host name).
fn file_name(site: &str) -> String {
    site.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-') {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// A temporary `cookies.txt` for one yt-dlp run. Dropping it, which happens
/// however the run ends, writes renewed cookies back to the jar and deletes
/// the file.
pub struct CookieFile {
    path: PathBuf,
    /// What Siphon put in it, to see what yt-dlp changed.
    written: Vec<Cookie>,
    /// The site whose jar it came from; None for a sign-in being checked.
    site: Option<(String, Arc<Sessions>)>,
}

impl CookieFile {
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The cookies as yt-dlp left them, for a sign-in that was just checked.
    pub fn read_back(&self) -> Vec<Cookie> {
        std::fs::read_to_string(&self.path)
            .map(|text| cookies::parse(&text).cookies)
            .unwrap_or_else(|_| self.written.clone())
    }
}

impl Drop for CookieFile {
    fn drop(&mut self) {
        if let Some((site, sessions)) = &self.site {
            sessions.write_back(site, &self.written, &self.path);
        }
        let _ = std::fs::remove_file(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cookie(domain: &str, name: &str, value: &str) -> Cookie {
        Cookie {
            domain: domain.into(),
            include_subdomains: true,
            path: "/".into(),
            secure: true,
            http_only: true,
            expires: 0,
            name: name.into(),
            value: value.into(),
        }
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("siphon-test-sessions-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn a_download_gets_only_its_own_sites_cookies_and_the_file_goes_with_it() {
        let dir = scratch("scope");
        let sessions = Arc::new(Sessions::at(dir.clone()));
        // Even if a jar were handed someone else's cookies, they'd be left out.
        sessions
            .save(
                "youtube.com",
                vec![
                    cookie("youtube.com", "SID", "fake-yt"),
                    cookie("google.com", "SID", "fake-google"),
                ],
                Method::File,
            )
            .unwrap();
        sessions
            .save(
                "instagram.com",
                vec![cookie("instagram.com", "sessionid", "fake-ig")],
                Method::File,
            )
            .unwrap();

        let file = sessions.cookie_file("https://youtu.be/abc").unwrap();
        let text = std::fs::read_to_string(file.path()).unwrap();
        assert!(text.contains("fake-yt"));
        assert!(!text.contains("fake-google"));
        assert!(!text.contains("fake-ig"));
        // Stored encrypted: no value appears on disk in the clear.
        let sealed = std::fs::read(sessions.jar_path("youtube.com")).unwrap();
        assert!(!String::from_utf8_lossy(&sealed).contains("fake-yt"));

        let path = file.path().to_path_buf();
        drop(file);
        assert!(!path.exists());
        // A site Siphon isn't signed in to gets no file at all.
        assert!(sessions.cookie_file("https://vimeo.com/1").is_none());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn renewed_cookies_reach_the_jar_when_the_run_ends() {
        let dir = scratch("renew");
        let sessions = Arc::new(Sessions::at(dir.clone()));
        sessions
            .save(
                "youtube.com",
                vec![
                    cookie("youtube.com", "SID", "1"),
                    cookie("youtube.com", "PSIDTS", "old"),
                ],
                Method::Window,
            )
            .unwrap();
        let file = sessions.cookie_file("https://www.youtube.com/watch?v=abc").unwrap();
        // What yt-dlp writes back when it's done: one cookie renewed, plus one for a
        // video host it talked to, which isn't the site's and must not be kept.
        std::fs::write(
            file.path(),
            cookies::write(&[
                cookie("youtube.com", "SID", "1"),
                cookie("youtube.com", "PSIDTS", "new"),
                cookie("googlevideo.com", "X", "fake"),
            ]),
        )
        .unwrap();
        drop(file);
        let jar = sessions.read_jar("youtube.com");
        let names: Vec<_> = jar.iter().map(|c| (c.name.as_str(), c.value.as_str())).collect();
        assert_eq!(names, [("SID", "1"), ("PSIDTS", "new")]);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_refusal_with_cookies_means_the_session_ran_out() {
        let dir = scratch("expiry");
        let sessions = Sessions::at(dir.clone());
        let refused = || AppError::new(ErrorKind::LoginRequired, "needs an account");

        // Not signed in yet: offer to sign in.
        let error = sessions.explain(refused(), "https://youtu.be/abc", false);
        assert_eq!(
            error.sign_in,
            Some(SignInNeed {
                site: "youtube.com".into(),
                expired: false
            })
        );

        sessions
            .save("youtube.com", vec![cookie("youtube.com", "SID", "1")], Method::Window)
            .unwrap();
        let error = sessions.explain(refused(), "https://youtu.be/abc", true);
        assert!(error.sign_in.unwrap().expired);
        assert!(sessions.list()[0].expired);
        // Kept for the next launch, and cleared by signing in again.
        assert!(Sessions::at(dir.clone()).list()[0].expired);
        sessions
            .save("youtube.com", vec![cookie("youtube.com", "SID", "2")], Method::Window)
            .unwrap();
        assert!(!sessions.list()[0].expired);

        // Other errors are left alone.
        let error = sessions.explain(
            AppError::new(ErrorKind::Unavailable, "gone"),
            "https://youtu.be/abc",
            true,
        );
        assert!(error.sign_in.is_none());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_youtube_tip_shows_once() {
        let dir = scratch("tip");
        let sessions = Sessions::at(dir.clone());
        let jar = || vec![cookie("youtube.com", "SID", "1")];
        assert!(sessions.save("youtube.com", jar(), Method::Window).unwrap());
        assert!(!sessions.save("youtube.com", jar(), Method::Window).unwrap());
        assert!(!sessions
            .save(
                "instagram.com",
                vec![cookie("instagram.com", "sessionid", "1")],
                Method::Window
            )
            .unwrap());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn signing_out_forgets_everything_about_the_site() {
        let dir = scratch("signout");
        let sessions = Arc::new(Sessions::at(dir.clone()));
        sessions
            .save("youtube.com", vec![cookie("youtube.com", "SID", "1")], Method::Window)
            .unwrap();
        std::fs::create_dir_all(sessions.profile_dir("youtube.com").join("Default")).unwrap();
        sessions.sign_out("youtube.com");
        assert!(sessions.list().is_empty());
        assert!(!sessions.jar_path("youtube.com").exists());
        assert!(!sessions.profile_dir("youtube.com").exists());
        assert!(sessions.cookie_file("https://youtu.be/abc").is_none());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn leftover_cookie_files_are_removed_at_launch() {
        let dir = scratch("sweep");
        let sessions = Arc::new(Sessions::at(dir.clone()));
        let path = sessions
            .trial_file(&[cookie("youtube.com", "SID", "1")])
            .unwrap()
            .path()
            .to_path_buf();
        // As if Siphon had stopped without cleaning up.
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "left behind").unwrap();
        Sessions::at(dir.clone());
        assert!(!path.exists());
        let _ = std::fs::remove_dir_all(dir);
    }
}
