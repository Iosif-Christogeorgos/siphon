//! Turns yt-dlp's stderr into a plain-language [`AppError`].

use crate::error::{AppError, ErrorKind, Retry};
use crate::sessions::cookies::redact;

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
        // Not "updating fixes this": the check is usually about the connection,
        // and the newest yt-dlp gets it too. (A download still tries a newer
        // yt-dlp first, in case there is one.)
        "The site wants to check you're not a bot. This usually happens after a lot of downloading in a short time, and passes on its own within a few hours. Switching yt-dlp to Nightly in Settings sometimes gets through sooner.",
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
    // What an account can't fix comes before what it can: these messages also
    // say "sign in" or mention cookies, and signing in would only waste the
    // user's time.
    (
        // YouTube: "Private video. Sign in if you've been granted access to this video."
        &["private video", "this video is private", "video is private"],
        ErrorKind::Unavailable,
        Retry::No,
        "This video is private. Only the people its owner shared it with can watch it.",
    ),
    (
        &[
            "requires payment",
            "rent this video",
            "buy this video",
            "purchase this video",
            "premium members",
            "music premium",
            "youtube premium",
        ],
        ErrorKind::Unavailable,
        Retry::No,
        "This video has to be bought, rented or watched with a paid membership, so it can't be downloaded.",
    ),
    (
        &["video-password", "protected by a password", "password-protected", "password protected"],
        ErrorKind::Unavailable,
        Retry::No,
        "This video is protected by a password, which Siphon can't enter.",
    ),
    (
        // Everything a site's own account unlocks: age gates, members-only and
        // logged-in-only posts. yt-dlp ends each of these with a hint to pass
        // cookies or credentials, which is the surest sign.
        &[
            "use --cookies",
            "--cookies-from-browser",
            "to provide account credentials",
            "for the authentication",
            "sign in to confirm your age",
            "confirm your age",
            "age-restricted",
            "age restricted",
            "inappropriate for some users",
            "only available for registered users",
            "login required",
            "log in for access",
            "requires authentication",
            "members-only",
            "members only",
            "join this channel",
            "channel's members",
            "subscriber-only",
            "subscribers only",
            "http error 401",
        ],
        ErrorKind::LoginRequired,
        Retry::No,
        "This video is only available to signed-in viewers of the site.",
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
            // Siphon's own proxy (see `throttle`) reports a site it couldn't reach this way.
            "couldn't reach the site",
            "tunnel connection failed",
            "unable to connect to proxy",
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

/// True when ffmpeg, fetching a trimmed part, lost its input before the end
/// (a connection that stopped sending) yet finished as if nothing happened.
pub fn section_cut_short(log: &str) -> bool {
    let log = log.to_lowercase();
    log.contains("error during demuxing") || log.contains(": partial file")
}

/// Classifies a failed run. `log` is everything yt-dlp wrote to stderr.
pub fn classify(log: &str) -> AppError {
    let haystack = log.to_lowercase();
    for (needles, kind, retry, message) in RULES {
        if needles.iter().any(|needle| haystack.contains(needle)) {
            return AppError::new(*kind, *message).retrying(*retry).with_details(redact(log));
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
    AppError::internal(redact(&message)).with_details(redact(log))
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
        // It says what helps, and doesn't promise that an update does.
        assert!(err.message.contains("passes on its own"));
        assert!(!err.message.to_lowercase().contains("updating"));
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
    fn notices_a_trimmed_part_cut_short() {
        // What ffmpeg wrote when its connection stalled (2026-11-17, local test server).
        let stalled = "[in#0/mov,mp4,m4a,3gp,3g2,mj2 @ 0001] stream 1, offset 0x6dbe4: partial file
            [in#0/mov,mp4,m4a,3gp,3g2,mj2 @ 0001] Error during demuxing: I/O error
            [out#0/mp4 @ 0002] video:144KiB audio:7KiB";
        assert!(section_cut_short(stalled));
        // HLS keep-alive warnings are harmless, and so is a clean finish.
        assert!(!section_cut_short(
            "[hls @ 0001] keepalive request failed for 'https://x/1.ts'"
        ));
        assert!(!section_cut_short("[download] Download completed"));
    }

    /// Real yt-dlp messages, from several sites: sign-in is offered only where
    /// an account can actually unlock the video.
    #[test]
    fn offers_sign_in_only_where_an_account_helps() {
        let kind = |log: &str| classify(log).kind;
        let fixable = [
            "ERROR: [youtube] abc: Sign in to confirm your age. This video may be inappropriate for some users. Use --cookies-from-browser or --cookies for the authentication. See  https://github.com/yt-dlp/yt-dlp/wiki/FAQ#how-do-i-pass-cookies-to-yt-dlp  for how to manually pass cookies.",
            "ERROR: [youtube] abc: Join this channel to get access to members-only content like this video, and other exclusive perks.",
            "ERROR: [youtube] abc: This video is available to this channel's members on level: Supporter (or any higher level). Join this YouTube channel from your computer or Android app.",
            "ERROR: [Instagram] C1a2B3: Requested content is not available, rate-limit reached or login required. Use --cookies, --cookies-from-browser, --username and --password, --netrc-cmd, or --netrc (instagram) to provide account credentials. See  https://github.com/yt-dlp/yt-dlp/wiki/FAQ#how-do-i-pass-cookies-to-yt-dlp  for how to manually pass cookies",
            "ERROR: [Instagram] C1a2B3: Instagram sent an empty media response. Check if this post is accessible in your browser without being logged-in. If it is not, then use --cookies-from-browser or --cookies for the authentication.",
            "ERROR: [twitter] 1789: NSFW tweet requires authentication. Use --cookies, --cookies-from-browser, --username and --password, --netrc-cmd, or --netrc (twitter) to provide account credentials.",
            "ERROR: [TikTok] 7301: This post may not be comfortable for some audiences. Log in for access. Use --cookies, --cookies-from-browser, --username and --password, --netrc-cmd, or --netrc (tiktok) to provide account credentials.",
            "ERROR: [facebook] 1234: This video is only available for registered users. Use --cookies-from-browser or --cookies for the authentication.",
            "ERROR: [generic] Unable to download webpage: HTTP Error 401: Unauthorized (caused by <HTTPError 401: Unauthorized>)",
        ];
        for log in fixable {
            assert_eq!(kind(log), ErrorKind::LoginRequired, "{log}");
        }

        let not_fixable = [
            // Private: the owner decides, and the message says "sign in" and "cookies" too.
            ("ERROR: [youtube] abc: Private video. Sign in if you've been granted access to this video. Use --cookies-from-browser or --cookies for the authentication.", ErrorKind::Unavailable),
            ("ERROR: [youtube] abc: This video requires payment to watch.", ErrorKind::Unavailable),
            ("ERROR: [youtube] abc: This video is only available for Music Premium members", ErrorKind::Unavailable),
            ("ERROR: [vimeo] 76979871: This video is protected by a password, use the --video-password option", ErrorKind::Unavailable),
            ("ERROR: [youtube] abc: Video unavailable. This video has been removed by the uploader", ErrorKind::Unavailable),
            ("ERROR: [youtube] abc: Video unavailable. The uploader has not made this video available in your country", ErrorKind::Unavailable),
            ("ERROR: [DRM] The requested site is known to use DRM protection. It will NOT be supported.", ErrorKind::Drm),
            // An account is what gets flagged here; it stays the bot check.
            ("ERROR: [youtube] abc: Sign in to confirm you’re not a bot. Use --cookies-from-browser or --cookies for the authentication.", ErrorKind::Outdated),
        ];
        for (log, expected) in not_fixable {
            assert_eq!(kind(log), expected, "{log}");
        }
    }

    #[test]
    fn unknown_errors_keep_the_original_line() {
        let err = classify("WARNING: x\nERROR: Something odd happened");
        assert_eq!(err.kind, ErrorKind::Internal);
        assert_eq!(err.message, "Something odd happened");
    }
}
