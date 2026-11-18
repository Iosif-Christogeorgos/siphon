//! Which site a link belongs to, and which cookie domains are that site's.
//!
//! Siphon knows nothing about how any site logs in; yt-dlp does. All it
//! needs is to give each download the cookies of its own site and no other.
//! A site is the domain anyone can register (`youtube.com` for
//! `music.youtube.com`, `bbc.co.uk` for `www.bbc.co.uk`), from the public
//! suffix list, plus the few sites known by more than one name.

use tauri::Url;

/// Other names of a site: links to the left belong to the site on the right.
const ALIASES: &[(&str, &str)] = &[
    ("youtu.be", "youtube.com"),
    ("youtube-nocookie.com", "youtube.com"),
    ("twitter.com", "x.com"),
    ("fb.watch", "facebook.com"),
    ("fb.com", "facebook.com"),
    ("redd.it", "reddit.com"),
    ("b23.tv", "bilibili.com"),
    ("nico.ms", "nicovideo.jp"),
];

/// Where to sign in, for sites whose sign-in page is known. Any other site
/// opens on the video's own page, which has its own way to sign in.
const SIGN_IN_PAGES: &[(&str, &str)] = &[
    (
        "youtube.com",
        "https://accounts.google.com/ServiceLogin?service=youtube&continue=https%3A%2F%2Fwww.youtube.com%2F",
    ),
    ("instagram.com", "https://www.instagram.com/accounts/login/"),
    ("x.com", "https://x.com/i/flow/login"),
    ("facebook.com", "https://www.facebook.com/login/"),
    ("tiktok.com", "https://www.tiktok.com/login"),
    ("twitch.tv", "https://www.twitch.tv/login"),
    ("reddit.com", "https://www.reddit.com/login/"),
    ("vimeo.com", "https://vimeo.com/log_in"),
];

/// The cookie that appears once a user is signed in, for sites where it is
/// known: the sign-in window then finishes by itself. Elsewhere the user
/// presses Done.
const SESSION_COOKIES: &[(&str, &str)] = &[
    ("youtube.com", "SAPISID"),
    ("instagram.com", "sessionid"),
    ("x.com", "auth_token"),
    ("facebook.com", "c_user"),
    ("tiktok.com", "sessionid"),
    ("twitch.tv", "auth-token"),
    ("reddit.com", "reddit_session"),
];

/// Google's answer when it won't let an embedded browser sign in ("This
/// browser or app may not be secure"). Siphon doesn't try to get around it.
pub fn is_google_refusal(url: &Url) -> bool {
    url.host_str() == Some("accounts.google.com")
        && (url.path().contains("/rejected") || url.as_str().contains("disallowed_useragent"))
}

/// The site a link belongs to: `youtube.com` for `https://youtu.be/abc`.
pub fn site_of(url: &str) -> Option<String> {
    let url = Url::parse(url.trim()).ok()?;
    let host = url
        .host_str()?
        .trim_end_matches('.')
        .trim_matches(['[', ']'])
        .to_ascii_lowercase();
    // An address or a single name (a test server) is its own site.
    let site = if host.parse::<std::net::IpAddr>().is_ok() || !host.contains('.') {
        host
    } else {
        psl::domain_str(&host).unwrap_or(&host).to_string()
    };
    Some(
        ALIASES
            .iter()
            .find(|(alias, _)| *alias == site)
            .map_or(site, |(_, name)| name.to_string()),
    )
}

/// Every domain whose cookies `site` needs: its own, and its other names.
pub fn cookie_domains(site: &str) -> Vec<String> {
    std::iter::once(site.to_string())
        .chain(
            ALIASES
                .iter()
                .filter(|(_, name)| *name == site)
                // Short links only redirect; their cookies are never read.
                .filter(|(alias, _)| matches!(*alias, "twitter.com" | "fb.com"))
                .map(|(alias, _)| alias.to_string()),
        )
        .collect()
}

/// The page the sign-in window opens on.
pub fn sign_in_page(site: &str, video_url: &str) -> String {
    SIGN_IN_PAGES
        .iter()
        .find(|(name, _)| *name == site)
        .map_or_else(|| video_url.to_string(), |(_, page)| page.to_string())
}

pub fn session_cookie(site: &str) -> Option<&'static str> {
    SESSION_COOKIES
        .iter()
        .find(|(name, _)| *name == site)
        .map(|(_, cookie)| *cookie)
}

/// Google accounts get flagged for heavy downloading: the first sign-in says so.
pub fn is_google(site: &str) -> bool {
    site == "youtube.com"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_belong_to_the_site_people_sign_in_to() {
        let site = |url: &str| site_of(url).unwrap();
        assert_eq!(site("https://www.youtube.com/watch?v=abc"), "youtube.com");
        assert_eq!(site("https://music.youtube.com/watch?v=abc"), "youtube.com");
        assert_eq!(site("https://youtu.be/abc"), "youtube.com");
        assert_eq!(site("https://m.youtube.com/shorts/abc"), "youtube.com");
        assert_eq!(site("https://twitter.com/user/status/1"), "x.com");
        assert_eq!(site("https://x.com/user/status/1"), "x.com");
        assert_eq!(site("https://www.instagram.com/p/abc/"), "instagram.com");
        assert_eq!(site("https://www.bbc.co.uk/iplayer/episode/abc"), "bbc.co.uk");
        assert_eq!(site("https://WWW.Vimeo.com./123"), "vimeo.com");
        // Different people's sites on a shared host stay apart.
        assert_eq!(site("https://alice.github.io/video"), "alice.github.io");
        // A local test server is its own site.
        assert_eq!(site("http://127.0.0.1:8123/clip.mp4"), "127.0.0.1");
        assert_eq!(site("http://localhost:8123/clip.mp4"), "localhost");
        assert_eq!(site_of("not a link"), None);
    }

    #[test]
    fn a_site_gets_its_own_cookies_and_its_other_names() {
        assert_eq!(cookie_domains("youtube.com"), ["youtube.com"]);
        assert_eq!(cookie_domains("x.com"), ["x.com", "twitter.com"]);
        assert_eq!(cookie_domains("vimeo.com"), ["vimeo.com"]);
    }

    #[test]
    fn known_sites_open_on_their_sign_in_page_and_others_on_the_video() {
        assert!(sign_in_page("youtube.com", "https://youtu.be/x").starts_with("https://accounts.google.com/"));
        assert_eq!(
            sign_in_page("example.org", "https://example.org/v/1"),
            "https://example.org/v/1"
        );
    }

    #[test]
    fn recognises_google_turning_the_window_away() {
        let url = |text: &str| Url::parse(text).unwrap();
        assert!(is_google_refusal(&url(
            "https://accounts.google.com/v3/signin/rejected?continue=x"
        )));
        assert!(is_google_refusal(&url(
            "https://accounts.google.com/signin/oauth/error?authError=disallowed_useragent"
        )));
        assert!(!is_google_refusal(&url(
            "https://accounts.google.com/v3/signin/identifier"
        )));
        assert!(!is_google_refusal(&url("https://www.youtube.com/rejected")));
    }
}
