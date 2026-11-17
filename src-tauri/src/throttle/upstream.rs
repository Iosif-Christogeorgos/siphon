//! The proxy this PC already uses, if any, so downloads still go through it.
//!
//! Before Siphon put its own proxy in between, yt-dlp found the system proxy
//! itself, the way Python does: from the `HTTP(S)_PROXY` variables if any
//! are set, otherwise from Windows' proxy settings. The same rules apply
//! here. Like yt-dlp, a setup script (PAC) is not followed, and only HTTP
//! proxies can be chained to: a SOCKS proxy is skipped and the site is
//! reached directly.

/// A proxy to reach a site through.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Upstream {
    pub host: String,
    pub port: u16,
    /// `user:password`, sent as basic proxy authentication.
    pub credentials: Option<String>,
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

/// `http://user:pass@proxy:8080/` or `proxy:8080`. Anything but an HTTP proxy is `None`.
fn parse_url(value: &str) -> Option<Upstream> {
    let value = value.trim();
    let rest = match value.split_once("://") {
        Some((scheme, rest)) if scheme.eq_ignore_ascii_case("http") => rest,
        Some(_) => return None,
        None => value,
    };
    let authority = rest.split('/').next()?;
    let (credentials, address) = match authority.rsplit_once('@') {
        Some((credentials, address)) => (Some(credentials.to_string()), address),
        None => (None, authority),
    };
    let (host, port) = split_host_port(address, 80)?;
    Some(Upstream {
        host,
        port,
        credentials,
    })
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
/// `http=proxy:8080;https=proxy:8443;socks=…` per kind of link.
fn windows_server(server: &str, https: bool) -> Option<Upstream> {
    if !server.contains('=') {
        return parse_url(server);
    }
    let wanted = if https { "https" } else { "http" };
    server.split(';').find_map(|entry| {
        let (kind, address) = entry.split_once('=')?;
        kind.trim()
            .eq_ignore_ascii_case(wanted)
            .then(|| parse_url(address))
            .flatten()
    })
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

    fn upstream(host: &str, port: u16) -> Option<Upstream> {
        Some(Upstream {
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
                host: "10.0.0.1".into(),
                port: 8080,
                credentials: Some("me:secret".into()),
            })
        );
        // Only HTTP proxies can be chained to.
        assert_eq!(parse_url("socks5://proxy.lan:1080"), None);
        assert_eq!(split_host_port("[::1]:8080", 80), Some(("::1".into(), 8080)));
        assert_eq!(split_host_port("example.com", 443), Some(("example.com".into(), 443)));
        assert_eq!(split_host_port("example.com:x", 443), None);
    }

    #[test]
    fn reads_windows_proxy_settings() {
        assert_eq!(windows_server("proxy:8080", true), upstream("proxy", 8080));
        let split = "http=web:80;https=secure:8443;socks=sock:1080";
        assert_eq!(windows_server(split, false), upstream("web", 80));
        assert_eq!(windows_server(split, true), upstream("secure", 8443));
        // No entry for this kind of link: straight to the site.
        assert_eq!(windows_server("socks=sock:1080", true), None);

        let exceptions = "*.internal.example;intranet*;<local>";
        assert!(bypassed_by_windows(exceptions, "files.internal.example"));
        assert!(bypassed_by_windows(exceptions, "intranet-2"));
        assert!(bypassed_by_windows(exceptions, "printer"));
        assert!(!bypassed_by_windows(exceptions, "www.youtube.com"));
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
