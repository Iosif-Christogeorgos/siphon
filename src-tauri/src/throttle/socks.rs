//! Reaching a site through a SOCKS proxy (SOCKS4, 4a and 5).
//!
//! Only the opening handshake differs from a direct connection: once the
//! proxy has said yes, the stream carries the site's bytes as they are. The
//! names follow yt-dlp's and curl's: `socks5` and `socks4` look the site's
//! name up on this PC, `socks5h` and `socks4a` let the proxy look it up, so
//! the name never reaches this PC's DNS.

use std::io;
use std::net::{IpAddr, Ipv4Addr};

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// Which SOCKS, and who looks the site's name up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Version {
    /// SOCKS4; with `remote_dns`, SOCKS4a.
    Four { remote_dns: bool },
    /// SOCKS5; with `remote_dns`, what `socks5h://` means.
    Five { remote_dns: bool },
}

/// Asks the proxy on the other end of `stream` for a connection to
/// `host:port`. `credentials` is `user:password` (SOCKS4 sends only the user).
pub async fn handshake(
    stream: &mut (impl AsyncRead + AsyncWrite + Unpin),
    version: Version,
    host: &str,
    port: u16,
    credentials: Option<&str>,
) -> io::Result<()> {
    let (user, password) = match credentials {
        Some(credentials) => credentials.split_once(':').unwrap_or((credentials, "")),
        None => ("", ""),
    };
    match version {
        Version::Four { remote_dns } => four(stream, remote_dns, host, port, user).await,
        Version::Five { remote_dns } => five(stream, remote_dns, host, port, user, password).await,
    }
}

/// Where the proxy should connect: an address, or a name for it to look up.
enum Target {
    Ip(IpAddr),
    Name(String),
}

async fn target(host: &str, port: u16, remote_dns: bool, ipv4_only: bool) -> io::Result<Target> {
    let host = host.trim_start_matches('[').trim_end_matches(']');
    if let Ok(ip) = host.parse::<IpAddr>() {
        return Ok(Target::Ip(ip));
    }
    if remote_dns {
        return Ok(Target::Name(host.to_string()));
    }
    let addresses = tokio::net::lookup_host((host, port)).await?;
    addresses
        .map(|address| address.ip())
        .find(|ip| !ipv4_only || ip.is_ipv4())
        .map(Target::Ip)
        .ok_or_else(|| io::Error::other(format!("{host} has no address a SOCKS4 proxy can reach")))
}

async fn four(
    stream: &mut (impl AsyncRead + AsyncWrite + Unpin),
    remote_dns: bool,
    host: &str,
    port: u16,
    user: &str,
) -> io::Result<()> {
    // CONNECT, the port, the address, the user ID; SOCKS4a puts 0.0.0.1
    // as the address and the name after the user ID.
    let mut ask = vec![4, 1];
    ask.extend_from_slice(&port.to_be_bytes());
    let name = match target(host, port, remote_dns, true).await? {
        Target::Ip(IpAddr::V4(ip)) => {
            ask.extend_from_slice(&ip.octets());
            None
        }
        Target::Ip(IpAddr::V6(_)) => return Err(io::Error::other("SOCKS4 can't reach an IPv6 address")),
        Target::Name(name) => {
            ask.extend_from_slice(&Ipv4Addr::new(0, 0, 0, 1).octets());
            Some(name)
        }
    };
    ask.extend_from_slice(user.as_bytes());
    ask.push(0);
    if let Some(name) = name {
        ask.extend_from_slice(name.as_bytes());
        ask.push(0);
    }
    stream.write_all(&ask).await?;

    let mut answer = [0; 8];
    stream.read_exact(&mut answer).await?;
    match answer[1] {
        0x5a => Ok(()),
        0x5b => Err(refused("refused the connection")),
        0x5c | 0x5d => Err(refused("couldn't confirm the user ID")),
        other => Err(refused(&format!("answered with code {other:#04x}"))),
    }
}

async fn five(
    stream: &mut (impl AsyncRead + AsyncWrite + Unpin),
    remote_dns: bool,
    host: &str,
    port: u16,
    user: &str,
    password: &str,
) -> io::Result<()> {
    // Which ways of signing in Siphon can do: none, or a user name and password.
    let signs_in = !user.is_empty();
    let offer: &[u8] = if signs_in { &[5, 2, 0, 2] } else { &[5, 1, 0] };
    stream.write_all(offer).await?;
    let mut chosen = [0; 2];
    stream.read_exact(&mut chosen).await?;
    if chosen[0] != 5 {
        return Err(io::Error::other("the proxy doesn't speak SOCKS5"));
    }
    match chosen[1] {
        0 => {}
        2 if signs_in => sign_in(stream, user, password).await?,
        _ => return Err(refused("wants a sign-in Siphon doesn't have")),
    }

    let mut ask = vec![5, 1, 0];
    match target(host, port, remote_dns, false).await? {
        Target::Ip(IpAddr::V4(ip)) => {
            ask.push(1);
            ask.extend_from_slice(&ip.octets());
        }
        Target::Ip(IpAddr::V6(ip)) => {
            ask.push(4);
            ask.extend_from_slice(&ip.octets());
        }
        Target::Name(name) => {
            let length = u8::try_from(name.len()).map_err(|_| io::Error::other("the site's name is too long"))?;
            ask.push(3);
            ask.push(length);
            ask.extend_from_slice(name.as_bytes());
        }
    }
    ask.extend_from_slice(&port.to_be_bytes());
    stream.write_all(&ask).await?;

    let mut answer = [0; 4];
    stream.read_exact(&mut answer).await?;
    if answer[1] != 0 {
        return Err(refused(match answer[1] {
            1 => "failed",
            2 => "isn't allowed to connect there",
            3 => "can't reach that network",
            4 => "can't reach the site",
            5 => "was refused by the site",
            6 => "timed out",
            7 => "doesn't support connecting",
            8 => "doesn't support that kind of address",
            _ => "refused",
        }));
    }
    // The address the proxy connected from, which nothing here needs.
    let address = match answer[3] {
        1 => 4,
        4 => 16,
        3 => usize::from(stream.read_u8().await?),
        _ => return Err(io::Error::other("the proxy answered with an unknown kind of address")),
    };
    let mut rest = vec![0; address + 2];
    stream.read_exact(&mut rest).await?;
    Ok(())
}

/// Signs in with a user name and password (RFC 1929).
async fn sign_in(stream: &mut (impl AsyncRead + AsyncWrite + Unpin), user: &str, password: &str) -> io::Result<()> {
    let too_long = || io::Error::other("the proxy's user name or password is too long for SOCKS5");
    let mut ask = vec![1, u8::try_from(user.len()).map_err(|_| too_long())?];
    ask.extend_from_slice(user.as_bytes());
    ask.push(u8::try_from(password.len()).map_err(|_| too_long())?);
    ask.extend_from_slice(password.as_bytes());
    stream.write_all(&ask).await?;
    let mut answer = [0; 2];
    stream.read_exact(&mut answer).await?;
    if answer[1] == 0 {
        Ok(())
    } else {
        Err(refused("didn't accept the user name and password"))
    }
}

fn refused(what: &str) -> io::Error {
    io::Error::new(io::ErrorKind::ConnectionRefused, format!("the SOCKS proxy {what}"))
}

#[cfg(test)]
mod tests {
    use tokio::io::duplex;

    use super::*;

    /// Runs the handshake against a scripted proxy: what the proxy sends
    /// back, and returns what Siphon sent it.
    async fn exchange(
        version: Version,
        host: &str,
        credentials: Option<&str>,
        replies: &[u8],
    ) -> (io::Result<()>, Vec<u8>) {
        let (mut ours, mut theirs) = duplex(1024);
        let replies = replies.to_vec();
        let proxy = tokio::spawn(async move {
            theirs.write_all(&replies).await.unwrap();
            let mut sent = Vec::new();
            let _ = theirs.read_to_end(&mut sent).await;
            sent
        });
        let result = handshake(&mut ours, version, host, 443, credentials).await;
        drop(ours);
        (result, proxy.await.unwrap())
    }

    const GRANTED_V4: &[u8] = &[5, 0, 0, 1, 10, 0, 0, 1, 0x1f, 0x90];

    #[tokio::test]
    async fn socks5h_lets_the_proxy_look_the_name_up() {
        let replies = [&[5, 0][..], GRANTED_V4].concat();
        let (result, sent) = exchange(Version::Five { remote_dns: true }, "www.youtube.com", None, &replies).await;
        result.unwrap();
        let mut expected = vec![5, 1, 0, 5, 1, 0, 3, 15];
        expected.extend_from_slice(b"www.youtube.com");
        expected.extend_from_slice(&[1, 187]);
        assert_eq!(sent, expected);
    }

    #[tokio::test]
    async fn socks5_sends_an_address_it_already_has() {
        let replies = [&[5, 0][..], GRANTED_V4].concat();
        let (result, sent) = exchange(Version::Five { remote_dns: false }, "[::1]", None, &replies).await;
        result.unwrap();
        let mut expected = vec![5, 1, 0, 5, 1, 0, 4];
        expected.extend_from_slice(&"::1".parse::<std::net::Ipv6Addr>().unwrap().octets());
        expected.extend_from_slice(&[1, 187]);
        assert_eq!(sent, expected);
    }

    #[tokio::test]
    async fn socks5_signs_in_with_a_user_name_and_password() {
        // Sign-in chosen, sign-in accepted, connection granted (bound to a name).
        let replies = [&[5, 2, 1, 0, 5, 0, 0, 3, 4][..], b"host", &[0, 80]].concat();
        let (result, sent) = exchange(
            Version::Five { remote_dns: true },
            "example.com",
            Some("me:secret"),
            &replies,
        )
        .await;
        result.unwrap();
        assert_eq!(&sent[..4], &[5, 2, 0, 2]);
        assert_eq!(&sent[4..15], b"\x01\x02me\x06secret");
    }

    #[tokio::test]
    async fn socks5_says_why_it_was_refused() {
        let replies = [5, 0, 5, 4, 0, 1, 0, 0, 0, 0, 0, 0];
        let (result, _) = exchange(Version::Five { remote_dns: true }, "example.com", None, &replies).await;
        assert_eq!(result.unwrap_err().to_string(), "the SOCKS proxy can't reach the site");

        let (result, _) = exchange(
            Version::Five { remote_dns: true },
            "example.com",
            Some("me:bad"),
            &[5, 2, 1, 1],
        )
        .await;
        assert_eq!(
            result.unwrap_err().to_string(),
            "the SOCKS proxy didn't accept the user name and password"
        );

        // It wants a password and Siphon has none.
        let (result, _) = exchange(Version::Five { remote_dns: true }, "example.com", None, &[5, 0xff]).await;
        assert_eq!(
            result.unwrap_err().to_string(),
            "the SOCKS proxy wants a sign-in Siphon doesn't have"
        );
    }

    #[tokio::test]
    async fn socks4a_sends_the_name_and_socks4_an_address() {
        let granted = [0, 0x5a, 0, 0, 0, 0, 0, 0];
        let (result, sent) = exchange(
            Version::Four { remote_dns: true },
            "example.com",
            Some("me:x"),
            &granted,
        )
        .await;
        result.unwrap();
        let mut expected = vec![4, 1, 1, 187, 0, 0, 0, 1];
        expected.extend_from_slice(b"me\0example.com\0");
        assert_eq!(sent, expected);

        let (result, sent) = exchange(Version::Four { remote_dns: false }, "10.1.2.3", None, &granted).await;
        result.unwrap();
        assert_eq!(sent, [4, 1, 1, 187, 10, 1, 2, 3, 0]);

        let (result, _) = exchange(
            Version::Four { remote_dns: false },
            "10.1.2.3",
            None,
            &[0, 0x5b, 0, 0, 0, 0, 0, 0],
        )
        .await;
        assert_eq!(
            result.unwrap_err().to_string(),
            "the SOCKS proxy refused the connection"
        );
    }
}
