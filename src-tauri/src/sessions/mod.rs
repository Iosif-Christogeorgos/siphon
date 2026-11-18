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
use crate::error::{AppError, Chance, Result, SignInNeed};
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
    /// When the sign-in is used; sites saved before this setting existed get the default.
    #[serde(default)]
    pub mode: Mode,
}

/// When a signed-in site's cookies go with a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    /// Only after the site refused without them (the default). Less of the
    /// account's activity is on record, and a video that plays for anyone
    /// keeps working when the signed-in way is broken (Instagram, 2026-11).
    #[default]
    WhenNeeded,
    /// Every time: on some sites an account gets better quality than anyone
    /// else (YouTube Premium's higher-bitrate 1080p).
    Always,
}

/// How one lookup treats the site's sign-in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Account {
    /// As the site's [`Mode`] says.
    Auto,
    /// With it: the user asked for this one ("Try with your sign-in").
    Use,
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

    fn mode(&self, site: &str) -> Mode {
        self.index
            .lock()
            .unwrap()
            .sites
            .get(site)
            .map(|entry| entry.mode)
            .unwrap_or_default()
    }

    /// Settings' "When needed" or "Always" for a signed-in site.
    pub fn set_mode(&self, site: &str, mode: Mode) {
        let mut index = self.index.lock().unwrap();
        if let Some(entry) = index.sites.get_mut(site) {
            entry.mode = mode;
            self.save_index(&index);
        }
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
        // Signing in again keeps the site's When needed / Always setting.
        let mode = index.sites.get(site).map(|old| old.mode).unwrap_or_default();
        index.sites.insert(
            site.to_string(),
            SiteSession {
                site: site.to_string(),
                method,
                signed_in_at: now(),
                last_used: Some(now()),
                expired: false,
                mode,
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

    /// Adds what signing in could do to an error from a yt-dlp run on `url`:
    /// which site, how likely it is to help (`AppError::chance`), whether
    /// Siphon is signed in there, and whether the run already had the site's
    /// cookies. A login wall that refuses them means the session ran out,
    /// which is also remembered, so Settings shows it; anything else that
    /// refuses them only means this account has no access.
    pub fn explain(&self, mut error: AppError, url: &str, had_cookies: bool) -> AppError {
        if error.chance == Chance::Never {
            return error;
        }
        let Some(site) = site::site_of(url) else { return error };
        if had_cookies && error.chance == Chance::Needed {
            self.mark_expired(&site);
        }
        error.sign_in = Some(SignInNeed {
            signed_in: self.has(&site),
            site,
            chance: error.chance,
            refused: had_cookies,
        });
        error
    }

    fn mark_expired(&self, site: &str) {
        let mut index = self.index.lock().unwrap();
        if let Some(entry) = index.sites.get_mut(site) {
            entry.expired = true;
            self.save_index(&index);
        }
    }

    /// Looks a link up, without an account first (see [`Sessions::lookup`]),
    /// and notes on the video whether its sign-in was used.
    pub async fn fetch_media(self: &Arc<Self>, bins: &BinaryManager, url: &str, account: Account) -> Result<Media> {
        let (mut media, with_account) = self
            .lookup(url, account, |cookies| async move {
                metadata::fetch(bins, url, cookies.as_deref()).await
            })
            .await?;
        if let Media::Video(video) = &mut media {
            video.with_account = with_account;
        }
        Ok(media)
    }

    /// Runs a lookup (`fetch`, given the cookie file to use, if any) the way
    /// the site's sign-in calls for, and says whether the sign-in was used.
    ///
    /// Without an account first, always, unless the site is set to Always or
    /// the user asked for this one: most videos need none, and an account
    /// should only be used, and only be asked for, where it is needed. Then,
    /// on a site Siphon is signed in to, a refusal an account could fix (a
    /// login wall, or members-only and the like) is tried once more with the
    /// sign-in, by itself. The bot check and unrecognised errors are not:
    /// those are the user's call (`Account::Use`).
    async fn lookup<F, Fut>(self: &Arc<Self>, url: &str, account: Account, mut fetch: F) -> Result<(Media, bool)>
    where
        F: FnMut(Option<PathBuf>) -> Fut,
        Fut: std::future::Future<Output = Result<Media>>,
    {
        let site = site::site_of(url);
        let signed_in = site.as_deref().is_some_and(|site| self.has(site));
        let always = site.as_deref().is_some_and(|site| self.mode(site) == Mode::Always);
        if !(signed_in && (account == Account::Use || always)) {
            match fetch(None).await {
                Ok(media) => return Ok((media, false)),
                Err(error) if signed_in && matches!(error.chance, Chance::Needed | Chance::Maybe) => {}
                Err(error) => return Err(self.explain(error, url, false)),
            }
        }
        let cookies = self.cookie_file(url);
        let with = cookies.is_some();
        match fetch(cookies.as_ref().map(|file| file.path().to_path_buf())).await {
            Ok(media) => Ok((media, with)),
            Err(error) => Err(self.explain(error, url, with)),
        }
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
    use crate::error::ErrorKind;

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

    fn refusal(chance: Chance) -> AppError {
        let mut error = AppError::new(ErrorKind::LoginRequired, "refused").with_details("ERROR: refused");
        error.chance = chance;
        error
    }

    #[test]
    fn a_login_wall_that_refuses_the_sign_in_means_it_ran_out_and_nothing_else_does() {
        let dir = scratch("expiry");
        let sessions = Sessions::at(dir.clone());
        let need = |error: AppError| error.sign_in.unwrap();

        // Not signed in yet: which site, and how likely an account is to help.
        let first = need(sessions.explain(refusal(Chance::Needed), "https://youtu.be/abc", false));
        assert_eq!(
            first,
            SignInNeed {
                site: "youtube.com".into(),
                chance: Chance::Needed,
                signed_in: false,
                refused: false
            }
        );

        sessions
            .save("youtube.com", vec![cookie("youtube.com", "SID", "1")], Method::Window)
            .unwrap();
        // Members-only and the like, refused with the sign-in: this account has
        // no access. The sign-in itself is fine, so it doesn't run out.
        let no_access = need(sessions.explain(refusal(Chance::Maybe), "https://youtu.be/abc", true));
        assert!(no_access.signed_in && no_access.refused);
        assert!(!sessions.list()[0].expired);

        // A login wall that refuses the sign-in: it ran out.
        let ran_out = need(sessions.explain(refusal(Chance::Needed), "https://youtu.be/abc", true));
        assert!(ran_out.refused);
        assert!(sessions.list()[0].expired);
        // Kept for the next launch, and cleared by signing in again.
        assert!(Sessions::at(dir.clone()).list()[0].expired);
        sessions
            .save("youtube.com", vec![cookie("youtube.com", "SID", "2")], Method::Window)
            .unwrap();
        assert!(!sessions.list()[0].expired);

        // What an account can't fix gets no offer at all.
        let mut gone = AppError::new(ErrorKind::Unavailable, "gone");
        gone.chance = Chance::Never;
        assert!(sessions.explain(gone, "https://youtu.be/abc", true).sign_in.is_none());
        let _ = std::fs::remove_dir_all(dir);
    }

    fn found() -> Media {
        Media::Video(Box::new(crate::ytdlp::metadata::VideoInfo {
            id: "abc".into(),
            title: "Test".into(),
            url: "https://www.youtube.com/watch?v=abc".into(),
            thumbnail: None,
            duration: None,
            channel: None,
            view_count: None,
            extractor: None,
            formats: Vec::new(),
            with_account: false,
        }))
    }

    /// Runs a lookup whose site answers `answers` in turn (Ok, or a refusal of
    /// that chance). Returns, for each request, whether it carried the
    /// sign-in, and how the lookup ended.
    async fn lookup(
        sessions: &Arc<Sessions>,
        account: Account,
        answers: Vec<std::result::Result<(), Chance>>,
    ) -> (Vec<bool>, Result<(Media, bool)>) {
        let mut sent = Vec::new();
        let mut answers = answers.into_iter();
        let result = sessions
            .lookup("https://www.youtube.com/watch?v=abc", account, |cookies| {
                sent.push(cookies.is_some());
                std::future::ready(match answers.next().expect("asked the site more often than expected") {
                    Ok(()) => Ok(found()),
                    Err(chance) => Err(refusal(chance)),
                })
            })
            .await;
        (sent, result)
    }

    #[tokio::test]
    async fn an_account_is_only_used_where_the_site_wanted_one() {
        let dir = scratch("anonymous");
        let sessions = Arc::new(Sessions::at(dir.clone()));

        // Not signed in: one request, without an account; a refusal offers the sign-in.
        let (sent, result) = lookup(&sessions, Account::Auto, vec![Ok(())]).await;
        assert_eq!(sent, [false]);
        assert!(!result.unwrap().1);
        let (sent, result) = lookup(&sessions, Account::Auto, vec![Err(Chance::Needed)]).await;
        assert_eq!(sent, [false]);
        let need = result.err().unwrap().sign_in.unwrap();
        assert!(!need.signed_in && !need.refused);

        sessions
            .save(
                "youtube.com",
                vec![cookie("youtube.com", "SAPISID", "1")],
                Method::Window,
            )
            .unwrap();

        // Signed in, and the video plays for anyone: still without the account.
        let (sent, result) = lookup(&sessions, Account::Auto, vec![Ok(())]).await;
        assert_eq!(sent, [false], "a video that needs no account went with one");
        assert!(!result.unwrap().1);

        // A login wall, or members-only: tried again with the sign-in, by itself.
        for chance in [Chance::Needed, Chance::Maybe] {
            let (sent, result) = lookup(&sessions, Account::Auto, vec![Err(chance), Ok(())]).await;
            assert_eq!(sent, [false, true], "{chance:?}");
            assert!(
                result.unwrap().1,
                "{chance:?}: the video should say it used the sign-in"
            );
        }

        // The bot check and unrecognised errors are the user's call: not retried,
        // and the error says Siphon could try with the sign-in.
        for chance in [Chance::Risky, Chance::Unknown] {
            let (sent, result) = lookup(&sessions, Account::Auto, vec![Err(chance)]).await;
            assert_eq!(sent, [false], "{chance:?}");
            let need = result.err().unwrap().sign_in.unwrap();
            assert!(need.signed_in && !need.refused, "{chance:?}");
        }
        // Nothing an account can't fix is tried with it.
        let (sent, result) = lookup(&sessions, Account::Auto, vec![Err(Chance::Never)]).await;
        assert_eq!(sent, [false]);
        assert!(result.err().unwrap().sign_in.is_none());

        // The user asked for this one, or set the site to Always: with the sign-in from the start.
        let (sent, _) = lookup(&sessions, Account::Use, vec![Ok(())]).await;
        assert_eq!(sent, [true]);
        sessions.set_mode("youtube.com", Mode::Always);
        let (sent, _) = lookup(&sessions, Account::Auto, vec![Ok(())]).await;
        assert_eq!(sent, [true]);
        // And the setting survives signing in again.
        sessions
            .save(
                "youtube.com",
                vec![cookie("youtube.com", "SAPISID", "2")],
                Method::Window,
            )
            .unwrap();
        assert_eq!(sessions.list()[0].mode, Mode::Always);
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
