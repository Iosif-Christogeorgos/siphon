//! Taking a sign-in over from a browser on this computer, through yt-dlp's
//! `--cookies-from-browser`.
//!
//! Firefox works. Chromium browsers on Windows mostly don't any more: they
//! lock their cookie database while open, and since 2024 encrypt cookies so
//! only the browser itself can read them ("app-bound encryption"). Every
//! such failure ends in a plain explanation, never a hang (the lookup has
//! the usual time limit).

use std::ffi::OsString;
use std::path::PathBuf;

use serde::Serialize;

use super::cookies::{self, Cookie};
use super::{site, Sessions};
use crate::binaries::BinaryManager;
use crate::error::{ErrorKind, Result};
use crate::ytdlp::metadata;

/// A browser yt-dlp can read cookies from.
#[derive(Debug, Clone, Serialize)]
pub struct Browser {
    /// yt-dlp's name for it.
    pub id: &'static str,
    pub name: &'static str,
}

/// yt-dlp's browsers on Windows, with the folder that holds their cookies,
/// from `%LOCALAPPDATA%` (true) or `%APPDATA%` (false). Firefox first: it is
/// the one that reliably works.
const KNOWN: &[(&str, &str, bool, &str)] = &[
    ("firefox", "Firefox", false, r"Mozilla\Firefox\Profiles"),
    ("chrome", "Chrome", true, r"Google\Chrome\User Data"),
    ("edge", "Edge", true, r"Microsoft\Edge\User Data"),
    ("brave", "Brave", true, r"BraveSoftware\Brave-Browser\User Data"),
    ("opera", "Opera", false, r"Opera Software\Opera Stable"),
    ("vivaldi", "Vivaldi", true, r"Vivaldi\User Data"),
    ("chromium", "Chromium", true, r"Chromium\User Data"),
    ("whale", "Whale", true, r"Naver\Naver Whale\User Data"),
];

/// The browsers that have been used on this computer: those with a cookie folder.
pub fn installed() -> Vec<Browser> {
    let folder = |var: &str| std::env::var_os(var).map(PathBuf::from);
    let (local, roaming) = (folder("LOCALAPPDATA"), folder("APPDATA"));
    KNOWN
        .iter()
        .filter(|(_, _, is_local, path)| {
            let base = if *is_local { &local } else { &roaming };
            base.as_ref().is_some_and(|base| base.join(path).is_dir())
        })
        .map(|(id, name, _, _)| Browser { id, name })
        .collect()
}

fn name_of(id: &str) -> &str {
    KNOWN.iter().find(|known| known.0 == id).map_or(id, |known| known.1)
}

/// Why taking cookies over from a browser didn't work, in a line and in full.
#[derive(Debug, PartialEq, Eq)]
pub struct Refusal {
    pub short: String,
    pub reason: String,
}

/// Reads `browser`'s cookies for `url`'s site and checks they unlock `url`.
/// The lookup is the check: it runs with the browser's cookies, and only if
/// it gets through are the site's own cookies (and no others) kept.
pub async fn import(
    bins: &BinaryManager,
    sessions: &Sessions,
    browser: &str,
    url: &str,
    site: &str,
) -> Result<std::result::Result<Vec<Cookie>, Refusal>> {
    let Some(&(id, ..)) = KNOWN.iter().find(|known| known.0 == browser) else {
        return Ok(Err(Refusal {
            short: "Siphon can't read that browser".into(),
            reason: "Choose another way to sign in.".into(),
        }));
    };
    // yt-dlp writes every cookie it read from the browser here when it's done:
    // deleted as soon as the site's own have been picked out (the guard's drop).
    let dump = sessions.trial_file(&[])?;
    let args: Vec<OsString> = vec![
        "--cookies-from-browser".into(),
        id.into(),
        "--cookies".into(),
        dump.path().into(),
    ];
    match metadata::fetch_with(bins, url, &args).await {
        Ok(_) => {
            let mine = cookies::scoped(dump.read_back(), &site::cookie_domains(site), super::now() as i64);
            if mine.is_empty() {
                // The video played without an account after all; nothing to keep.
                return Ok(Err(not_signed_in(id, site)));
            }
            Ok(Ok(mine))
        }
        Err(error) => {
            let log = format!("{} {}", error.message, error.details.as_deref().unwrap_or_default()).to_lowercase();
            if let Some(refusal) = refusal(id, site, &log, error.kind) {
                Ok(Err(refusal))
            } else {
                Err(error)
            }
        }
    }
}

/// Turns what yt-dlp said into the reason, where it is about the browser or
/// the account rather than the network or the site.
fn refusal(id: &str, site: &str, log: &str, kind: ErrorKind) -> Option<Refusal> {
    let name = name_of(id);
    if log.contains("could not copy") && log.contains("cookie database") {
        return Some(Refusal {
            short: format!("Close {name} first"),
            reason: format!(
                "{name} locks its cookies while it's open. Close every {name} window, make sure it isn't still running in the background, then try again."
            ),
        });
    }
    if log.contains("failed to decrypt") || log.contains("could not be decrypted") || log.contains("cannot decrypt") {
        return Some(Refusal {
            short: format!("{name}'s cookies can't be read"),
            reason: format!(
                "{name} encrypts its cookies on Windows so that only {name} can read them. Use Siphon's sign-in window, Firefox, or a cookies.txt file instead."
            ),
        });
    }
    if log.contains("could not find") && log.contains("cookies database") {
        return Some(Refusal {
            short: format!("{name} has no cookies"),
            reason: format!("Siphon found no {name} profile with cookies. Sign in to {site} in {name} first."),
        });
    }
    (kind == ErrorKind::LoginRequired).then(|| not_signed_in(id, site))
}

fn not_signed_in(id: &str, site: &str) -> Refusal {
    let name = name_of(id);
    Refusal {
        short: format!("{name} isn't signed in to {site}"),
        reason: format!("Sign in to {site} in {name}, then try again. Or choose another way to sign in."),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What yt-dlp says when each way of reading a browser fails (from `yt_dlp/cookies.py`).
    #[test]
    fn explains_each_way_a_browser_can_refuse() {
        let short = |id: &str, log: &str, kind: ErrorKind| {
            refusal(id, "youtube.com", &log.to_lowercase(), kind).map(|r| r.short)
        };
        assert_eq!(
            short("chrome", "ERROR: Could not copy Chrome cookie database. See  https://github.com/yt-dlp/yt-dlp/issues/7271  for more info", ErrorKind::Internal).as_deref(),
            Some("Close Chrome first")
        );
        assert_eq!(
            short("brave", "ERROR: Failed to decrypt with DPAPI. See  https://github.com/yt-dlp/yt-dlp/issues/10927  for more info", ErrorKind::Internal).as_deref(),
            Some("Brave's cookies can't be read")
        );
        assert_eq!(
            short("firefox", "FileNotFoundError: could not find firefox cookies database in C:\\Users\\x\\AppData\\Roaming\\Mozilla\\Firefox\\Profiles", ErrorKind::Internal).as_deref(),
            Some("Firefox has no cookies")
        );
        assert_eq!(
            short("firefox", "ERROR: [youtube] abc: Sign in to confirm your age. Use --cookies-from-browser or --cookies for the authentication.", ErrorKind::LoginRequired).as_deref(),
            Some("Firefox isn't signed in to youtube.com")
        );
        // A dropped connection is not the browser's fault: shown as the error it is.
        assert_eq!(
            short(
                "firefox",
                "ERROR: Unable to download webpage: timed out",
                ErrorKind::Network
            ),
            None
        );
    }
}
