//! The local proxy a download's connections go through.
//!
//! A plain HTTP proxy, listening on 127.0.0.1: `CONNECT` opens a tunnel (all
//! that HTTPS needs; the encrypted bytes pass through untouched), and a
//! request for an `http://` link is passed on as it is. What comes back from
//! the site is paced by the [`Limiter`] and counted by the [`Meter`]. What
//! goes out (requests) is not limited.

use std::io;
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinSet;

use super::limiter::{FlowId, Limiter, CHUNK};
use super::socks;
use super::upstream::{self, split_host_port, Kind, Upstream};
use super::Meter;

/// A request head larger than this is not a request we should pass on.
const MAX_HEAD: usize = 64 * 1024;
/// Reaching a site (or the system proxy) gives up after this long.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
/// Read size without a limit: larger reads cost less at high speed.
const UNLIMITED_READ: usize = 64 * 1024;
/// Once the site has finished, how long the other direction may take to wind down.
const LINGER: Duration = Duration::from_secs(5);

/// What every connection of every download shares.
pub(super) struct Shared {
    pub limiter: Arc<Limiter>,
    pub meter: Arc<Meter>,
}

/// Accepts one download's connections until the task is aborted. Aborting it
/// also ends the connections, which belong to the set it owns.
pub(super) async fn serve(listener: TcpListener, flow: FlowId, shared: Arc<Shared>) {
    let mut connections = JoinSet::new();
    loop {
        tokio::select! {
            accepted = listener.accept() => match accepted {
                Ok((client, _)) => {
                    connections.spawn(handle(client, flow, Arc::clone(&shared)));
                }
                // Out of sockets for a moment, say: don't spin.
                Err(_) => tokio::time::sleep(Duration::from_millis(50)).await,
            },
            Some(_) = connections.join_next() => {}
        }
    }
}

async fn handle(mut client: TcpStream, flow: FlowId, shared: Arc<Shared>) {
    let _ = client.set_nodelay(true);
    let Ok((head, early)) = read_head(&mut client).await else {
        return;
    };
    let Some(request) = RequestHead::parse(&head) else {
        let _ = reply_error(
            &mut client,
            "400 Bad Request",
            "Siphon's proxy couldn't read the request.",
        )
        .await;
        return;
    };

    let opened = if request.method.eq_ignore_ascii_case("CONNECT") {
        open_tunnel(&request).await
    } else {
        open_forward(&request).await
    };
    let (mut server, to_client) = match opened {
        Ok(opened) => opened,
        Err(error) => {
            let reason = format!("502 Siphon couldn't reach the site ({error})").replace(['\r', '\n'], " ");
            let _ = reply_error(&mut client, &reason, "").await;
            return;
        }
    };
    if !to_client.is_empty() && client.write_all(&to_client).await.is_err() {
        return;
    }
    // Whatever the client sent after its request head (the start of a TLS
    // handshake, a request body) goes on to the site.
    if !early.is_empty() && server.write_all(&early).await.is_err() {
        return;
    }
    relay(client, server, flow, &shared).await;
}

/// `CONNECT host:port`: a tunnel to the site, through the system proxy if there is one.
/// Returns the connection and what to tell the client.
async fn open_tunnel(request: &RequestHead<'_>) -> io::Result<(TcpStream, Vec<u8>)> {
    let (host, port) = split_host_port(request.target, 443).ok_or_else(|| invalid("a bad CONNECT address"))?;
    let server = tunnel_to(&host, port, upstream::for_host(&host, true)).await?;
    Ok((server, b"HTTP/1.1 200 Connection established\r\n\r\n".to_vec()))
}

/// Opens a tunnel to `host:port`, directly or by asking `upstream` for one.
async fn tunnel_to(host: &str, port: u16, upstream: Option<Upstream>) -> io::Result<TcpStream> {
    let Some(upstream) = upstream else {
        return connect(host, port).await;
    };
    let mut proxy = connect(&upstream.host, upstream.port).await?;
    if let Kind::Socks(version) = upstream.kind {
        let asked = socks::handshake(&mut proxy, version, host, port, upstream.credentials.as_deref());
        tokio::time::timeout(CONNECT_TIMEOUT, asked)
            .await
            .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "the SOCKS proxy didn't answer"))??;
        return Ok(proxy);
    }
    let authority = if host.contains(':') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    };
    let mut ask = format!("CONNECT {authority} HTTP/1.1\r\nHost: {authority}\r\n");
    if let Some(credentials) = &upstream.credentials {
        ask.push_str(&format!(
            "Proxy-Authorization: Basic {}\r\n",
            base64(credentials.as_bytes())
        ));
    }
    ask.push_str("\r\n");
    // A proxy that takes the connection but never answers mustn't hold it forever.
    let asked = async {
        proxy.write_all(ask.as_bytes()).await?;
        read_head(&mut proxy).await
    };
    let (answer, _) = tokio::time::timeout(CONNECT_TIMEOUT, asked)
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "the system proxy didn't answer"))??;
    let status = String::from_utf8_lossy(&answer);
    let status = status.lines().next().unwrap_or_default();
    let code = status.split_whitespace().nth(1).unwrap_or_default();
    if code.starts_with('2') {
        Ok(proxy)
    } else {
        Err(io::Error::other(format!("the system proxy answered {status}")))
    }
}

/// `GET http://host/path`: the same request, sent on to the site (or the
/// system proxy) on a connection of its own that closes after the answer.
async fn open_forward(request: &RequestHead<'_>) -> io::Result<(TcpStream, Vec<u8>)> {
    let (host, ..) = forward_target(request.target)?;
    forward(request, upstream::for_host(&host, false)).await
}

/// `http://host:port/path` → host, port and path.
fn forward_target(target: &str) -> io::Result<(String, u16, &str)> {
    let rest = target
        .get(..7)
        .filter(|scheme| scheme.eq_ignore_ascii_case("http://"))
        .map(|_| &target[7..])
        .ok_or_else(|| invalid("not an http:// link"))?;
    let (authority, path) = match rest.find('/') {
        Some(slash) => (&rest[..slash], &rest[slash..]),
        None => (rest, "/"),
    };
    let (host, port) = split_host_port(authority, 80).ok_or_else(|| invalid("a bad address"))?;
    Ok((host, port, path))
}

async fn forward(request: &RequestHead<'_>, upstream: Option<Upstream>) -> io::Result<(TcpStream, Vec<u8>)> {
    let target = request.target;
    let (host, port, path) = forward_target(target)?;
    // An HTTP proxy is sent the request; through a SOCKS proxy, the site is.
    let (server, upstream) = match upstream {
        Some(proxy) if proxy.kind == Kind::Http => (connect(&proxy.host, proxy.port).await?, Some(proxy)),
        other => (tunnel_to(&host, port, other).await?, None),
    };
    // A proxy is sent the whole link, a site only the path.
    let mut head = format!(
        "{} {} {}\r\n",
        request.method,
        if upstream.is_some() { target } else { path },
        request.version
    );
    for (name, value) in &request.headers {
        let hop_by_hop = ["connection", "proxy-connection", "keep-alive", "proxy-authorization"]
            .iter()
            .any(|hop| name.eq_ignore_ascii_case(hop));
        if !hop_by_hop {
            head.push_str(&format!("{name}: {value}\r\n"));
        }
    }
    if let Some(credentials) = upstream.as_ref().and_then(|proxy| proxy.credentials.as_ref()) {
        head.push_str(&format!(
            "Proxy-Authorization: Basic {}\r\n",
            base64(credentials.as_bytes())
        ));
    }
    // One request per connection: the next one may be for another site.
    head.push_str("Connection: close\r\n\r\n");
    let mut server = server;
    server.write_all(head.as_bytes()).await?;
    Ok((server, Vec::new()))
}

async fn connect(host: &str, port: u16) -> io::Result<TcpStream> {
    let stream = tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect((host, port)))
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "timed out"))??;
    let _ = stream.set_nodelay(true);
    Ok(stream)
}

/// Passes bytes both ways until the site is done; what comes back is paced and counted.
async fn relay(client: TcpStream, server: TcpStream, flow: FlowId, shared: &Shared) {
    let (mut client_read, mut client_write) = client.into_split();
    let (server_read, mut server_write) = server.into_split();
    let up = async {
        let _ = tokio::io::copy(&mut client_read, &mut server_write).await;
        let _ = server_write.shutdown().await;
    };
    let down = async {
        let _ = paced_copy(server_read, &mut client_write, flow, shared).await;
        let _ = client_write.shutdown().await;
    };
    tokio::pin!(up, down);
    tokio::select! {
        () = &mut down => {
            // The site has finished: give the client a moment to finish too.
            let _ = tokio::time::timeout(LINGER, &mut up).await;
        }
        () = &mut up => down.await,
    }
}

/// Copies `from` to `to`, asking the limiter before passing on each read.
async fn paced_copy(
    mut from: impl AsyncRead + Unpin,
    to: &mut (impl AsyncWrite + Unpin),
    flow: FlowId,
    shared: &Shared,
) -> io::Result<()> {
    let mut buffer = vec![0; UNLIMITED_READ];
    loop {
        // Small reads while limited, so the limit is shared out finely.
        let size = if shared.limiter.is_limited() {
            CHUNK
        } else {
            UNLIMITED_READ
        };
        let read = from.read(&mut buffer[..size]).await?;
        if read == 0 {
            return Ok(());
        }
        // Not read further until allowed: the site's sending then slows to
        // match, because the connection's buffers fill up.
        shared.limiter.acquire(flow, read).await;
        shared.meter.add(read as u64);
        to.write_all(&buffer[..read]).await?;
    }
}

/// Reads up to and including the blank line after the headers. Returns the
/// head and whatever arrived after it.
async fn read_head(stream: &mut (impl AsyncRead + Unpin)) -> io::Result<(Vec<u8>, Vec<u8>)> {
    let mut buffer = Vec::with_capacity(1024);
    let mut chunk = [0; 4096];
    loop {
        let read = stream.read(&mut chunk).await?;
        if read == 0 {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        // Look again only where the end could have arrived.
        let from = buffer.len().saturating_sub(3);
        buffer.extend_from_slice(&chunk[..read]);
        if let Some(end) = buffer[from..].windows(4).position(|window| window == b"\r\n\r\n") {
            let rest = buffer.split_off(from + end + 4);
            return Ok((buffer, rest));
        }
        if buffer.len() > MAX_HEAD {
            return Err(invalid("a request head that never ends"));
        }
    }
}

async fn reply_error(client: &mut TcpStream, status: &str, body: &str) -> io::Result<()> {
    let reply = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    client.write_all(reply.as_bytes()).await?;
    client.shutdown().await
}

struct RequestHead<'a> {
    method: &'a str,
    target: &'a str,
    version: &'a str,
    headers: Vec<(&'a str, &'a str)>,
}

impl<'a> RequestHead<'a> {
    fn parse(head: &'a [u8]) -> Option<Self> {
        let text = std::str::from_utf8(head).ok()?;
        let mut lines = text.split("\r\n");
        let mut first = lines.next()?.split(' ');
        let (method, target, version) = (first.next()?, first.next()?, first.next()?);
        if method.is_empty() || target.is_empty() || !version.starts_with("HTTP/") {
            return None;
        }
        let headers = lines
            .filter(|line| !line.is_empty())
            .map(|line| line.split_once(':').map(|(name, value)| (name.trim(), value.trim())))
            .collect::<Option<Vec<_>>>()?;
        Some(Self {
            method,
            target,
            version,
            headers,
        })
    }
}

fn invalid(what: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, what.to_string())
}

/// Standard base64, for basic proxy authentication.
fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for group in bytes.chunks(3) {
        let n = group
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, byte)| n | (u32::from(*byte) << (16 - 8 * i)));
        for i in 0..4 {
            if i <= group.len() {
                out.push(ALPHABET[((n >> (18 - 6 * i)) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use super::*;

    /// A one-request site: records the request head it got and answers `body`.
    async fn site(body: Vec<u8>) -> (u16, tokio::task::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let task = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let (head, _) = read_head(&mut stream).await.unwrap();
            stream
                .write_all(format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", body.len()).as_bytes())
                .await
                .unwrap();
            stream.write_all(&body).await.unwrap();
            stream.shutdown().await.unwrap();
            String::from_utf8(head).unwrap()
        });
        (port, task)
    }

    /// A proxy for one download, as `Throttle::flow` starts it.
    async fn proxy(limit: Option<u64>) -> (u16, Arc<Shared>) {
        let limiter = Arc::new(Limiter::new(limit));
        let scheduler = Arc::clone(&limiter);
        tokio::spawn(async move { scheduler.run().await });
        let shared = Arc::new(Shared {
            limiter,
            meter: Arc::new(Meter::default()),
        });
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(serve(listener, 1, Arc::clone(&shared)));
        (port, shared)
    }

    async fn read_all(stream: &mut TcpStream) -> Vec<u8> {
        let mut out = Vec::new();
        stream.read_to_end(&mut out).await.unwrap();
        out
    }

    #[tokio::test]
    async fn passes_a_plain_request_on_to_the_site() {
        let (site_port, site) = site(b"hello".to_vec()).await;
        let (proxy_port, shared) = proxy(None).await;
        let mut client = TcpStream::connect(("127.0.0.1", proxy_port)).await.unwrap();
        client
            .write_all(
                format!(
                    "GET http://127.0.0.1:{site_port}/clip.mp4?x=1 HTTP/1.1\r\nHost: 127.0.0.1:{site_port}\r\nProxy-Connection: keep-alive\r\nRange: bytes=5-\r\n\r\n"
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        let answer = String::from_utf8(read_all(&mut client).await).unwrap();
        assert!(answer.starts_with("HTTP/1.1 200 OK") && answer.ends_with("hello"));

        let head = site.await.unwrap();
        assert!(head.starts_with("GET /clip.mp4?x=1 HTTP/1.1\r\n"), "{head}");
        assert!(head.contains("Range: bytes=5-\r\n"));
        assert!(head.contains("Connection: close\r\n"));
        assert!(!head.to_lowercase().contains("proxy-connection"));
        assert!(shared.meter.total() >= 5);
    }

    #[tokio::test]
    async fn tunnels_a_connect() {
        let (site_port, site) = site(b"through the tunnel".to_vec()).await;
        let (proxy_port, _) = proxy(None).await;
        let mut client = TcpStream::connect(("127.0.0.1", proxy_port)).await.unwrap();
        // The tunnelled request rides along with the CONNECT, as a TLS ClientHello can.
        client
            .write_all(
                format!("CONNECT 127.0.0.1:{site_port} HTTP/1.1\r\n\r\nGET / HTTP/1.1\r\nHost: x\r\n\r\n").as_bytes(),
            )
            .await
            .unwrap();
        let answer = String::from_utf8(read_all(&mut client).await).unwrap();
        assert!(answer.starts_with("HTTP/1.1 200 Connection established\r\n\r\nHTTP/1.1 200 OK"));
        assert!(answer.ends_with("through the tunnel"));
        assert!(site.await.unwrap().starts_with("GET / HTTP/1.1"));
    }

    #[tokio::test]
    async fn chains_to_the_system_proxy() {
        // A system proxy that grants any CONNECT and remembers what it was asked.
        let upstream = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let upstream_port = upstream.local_addr().unwrap().port();
        let asked = tokio::spawn(async move {
            let (mut stream, _) = upstream.accept().await.unwrap();
            let (head, _) = read_head(&mut stream).await.unwrap();
            stream.write_all(b"HTTP/1.1 200 OK\r\n\r\n").await.unwrap();
            String::from_utf8(head).unwrap()
        });
        let via = Upstream {
            kind: Kind::Http,
            host: "127.0.0.1".into(),
            port: upstream_port,
            credentials: Some("me:secret".into()),
        };
        tunnel_to("www.example.com", 443, Some(via)).await.unwrap();
        let head = asked.await.unwrap();
        assert!(head.starts_with("CONNECT www.example.com:443 HTTP/1.1\r\n"));
        assert!(head.contains("Proxy-Authorization: Basic bWU6c2VjcmV0\r\n"));
    }

    #[tokio::test(start_paused = true)]
    async fn gives_up_on_a_system_proxy_that_never_answers() {
        // A system proxy that takes the connection and reads the request, but never answers.
        let upstream = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let upstream_port = upstream.local_addr().unwrap().port();
        let _silent = tokio::spawn(async move {
            let (mut stream, _) = upstream.accept().await.unwrap();
            let _ = read_head(&mut stream).await;
            std::future::pending::<()>().await;
        });
        let via = Upstream {
            kind: Kind::Http,
            host: "127.0.0.1".into(),
            port: upstream_port,
            credentials: None,
        };
        let started = tokio::time::Instant::now();
        let error = tunnel_to("www.example.com", 443, Some(via)).await.unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert_eq!(started.elapsed(), CONNECT_TIMEOUT);
    }

    /// A SOCKS5 proxy for one connection: grants it, then really connects to
    /// the address it was asked for and relays both ways. Returns that address.
    async fn socks5_proxy() -> (Upstream, tokio::task::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let task = tokio::spawn(async move {
            let (mut client, _) = listener.accept().await.unwrap();
            let mut greeting = [0; 3];
            client.read_exact(&mut greeting).await.unwrap();
            assert_eq!(greeting, [5, 1, 0]);
            client.write_all(&[5, 0]).await.unwrap();
            let mut ask = [0; 4];
            client.read_exact(&mut ask).await.unwrap();
            assert_eq!(&ask[..3], &[5, 1, 0]);
            let host = match ask[3] {
                1 => {
                    let mut ip = [0; 4];
                    client.read_exact(&mut ip).await.unwrap();
                    std::net::Ipv4Addr::from(ip).to_string()
                }
                3 => {
                    let mut name = vec![0; usize::from(client.read_u8().await.unwrap())];
                    client.read_exact(&mut name).await.unwrap();
                    String::from_utf8(name).unwrap()
                }
                other => panic!("address type {other}"),
            };
            let port = client.read_u16().await.unwrap();
            // The test sites listen on 127.0.0.1 only; don't try ::1 first.
            let address = if host == "localhost" {
                "127.0.0.1"
            } else {
                host.as_str()
            };
            let mut site = TcpStream::connect((address, port)).await.unwrap();
            client.write_all(&[5, 0, 0, 1, 127, 0, 0, 1, 0, 0]).await.unwrap();
            let _ = tokio::io::copy_bidirectional(&mut client, &mut site).await;
            format!("{host}:{port}")
        });
        let upstream = Upstream {
            kind: Kind::Socks(socks::Version::Five { remote_dns: true }),
            host: "127.0.0.1".into(),
            port,
            credentials: None,
        };
        (upstream, task)
    }

    #[tokio::test]
    async fn tunnels_through_a_socks_proxy() {
        let (site_port, site) = site(b"through socks".to_vec()).await;
        let (via, asked) = socks5_proxy().await;
        let mut server = tunnel_to("localhost", site_port, Some(via)).await.unwrap();
        server.write_all(b"GET / HTTP/1.1\r\nHost: x\r\n\r\n").await.unwrap();
        let answer = String::from_utf8(read_all(&mut server).await).unwrap();
        assert!(answer.ends_with("through socks"), "{answer}");
        assert!(site.await.unwrap().starts_with("GET / HTTP/1.1"));
        // The test proxy relays until both ends have closed.
        drop(server);
        // socks5h: the proxy was given the name, not an address looked up here.
        assert_eq!(asked.await.unwrap(), format!("localhost:{site_port}"));
    }

    #[tokio::test]
    async fn never_goes_around_a_socks_proxy_that_is_down() {
        // A site that is up, and a SOCKS proxy on a port nothing listens on.
        let (site_port, _site) = site(b"direct".to_vec()).await;
        let closed = TcpListener::bind("127.0.0.1:0")
            .await
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let via = Upstream {
            kind: Kind::Socks(socks::Version::Five { remote_dns: true }),
            host: "127.0.0.1".into(),
            port: closed,
            credentials: None,
        };
        assert!(tunnel_to("127.0.0.1", site_port, Some(via)).await.is_err());
    }

    #[tokio::test]
    async fn sends_a_plain_request_through_a_socks_proxy_as_to_the_site() {
        let (site_port, site) = site(b"hello".to_vec()).await;
        let (via, _) = socks5_proxy().await;
        let head = format!("GET http://127.0.0.1:{site_port}/clip.mp4 HTTP/1.1\r\nHost: 127.0.0.1:{site_port}\r\n\r\n");
        let request = RequestHead::parse(head.as_bytes()).unwrap();
        let (mut server, _) = forward(&request, Some(via)).await.unwrap();
        let answer = String::from_utf8(read_all(&mut server).await).unwrap();
        assert!(answer.ends_with("hello"));
        // Only the path, as to the site itself, and no proxy sign-in header.
        let head = site.await.unwrap();
        assert!(head.starts_with("GET /clip.mp4 HTTP/1.1\r\n"), "{head}");
        assert!(!head.contains("Proxy-Authorization"));
    }

    #[tokio::test]
    async fn says_why_a_site_couldnt_be_reached() {
        // Nothing listens on a port that was just freed.
        let port = TcpListener::bind("127.0.0.1:0")
            .await
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let (proxy_port, _) = proxy(None).await;
        let mut client = TcpStream::connect(("127.0.0.1", proxy_port)).await.unwrap();
        client
            .write_all(format!("CONNECT 127.0.0.1:{port} HTTP/1.1\r\n\r\n").as_bytes())
            .await
            .unwrap();
        let answer = String::from_utf8(read_all(&mut client).await).unwrap();
        assert!(
            answer.starts_with("HTTP/1.1 502 Siphon couldn't reach the site"),
            "{answer}"
        );
    }

    #[tokio::test]
    async fn paces_what_comes_back() {
        let (site_port, _) = site(vec![7; 512 * 1024]).await;
        let (proxy_port, _) = proxy(Some(1024 * 1024)).await;
        let started = Instant::now();
        let mut client = TcpStream::connect(("127.0.0.1", proxy_port)).await.unwrap();
        client
            .write_all(format!("GET http://127.0.0.1:{site_port}/ HTTP/1.1\r\n\r\n").as_bytes())
            .await
            .unwrap();
        let answer = read_all(&mut client).await;
        assert!(answer.len() > 512 * 1024);
        // Half a megabyte at one megabyte a second.
        let took = started.elapsed();
        assert!(took >= Duration::from_millis(450), "took {took:?}");
        assert!(took < Duration::from_millis(1500), "took {took:?}");
    }

    #[test]
    fn encodes_base64() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"me:secret"), "bWU6c2VjcmV0");
    }
}
