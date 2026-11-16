//! Turns yt-dlp's stderr into a plain-language [`AppError`].

use crate::error::{AppError, ErrorKind, Retry};

/// Substring → (kind, whether to try again, friendly message). Checked in
/// order against the lower-cased log, so more specific phrases come first.
const RULES: &[(&[&str], ErrorKind, Retry, &str)] = &[
    (
        &[
            "drm protected",
            "drm-protected",
            "known to use drm",
            "[drm]",
            "widevine",
            "fairplay",
        ],
        ErrorKind::Drm,
        Retry::No,
        "This video is protected by DRM, so it can't be downloaded. Siphon doesn't bypass copy protection.",
    ),
    (
        // Asking again straight away only makes a site more suspicious.
        &["not a bot", "confirm you’re not", "confirm you're not"],
        ErrorKind::Outdated,
        Retry::No,
        "The site asked for a bot check. Updating the download engine usually fixes this.",
    ),
    (
        // The formats were looked up a while ago and the site has since
        // changed what it offers: a fresh lookup usually settles it.
        &["requested format is not available"],
        ErrorKind::Outdated,
        Retry::Soon,
        "The site changed something and the download engine couldn't read it. An update usually fixes this.",
    ),
    (
        &[
            "no video formats found",
            "nsig extraction failed",
            "signature extraction failed",
            "unable to extract",
        ],
        ErrorKind::Outdated,
        Retry::No,
        "The site changed something and the download engine couldn't read it. An update usually fixes this.",
    ),
    (
        &["http error 429", "too many requests", "rate-limited", "rate limited"],
        ErrorKind::Outdated,
        Retry::Later,
        "The site is limiting requests right now. Wait a little and try again.",
    ),
    (
        // Often an outdated yt-dlp, but sites also refuse a request now and
        // then for no lasting reason, so this one is tried again as well.
        &["http error 403"],
        ErrorKind::Outdated,
        Retry::Soon,
        "The site refused the download. Updating the download engine usually fixes this.",
    ),
    (
        &["unsupported url", "no suitable extractor", "is not a valid url"],
        ErrorKind::Unsupported,
        Retry::No,
        "Siphon doesn't recognise this link. Make sure it points to a page with a video.",
    ),
    (
        &[
            "sign in",
            "log in",
            "login required",
            "members-only",
            "members only",
            "private video",
            "age-restricted",
            "confirm your age",
            "cookies",
        ],
        ErrorKind::LoginRequired,
        Retry::No,
        "This video needs an account to watch (private, members-only or age-restricted).",
    ),
    (
        &[
            "not available in your country",
            "geo-restricted",
            "geo restricted",
            "blocked it in your country",
            "not available from your location",
        ],
        ErrorKind::Unavailable,
        Retry::No,
        "This video isn't available in your region.",
    ),
    (
        &[
            "video unavailable",
            "has been removed",
            "no longer available",
            "does not exist",
            "http error 404",
            "http error 410",
            "account has been terminated",
            "is not currently live",
            "premieres in",
            "this live event will begin",
        ],
        ErrorKind::Unavailable,
        Retry::No,
        "This video isn't available. It may have been removed, made private or not started yet.",
    ),
    (
        &["no space left", "not enough space", "errno 28"],
        ErrorKind::DiskFull,
        Retry::No,
        "There isn't enough free disk space to finish this download.",
    ),
    (
        // yt-dlp hands section downloads to ffmpeg, which reports a refused
        // or dropped connection only as an exit code.
        &["ffmpeg exited with code"],
        ErrorKind::Network,
        Retry::Soon,
        "The site dropped the connection while sending the video. Try again; it usually works the second time.",
    ),
    (
        &["http error 500", "http error 502", "http error 503", "http error 504"],
        ErrorKind::Network,
        Retry::Soon,
        "The site is having trouble right now. Try again in a moment.",
    ),
    (
        &[
            "getaddrinfo failed",
            "name or service not known",
            "name resolution",
            "timed out",
            "connection reset",
            "connection refused",
            "connection aborted",
            "connection broken",
            "remote end closed",
            "forcibly closed",
            "actively refused",
            "incompleteread",
            "network is unreachable",
            "unable to download webpage",
            "transporterror",
            "ssl:",
            "winerror 10053",
            "winerror 10054",
            "winerror 10060",
            "winerror 10061",
            "winerror 11001",
        ],
        ErrorKind::Network,
        Retry::Soon,
        "Couldn't reach the site. Check your internet connection and try again.",
    ),
];

/// Classifies a failed run. `log` is everything yt-dlp wrote to stderr.
pub fn classify(log: &str) -> AppError {
    let haystack = log.to_lowercase();
    for (needles, kind, retry, message) in RULES {
        if needles.iter().any(|needle| haystack.contains(needle)) {
            return AppError::new(*kind, *message).retrying(*retry).with_details(log);
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
    fn says_which_failures_are_worth_another_try() {
        let retry = |log: &str| classify(log).retry;
        // A refused request and a dropped connection pass; a bot check does not.
        assert_eq!(
            retry("ERROR: unable to download video data: HTTP Error 403: Forbidden"),
            Retry::Soon
        );
        assert_eq!(
            retry(
                "ERROR: ConnectionResetError(10054, 'An existing connection was forcibly closed by the remote host')"
            ),
            Retry::Soon
        );
        assert_eq!(
            retry("ERROR: [youtube] abc: Requested format is not available"),
            Retry::Soon
        );
        assert_eq!(retry("ERROR: HTTP Error 429: Too Many Requests"), Retry::Later);
        assert_eq!(
            retry("ERROR: [youtube] abc: Sign in to confirm you’re not a bot."),
            Retry::No
        );
        assert_eq!(retry("ERROR: [youtube] abc: Video unavailable"), Retry::No);
        assert_eq!(retry("ERROR: Something odd happened"), Retry::No);
    }

    #[test]
    fn unknown_errors_keep_the_original_line() {
        let err = classify("WARNING: x\nERROR: Something odd happened");
        assert_eq!(err.kind, ErrorKind::Internal);
        assert_eq!(err.message, "Something odd happened");
    }
}
