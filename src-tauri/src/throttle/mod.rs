//! The download speed limit: one limit for all downloads together.
//!
//! yt-dlp's own `--limit-rate` can't do this: it limits one process, is fixed
//! when the process starts, and doesn't reach ffmpeg, which fetches trimmed
//! parts and some streams. So every download's connections go through a
//! small proxy inside Siphon instead ([`proxy`]), which paces what comes back
//! with one shared, fair [`Limiter`].
//!
//! Every download goes through it, limited or not, so that setting or
//! changing the limit takes effect on downloads that are already running.
//! Without a limit the proxy only passes the bytes on (and counts them, for
//! the total speed shown in the title bar).

mod limiter;
mod proxy;
mod socks;
mod upstream;

pub use upstream::follow_system_proxy;

/// A connection to `host:port` (a TLS port) the way downloads go: through the
/// system proxy, HTTP or SOCKS, if this PC uses one, or straight there.
pub async fn connect(host: &str, port: u16) -> std::io::Result<tokio::net::TcpStream> {
    proxy::tunnel_to(host, port, upstream::for_host(host, true)).await
}

use std::net::{Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter};
use tokio::net::TcpListener;
use tokio::process::Command;
use tokio::task::JoinHandle;

use crate::error::{AppError, Result};
use limiter::{FlowId, Limiter};

/// Event carrying a [`Speed`]: the total download speed, twice a second
/// while anything downloads, then once more with zero.
pub const SPEED_EVENT: &str = "speed://total";

/// How often the total speed is reported, and how far back it looks.
const SPEED_TICK: Duration = Duration::from_millis(500);
const SPEED_WINDOW: usize = 2;

#[derive(Debug, Clone, Copy, Serialize)]
pub struct Speed {
    pub bytes_per_second: u64,
}

/// Counts the bytes every download has received.
#[derive(Default)]
pub struct Meter(AtomicU64);

impl Meter {
    fn add(&self, bytes: u64) {
        self.0.fetch_add(bytes, Ordering::Relaxed);
    }

    fn total(&self) -> u64 {
        self.0.load(Ordering::Relaxed)
    }
}

pub struct Throttle {
    shared: Arc<proxy::Shared>,
    next_flow: AtomicU64,
}

impl Throttle {
    /// `limit` in bytes per second; `None` is unlimited.
    pub fn new(limit: Option<u64>) -> Self {
        Self {
            shared: Arc::new(proxy::Shared {
                limiter: Arc::new(Limiter::new(limit)),
                meter: Arc::new(Meter::default()),
            }),
            next_flow: AtomicU64::new(1),
        }
    }

    /// Starts handing out the limit and reporting the total speed.
    pub fn start(&self, app: AppHandle) {
        let limiter = Arc::clone(&self.shared.limiter);
        tauri::async_runtime::spawn(async move { limiter.run().await });
        let meter = Arc::clone(&self.shared.meter);
        tauri::async_runtime::spawn(report_speed(app, meter));
    }

    /// Changes the limit. Running downloads follow it at once.
    pub fn set_limit(&self, limit: Option<u64>) {
        self.shared.limiter.set_rate(limit);
    }

    /// A way through the limit for one download. Everything the download
    /// fetches while the flow exists counts as one share.
    ///
    /// Each flow listens on a port of its own, which Windows picks, on the
    /// loopback address only: nothing outside this computer can reach it.
    pub async fn flow(&self) -> Result<Flow> {
        let failed = |err: std::io::Error| {
            AppError::internal("Couldn't prepare the download.").with_details(format!("local proxy: {err}"))
        };
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.map_err(failed)?;
        let address = listener.local_addr().map_err(failed)?;
        let id = self.next_flow.fetch_add(1, Ordering::Relaxed);
        // The task owns the listener and every connection it accepted, so
        // aborting it (see `Drop`) closes them all.
        let server = tokio::spawn(proxy::serve(listener, id, Arc::clone(&self.shared)));
        Ok(Flow {
            id,
            address,
            server,
            limiter: Arc::clone(&self.shared.limiter),
        })
    }
}

/// One download's way through the proxy. Dropping it closes the listener
/// and ends any connection still open on it. A download holds its flow
/// only while it runs: it is dropped when the download finishes, fails, is
/// paused or is cancelled, and a resumed download gets a new one.
pub struct Flow {
    id: FlowId,
    address: SocketAddr,
    server: JoinHandle<()>,
    limiter: Arc<Limiter>,
}

impl Flow {
    /// Sends a yt-dlp command's downloads through this flow. yt-dlp passes
    /// the proxy on to the ffmpeg it starts for trimmed parts.
    pub fn route(&self, cmd: &mut Command) {
        cmd.arg("--proxy").arg(format!("http://{}", self.address));
        // ffmpeg would go around the proxy for the sites these name. The
        // proxy honours them itself when it picks the way to the site.
        cmd.env_remove("NO_PROXY").env_remove("no_proxy");
    }

    /// Lets this download go first for up to `bytes_per_second` (a live
    /// recording, which has to keep up with its stream). See `limiter`.
    pub fn reserve(&self, bytes_per_second: u64) {
        self.limiter.set_reserve(self.id, Some(bytes_per_second));
    }
}

#[cfg(test)]
impl Flow {
    pub fn address(&self) -> SocketAddr {
        self.address
    }
}

impl Drop for Flow {
    fn drop(&mut self) {
        self.server.abort();
        self.limiter.forget(self.id);
    }
}

/// Reports the total download speed while anything downloads.
async fn report_speed(app: AppHandle, meter: Arc<Meter>) {
    let mut tick = tokio::time::interval(SPEED_TICK);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut last = meter.total();
    let mut recent = [0u64; SPEED_WINDOW];
    let mut reported_zero = true;
    loop {
        tick.tick().await;
        let total = meter.total();
        recent.rotate_left(1);
        recent[SPEED_WINDOW - 1] = total - last;
        last = total;
        let window = SPEED_TICK.as_secs_f64() * SPEED_WINDOW as f64;
        let bytes_per_second = (recent.iter().sum::<u64>() as f64 / window) as u64;
        // Quiet when nothing moves, apart from the one zero that says so.
        if bytes_per_second > 0 || !reported_zero {
            let _ = app.emit(SPEED_EVENT, Speed { bytes_per_second });
            reported_zero = bytes_per_second == 0;
        }
    }
}

#[cfg(test)]
mod tests {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;

    use super::*;

    /// Lets the runtime carry out an abort.
    async fn settle() {
        for _ in 0..10 {
            tokio::task::yield_now().await;
        }
    }

    #[tokio::test]
    async fn each_download_listens_on_its_own_loopback_port() {
        let throttle = Throttle::new(None);
        let (first, second) = (throttle.flow().await.unwrap(), throttle.flow().await.unwrap());
        for flow in [&first, &second] {
            assert_eq!(flow.address.ip(), Ipv4Addr::LOCALHOST, "never 0.0.0.0");
            assert_ne!(flow.address.port(), 0);
        }
        assert_ne!(first.address.port(), second.address.port());

        let mut cmd = Command::new("yt-dlp");
        first.route(&mut cmd);
        let args: Vec<_> = cmd
            .as_std()
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert_eq!(args, ["--proxy", &format!("http://127.0.0.1:{}", first.address.port())]);
    }

    #[tokio::test]
    async fn dropping_a_flow_closes_its_listener_and_open_connections() {
        // A site that accepts a tunnel and then keeps it open.
        let site = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let site_port = site.local_addr().unwrap().port();
        tokio::spawn(async move {
            let (_held, _) = site.accept().await.unwrap();
            std::future::pending::<()>().await;
        });

        let throttle = Throttle::new(None);
        let flow = throttle.flow().await.unwrap();
        let address = flow.address;
        let mut open = TcpStream::connect(address).await.unwrap();
        open.write_all(format!("CONNECT 127.0.0.1:{site_port} HTTP/1.1\r\n\r\n").as_bytes())
            .await
            .unwrap();
        let mut answer = [0; 39];
        tokio::time::timeout(Duration::from_secs(5), open.read_exact(&mut answer))
            .await
            .expect("the tunnel should open")
            .unwrap();
        assert!(answer.starts_with(b"HTTP/1.1 200"));

        drop(flow);
        settle().await;
        // The tunnel that was open is closed …
        let mut rest = Vec::new();
        let ended = tokio::time::timeout(Duration::from_secs(2), open.read_to_end(&mut rest)).await;
        assert!(ended.is_ok(), "the open connection was left running");
        // … and nothing listens on the port any more.
        assert!(TcpStream::connect(address).await.is_err());
    }
}
