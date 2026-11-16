//! Turns yt-dlp's stderr into a plain-language [`AppError`].

use crate::error::{AppError, ErrorKind};

/// Substring → (kind, friendly message). Checked in order against the
/// lower-cased log, so more specific phrases come first.
const RULES: &[(&[&str], ErrorKind, &str)] = &[
    (
        &["drm protected", "drm-protected", "known to use drm", "[drm]", "widevine", "fairplay"],
        ErrorKind::Drm,
        "This video is protected by DRM, so it can't be downloaded. Siphon doesn't bypass copy protection.",
    ),
    (
        &["not a bot", "confirm you’re not", "confirm you're not"],
        ErrorKind::Outdated,
        "The site asked for a bot check. Updating the download engine usually fixes this.",
    ),
    (
        &["requested format is not available", "no video formats found", "nsig extraction failed", "signature extraction failed", "unable to extract"],
        ErrorKind::Outdated,
        "The site changed something and the download engine couldn't read it. An update usually fixes this.",
    ),
    (
        &["http error 429", "too many requests", "rate-limited", "rate limited"],
        ErrorKind::Outdated,
        "The site is limiting requests right now. Wait a little and try again.",
    ),
    (
        &["http error 403"],
        ErrorKind::Outdated,
        "The site refused the download. Updating the download engine usually fixes this.",
    ),
    (
        &["unsupported url", "no suitable extractor", "is not a valid url"],
        ErrorKind::Unsupported,
        "Siphon doesn't recognise this link. Make sure it points to a page with a video.",
    ),
    (
        &["sign in", "log in", "login required", "members-only", "members only", "private video", "age-restricted", "confirm your age", "cookies"],
        ErrorKind::LoginRequired,
        "This video needs an account to watch (private, members-only or age-restricted).",
    ),
    (
        &["not available in your country", "geo-restricted", "geo restricted", "blocked it in your country", "not available from your location"],
        ErrorKind::Unavailable,
        "This video isn't available in your region.",
    ),
    (
        &["video unavailable", "has been removed", "no longer available", "does not exist", "http error 404", "http error 410", "account has been terminated", "is not currently live", "premieres in", "this live event will begin"],
        ErrorKind::Unavailable,
        "This video isn't available. It may have been removed, made private or not started yet.",
    ),
    (
        &["no space left", "not enough space", "errno 28"],
        ErrorKind::DiskFull,
        "There isn't enough free disk space to finish this download.",
    ),
    (
        &["getaddrinfo failed", "name or service not known", "name resolution", "timed out", "connection reset", "connection refused", "connection aborted", "network is unreachable", "unable to download webpage", "transporterror", "ssl:", "winerror 10060", "winerror 11001"],
        ErrorKind::Network,
        "Couldn't reach the site. Check your internet connection and try again.",
    ),
];

/// Classifies a failed run. `log` is everything yt-dlp wrote to stderr.
pub fn classify(log: &str) -> AppError {
    let haystack = log.to_lowercase();
    for (needles, kind, message) in RULES {
        if needles.iter().any(|needle| haystack.contains(needle)) {
            return AppError::new(*kind, *message).with_details(log);
        }
    }

    // Unknown: surface yt-dlp's own last error line, minus its prefix noise.
    let message = log
        .lines()
        .rev()
        .find_map(|line| line.trim().strip_prefix("ERROR:"))
        .map(|line| line.trim().to_string())
        .filter(|line| !line.is_empty() && line.len() <= 200)
        .unwrap_or_else(|| "Something went wrong while talking to the site.".to_string());
    AppError::internal(message).with_details(log)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_drm() {
        let err = classify("ERROR: [DRM] The requested site is known to use DRM protection.");
        assert_eq!(err.kind, ErrorKind::Drm);
    }

    #[test]
    fn bot_check_counts_as_outdated() {
        let err = classify("ERROR: [youtube] abc: Sign in to confirm you’re not a bot.");
        assert_eq!(err.kind, ErrorKind::Outdated);
    }

    #[test]
    fn unknown_errors_keep_the_original_line() {
        let err = classify("WARNING: x\nERROR: Something odd happened");
        assert_eq!(err.kind, ErrorKind::Internal);
        assert_eq!(err.message, "Something odd happened");
    }
}
