//! Cookies in the Netscape `cookies.txt` format yt-dlp reads and writes,
//! and which of them belong to which site.

use std::collections::HashMap;

/// One cookie, as a line of `cookies.txt` holds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cookie {
    /// Lower case, without a leading dot: `youtube.com`.
    pub domain: String,
    /// Sent to subdomains too (written as a leading dot and `TRUE`).
    pub include_subdomains: bool,
    pub path: String,
    pub secure: bool,
    pub http_only: bool,
    /// Unix seconds; 0 for a cookie that lasts as long as the browser session.
    pub expires: i64,
    pub name: String,
    pub value: String,
}

impl Cookie {
    /// The same cookie as far as a browser is concerned: a newer one replaces it.
    fn key(&self) -> (&str, &str, &str) {
        (&self.domain, &self.path, &self.name)
    }

    fn expired(&self, now: i64) -> bool {
        self.expires != 0 && self.expires <= now
    }
}

/// What a `cookies.txt` held.
#[derive(Debug, Default)]
pub struct Parsed {
    pub cookies: Vec<Cookie>,
    /// Lines that were neither a comment nor a cookie.
    pub bad_lines: usize,
}

/// Reads Netscape-format cookies. Lenient the way browsers' exporters need:
/// `#HttpOnly_` lines are cookies, a missing value is empty, an expiry may
/// have decimals, and the subdomain flag follows the leading dot.
pub fn parse(text: &str) -> Parsed {
    let mut parsed = Parsed::default();
    for line in text.lines() {
        let line = line.trim_end_matches('\r');
        let (line, http_only) = match line.strip_prefix("#HttpOnly_") {
            Some(rest) => (rest, true),
            None => (line, false),
        };
        if line.trim().is_empty() || line.starts_with('#') {
            continue;
        }
        match parse_line(line, http_only) {
            Some(cookie) => parsed.cookies.push(cookie),
            None => parsed.bad_lines += 1,
        }
    }
    parsed
}

fn parse_line(line: &str, http_only: bool) -> Option<Cookie> {
    let fields: Vec<&str> = line.split('\t').collect();
    let (domain, flag, path, secure, expires, name, value) = match fields.as_slice() {
        [domain, flag, path, secure, expires, name, value] => (*domain, *flag, *path, *secure, *expires, *name, *value),
        [domain, flag, path, secure, expires, name] => (*domain, *flag, *path, *secure, *expires, *name, ""),
        _ => return None,
    };
    let boolean = |text: &str| match text.trim().to_ascii_uppercase().as_str() {
        "TRUE" => Some(true),
        "FALSE" => Some(false),
        _ => None,
    };
    let flag = boolean(flag)?;
    let secure = boolean(secure)?;
    let expires = match expires.trim() {
        "" => 0,
        text => text.parse::<f64>().ok().filter(|n| n.is_finite())?.max(0.0) as i64,
    };
    let host = domain.trim().to_ascii_lowercase();
    let bare = host.trim_start_matches('.');
    if bare.is_empty() || name.is_empty() || bare.contains(char::is_whitespace) {
        return None;
    }
    Some(Cookie {
        include_subdomains: flag || host.starts_with('.'),
        domain: bare.to_string(),
        path: if path.is_empty() { "/".into() } else { path.into() },
        secure,
        http_only,
        expires,
        name: name.into(),
        value: value.into(),
    })
}

/// Writes cookies as a `cookies.txt` yt-dlp accepts (it insists on the header).
pub fn write(cookies: &[Cookie]) -> String {
    let mut text = String::from(
        "# Netscape HTTP Cookie File\n# Written by Siphon. Holds one site's sign-in; deleted after use.\n\n",
    );
    for cookie in cookies {
        let bool_text = |value: bool| if value { "TRUE" } else { "FALSE" };
        text.push_str(&format!(
            "{}{}{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
            if cookie.http_only { "#HttpOnly_" } else { "" },
            // The dot and the flag must agree, or Python's cookie loader refuses the line.
            if cookie.include_subdomains { "." } else { "" },
            cookie.domain,
            bool_text(cookie.include_subdomains),
            cookie.path,
            bool_text(cookie.secure),
            cookie.expires,
            cookie.name,
            cookie.value,
        ));
    }
    text
}

/// Whether a cookie for `domain` is one of `site_domains`' own: the domain
/// itself or a subdomain of it, never a look-alike (`notyoutube.com`,
/// `youtube.com.example.net`).
pub fn belongs_to(domain: &str, site_domains: &[String]) -> bool {
    let domain = domain.trim_start_matches('.').to_ascii_lowercase();
    site_domains.iter().any(|site| {
        domain == *site
            || domain
                .strip_suffix(site.as_str())
                .is_some_and(|rest| rest.ends_with('.'))
    })
}

/// Only the cookies of `site_domains`, and none that have expired.
pub fn scoped(cookies: impl IntoIterator<Item = Cookie>, site_domains: &[String], now: i64) -> Vec<Cookie> {
    cookies
        .into_iter()
        .filter(|cookie| belongs_to(&cookie.domain, site_domains) && !cookie.expired(now))
        .collect()
}

/// What changed between the cookies Siphon handed yt-dlp and the ones it
/// wrote back: the site set or renewed some, and dropped others.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Changes {
    pub set: Vec<Cookie>,
    pub removed: Vec<Cookie>,
}

impl Changes {
    pub fn is_empty(&self) -> bool {
        self.set.is_empty() && self.removed.is_empty()
    }
}

pub fn changes(before: &[Cookie], after: &[Cookie]) -> Changes {
    let old: HashMap<_, _> = before.iter().map(|cookie| (cookie.key(), cookie)).collect();
    let new: HashMap<_, _> = after.iter().map(|cookie| (cookie.key(), cookie)).collect();
    Changes {
        set: after
            .iter()
            .filter(|cookie| old.get(&cookie.key()) != Some(cookie))
            .cloned()
            .collect(),
        removed: before
            .iter()
            .filter(|cookie| !new.contains_key(&cookie.key()))
            .cloned()
            .collect(),
    }
}

/// Applies changes one download made to the stored jar. Only what that
/// download changed is touched, so two downloads of one site, each renewing
/// a different cookie, both keep their renewals.
pub fn apply(jar: &mut Vec<Cookie>, changes: &Changes, now: i64) {
    let touched: Vec<_> = changes.set.iter().chain(&changes.removed).map(Cookie::key).collect();
    jar.retain(|cookie| !touched.contains(&cookie.key()));
    jar.extend(changes.set.iter().cloned());
    jar.retain(|cookie| !cookie.expired(now));
}

/// Hides cookie values in text that may be logged or shown: request and
/// response headers, and `cookies.txt` lines.
pub fn redact(text: &str) -> String {
    text.lines()
        .map(|line| {
            let lower = line.to_ascii_lowercase();
            if let Some(at) = ["set-cookie:", "cookie:"]
                .iter()
                .find_map(|header| lower.find(header).map(|at| at + header.len()))
            {
                format!("{} [hidden]", &line[..at])
            } else if line.split('\t').count() >= 7 {
                // A cookies.txt line: keep everything up to the name.
                let fields: Vec<&str> = line.split('\t').collect();
                format!("{}\t[hidden]", fields[..6].join("\t"))
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_800_000_000;

    fn cookie(domain: &str, name: &str, value: &str) -> Cookie {
        Cookie {
            domain: domain.into(),
            include_subdomains: true,
            path: "/".into(),
            secure: true,
            http_only: false,
            expires: NOW + 3600,
            name: name.into(),
            value: value.into(),
        }
    }

    fn domains(list: &[&str]) -> Vec<String> {
        list.iter().map(|domain| domain.to_string()).collect()
    }

    #[test]
    fn writes_what_ytdlp_reads() {
        let mut session = cookie("youtube.com", "SID", "fake-sid");
        session.expires = 0;
        session.http_only = true;
        let mut host_only = cookie("www.youtube.com", "PREF", "f6=40000000");
        host_only.include_subdomains = false;
        host_only.secure = false;
        let text = write(&[session.clone(), host_only.clone()]);

        assert!(text.starts_with("# Netscape HTTP Cookie File\n"));
        assert!(text.contains("#HttpOnly_.youtube.com\tTRUE\t/\tTRUE\t0\tSID\tfake-sid\n"));
        assert!(text.contains(&format!(
            "www.youtube.com\tFALSE\t/\tFALSE\t{}\tPREF\tf6=40000000\n",
            NOW + 3600
        )));
        // And reads back exactly.
        let parsed = parse(&text);
        assert_eq!(parsed.bad_lines, 0);
        assert_eq!(parsed.cookies, vec![session, host_only]);
    }

    #[test]
    fn reads_what_browser_extensions_export() {
        let text = "# Netscape HTTP Cookie File\r\n\
            # This is a generated file! Do not edit.\r\n\
            \r\n\
            .instagram.com\tTRUE\t/\tTRUE\t1800000000.52\tsessionid\tfake%3Avalue\r\n\
            #HttpOnly_.x.com\tTRUE\t/\tTRUE\t1800000000\tauth_token\tfake\r\n\
            example.com\tFALSE\t/\tFALSE\t\tempty_expiry\tv\r\n\
            .example.com\tTRUE\t/\tFALSE\t0\tno_value\r\n\
            this line is not a cookie\r\n";
        let parsed = parse(text);
        assert_eq!(parsed.bad_lines, 1);
        let names: Vec<_> = parsed
            .cookies
            .iter()
            .map(|c| (c.domain.as_str(), c.name.as_str(), c.http_only))
            .collect();
        assert_eq!(
            names,
            [
                ("instagram.com", "sessionid", false),
                ("x.com", "auth_token", true),
                ("example.com", "empty_expiry", false),
                ("example.com", "no_value", false),
            ]
        );
        assert_eq!(parsed.cookies[0].expires, 1_800_000_000);
        assert_eq!(parsed.cookies[2].expires, 0);
        assert_eq!(parsed.cookies[3].value, "");
    }

    #[test]
    fn a_file_that_is_not_cookies_gives_nothing() {
        let parsed = parse("{\"cookies\": []}\nname=value; other=1\n");
        assert!(parsed.cookies.is_empty());
        assert_eq!(parsed.bad_lines, 2);
    }

    #[test]
    fn no_cookie_ever_reaches_another_site() {
        let youtube = domains(&["youtube.com"]);
        assert!(belongs_to("youtube.com", &youtube));
        assert!(belongs_to(".youtube.com", &youtube));
        assert!(belongs_to("music.youtube.com", &youtube));
        assert!(!belongs_to("notyoutube.com", &youtube));
        assert!(!belongs_to("youtube.com.example.net", &youtube));
        assert!(!belongs_to("google.com", &youtube));
        assert!(!belongs_to("com", &youtube));

        let jar = vec![
            cookie("youtube.com", "SID", "a"),
            cookie("google.com", "SID", "b"),
            cookie("accounts.google.com", "LSID", "c"),
            cookie("instagram.com", "sessionid", "d"),
        ];
        let kept = scoped(jar, &youtube, NOW);
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].value, "a");

        // A site with two names gets both, and nothing else.
        let x = domains(&["x.com", "twitter.com"]);
        let jar = vec![
            cookie("x.com", "a", "1"),
            cookie("twitter.com", "b", "2"),
            cookie("t.co", "c", "3"),
        ];
        assert_eq!(scoped(jar, &x, NOW).len(), 2);
    }

    #[test]
    fn expired_cookies_are_left_out() {
        let mut old = cookie("youtube.com", "old", "x");
        old.expires = NOW - 1;
        let mut session = cookie("youtube.com", "session", "x");
        session.expires = 0;
        let kept = scoped(vec![old, session], &domains(&["youtube.com"]), NOW);
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].name, "session");
    }

    #[test]
    fn renewed_cookies_are_written_back_without_undoing_another_download() {
        let jar_at_start = vec![
            cookie("youtube.com", "SID", "1"),
            cookie("youtube.com", "PSIDTS", "1"),
            cookie("youtube.com", "VISITOR", "1"),
        ];

        // Two downloads ran at once: A renewed PSIDTS, B renewed VISITOR.
        let a_after = vec![
            cookie("youtube.com", "SID", "1"),
            cookie("youtube.com", "PSIDTS", "2"),
            cookie("youtube.com", "VISITOR", "1"),
        ];
        let b_after = vec![
            cookie("youtube.com", "SID", "1"),
            cookie("youtube.com", "PSIDTS", "1"),
            cookie("youtube.com", "VISITOR", "2"),
        ];

        let mut jar = jar_at_start.clone();
        let a = changes(&jar_at_start, &a_after);
        assert_eq!(a.set, vec![cookie("youtube.com", "PSIDTS", "2")]);
        assert!(a.removed.is_empty());
        apply(&mut jar, &a, NOW);
        apply(&mut jar, &changes(&jar_at_start, &b_after), NOW);

        let value = |jar: &[Cookie], name: &str| jar.iter().find(|c| c.name == name).map(|c| c.value.clone());
        assert_eq!(value(&jar, "PSIDTS").as_deref(), Some("2"));
        assert_eq!(value(&jar, "VISITOR").as_deref(), Some("2"));
        assert_eq!(value(&jar, "SID").as_deref(), Some("1"));

        // A cookie the site deleted goes from the jar too.
        let dropped = changes(&jar_at_start, &jar_at_start[1..]);
        apply(&mut jar, &dropped, NOW);
        assert_eq!(value(&jar, "SID"), None);

        // Nothing changed: nothing to write.
        assert!(changes(&jar_at_start, &jar_at_start).is_empty());
    }

    #[test]
    fn values_never_show_in_logs() {
        let log = "[debug] Request: Cookie: SID=fake-secret; HSID=other\n\
            < set-cookie: VISITOR=fake-secret; Path=/\n\
            .youtube.com\tTRUE\t/\tTRUE\t0\tSID\tfake-secret\n\
            ERROR: [youtube] abc: Sign in to confirm your age.";
        let clean = redact(log);
        assert!(!clean.contains("fake-secret"), "{clean}");
        assert!(clean.contains("Cookie: [hidden]"));
        assert!(clean.contains(".youtube.com\tTRUE\t/\tTRUE\t0\tSID\t[hidden]"));
        assert!(clean.contains("Sign in to confirm your age."));
    }
}
