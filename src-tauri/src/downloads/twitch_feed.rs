//! What a Twitch recording from now reads: a playlist Siphon serves on
//! 127.0.0.1, made from Twitch's own, so that the recording has no ads in it
//! (see `downloads::live`).
//!
//! Twitch shows viewers without an account ads in place of the stream: about
//! 30 seconds as a recording starts, and whenever the streamer runs some.
//! - Where the channel keeps a public VOD, the stream is read from the VOD,
//!   from its newest segment on ([`Feed::vod`]). Twitch saves VODs without
//!   those ads, so nothing is missed while one plays. The VOD grows ten
//!   seconds at a time, a little behind the stream: a recording starts with
//!   what aired just before Record, and Stop records on until the VOD has
//!   what aired when it was pressed ([`CatchUp`]; 8 to 15 s in testing,
//!   2026-11-22). The recording is on the VOD's clock, so its chat is placed
//!   as Twitch's replay places it.
//! - Otherwise the stream's own playlist is served without the segments
//!   Twitch marks as ads ([`Feed::live`]): the stream's own are titled
//!   `live` (`#EXTINF:2.000,live`), an ad's with the advertiser's name.
//!   Twitch doesn't send the stream during an ad, so the recording jumps
//!   over it. ffmpeg closes the jump up, so the file plays on without a gap,
//!   and where it jumps and when what follows aired is kept ([`Mark`]) for
//!   the chat. (Tested 2026-11-22: a 12-second jump was closed up, picture
//!   and sound stayed together, and the segment after it began exactly where
//!   the lengths of the segments served before it put it.)
//!
//! yt-dlp still makes the recording, from a lookup whose stream is the
//! served playlist (`--load-info-json`), so recording, Stop, crashes and
//! recovery work as for any other stream.

use std::collections::{HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;

use crate::chat::parse_timestamp;

/// How many of the stream's segments a served live playlist lists: about
/// as many as Twitch's own (30 s).
const LIVE_WINDOW: usize = 15;
/// How long the first request waits for the stream itself while Twitch
/// shows the ad it opens a playlist with.
const FIRST_SEGMENT_WAIT: Duration = Duration::from_secs(3 * 60);
/// How often Twitch's playlist is read while it shows only an ad, as a
/// player reads it: the ad only plays out while the playlist is read.
const AD_POLL: Duration = Duration::from_secs(2);
/// A VOD that hasn't grown for this long has stopped (it grows ten seconds
/// at a time while the stream airs): the stream dropped or ended. Its
/// playlist is then answered as Twitch answers a dropped stream's, "not
/// found", so the recording waits for the stream as it does for any.
const VOD_STALL: Duration = Duration::from_secs(60);

/// A segment of one of Twitch's media playlists.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Segment {
    /// Where it is, in full.
    pub uri: String,
    pub duration: f64,
    /// The stream itself rather than an ad.
    pub live: bool,
    /// When it aired, as the playlist writes it (`#EXT-X-PROGRAM-DATE-TIME`).
    pub date: Option<String>,
    /// The same, in seconds since 1970.
    pub aired: Option<f64>,
    /// Its initialisation segment (`#EXT-X-MAP`), for fragmented MP4.
    pub init: Option<String>,
}

impl Segment {
    /// What tells it from the others from one reading of the playlist to the
    /// next: when it aired, where the playlist says.
    fn key(&self) -> &str {
        self.date.as_deref().unwrap_or(&self.uri)
    }
}

/// A media playlist, as far as recording it needs.
#[derive(Debug, Clone, Default, PartialEq)]
pub(super) struct Playlist {
    pub target: Option<u32>,
    pub segments: Vec<Segment>,
    /// `#EXT-X-ENDLIST`: there will be no more.
    pub ended: bool,
}

/// Reads a media playlist; relative links are taken from `base`, its address.
pub(super) fn parse(text: &str, base: &str) -> Playlist {
    let absolute = |link: &str| {
        if link.starts_with("http") {
            link.to_string()
        } else {
            let dir = base.rsplit_once('/').map_or(base, |(dir, _)| dir);
            format!("{dir}/{link}")
        }
    };
    let mut playlist = Playlist::default();
    let (mut date, mut duration, mut live, mut init) = (None, 0.0, false, None::<String>);
    for line in text.lines().map(str::trim) {
        if let Some(value) = line.strip_prefix("#EXT-X-TARGETDURATION:") {
            playlist.target = value.trim().parse().ok();
        } else if let Some(value) = line.strip_prefix("#EXT-X-PROGRAM-DATE-TIME:") {
            date = Some(value.to_string());
        } else if let Some(info) = line.strip_prefix("#EXTINF:") {
            let (length, title) = info.split_once(',').unwrap_or((info, ""));
            duration = length.trim().parse().unwrap_or(0.0);
            live = title == "live";
        } else if let Some(map) = line.strip_prefix("#EXT-X-MAP:") {
            // `URI="…"`, perhaps with a byte range after it.
            init = map
                .split_once("URI=\"")
                .and_then(|(_, rest)| rest.split_once('"'))
                .map(|(link, _)| absolute(link));
        } else if line == "#EXT-X-ENDLIST" {
            playlist.ended = true;
        } else if !line.is_empty() && !line.starts_with('#') {
            let date = date.take();
            playlist.segments.push(Segment {
                uri: absolute(line),
                duration,
                live,
                aired: date.as_deref().and_then(parse_timestamp),
                date,
                init: init.clone(),
            });
            (duration, live) = (0.0, false);
        }
    }
    playlist
}

/// Where a recording read without the ads starts, or goes on after one: how
/// far into what was served (the sum of the served segments' lengths before
/// it), and when that aired. The recording starts with the first segment
/// served (see [`LiveState::take`]), and ffmpeg closes up each jump, so
/// that is where it is in the recording, from the file's first timestamp.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Mark {
    pub offset: f64,
    /// In seconds since 1970.
    pub aired: f64,
}

/// A served playlist's beginning: version, the longest segment, where the
/// numbering starts.
fn head(out: &mut String, segments: &[&Segment], target: Option<u32>, first: u64) {
    let cmaf = segments.iter().any(|segment| segment.init.is_some());
    let longest = segments.iter().map(|segment| segment.duration).fold(0.0, f64::max);
    let target = target.unwrap_or(0).max(longest.ceil() as u32).max(1);
    out.push_str(&format!(
        "#EXTM3U\n#EXT-X-VERSION:{}\n#EXT-X-TARGETDURATION:{target}\n#EXT-X-MEDIA-SEQUENCE:{first}\n",
        if cmaf { 6 } else { 3 }
    ));
}

/// One segment as served, with its initialisation segment where it changes.
fn segment_lines(out: &mut String, segment: &Segment, title: &str, init: &mut Option<String>) {
    if segment.init.is_some() && segment.init != *init {
        out.push_str(&format!(
            "#EXT-X-MAP:URI=\"{}\"\n",
            segment.init.as_deref().unwrap_or_default()
        ));
    }
    init.clone_from(&segment.init);
    if let Some(date) = &segment.date {
        out.push_str(&format!("#EXT-X-PROGRAM-DATE-TIME:{date}\n"));
    }
    out.push_str(&format!("#EXTINF:{:.3},{title}\n{}\n", segment.duration, segment.uri));
}

/// A VOD's playlist from segment `start` on, as a live one that grows. With
/// `until`, a second of the VOD that it has: only as far as the segment
/// holding that second, and ended there.
fn vod_playlist(playlist: &Playlist, start: usize, until: Option<f64>) -> String {
    let mut at: f64 = playlist
        .segments
        .iter()
        .take(start)
        .map(|segment| segment.duration)
        .sum();
    let mut segments = Vec::new();
    for segment in playlist.segments.iter().skip(start) {
        if until.is_some_and(|until| at >= until) && !segments.is_empty() {
            break;
        }
        segments.push(segment);
        at += segment.duration;
    }
    let mut out = String::new();
    head(&mut out, &segments, playlist.target, start as u64);
    out.push_str("#EXT-X-PLAYLIST-TYPE:EVENT\n");
    let mut init = None;
    for segment in segments {
        segment_lines(&mut out, segment, "", &mut init);
    }
    if playlist.ended || until.is_some() {
        out.push_str("#EXT-X-ENDLIST\n");
    }
    out
}

/// A segment of the stream itself kept for the served playlist.
#[derive(Debug, Clone)]
struct Kept {
    /// Its number in the served playlist (`#EXT-X-MEDIA-SEQUENCE` counts these).
    number: u64,
    segment: Segment,
    /// An ad came between it and the one before.
    after_ad: bool,
}

/// The stream's own segments as they come, without the ads.
#[derive(Debug, Default)]
struct LiveState {
    kept: VecDeque<Kept>,
    seen: HashSet<String>,
    next: u64,
    /// The lengths of all the segments kept so far.
    served: f64,
    /// An ad came since the last segment kept.
    ad: bool,
    /// The last playlist's target duration.
    target: Option<u32>,
    marks: Vec<Mark>,
    ended: bool,
}

impl LiveState {
    /// Takes in what Twitch's playlist lists now: the stream's own segments
    /// not kept yet, in order, noting where an ad came between them. The
    /// first time, only the newest: the first playlist served lists that one
    /// alone, so the recording starts with it, where its first mark says.
    fn take(&mut self, playlist: &Playlist) {
        self.target = playlist.target.or(self.target);
        self.ended |= playlist.ended;
        let from = match self.kept.back() {
            None => match playlist.segments.iter().rposition(|segment| segment.live) {
                Some(newest) => newest,
                None => return,
            },
            Some(last) => match playlist
                .segments
                .iter()
                .position(|segment| segment.key() == last.segment.key())
            {
                Some(at) => at + 1,
                // Gone from Twitch's playlist: what came between went unseen
                // (a long ad break). Only what aired after it is new.
                None => {
                    self.ad = true;
                    playlist
                        .segments
                        .iter()
                        .position(|segment| match (segment.aired, last.segment.aired) {
                            (Some(aired), Some(last)) => aired > last,
                            _ => true,
                        })
                        .unwrap_or(playlist.segments.len())
                }
            },
        };
        for segment in &playlist.segments[from..] {
            if !segment.live {
                self.ad |= self.next > 0;
                continue;
            }
            if !self.seen.insert(segment.key().to_string()) {
                continue;
            }
            let first = self.next == 0;
            if first || self.ad {
                if let Some(aired) = segment.aired {
                    self.marks.push(Mark {
                        offset: self.served,
                        aired,
                    });
                }
            }
            self.kept.push_back(Kept {
                number: self.next,
                segment: segment.clone(),
                after_ad: self.ad && !first,
            });
            self.next += 1;
            self.served += segment.duration;
            self.ad = false;
            while self.kept.len() > LIVE_WINDOW {
                if let Some(old) = self.kept.pop_front() {
                    self.seen.remove(old.segment.key());
                }
            }
        }
    }

    /// The playlist served: the last segments kept, numbered as they were
    /// first served, with a discontinuity where an ad was left out.
    fn playlist(&self) -> String {
        let segments: Vec<&Segment> = self.kept.iter().map(|kept| &kept.segment).collect();
        let mut out = String::new();
        head(
            &mut out,
            &segments,
            self.target,
            self.kept.front().map_or(0, |kept| kept.number),
        );
        let mut init = None;
        for (index, kept) in self.kept.iter().enumerate() {
            if kept.after_ad && index > 0 {
                out.push_str("#EXT-X-DISCONTINUITY\n");
            }
            segment_lines(&mut out, &kept.segment, "live", &mut init);
        }
        if self.ended {
            out.push_str("#EXT-X-ENDLIST\n");
        }
        out
    }
}

fn now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |since| since.as_secs_f64())
}

/// How a VOD being read stands: what the served playlist and Stop share.
#[derive(Debug)]
struct VodRead {
    /// Served from this segment on.
    start: usize,
    /// When the VOD's second 0 aired, about, in seconds since 1970.
    first_frame: f64,
    /// How long the VOD was when it was last read, and when it last grew.
    length: f64,
    grew: Instant,
    /// When Stop was pressed, in seconds since 1970, and as an instant.
    stop: Option<(f64, Instant)>,
}

/// Stop, for a recording read from a VOD: it records on until the VOD has
/// what aired when Stop was pressed, and then ends by itself.
#[derive(Debug, Clone)]
pub(super) struct CatchUp(Arc<Mutex<VodRead>>);

impl CatchUp {
    /// Asks it to end where Stop was pressed now. False if it was asked
    /// already: pressed again, Stop stops it at once.
    pub(super) fn ask(&self) -> bool {
        let mut read = self.0.lock().unwrap();
        if read.stop.is_some() {
            return false;
        }
        read.stop = Some((now(), Instant::now()));
        true
    }

    /// How long ago it was asked to end.
    pub(super) fn asked_for(&self) -> Option<Duration> {
        self.0.lock().unwrap().stop.map(|(_, at)| at.elapsed())
    }

    /// Once asked, how many seconds there are left to record: the VOD grows
    /// ten seconds at a time, so in between it is counted down.
    pub(super) fn left(&self) -> Option<f64> {
        let read = self.0.lock().unwrap();
        let (asked, _) = read.stop?;
        let length = read.length + read.grew.elapsed().as_secs_f64().min(10.0);
        Some((asked - read.first_frame - length).max(1.0))
    }
}

enum Source {
    Vod(Arc<Mutex<VodRead>>),
    Live(LiveState),
}

struct Shared {
    http: reqwest::Client,
    upstream: String,
    source: tokio::sync::Mutex<Source>,
    /// The live source's marks, for the chat's timing.
    marks: Arc<Mutex<Vec<Mark>>>,
    stalled: Arc<AtomicBool>,
}

/// Why a playlist couldn't be served: the status to answer with (Twitch's
/// own where it gave one, such as "not found" for a stream that dropped), and
/// what went wrong.
#[derive(Debug, PartialEq)]
struct Refusal {
    status: u16,
    why: String,
}

/// A playlist served on 127.0.0.1 while a recording part reads it. Dropping
/// it closes it.
pub(super) struct Feed {
    /// Where the recording reads it.
    pub url: String,
    /// Read from a VOD: the second of it the recording starts with.
    pub vod_start: Option<f64>,
    marks: Arc<Mutex<Vec<Mark>>>,
    catch_up: Option<CatchUp>,
    stalled: Arc<AtomicBool>,
    server: JoinHandle<()>,
}

impl Drop for Feed {
    fn drop(&mut self) {
        self.server.abort();
    }
}

async fn fetch(http: &reqwest::Client, url: &str) -> Result<Playlist, Refusal> {
    let broken = |err: reqwest::Error| Refusal {
        status: 502,
        why: err.to_string(),
    };
    let response = http.get(url).send().await.map_err(broken)?;
    let status = response.status();
    if !status.is_success() {
        return Err(Refusal {
            status: status.as_u16(),
            why: format!("Twitch answered {status}"),
        });
    }
    let text = response.text().await.map_err(broken)?;
    Ok(parse(&text, url))
}

impl Feed {
    /// The VOD whose playlist is `upstream`, from its newest segment on.
    /// `first_frame`: when its second 0 aired, about (seconds since 1970).
    pub(super) async fn vod(http: reqwest::Client, upstream: &str, first_frame: f64) -> Result<Feed, String> {
        let playlist = fetch(&http, upstream).await.map_err(|refusal| refusal.why)?;
        if playlist.ended {
            return Err("the VOD has ended".to_string());
        }
        let Some(start) = playlist.segments.len().checked_sub(1) else {
            return Err("the VOD has no segment yet".to_string());
        };
        let vod_start: f64 = playlist.segments[..start].iter().map(|segment| segment.duration).sum();
        let read = Arc::new(Mutex::new(VodRead {
            start,
            first_frame,
            length: vod_start + playlist.segments[start].duration,
            grew: Instant::now(),
            stop: None,
        }));
        let mut feed = Feed::open(http, upstream, Source::Vod(Arc::clone(&read))).await?;
        feed.vod_start = Some(vod_start);
        feed.catch_up = Some(CatchUp(read));
        Ok(feed)
    }

    /// The stream whose playlist is `upstream`, without its ads.
    pub(super) async fn live(http: reqwest::Client, upstream: &str) -> Result<Feed, String> {
        fetch(&http, upstream).await.map_err(|refusal| refusal.why)?;
        Feed::open(http, upstream, Source::Live(LiveState::default())).await
    }

    async fn open(http: reqwest::Client, upstream: &str, source: Source) -> Result<Feed, String> {
        let listener = TcpListener::bind(("127.0.0.1", 0))
            .await
            .map_err(|err| err.to_string())?;
        let port = listener.local_addr().map_err(|err| err.to_string())?.port();
        let marks = Arc::new(Mutex::new(Vec::new()));
        let stalled = Arc::new(AtomicBool::new(false));
        let shared = Arc::new(Shared {
            http,
            upstream: upstream.to_string(),
            source: tokio::sync::Mutex::new(source),
            marks: Arc::clone(&marks),
            stalled: Arc::clone(&stalled),
        });
        let server = tokio::spawn(serve(listener, shared));
        Ok(Feed {
            url: format!("http://127.0.0.1:{port}/stream.m3u8"),
            vod_start: None,
            marks,
            catch_up: None,
            stalled,
            server,
        })
    }

    /// Where the recording starts and goes on after each ad left out, as it
    /// comes (read without the ads).
    pub(super) fn marks(&self) -> Arc<Mutex<Vec<Mark>>> {
        Arc::clone(&self.marks)
    }

    /// What Stop asks of it (read from a VOD).
    pub(super) fn catch_up(&self) -> Option<CatchUp> {
        self.catch_up.clone()
    }

    /// The VOD stopped growing, and the recording was told the stream dropped.
    pub(super) fn stalled(&self) -> bool {
        self.stalled.load(Ordering::Relaxed)
    }
}

async fn serve(listener: TcpListener, shared: Arc<Shared>) {
    loop {
        let Ok((client, _)) = listener.accept().await else {
            tokio::time::sleep(Duration::from_millis(50)).await;
            continue;
        };
        tokio::spawn(answer(client, Arc::clone(&shared)));
    }
}

/// Answers one request, whatever it asks for, with the playlist as it is now.
async fn answer(mut client: TcpStream, shared: Arc<Shared>) {
    let mut head = Vec::new();
    let mut chunk = [0; 2048];
    while !head.windows(4).any(|window| window == b"\r\n\r\n") && head.len() < 16 * 1024 {
        match client.read(&mut chunk).await {
            Ok(0) | Err(_) => return,
            Ok(read) => head.extend_from_slice(&chunk[..read]),
        }
    }
    let reply = match playlist(&shared).await {
        Ok(body) => format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/vnd.apple.mpegurl\r\nContent-Length: {}\r\nCache-Control: no-cache\r\nConnection: close\r\n\r\n{body}",
            body.len()
        ),
        Err(refusal) => {
            let body = format!("Siphon couldn't serve Twitch's playlist ({})", refusal.why);
            let reason = reqwest::StatusCode::from_u16(refusal.status)
                .ok()
                .and_then(|status| status.canonical_reason())
                .unwrap_or("Error");
            format!(
                "HTTP/1.1 {} {reason}\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                refusal.status,
                body.len()
            )
        }
    };
    let _ = client.write_all(reply.as_bytes()).await;
    let _ = client.shutdown().await;
}

/// The playlist to serve now, from Twitch's as it is now.
async fn playlist(shared: &Shared) -> Result<String, Refusal> {
    let waited = Instant::now();
    loop {
        let upstream = fetch(&shared.http, &shared.upstream).await?;
        let mut source = shared.source.lock().await;
        match &mut *source {
            Source::Vod(read) => return vod_answer(read, &upstream, &shared.stalled),
            Source::Live(state) => {
                state.take(&upstream);
                shared.marks.lock().unwrap().clone_from(&state.marks);
                // The first answer waits for the stream itself: an empty
                // playlist would end the recording before it began.
                if !state.kept.is_empty() || state.ended || waited.elapsed() > FIRST_SEGMENT_WAIT {
                    return Ok(state.playlist());
                }
            }
        }
        drop(source);
        tokio::time::sleep(AD_POLL).await;
    }
}

/// The VOD's playlist to serve, from Twitch's as it is now: ended where Stop
/// was pressed once the VOD has that far, and "not found" once it has
/// stopped growing (`stalled` says so).
fn vod_answer(read: &Mutex<VodRead>, upstream: &Playlist, stalled: &AtomicBool) -> Result<String, Refusal> {
    let mut read = read.lock().unwrap();
    let length: f64 = upstream.segments.iter().map(|segment| segment.duration).sum();
    if length > read.length {
        read.length = length;
        read.grew = Instant::now();
    }
    if let Some((asked, _)) = read.stop {
        // The VOD's second that aired as Stop was pressed.
        let wanted = asked - read.first_frame;
        if length >= wanted || upstream.ended {
            return Ok(vod_playlist(upstream, read.start, Some(wanted)));
        }
    }
    if !upstream.ended && read.grew.elapsed() > VOD_STALL {
        stalled.store(true, Ordering::Relaxed);
        return Err(Refusal {
            status: 404,
            why: "the VOD has stopped growing".to_string(),
        });
    }
    Ok(vod_playlist(upstream, read.start, None))
}

/// A lookup as yt-dlp printed it, with format `format_id`'s stream at `url`
/// instead: what yt-dlp records from (`--load-info-json`). `named`: the id
/// and title it is named by, where they are another lookup's (the VOD's
/// lookup is named as the stream's, as the recording's other parts are).
///
/// yt-dlp adds the date and time to a live stream's title each time it
/// reads a lookup, so the title is the one from before that (`fulltitle`):
/// otherwise the name had the date twice. Not when it reads the stream from
/// its start, as it reads a VOD: `named`'s title is kept as it is.
pub(super) fn lookup_with_stream(
    json: &[u8],
    format_id: &str,
    url: &str,
    named: Option<(&str, &str)>,
) -> Option<Vec<u8>> {
    let mut lookup: serde_json::Value = serde_json::from_slice(json).ok()?;
    let fields = lookup.as_object_mut()?;
    let title = match named {
        Some((id, title)) => {
            fields.insert("id".into(), id.into());
            Some(title.to_string())
        }
        None => fields
            .get("fulltitle")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string),
    };
    if let Some(title) = title {
        fields.insert("title".into(), title.clone().into());
        fields.insert("fulltitle".into(), title.into());
    }
    let format = lookup
        .get_mut("formats")?
        .as_array_mut()?
        .iter_mut()
        .find(|format| format.get("format_id").and_then(serde_json::Value::as_str) == Some(format_id))?;
    format["url"] = serde_json::Value::String(url.to_string());
    if let Some(format) = format.as_object_mut() {
        // Twitch's own addresses for it: yt-dlp must not go there instead.
        format.remove("manifest_url");
        format.remove("fragment_base_url");
    }
    serde_json::to_vec(&lookup).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Shaped as Twitch sent them on 2026-11-21: a stitched ad, then the stream.
    fn live_playlist(sequence: &[(&str, &str, &str)]) -> Playlist {
        let mut text = String::from("#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:5\n");
        for (date, title, uri) in sequence {
            text.push_str(&format!(
                "#EXT-X-PROGRAM-DATE-TIME:2026-11-21T23:05:{date}Z\n#EXTINF:2.000,{title}\n{uri}\n"
            ));
        }
        parse(&text, "https://usher.example/v1/playlist/x.m3u8")
    }

    const AD: &str = "Amazon|2474283100495.0";

    #[test]
    fn reads_a_playlist_with_ads_vods_and_fragmented_mp4() {
        let playlist = live_playlist(&[("08.017", AD, "https://cdn.example/ad.ts"), ("10.017", "live", "a.ts")]);
        assert_eq!(playlist.target, Some(5));
        assert!(!playlist.segments[0].live && playlist.segments[1].live);
        assert_eq!(playlist.segments[1].uri, "https://usher.example/v1/playlist/a.ts");
        assert!((playlist.segments[1].aired.unwrap() - 1_795_302_310.017).abs() < 1e-6);
        let vod = parse(
            "#EXTM3U\n#EXT-X-TARGETDURATION:10\n#EXT-X-MAP:URI=\"init-0.mp4\"\n#EXTINF:10.000,\n0.mp4\n\
             #EXTINF:10.000,\n1-muted.mp4\n#EXTINF:4.5,\n2.mp4\n#EXT-X-ENDLIST\n",
            "https://vod.example/abc/chunked/index-dvr.m3u8",
        );
        assert!(vod.ended && vod.segments.len() == 3);
        assert_eq!(
            vod.segments[2].init.as_deref(),
            Some("https://vod.example/abc/chunked/init-0.mp4")
        );
    }

    const VOD: &str =
        "#EXTM3U\n#EXT-X-TARGETDURATION:10\n#EXTINF:10.000,\n0.ts\n#EXTINF:10.000,\n1.ts\n#EXTINF:10.000,\n2.ts\n";

    fn vod() -> Playlist {
        parse(VOD, "https://vod.example/abc/chunked/index-dvr.m3u8")
    }

    #[test]
    fn a_vod_is_served_from_its_newest_segment_as_it_grows() {
        assert_eq!(
            vod_playlist(&vod(), 2, None),
            "#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:10\n#EXT-X-MEDIA-SEQUENCE:2\n#EXT-X-PLAYLIST-TYPE:EVENT\n\
             #EXTINF:10.000,\nhttps://vod.example/abc/chunked/2.ts\n"
        );
        let ended = parse(
            &format!("{VOD}#EXTINF:10.000,\n3.ts\n#EXT-X-ENDLIST\n"),
            "https://vod.example/abc/chunked/index-dvr.m3u8",
        );
        assert!(vod_playlist(&ended, 2, None).ends_with("3.ts\n#EXT-X-ENDLIST\n"));
        // Stopped at its second 15: up to the segment holding it, and no further.
        let stopped = vod_playlist(&vod(), 0, Some(15.0));
        assert!(stopped.ends_with("/1.ts\n#EXT-X-ENDLIST\n"), "{stopped}");
        // Stopped before the first segment served: that one still.
        assert!(vod_playlist(&vod(), 2, Some(5.0)).ends_with("/2.ts\n#EXT-X-ENDLIST\n"));
    }

    fn reading(length: f64, grew_ago: u64, first_frame: f64) -> Mutex<VodRead> {
        Mutex::new(VodRead {
            start: 2,
            first_frame,
            length,
            grew: Instant::now() - Duration::from_secs(grew_ago),
            stop: None,
        })
    }

    #[test]
    fn stop_records_on_until_the_vod_has_what_aired_then() {
        let stalled = AtomicBool::new(false);
        // The VOD began airing 25 s ago, and has 30 s: what aired then is in it.
        let catch_up = CatchUp(Arc::new(reading(30.0, 0, now() - 25.0)));
        assert!(catch_up.left().is_none(), "not asked yet");
        assert!(catch_up.ask());
        assert!(!catch_up.ask(), "pressed again, it stops at once");
        let served = vod_answer(&catch_up.0, &vod(), &stalled).unwrap();
        assert!(served.ends_with("/2.ts\n#EXT-X-ENDLIST\n"), "{served}");
        // It began 60 s ago: the VOD's 30 s don't reach Stop yet.
        let catch_up = CatchUp(Arc::new(reading(30.0, 0, now() - 60.0)));
        catch_up.ask();
        let served = vod_answer(&catch_up.0, &vod(), &stalled).unwrap();
        assert!(!served.contains("ENDLIST"), "{served}");
        let left = catch_up.left().unwrap();
        assert!((29.0..=31.0).contains(&left), "{left}");
    }

    #[test]
    fn a_vod_that_stopped_growing_is_answered_as_a_dropped_stream() {
        let stalled = AtomicBool::new(false);
        let read = reading(30.0, VOD_STALL.as_secs() + 1, 0.0);
        let refused = vod_answer(&read, &vod(), &stalled).unwrap_err();
        assert_eq!(refused.status, 404);
        assert!(stalled.load(Ordering::Relaxed));
        // Grown since: served.
        let read = reading(20.0, VOD_STALL.as_secs() + 1, 0.0);
        assert!(vod_answer(&read, &vod(), &AtomicBool::new(false)).is_ok());
    }

    #[test]
    fn the_stream_is_served_without_its_ads_from_its_newest_segment() {
        let mut state = LiveState::default();
        // A new playlist: the ad Twitch opens it with.
        state.take(&live_playlist(&[("00.017", AD, "ad1"), ("02.017", AD, "ad2")]));
        assert!(state.kept.is_empty() && state.marks.is_empty());
        // The stream after it: from its newest segment.
        state.take(&live_playlist(&[
            ("02.017", AD, "ad2"),
            ("30.017", "live", "a"),
            ("32.017", "live", "b"),
        ]));
        let served = state.playlist();
        assert!(served.contains("#EXT-X-MEDIA-SEQUENCE:0\n"), "{served}");
        assert!(!served.contains("/a\n") && served.contains("/b\n"), "{served}");
        assert_eq!(
            state.marks,
            vec![Mark {
                offset: 0.0,
                aired: 1_795_302_332.017
            }]
        );
        // An ad break, then the stream again: a discontinuity, and a mark
        // where the recording goes on, after the 2 s served before it.
        state.take(&live_playlist(&[
            ("32.017", "live", "b"),
            ("34.017", AD, "ad3"),
            ("36.017", AD, "ad4"),
        ]));
        state.take(&live_playlist(&[("36.017", AD, "ad4"), ("50.017", "live", "c")]));
        let served = state.playlist();
        assert!(served.contains("/b\n#EXT-X-DISCONTINUITY\n#EXT-X-PROGRAM-DATE-TIME:2026-11-21T23:05:50.017Z\n#EXTINF:2.000,live\nhttps://usher.example/v1/playlist/c\n"), "{served}");
        assert_eq!(
            state.marks[1],
            Mark {
                offset: 2.0,
                aired: 1_795_302_350.017
            }
        );
        // Read again: nothing new, numbering unchanged.
        state.take(&live_playlist(&[("50.017", "live", "c")]));
        assert_eq!(
            state.kept.iter().map(|kept| kept.number).collect::<Vec<_>>(),
            vec![0, 1]
        );
        // A break so long the last segment kept is gone from Twitch's
        // playlist: what is newer is taken, after a jump.
        state.take(&live_playlist(&[("48.017", "live", "old"), ("58.017", "live", "d")]));
        assert!(state.playlist().ends_with("#EXT-X-DISCONTINUITY\n#EXT-X-PROGRAM-DATE-TIME:2026-11-21T23:05:58.017Z\n#EXTINF:2.000,live\nhttps://usher.example/v1/playlist/d\n"));
        assert_eq!(state.marks.len(), 3);
        assert!((state.marks[2].offset - 4.0).abs() < 1e-9);
    }

    #[test]
    fn a_long_window_keeps_its_numbering_as_it_slides() {
        let mut state = LiveState::default();
        // Each read lists the segment before too, as Twitch's 30-second window does.
        for second in 1..41_u32 {
            let (before, now) = (format!("{:02}.017", second - 1), format!("{:02}.017", second));
            let (before_uri, now_uri) = (format!("s{}", second - 1), format!("s{second}"));
            state.take(&live_playlist(&[
                (&before, "live", &before_uri),
                (&now, "live", &now_uri),
            ]));
        }
        // s1 (the newest at first) to s40: 40 kept, numbered 0 to 39.
        let served = state.playlist();
        assert!(
            served.contains(&format!("#EXT-X-MEDIA-SEQUENCE:{}\n", 40 - LIVE_WINDOW)),
            "{served}"
        );
        assert_eq!(served.matches("#EXTINF").count(), LIVE_WINDOW);
        assert!(!served.contains("DISCONTINUITY"));
        assert_eq!(state.marks.len(), 1);
    }

    #[test]
    fn yt_dlp_is_given_the_served_playlist_for_the_chosen_format() {
        let json = br#"{"id": "v1", "title": "dona (live) 2026-11-22 03:10", "fulltitle": "dona (live)", "formats": [
            {"format_id": "720p60", "url": "https://usher.example/720.m3u8"},
            {"format_id": "1080p60", "url": "https://usher.example/1080.m3u8", "manifest_url": "https://usher.example/master.m3u8"}]}"#;
        let changed = lookup_with_stream(json, "1080p60", "http://127.0.0.1:5000/stream.m3u8", None).unwrap();
        let value: serde_json::Value = serde_json::from_slice(&changed).unwrap();
        assert_eq!(value["formats"][1]["url"], "http://127.0.0.1:5000/stream.m3u8");
        assert!(value["formats"][1].get("manifest_url").is_none());
        assert_eq!(value["formats"][0]["url"], "https://usher.example/720.m3u8");
        assert_eq!(value["id"], "v1");
        assert_eq!(value["title"], "dona (live)", "yt-dlp adds the date again");
        assert!(lookup_with_stream(json, "480p", "x", None).is_none());
        // The VOD's lookup, named as the stream's.
        let named = lookup_with_stream(json, "720p60", "x", Some(("4012", "zackrawrr (live)"))).unwrap();
        let value: serde_json::Value = serde_json::from_slice(&named).unwrap();
        assert_eq!(
            (&value["id"], &value["title"]),
            (&"4012".into(), &"zackrawrr (live)".into())
        );
    }
}
