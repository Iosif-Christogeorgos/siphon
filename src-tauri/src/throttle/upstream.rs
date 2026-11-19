//! The proxy this PC already uses, if any, so downloads still go through it.
//!
//! Before Siphon put its own proxy in between, yt-dlp found the system proxy
//! itself, the way Python does: from the `HTTP(S)_PROXY` and `ALL_PROXY`
//! variables if any are set, otherwise from Windows' proxy settings. The same
//! rules apply here, for HTTP and SOCKS proxies alike. Like yt-dlp, a setup
//! script (PAC) is not followed.

use super::socks;

/// A proxy to reach a site through.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Upstream {
    pub kind: Kind,
    pub host: String,
    pub port: u16,
    /// `user:password`: basic proxy authentication for HTTP, the SOCKS sign-in for SOCKS.
    pub credentials: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Http,
    Socks(socks::Version),
}

impl Upstream {
    /// As a link (`socks5h://me:secret@proxy:1080`), for clients that take
    /// their proxy that way. The user and password are percent-encoded again.
    pub fn url(&self) -> String {
        use socks::Version::{Five, Four};

        let scheme = match self.kind {
            Kind::Http => "http",
            Kind::Socks(Five { remote_dns: false }) => "socks5",
            Kind::Socks(Five { remote_dns: true }) => "socks5h",
            Kind::Socks(Four { remote_dns: false }) => "socks4",
            Kind::Socks(Four { remote_dns: true }) => "socks4a",
        };
        let credentials = match self.credentials.as_deref() {
            Some(credentials) => match credentials.split_once(':') {
                Some((user, password)) => format!("{}:{}@", encode(user), encode(password)),
                None => format!("{}@", encode(credentials)),
            },
            None => String::new(),
        };
        let host = if self.host.contains(':') {
            format!("[{}]", self.host)
        } else {
            self.host.clone()
        };
        format!("{scheme}://{credentials}{host}:{}", self.port)
    }
}

/// Percent-encodes everything but letters, digits and `-._~`, for a link's user or password.
fn encode(text: &str) -> String {
    text.bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => (byte as char).to_string(),
            _ => format!("%{byte:02X}"),
        })
        .collect()
}

/// Makes an HTTP client go the way downloads go: through the proxy
/// [`for_host`] picks for each request, or straight to the site. For the
/// update checks. reqwest's own reading of Windows' settings is switched off,
/// because it takes `ProxyServer` as a single address: the per-kind form
/// (`http=…;https=…`, `socks=…`) went around the proxy or failed.
pub fn follow_system_proxy(client: reqwest::ClientBuilder) -> reqwest::ClientBuilder {
    client.no_proxy().proxy(reqwest::Proxy::custom(|url| {
        for_host(url.host_str()?, url.scheme() == "https").map(|upstream| upstream.url())
    }))
}

/// Where a request for `host` should go: through which proxy, or `None` for direct.
pub fn for_host(host: &str, https: bool) -> Option<Upstream> {
    if is_loopback(host) {
        return None;
    }
    let env = |name: &str| {
        std::env::var(name)
            .or_else(|_| std::env::var(name.to_lowercase()))
            .ok()
            .filter(|value| !value.trim().is_empty())
    };
    let variable = if https { "HTTPS_PROXY" } else { "HTTP_PROXY" };
    let from_env = env(variable).or_else(|| env("ALL_PROXY"));
    let any_env = ["HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY"]
        .iter()
        .any(|name| env(name).is_some());
    if any_env {
        // Set variables win, and Windows' settings aren't read at all (as in Python).
        let bypassed = env("NO_PROXY").is_some_and(|list| no_proxy_matches(&list, host));
        return from_env.filter(|_| !bypassed).and_then(|value| parse_url(&value));
    }
    let (server, exceptions) = windows_settings()?;
    if bypassed_by_windows(&exceptions, host) {
        return None;
    }
    windows_server(&server, https)
}

fn is_loopback(host: &str) -> bool {
    let host = host.trim_start_matches('[').trim_end_matches(']');
    host.eq_ignore_ascii_case("localhost") || host.parse::<std::net::IpAddr>().is_ok_and(|ip| ip.is_loopback())
}

/// `http://user:pass@proxy:8080/`, `socks5h://proxy:1080` or `proxy:8080` (HTTP).
/// A kind of proxy Siphon can't go through (`https://`, say) is `None`.
fn parse_url(value: &str) -> Option<Upstream> {
    use socks::Version::{Five, Four};

    let value = value.trim();
    let (scheme, rest) = value.split_once("://").unwrap_or(("http", value));
    let (kind, default_port) = match scheme.to_ascii_lowercase().as_str() {
        "http" => (Kind::Http, 80),
        "socks5" => (Kind::Socks(Five { remote_dns: false }), 1080),
        "socks5h" => (Kind::Socks(Five { remote_dns: true }), 1080),
        // Windows' and Python's default SOCKS is SOCKS4.
        "socks4" | "socks" => (Kind::Socks(Four { remote_dns: false }), 1080),
        "socks4a" => (Kind::Socks(Four { remote_dns: true }), 1080),
        _ => return None,
    };
    let authority = rest.split('/').next()?;
    let (credentials, address) = match authority.rsplit_once('@') {
        Some((credentials, address)) => (Some(decode_credentials(credentials)), address),
        None => (None, authority),
    };
    let (host, port) = split_host_port(address, default_port)?;
    Some(Upstream {
        kind,
        host,
        port,
        credentials,
    })
}

/// `me%40work:p%3Ass` → `me@work:p:ss`: a link's user and password are
/// percent-encoded, as Python decodes them.
fn decode_credentials(credentials: &str) -> String {
    let (user, password) = credentials.split_once(':').unwrap_or((credentials, ""));
    let decode = |text: &str| {
        let bytes = text.as_bytes();
        let mut out = Vec::with_capacity(bytes.len());
        let mut i = 0;
        while i < bytes.len() {
            let hex = (bytes[i] == b'%')
                .then(|| bytes.get(i + 1..i + 3))
                .flatten()
                .and_then(|hex| u8::from_str_radix(std::str::from_utf8(hex).ok()?, 16).ok());
            match hex {
                Some(byte) => {
                    out.push(byte);
                    i += 3;
                }
                None => {
                    out.push(bytes[i]);
                    i += 1;
                }
            }
        }
        String::from_utf8_lossy(&out).into_owned()
    };
    if credentials.contains(':') {
        format!("{}:{}", decode(user), decode(password))
    } else {
        decode(user)
    }
}

/// `example.com:443` → ("example.com", 443); `[::1]:8080` → ("::1", 8080).
pub fn split_host_port(address: &str, default_port: u16) -> Option<(String, u16)> {
    let (host, port) = if let Some(rest) = address.strip_prefix('[') {
        let (host, after) = rest.split_once(']')?;
        (host, after.strip_prefix(':'))
    } else {
        match address.rsplit_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (address, None),
        }
    };
    let port = match port {
        Some(port) => port.parse().ok()?,
        None => default_port,
    };
    (!host.is_empty()).then(|| (host.to_string(), port))
}

/// `NO_PROXY`: a comma-separated list of names, each also covering its subdomains, or `*`.
fn no_proxy_matches(list: &str, host: &str) -> bool {
    let host = host.to_ascii_lowercase();
    list.split(',')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .any(|entry| {
            let entry = entry
                .trim_start_matches("*.")
                .trim_start_matches('.')
                .to_ascii_lowercase();
            entry == "*" || host == entry || host.ends_with(&format!(".{entry}"))
        })
}

/// Windows' "ProxyServer": `proxy:8080` for everything, or
/// `http=proxy:8080;https=proxy:8443;socks=…` per kind of link. As in
/// Python, a `socks` entry (SOCKS4 unless it says otherwise) is used for
/// links whose kind has no entry of its own.
fn windows_server(server: &str, https: bool) -> Option<Upstream> {
    if !server.contains('=') {
        return parse_url(server);
    }
    let entry = |wanted: &str| {
        server.split(';').find_map(|entry| {
            let (kind, address) = entry.split_once('=')?;
            kind.trim().eq_ignore_ascii_case(wanted).then(|| address.trim())
        })
    };
    if let Some(address) = entry(if https { "https" } else { "http" }) {
        return parse_url(address);
    }
    let socks = entry("socks")?;
    if socks.contains("://") {
        parse_url(socks)
    } else {
        parse_url(&format!("socks4://{socks}"))
    }
}

/// Windows' "ProxyOverride": `;`-separated names with `*` wildcards; `<local>`
/// means any name without a dot.
fn bypassed_by_windows(exceptions: &str, host: &str) -> bool {
    let host = host.to_ascii_lowercase();
    exceptions
        .split(';')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .any(|entry| {
            if entry.eq_ignore_ascii_case("<local>") {
                !host.contains('.')
            } else {
                wildcard(&entry.to_ascii_lowercase(), &host)
            }
        })
}

/// Matches `text` against `pattern`, where `*` stands for any run of characters.
fn wildcard(pattern: &str, text: &str) -> bool {
    match pattern.split_once('*') {
        None => pattern == text,
        Some((prefix, rest)) => {
            let Some(text) = text.strip_prefix(prefix) else {
                return false;
            };
            (0..=text.len()).any(|skip| text.is_char_boundary(skip) && wildcard(rest, &text[skip..]))
        }
    }
}

/// The proxy server and exceptions from Windows' settings, when a proxy is switched on.
fn windows_settings() -> Option<(String, String)> {
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;

    let key = RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey(r"Software\Microsoft\Windows\CurrentVersion\Internet Settings")
        .ok()?;
    let enabled: u32 = key.get_value("ProxyEnable").ok()?;
    if enabled == 0 {
        return None;
    }
    let server: String = key.get_value("ProxyServer").ok()?;
    let exceptions: String = key.get_value("ProxyOverride").unwrap_or_default();
    Some((server, exceptions))
}

#[cfg(test)]
mod tests {
    use super::*;

    use socks::Version::{Five, Four};

    fn upstream(host: &str, port: u16) -> Option<Upstream> {
        via(Kind::Http, host, port)
    }

    fn via(kind: Kind, host: &str, port: u16) -> Option<Upstream> {
        Some(Upstream {
            kind,
            host: host.into(),
            port,
            credentials: None,
        })
    }

    #[test]
    fn reads_proxy_addresses() {
        assert_eq!(parse_url("http://proxy.lan:3128/"), upstream("proxy.lan", 3128));
        assert_eq!(parse_url("proxy.lan:3128"), upstream("proxy.lan", 3128));
        assert_eq!(parse_url("proxy.lan"), upstream("proxy.lan", 80));
        assert_eq!(
            parse_url("http://me:secret@10.0.0.1:8080"),
            Some(Upstream {
                kind: Kind::Http,
                host: "10.0.0.1".into(),
                port: 8080,
                credentials: Some("me:secret".into()),
            })
        );
        // A proxy Siphon can't speak to (over TLS) is left out, as before.
        assert_eq!(parse_url("https://proxy.lan:443"), None);
        assert_eq!(split_host_port("[::1]:8080", 80), Some(("::1".into(), 8080)));
        assert_eq!(split_host_port("example.com", 443), Some(("example.com".into(), 443)));
        assert_eq!(split_host_port("example.com:x", 443), None);
    }

    #[test]
    fn reads_socks_proxy_addresses() {
        let socks5 = Kind::Socks(Five { remote_dns: false });
        assert_eq!(parse_url("socks5://proxy.lan:9050"), via(socks5, "proxy.lan", 9050));
        assert_eq!(
            parse_url("SOCKS5H://proxy.lan"),
            via(Kind::Socks(Five { remote_dns: true }), "proxy.lan", 1080)
        );
        let socks4 = Kind::Socks(Four { remote_dns: false });
        assert_eq!(parse_url("socks4://10.0.0.1:1080"), via(socks4, "10.0.0.1", 1080));
        assert_eq!(parse_url("socks://10.0.0.1"), via(socks4, "10.0.0.1", 1080));
        assert_eq!(
            parse_url("socks4a://proxy.lan:1080"),
            via(Kind::Socks(Four { remote_dns: true }), "proxy.lan", 1080)
        );
        // A user name and password are percent-encoded in a link.
        assert_eq!(
            parse_url("socks5h://me%40work:p%3Ass%25@127.0.0.1:1080")
                .unwrap()
                .credentials
                .as_deref(),
            Some("me@work:p:ss%")
        );
    }

    #[test]
    fn reads_windows_proxy_settings() {
        assert_eq!(windows_server("proxy:8080", true), upstream("proxy", 8080));
        let split = "http=web:80;https=secure:8443;socks=sock:1080";
        assert_eq!(windows_server(split, false), upstream("web", 80));
        assert_eq!(windows_server(split, true), upstream("secure", 8443));
        // Only a SOCKS entry: it serves every kind of link, as SOCKS4 (Windows' default).
        let socks4 = Kind::Socks(Four { remote_dns: false });
        assert_eq!(windows_server("socks=sock:1080", true), via(socks4, "sock", 1080));
        assert_eq!(
            windows_server("http=web:80;socks=sock:1080", true),
            via(socks4, "sock", 1080)
        );
        assert_eq!(
            windows_server("socks=socks5h://sock:9050", false),
            via(Kind::Socks(Five { remote_dns: true }), "sock", 9050)
        );
        // Neither an entry for this kind of link nor a SOCKS one: straight to the site.
        assert_eq!(windows_server("ftp=files:21", true), None);

        let exceptions = "*.internal.example;intranet*;<local>";
        assert!(bypassed_by_windows(exceptions, "files.internal.example"));
        assert!(bypassed_by_windows(exceptions, "intranet-2"));
        assert!(bypassed_by_windows(exceptions, "printer"));
        assert!(!bypassed_by_windows(exceptions, "www.youtube.com"));
    }

    #[test]
    fn writes_proxies_as_links() {
        assert_eq!(windows_server("proxy:8080", true).unwrap().url(), "http://proxy:8080");
        assert_eq!(
            windows_server("socks=sock:1080", true).unwrap().url(),
            "socks4://sock:1080"
        );
        for link in [
            "socks4a://proxy.lan:1080",
            "socks5://10.0.0.1:9050",
            "socks5h://proxy.lan:1080",
            "http://[::1]:8080",
            "http://me:secret@proxy.lan:3128",
        ] {
            assert_eq!(parse_url(link).unwrap().url(), link);
        }
        // Decoded when read, encoded again when written: the same user and password.
        let tricky = "socks5h://me%40work:p%3Ass%25@127.0.0.1:1080";
        assert_eq!(parse_url(tricky).unwrap().url(), tricky);
        assert_eq!(
            parse_url("socks4://me@sock:1080").unwrap().url(),
            "socks4://me@sock:1080"
        );
    }

    #[test]
    fn honours_no_proxy() {
        assert!(no_proxy_matches("example.com, .lan", "cdn.example.com"));
        assert!(no_proxy_matches("example.com", "example.com"));
        assert!(no_proxy_matches(".lan", "nas.lan"));
        assert!(no_proxy_matches("*", "anything"));
        assert!(!no_proxy_matches("example.com", "notexample.com"));
    }

    #[test]
    fn never_proxies_this_computer() {
        assert!(is_loopback("127.0.0.1"));
        assert!(is_loopback("localhost"));
        assert!(is_loopback("[::1]"));
        assert!(!is_loopback("youtube.com"));
    }
}
