//! Turns yt-dlp's stderr into a plain-language [`AppError`].

use crate::error::{AppError, Chance, ErrorKind, Retry};
use crate::sessions::cookies::redact;

/// One way a lookup or download says it failed.
struct Rule {
    /// Any of these, in the lower-cased log.
    needles: &'static [&'static str],
    kind: ErrorKind,
    retry: Retry,
    /// Whether signing in to the site could help. Siphon always tries without
    /// an account first; this only decides what it offers once that failed.
    chance: Chance,
    message: &'static str,
}

/// Checked in order, so more specific phrases come first. In particular,
/// what an account can't fix (or only might) comes before the login wall:
/// those messages often mention signing in or cookies too.
const RULES: &[Rule] = &[
    Rule {
        needles: &["drm protected", "drm-protected", "known to use drm", "[drm]", "widevine", "fairplay"],
        kind: ErrorKind::Drm,
        retry: Retry::No,
        chance: Chance::Never,
        message: "This video is protected by DRM, so it can't be downloaded. Siphon doesn't bypass copy protection.",
    },
    Rule {
        // Asking again straight away only makes a site more suspicious.
        needles: &["not a bot", "confirm you’re not", "confirm you're not"],
        kind: ErrorKind::Outdated,
        retry: Retry::No,
        // Signing in often gets past it, and is also what gets accounts flagged.
        chance: Chance::Risky,
        // Not "updating fixes this": the check is usually about the connection,
        // and the newest yt-dlp gets it too. (A download still tries a newer
        // yt-dlp first, in case there is one.)
        message: "The site wants to check you're not a bot. This usually happens after a lot of downloading in a short time, and passes on its own within a few hours. Switching yt-dlp to Nightly in Settings sometimes gets through sooner.",
    },
    Rule {
        // The formats were looked up a while ago and the site has since
        // changed what it offers: a fresh lookup usually settles it.
        needles: &["requested format is not available"],
        kind: ErrorKind::Outdated,
        retry: Retry::Soon,
        chance: Chance::Never,
        message: "The site changed something and the download engine couldn't read it. An update usually fixes this.",
    },
    Rule {
        needles: &[
            "no video formats found",
            "nsig extraction failed",
            "signature extraction failed",
            "unable to extract",
        ],
        kind: ErrorKind::Outdated,
        retry: Retry::No,
        chance: Chance::Never,
        message: "The site changed something and the download engine couldn't read it. An update usually fixes this.",
    },
    Rule {
        needles: &["http error 429", "too many requests", "rate-limited", "rate limited"],
        kind: ErrorKind::Outdated,
        retry: Retry::Later,
        chance: Chance::Never,
        message: "The site is limiting requests right now. Wait a little and try again.",
    },
    Rule {
        // Often an outdated yt-dlp, but sites also refuse a request now and
        // then for no lasting reason, so this one is tried again as well.
        needles: &["http error 403"],
        kind: ErrorKind::Outdated,
        retry: Retry::Soon,
        chance: Chance::Never,
        message: "The site refused the download. Updating the download engine usually fixes this.",
    },
    Rule {
        needles: &["unsupported url", "no suitable extractor", "is not a valid url"],
        kind: ErrorKind::Unsupported,
        retry: Retry::No,
        chance: Chance::Never,
        message: "Siphon doesn't recognise this link. Make sure it points to a page with a video.",
    },
    Rule {
        // YouTube: "Private video. Sign in if you've been granted access to this video."
        needles: &["private video", "this video is private", "video is private"],
        kind: ErrorKind::Unavailable,
        retry: Retry::No,
        chance: Chance::Maybe,
        message: "This video is private. Only the people its owner shared it with can watch it.",
    },
    Rule {
        // Bought or rented videos are protected by DRM even for their buyer.
        needles: &["requires payment", "rent this video", "buy this video", "purchase this video"],
        kind: ErrorKind::Unavailable,
        retry: Retry::No,
        chance: Chance::Never,
        message: "This video has to be bought or rented, so it can't be downloaded.",
    },
    Rule {
        needles: &["premium members", "music premium", "youtube premium"],
        kind: ErrorKind::Unavailable,
        retry: Retry::No,
        chance: Chance::Maybe,
        message: "This video is only for paid members of the site.",
    },
    Rule {
        needles: &["video-password", "protected by a password", "password-protected", "password protected"],
        kind: ErrorKind::Unavailable,
        retry: Retry::No,
        chance: Chance::Never,
        message: "This video is protected by a password, which Siphon can't enter.",
    },
    Rule {
        // Only for the accounts that have joined or paid. Patreon says just
        // "You do not have access to this post", whether signed in or not.
        needles: &[
            "members-only",
            "members only",
            "join this channel",
            "channel's members",
            "subscriber-only",
            "subscribers only",
            "you do not have access to this post",
        ],
        kind: ErrorKind::LoginRequired,
        retry: Retry::No,
        chance: Chance::Maybe,
        message: "This video is only for the channel's or creator's members.",
    },
    Rule {
        // A login wall: age gates, posts for signed-in viewers. yt-dlp ends
        // these with a hint to pass cookies or credentials, the surest sign.
        needles: &[
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
            "http error 401",
        ],
        kind: ErrorKind::LoginRequired,
        retry: Retry::No,
        chance: Chance::Needed,
        message: "This video is only available to signed-in viewers of the site.",
    },
    Rule {
        needles: &[
            "not available in your country",
            "geo-restricted",
            "geo restricted",
            "blocked it in your country",
            "not available from your location",
        ],
        kind: ErrorKind::Unavailable,
        retry: Retry::No,
        chance: Chance::Never,
        message: "This video isn't available in your region.",
    },
    Rule {
        needles: &[
            "video unavailable",
            "this video is unavailable",
            "this video is not available",
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
        kind: ErrorKind::Unavailable,
        retry: Retry::No,
        chance: Chance::Never,
        message: "This video isn't available. It may have been removed, made private or not started yet.",
    },
    Rule {
        // The site answered, with a refusal no rule above knows (400, 418, …).
        // yt-dlp says "Unable to download webpage" for these too, which the
        // connection rule below would otherwise take for a lost connection.
        needles: &["http error 4"],
        kind: ErrorKind::Unavailable,
        retry: Retry::No,
        chance: Chance::Unknown,
        message: "The site turned the request down without saying why. Details has its exact answer.",
    },
    Rule {
        needles: &["no space left", "not enough space", "errno 28"],
        kind: ErrorKind::DiskFull,
        retry: Retry::No,
        chance: Chance::Never,
        message: "There isn't enough free disk space to finish this download.",
    },
    Rule {
        // yt-dlp hands section downloads to ffmpeg, which reports a refused
        // or dropped connection only as an exit code.
        needles: &["ffmpeg exited with code"],
        kind: ErrorKind::Network,
        retry: Retry::Soon,
        chance: Chance::Never,
        message: "The site dropped the connection while sending the video. Try again; it usually works the second time.",
    },
    Rule {
        needles: &["http error 500", "http error 502", "http error 503", "http error 504"],
        kind: ErrorKind::Network,
        retry: Retry::Soon,
        chance: Chance::Never,
        message: "The site is having trouble right now. Try again in a moment.",
    },
    Rule {
        needles: &[
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
        kind: ErrorKind::Network,
        retry: Retry::Soon,
        chance: Chance::Never,
        message: "Couldn't reach the site. Check your internet connection and try again.",
    },
];

/// True when ffmpeg, fetching a trimmed part, lost its input before the end
/// (a connection that stopped sending) yet finished as if nothing happened.
pub fn section_cut_short(log: &str) -> bool {
    let log = log.to_lowercase();
    log.contains("error during demuxing") || log.contains(": partial file")
}

/// True when a lookup failed because the stream hasn't started: a scheduled
/// YouTube stream or premiere. Recording can wait for it.
pub fn is_scheduled(log: &str) -> bool {
    let log = log.to_lowercase();
    [
        "this live event will begin",
        "premieres in",
        "premiere will begin",
        "scheduled for",
    ]
    .iter()
    .any(|needle| log.contains(needle))
}

/// True when a lookup failed because a channel link's channel isn't streaming
/// now (yt-dlp's "The channel is not currently live", on Twitch, Kick,
/// YouTube's `/live` and others). Recording can wait for it to go live.
pub fn is_offline(log: &str) -> bool {
    log.to_lowercase().contains("is not currently live")
}

/// True when yt-dlp was asked for a stream from its start and the site
/// doesn't keep it as it airs: a Twitch channel that saves no VODs ("there
/// are no formats that can be downloaded from the start").
pub fn not_kept_from_start(log: &str) -> bool {
    log.contains("no formats that can be downloaded from the start")
}

/// True when a live recording that finished without an error ended because
/// its stream did: its playlist said so, and nothing failed on the way. A
/// stream that stops sending ends ffmpeg without an error too, but leaves
/// "HTTP error 404" and "Failed to reload playlist" in its log (seen
/// 2026-11-19); that one may come back, and is waited for.
/// Siphon's own progress lines are left out: recording from the start, yt-dlp
/// prints them there, and they carry the file's path, so a title or folder
/// with "error" or "connection" in it would read as a stream that dropped.
pub fn stream_over(log: &str) -> bool {
    log.lines()
        .filter(|line| !line.contains("SIPHON_"))
        .map(str::to_lowercase)
        .all(|line| {
            !["error", "failed", "timed out", "connection"]
                .iter()
                .any(|word| line.contains(word))
        })
}

/// Classifies a failed run. `log` is everything yt-dlp wrote to stderr.
pub fn classify(log: &str) -> AppError {
    let haystack = log.to_lowercase();
    for rule in RULES {
        if rule.needles.iter().any(|needle| haystack.contains(needle)) {
            let mut error = AppError::new(rule.kind, rule.message)
                .retrying(rule.retry)
                .with_details(redact(log));
            error.chance = rule.chance;
            return error;
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
    let mut error = AppError::internal(redact(&message)).with_details(redact(log));
    // An answer Siphon doesn't recognise: an account rarely helps, but might.
    error.chance = Chance::Unknown;
    error
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tells_a_stream_that_ended_from_one_that_dropped() {
        // A playlist that ended: ffmpeg says nothing at -loglevel warning.
        assert!(stream_over(""));
        // From the start, yt-dlp's own progress lines are all there is.
        assert!(stream_over(
            "1: SIPHON_DL|downloading|1024|NA|NA|140|none|mp4a.40.2|C:\r.f140.mkv"
        ));
        // … whatever the stream is called, or the folder it goes to.
        assert!(stream_over(
            "1: SIPHON_DL|downloading|1024|NA|NA|140|none|mp4a.40.2|D:\\Errors\\Connection lost - failed run.siphon-ab.rec1.f140.mkv"
        ));
        // The server went away (2026-11-19).
        let dropped = "[http @ 0000027533a89b80] HTTP error 404 Not Found
                       [in#0/hls @ 0000027533a7ea40] Failed to reload playlist 0";
        assert!(!stream_over(dropped));
    }

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

    /// Real yt-dlp messages, from several sites: how likely an account is to
    /// help each. Only a login wall is `Needed`; nothing that works without an
    /// account is ever asked to sign in, because Siphon tries without one first.
    #[test]
    fn says_how_likely_an_account_is_to_help() {
        let chance = |log: &str| classify(log).chance;
        let needed = [
            "ERROR: [youtube] abc: Sign in to confirm your age. This video may be inappropriate for some users. Use --cookies-from-browser or --cookies for the authentication. See  https://github.com/yt-dlp/yt-dlp/wiki/FAQ#how-do-i-pass-cookies-to-yt-dlp  for how to manually pass cookies.",
            "ERROR: [Instagram] C1a2B3: Requested content is not available, rate-limit reached or login required. Use --cookies, --cookies-from-browser, --username and --password, --netrc-cmd, or --netrc (instagram) to provide account credentials. See  https://github.com/yt-dlp/yt-dlp/wiki/FAQ#how-do-i-pass-cookies-to-yt-dlp  for how to manually pass cookies",
            "ERROR: [Instagram] C1a2B3: Instagram sent an empty media response. Check if this post is accessible in your browser without being logged-in. If it is not, then use --cookies-from-browser or --cookies for the authentication.",
            "ERROR: [twitter] 1789: NSFW tweet requires authentication. Use --cookies, --cookies-from-browser, --username and --password, --netrc-cmd, or --netrc (twitter) to provide account credentials.",
            "ERROR: [TikTok] 7301: This post may not be comfortable for some audiences. Log in for access. Use --cookies, --cookies-from-browser, --username and --password, --netrc-cmd, or --netrc (tiktok) to provide account credentials.",
            "ERROR: [facebook] 1234: This video is only available for registered users. Use --cookies-from-browser or --cookies for the authentication.",
            "ERROR: [generic] Unable to download webpage: HTTP Error 401: Unauthorized (caused by <HTTPError 401: Unauthorized>)",
        ];
        for log in needed {
            assert_eq!(chance(log), Chance::Needed, "{log}");
            assert_eq!(classify(log).kind, ErrorKind::LoginRequired, "{log}");
        }

        // Only if the account has access: offered as "might work", and never
        // called a run-out sign-in when the account is simply refused.
        let maybe = [
            "ERROR: [youtube] abc: Join this channel to get access to members-only content like this video, and other exclusive perks.",
            "ERROR: [youtube] abc: This video is available to this channel's members on level: Supporter (or any higher level). Join this YouTube channel from your computer or Android app.",
            "ERROR: [youtube] abc: Private video. Sign in if you've been granted access to this video. Use --cookies-from-browser or --cookies for the authentication.",
            "ERROR: [youtube] abc: This video is only available for Music Premium members",
            "ERROR: [patreon] 123456: You do not have access to this post",
        ];
        for log in maybe {
            assert_eq!(chance(log), Chance::Maybe, "{log}");
        }

        assert_eq!(
            chance("ERROR: [youtube] abc: Sign in to confirm you’re not a bot. Use --cookies-from-browser or --cookies for the authentication."),
            Chance::Risky
        );
        assert_eq!(chance("ERROR: Something odd happened"), Chance::Unknown);
        // A refusal no rule knows is the site's answer, not a lost connection (found 2026-11-18).
        let teapot =
            "ERROR: [generic] Unable to download webpage: HTTP Error 418: I'm a Teapot (caused by <HTTPError 418>)";
        assert_eq!(chance(teapot), Chance::Unknown);
        assert_eq!(classify(teapot).kind, ErrorKind::Unavailable);
        assert_eq!(
            chance("ERROR: [Instagram] CWqAgUZgCku: Video info extraction failed: HTTP Error 400: Bad Request"),
            Chance::Unknown
        );

        let never = [
            ("ERROR: [youtube] abc: This video requires payment to watch.", ErrorKind::Unavailable),
            ("ERROR: [vimeo] 76979871: This video is protected by a password, use the --video-password option", ErrorKind::Unavailable),
            ("ERROR: [youtube] abc: Video unavailable. This video has been removed by the uploader", ErrorKind::Unavailable),
            ("ERROR: [youtube] 1ARh2RJwd2w: This video is unavailable", ErrorKind::Unavailable),
            ("ERROR: [youtube] abc: Video unavailable. The uploader has not made this video available in your country", ErrorKind::Unavailable),
            ("ERROR: [DRM] The requested site is known to use DRM protection. It will NOT be supported.", ErrorKind::Drm),
            ("ERROR: unable to download video data: HTTP Error 403: Forbidden", ErrorKind::Outdated),
            ("ERROR: HTTP Error 429: Too Many Requests", ErrorKind::Outdated),
            ("ERROR: Unable to download webpage: timed out", ErrorKind::Network),
        ];
        for (log, kind) in never {
            assert_eq!(chance(log), Chance::Never, "{log}");
            assert_eq!(classify(log).kind, kind, "{log}");
        }
    }

    #[test]
    fn unknown_errors_keep_the_original_line() {
        let err = classify("WARNING: x\nERROR: Something odd happened");
        assert_eq!(err.kind, ErrorKind::Internal);
        assert_eq!(err.message, "Something odd happened");
    }
}
