//! One speed limit for every download together, shared out fairly.
//!
//! A token bucket fills at the limit. Readers ask for tokens before passing
//! on what they read, and one scheduler hands them out. When several
//! downloads ("flows") are waiting, the one that has been given the fewest
//! bytes goes next, so each gets an equal share however many connections it
//! opens (yt-dlp fetches four pieces of a stream at once). A flow that wants
//! less than its share leaves the rest to the others.
//!
//! A flow can also reserve a rate: a live recording has to keep up with the
//! stream, or it falls behind and fails. It earns credit at that rate, and
//! while it has credit for what it asks, it goes first. Everything it is given
//! is paid from its credit, so a reserve below its fair share changes nothing,
//! and one above it takes only what it needs from the others.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use tokio::sync::{oneshot, Notify};
use tokio::time::Instant;

/// Identifies one download's connections.
pub type FlowId = u64;

/// The most a reader takes in one go while a limit is set, and so the unit
/// the limit is handed out in: 16 KiB is about 3 ms at 5 MB/s.
pub const CHUNK: usize = 16 * 1024;

/// How long the bucket may save up for: after a pause, a burst of at most
/// this much of a second's worth goes out at once.
const BURST: f64 = 0.05;

/// How many seconds of its reserve a flow may save up. A live stream arrives
/// in segments a few seconds apart, and each has to come through at once:
/// the credit earned between them pays for that.
const RESERVE_SAVINGS: f64 = 10.0;

pub struct Limiter {
    state: Mutex<State>,
    /// Wakes the scheduler: a request arrived or the limit changed.
    changed: Notify,
    /// Mirrors `state.rate.is_some()`, for readers that check every read.
    limited: AtomicBool,
}

struct State {
    /// Bytes per second; `None` is unlimited.
    rate: Option<u64>,
    tokens: f64,
    refilled: Instant,
    flows: HashMap<FlowId, Flow>,
    /// What the flow served last had been given before it: a flow that starts
    /// waiting starts here, so time spent idle earns it no catching up.
    clock: u64,
}

#[derive(Default)]
struct Flow {
    /// Bytes granted so far, on the shared clock.
    served: u64,
    waiting: VecDeque<Request>,
    /// Bytes per second this flow goes first for, if any.
    reserve: Option<u64>,
    /// Bytes it may still be given ahead of the others.
    credit: f64,
}

impl Flow {
    /// Whether the request at the front can be paid from the reserve.
    fn first_in_line(&self, bytes: u64) -> bool {
        self.reserve.is_some() && self.credit >= bytes as f64
    }
}

struct Request {
    bytes: u64,
    grant: oneshot::Sender<()>,
}

impl Limiter {
    pub fn new(rate: Option<u64>) -> Self {
        Self {
            state: Mutex::new(State {
                rate,
                tokens: 0.0,
                refilled: Instant::now(),
                flows: HashMap::new(),
                clock: 0,
            }),
            changed: Notify::new(),
            limited: AtomicBool::new(rate.is_some()),
        }
    }

    pub fn is_limited(&self) -> bool {
        self.limited.load(Ordering::Relaxed)
    }

    /// Changes the limit. Waiting readers are served at the new rate at once.
    pub fn set_rate(&self, rate: Option<u64>) {
        let mut state = self.state.lock().unwrap();
        state.refill(Instant::now());
        state.rate = rate;
        state.tokens = state.tokens.min(state.capacity());
        self.limited.store(rate.is_some(), Ordering::Relaxed);
        drop(state);
        self.changed.notify_one();
    }

    /// Waits until `flow` may pass on `bytes`. Returns at once when there is no limit.
    pub async fn acquire(&self, flow: FlowId, bytes: usize) {
        let granted = {
            let mut state = self.state.lock().unwrap();
            if state.rate.is_none() {
                return;
            }
            let clock = state.clock;
            let entry = state.flows.entry(flow).or_default();
            if entry.waiting.is_empty() {
                entry.served = entry.served.max(clock);
            }
            let (grant, granted) = oneshot::channel();
            entry.waiting.push_back(Request {
                bytes: bytes as u64,
                grant,
            });
            granted
        };
        self.changed.notify_one();
        // An error only means the limiter is gone, along with any reason to wait.
        let _ = granted.await;
    }

    /// Lets `flow` go first for up to `rate` bytes per second (see the module
    /// notes); `None` makes it an ordinary flow again.
    pub fn set_reserve(&self, flow: FlowId, rate: Option<u64>) {
        let mut state = self.state.lock().unwrap();
        state.refill(Instant::now());
        let entry = state.flows.entry(flow).or_default();
        entry.reserve = rate.filter(|rate| *rate > 0);
        entry.credit = 0.0;
        drop(state);
        self.changed.notify_one();
    }

    /// Forgets a flow whose download has ended.
    pub fn forget(&self, flow: FlowId) {
        self.state.lock().unwrap().flows.remove(&flow);
    }

    /// Hands out tokens for as long as the limiter exists. Run once, in its own task.
    pub async fn run(&self) {
        loop {
            match self.grant_ready() {
                Some(wait) => {
                    tokio::select! {
                        () = tokio::time::sleep(wait) => {}
                        () = self.changed.notified() => {}
                    }
                }
                None => self.changed.notified().await,
            }
        }
    }

    /// Grants every request the bucket can pay for, fairest first. Returns how
    /// long until the next one can be paid for, or `None` if nothing waits.
    fn grant_ready(&self) -> Option<Duration> {
        let mut state = self.state.lock().unwrap();
        state.refill(Instant::now());
        loop {
            // A flow with credit for its reserve first; otherwise the waiting flow
            // that has been given the least (ties: the oldest id).
            let next = state
                .flows
                .iter_mut()
                .filter_map(|(id, flow)| {
                    // A reader that stopped waiting (its connection closed) needs nothing.
                    while flow.waiting.front().is_some_and(|request| request.grant.is_closed()) {
                        flow.waiting.pop_front();
                    }
                    let bytes = flow.waiting.front()?.bytes;
                    Some((!flow.first_in_line(bytes), flow.served, *id, bytes))
                })
                .min();
            let (_, served, id, bytes) = next?;
            if let Some(rate) = state.rate {
                let missing = bytes as f64 - state.tokens;
                if missing > 0.0 {
                    return Some(Duration::from_secs_f64(missing / rate as f64));
                }
                state.tokens -= bytes as f64;
            }
            // A reserved flow served ahead of its turn doesn't move the others' clock.
            state.clock = state.clock.max(
                served.min(
                    state
                        .flows
                        .values()
                        .filter(|flow| !flow.waiting.is_empty())
                        .map(|flow| flow.served)
                        .min()
                        .unwrap_or(served),
                ),
            );
            let flow = state.flows.get_mut(&id).expect("just found");
            flow.served += bytes;
            flow.credit = (flow.credit - bytes as f64).max(0.0);
            if let Some(request) = flow.waiting.pop_front() {
                let _ = request.grant.send(());
            }
        }
    }
}

impl State {
    /// The most the bucket holds: enough for one read, or a twentieth of a
    /// second at the limit, whichever is more.
    fn capacity(&self) -> f64 {
        self.rate.map_or(0.0, |rate| (rate as f64 * BURST).max(CHUNK as f64))
    }

    fn refill(&mut self, now: Instant) {
        let elapsed = now.saturating_duration_since(self.refilled).as_secs_f64();
        self.refilled = now;
        if let Some(rate) = self.rate {
            self.tokens = (self.tokens + elapsed * rate as f64).min(self.capacity());
        }
        for flow in self.flows.values_mut() {
            if let Some(reserve) = flow.reserve {
                let reserve = reserve as f64;
                flow.credit = (flow.credit + elapsed * reserve).min(reserve * RESERVE_SAVINGS);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicU64;
    use std::sync::Arc;

    use super::*;

    const MB: u64 = 1024 * 1024;

    /// A limiter with its scheduler running.
    fn limiter(rate: Option<u64>) -> Arc<Limiter> {
        let limiter = Arc::new(Limiter::new(rate));
        let scheduler = Arc::clone(&limiter);
        tokio::spawn(async move { scheduler.run().await });
        limiter
    }

    /// A reader that takes a full chunk as fast as it is allowed to,
    /// optionally resting between reads, and counts what it got.
    fn reader(limiter: &Arc<Limiter>, flow: FlowId, rest: Option<Duration>) -> Arc<AtomicU64> {
        let count = Arc::new(AtomicU64::new(0));
        let (limiter, counted) = (Arc::clone(limiter), Arc::clone(&count));
        tokio::spawn(async move {
            loop {
                limiter.acquire(flow, CHUNK).await;
                counted.fetch_add(CHUNK as u64, Ordering::Relaxed);
                match rest {
                    Some(rest) => tokio::time::sleep(rest).await,
                    None => tokio::task::yield_now().await,
                }
            }
        });
        count
    }

    fn read(count: &AtomicU64) -> u64 {
        count.load(Ordering::Relaxed)
    }

    fn assert_near(actual: u64, expected: u64, tolerance: u64) {
        assert!(
            actual.abs_diff(expected) <= tolerance,
            "{actual} is not within {tolerance} of {expected}"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn keeps_to_the_limit() {
        let limiter = limiter(Some(2 * MB));
        let count = reader(&limiter, 1, None);
        tokio::time::sleep(Duration::from_secs(3)).await;
        assert_near(read(&count), 6 * MB, 2 * CHUNK as u64);
    }

    #[tokio::test(start_paused = true)]
    async fn downloads_share_equally_however_many_connections_they_open() {
        let limiter = limiter(Some(4 * MB));
        // One download on four connections, as yt-dlp fetches a segmented stream …
        let four: Vec<_> = (0..4).map(|_| reader(&limiter, 1, None)).collect();
        // … and two on one each.
        let second = reader(&limiter, 2, None);
        let third = reader(&limiter, 3, None);
        tokio::time::sleep(Duration::from_secs(3)).await;

        let first: u64 = four.iter().map(|count| read(count)).sum();
        let (second, third) = (read(&second), read(&third));
        assert_near(first + second + third, 12 * MB, 4 * CHUNK as u64);
        for share in [first, second, third] {
            assert_near(share, 4 * MB, 4 * CHUNK as u64);
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_share_left_unused_goes_to_the_others() {
        let limiter = limiter(Some(4 * MB));
        // Wants about 160 KB/s at most: a slow site.
        let slow = reader(&limiter, 1, Some(Duration::from_millis(100)));
        let fast = reader(&limiter, 2, None);
        tokio::time::sleep(Duration::from_secs(2)).await;

        assert_near(read(&slow), 20 * CHUNK as u64, 2 * CHUNK as u64);
        assert_near(read(&slow) + read(&fast), 8 * MB, 2 * CHUNK as u64);
    }

    #[tokio::test(start_paused = true)]
    async fn a_new_download_gets_no_catch_up_burst() {
        let limiter = limiter(Some(2 * MB));
        let early = reader(&limiter, 1, None);
        tokio::time::sleep(Duration::from_secs(2)).await;
        let early_before = read(&early);
        let late = reader(&limiter, 2, None);
        tokio::time::sleep(Duration::from_secs(2)).await;

        // From the moment it starts, the late one gets half, not everything until it has caught up.
        assert_near(read(&late), 2 * MB, 2 * CHUNK as u64);
        assert_near(read(&early) - early_before, 2 * MB, 2 * CHUNK as u64);
    }

    #[tokio::test(start_paused = true)]
    async fn a_new_limit_applies_at_once() {
        let limiter = limiter(Some(MB));
        let count = reader(&limiter, 1, None);
        tokio::time::sleep(Duration::from_secs(1)).await;
        let at_one = read(&count);
        assert_near(at_one, MB, 2 * CHUNK as u64);

        limiter.set_rate(Some(4 * MB));
        tokio::time::sleep(Duration::from_millis(500)).await;
        assert_near(read(&count) - at_one, 2 * MB, 2 * CHUNK as u64);

        // And lifting it lets a waiting reader through without delay.
        limiter.set_rate(Some(1));
        tokio::time::sleep(Duration::from_millis(100)).await;
        let held = read(&count);
        let lifted = Instant::now();
        limiter.set_rate(None);
        // (Not a sleep: with time paused and a reader that never waits, time would stand still.)
        for _ in 0..1000 {
            tokio::task::yield_now().await;
        }
        assert_eq!(Instant::now(), lifted);
        assert!(read(&count) > held + 100 * CHUNK as u64);
    }

    #[tokio::test(start_paused = true)]
    async fn a_recording_gets_its_reserve_ahead_of_the_others() {
        let limiter = limiter(Some(4 * MB));
        limiter.set_reserve(1, Some(3 * MB));
        let live = reader(&limiter, 1, None);
        let other = reader(&limiter, 2, None);
        tokio::time::sleep(Duration::from_secs(4)).await;

        // A fair share would be 2 MB/s each; the recording needs 3.
        assert_near(read(&live), 12 * MB, MB / 2);
        assert_near(read(&other), 4 * MB, MB / 2);
    }

    #[tokio::test(start_paused = true)]
    async fn a_reserve_below_the_fair_share_changes_nothing() {
        let limiter = limiter(Some(4 * MB));
        limiter.set_reserve(1, Some(MB));
        let live = reader(&limiter, 1, None);
        let other = reader(&limiter, 2, None);
        tokio::time::sleep(Duration::from_secs(4)).await;

        assert_near(read(&live), 8 * MB, MB / 2);
        assert_near(read(&other), 8 * MB, MB / 2);
    }

    #[tokio::test(start_paused = true)]
    async fn a_reserve_above_the_limit_takes_all_of_it() {
        let limiter = limiter(Some(2 * MB));
        limiter.set_reserve(1, Some(5 * MB));
        let live = reader(&limiter, 1, None);
        let other = reader(&limiter, 2, None);
        tokio::time::sleep(Duration::from_secs(4)).await;

        assert_near(read(&live), 8 * MB, MB / 4);
        assert!(read(&other) < MB / 4, "the other download got {}", read(&other));
    }

    #[tokio::test(start_paused = true)]
    async fn a_recording_saves_up_for_the_segments_it_fetches_in_bursts() {
        let limiter = limiter(Some(4 * MB));
        limiter.set_reserve(1, Some(MB));
        let other = reader(&limiter, 2, None);
        // Idle for 4 s while the other download takes everything …
        tokio::time::sleep(Duration::from_secs(4)).await;
        let before = read(&other);
        // … then a 4 MB segment arrives: it goes first, at the whole limit.
        let started = Instant::now();
        for _ in 0..(4 * MB / CHUNK as u64) {
            limiter.acquire(1, CHUNK).await;
        }
        let took = started.elapsed();
        assert!(took < Duration::from_millis(1100), "the segment took {took:?}");
        assert!(read(&other) - before < MB / 2);
    }

    #[tokio::test(start_paused = true)]
    async fn without_a_limit_nothing_waits() {
        let limiter = limiter(None);
        let started = Instant::now();
        for _ in 0..10_000 {
            limiter.acquire(1, CHUNK).await;
        }
        assert_eq!(Instant::now(), started);
        assert!(!limiter.is_limited());
    }
}
