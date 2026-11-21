//! A recording's chat (see `crate::chat`): Twitch's chat read as the stream
//! airs, and when the recording aired, so every message can be placed in the
//! finished file.
//!
//! Everything is written to the recording's work folder as it comes, which
//! is kept until the recording is finished, so neither a crash nor Siphon
//! closing loses any of it:
//! - `chat.json`: the channel, its emotes and badges, as the recording began;
//! - `chat.jsonl`: every message, deletion and timeout, as it came;
//! - `aired-N.json`: when a moment of the recording's Nth stretch aired;
//! - from the start: `vod.json`, the VOD the stream is saved to, and
//!   `replay.json`, what its replay has from before the recording began.
//!
//! Recorded from the start, a recording is the stream's VOD from its first
//! frame (yt-dlp records Twitch from the start from it), so its chat is
//! placed as the VOD's replay places it: what was said before Record comes
//! from the replay, what was said after from IRC, both on one clock.
//!
//! Recorded from now, when a stretch aired is found by matching a frame. The stream's playlist
//! says when each of its two-second segments aired; the first frame of a
//! recent one is decoded and looked for among the recording's last frames.
//! The playlist is read as often as a player reads it, because Twitch starts
//! it with an ad for viewers without an account that only plays out then.
//! Decoding is exact, so the frame decodes the same in both, whatever ads
//! Twitch stitched into either. Until it is found, a stretch is placed by
//! when its file appeared, which is a few seconds out.

use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tauri::AppHandle;
use tokio::task::JoinHandle;

use super::live::recording_files;
use crate::chat::irc::{self, Event};
use crate::chat::twitch::Twitch;
use crate::chat::{emotes, parse_timestamp, ChatLog, Message};
use crate::log;
use crate::process;

/// The channel's emotes and badges, in the work folder.
const SNAPSHOT: &str = "chat.json";
/// Everything the chat said, one event a line, in the work folder.
const EVENTS: &str = "chat.jsonl";
/// Recorded from the start: the stream's VOD, looked up as the recording
/// began (Twitch stops naming a stream's VOD once the stream ends).
const VOD: &str = "vod.json";
/// Recorded from the start: what the chat said before the recording began.
const BEFORE: &str = "replay.json";
/// How long into a recording from the start its chat from before Record is
/// fetched: by then Twitch's replay has the last of it (a message showed in
/// the replay within three minutes of being sent, 2026-11-21).
const BEFORE_AFTER: Duration = Duration::from_secs(2 * 60);
/// A recording's file appears about this long after what it starts with aired:
/// ffmpeg starts three segments back from the newest, which is itself behind.
const GUESSED_DELAY: f64 = 8.0;
/// How often the stream's playlist is read while the frame is looked for: as
/// often as a player reads it. Twitch starts a new playlist with an ad for
/// viewers without an account, and the ad only plays out while the playlist
/// is read (2026-11-21: read every 15 s, a playlist was still on its ad after
/// five minutes; read every 3 s, its 30-second ad ended on time).
const PLAYLIST_EVERY: Duration = Duration::from_secs(2);
/// How often the frame is looked for once the playlist lists the stream
/// itself, and for how long in all before giving up.
const MATCH_EVERY: Duration = Duration::from_secs(15);
const MATCH_FOR: Duration = Duration::from_secs(10 * 60);
/// How far back from the end of the recording the frame is looked for.
const MATCH_WINDOW: f64 = 40.0;
/// A timed-out user's messages from this long before go with them, as in
/// Twitch's own chat, which takes away what is still on screen.
const CLEARED_SPAN: f64 = 120.0;
/// A message is shown this long before the moment it was sent aired, as
/// Twitch's own replay of the stream shows it: the replay counts from when
/// the VOD was created, a few seconds after the stream's first frame aired
/// (4.3, 5.0 and 5.8 s on three channels, 2026-11-21), which is also about
/// how far behind viewers watch, so reactions come just after what they
/// react to. Placed by when it aired, chat came 4–6 s later than on Twitch.
pub(super) const REPLAY_LEAD: f64 = 5.0;

/// When a moment of a recording's stretch aired.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub(super) struct Aired {
    /// The moment, in the stretch file's own timestamps (seconds).
    pub at: f64,
    /// When it aired, in seconds since 1970.
    pub aired: f64,
    /// Found by matching a frame, rather than guessed from when the file appeared.
    pub exact: bool,
    /// The stretch is the stream's VOD from its first frame (recorded from the
    /// start): placed by the VOD's replay where it can be had, rather than by
    /// `aired`, the stream's start to the second.
    #[serde(default)]
    pub vod_start: bool,
}

/// The VOD a stream recorded from the start is saved to, and what is wanted
/// from its replay.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct VodRef {
    id: String,
    /// When the stream began, in seconds since 1970.
    since: f64,
    /// When the recording began: the replay is wanted up to here.
    until: f64,
}

/// What the chat said before a recording from the start began, from the
/// VOD's replay.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Before {
    /// When the VOD starts as its replay counts, in seconds since 1970.
    origin: f64,
    /// Each at the moment it was sent, on the clock IRC's messages are on.
    messages: Vec<Message>,
}

/// A stretch of the recording as the finished file holds it.
pub(super) struct Stretch {
    /// Its number (1 for the first), as `aired-N.json` names it.
    pub number: usize,
    /// Where its file's timestamps start.
    pub start: f64,
    /// How much of it the file keeps, in seconds.
    pub length: f64,
    /// Where it starts in the finished file.
    pub offset: f64,
}

/// What a stretch's timing is worked out from.
pub(super) struct PartWatch {
    pub number: usize,
    /// Where the stretch's file is, and what its name carries.
    pub dir: PathBuf,
    pub needle: String,
    pub ffmpeg: PathBuf,
    pub ffprobe: PathBuf,
    /// The stream's playlist, which says when each segment aired.
    pub playlist: Option<String>,
    /// Recorded from the start: when the stream began, which the file starts with.
    pub began: Option<f64>,
}

/// The tasks keeping a recording's chat; they end with it.
pub(super) struct ChatCapture {
    app: AppHandle,
    work: PathBuf,
    tasks: Vec<JoinHandle<()>>,
}

impl Drop for ChatCapture {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
    }
}

fn now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |since| since.as_secs_f64())
}

fn aired_file(work: &Path, number: usize) -> PathBuf {
    work.join(format!("aired-{number}.json"))
}

impl ChatCapture {
    /// Starts reading `channel`'s chat into `work`, and keeps its emotes and
    /// badges there. `from_start`: the recording is from the stream's start,
    /// which began then (seconds since 1970); what was said before is fetched
    /// from its VOD's replay.
    pub(super) fn start(app: &AppHandle, channel: &str, work: &Path, from_start: Option<f64>) -> ChatCapture {
        let mut capture = ChatCapture {
            app: app.clone(),
            work: work.to_path_buf(),
            tasks: Vec::new(),
        };
        let _ = std::fs::create_dir_all(work);

        if let Some(since) = from_start {
            let (app, login, work) = (app.clone(), channel.to_string(), work.to_path_buf());
            let began = now();
            capture.tasks.push(tokio::spawn(async move {
                // Kept from the first time: Siphon restarted while recording.
                if work.join(VOD).exists() {
                    return;
                }
                let Ok(twitch) = Twitch::new() else { return };
                let vod = match twitch.live_vod(&login).await {
                    Ok(Some(id)) => VodRef {
                        id,
                        since,
                        until: began,
                    },
                    Ok(None) => {
                        log::line(&app, &format!("chat #{login}: the stream keeps no VOD to read back"));
                        return;
                    }
                    Err(error) => {
                        log::line(
                            &app,
                            &format!("chat #{login}: its VOD couldn't be found: {}", error.message),
                        );
                        return;
                    }
                };
                if save_json(&work.join(VOD), &vod).await.is_err() {
                    return;
                }
                tokio::time::sleep(BEFORE_AFTER).await;
                fetch_before(&app, &work, &vod).await;
            }));
        }

        let (app, login, snapshot) = (app.clone(), channel.to_string(), work.join(SNAPSHOT));
        capture.tasks.push(tokio::spawn(async move {
            // Kept from the first time: a stream that came back has the same emotes.
            if snapshot.exists() {
                return;
            }
            let fetched = async {
                let twitch = Twitch::new()?;
                let channel = twitch.channel(&login).await?;
                let ((emote_list, _), badges) =
                    tokio::join!(emotes::fetch(twitch.http(), &channel.id), twitch.badges(&channel.id));
                Ok::<_, crate::error::AppError>(ChatLog {
                    channel,
                    messages: Vec::new(),
                    emotes: emote_list,
                    badges: badges.unwrap_or_default(),
                })
            }
            .await;
            match fetched {
                Ok(log) => {
                    let _ = log.save(&snapshot).await;
                }
                Err(error) => log::line(&app, &format!("chat #{login}: no emotes or badges: {}", error.message)),
            }
        }));

        let (app, login, events) = (capture.app.clone(), channel.to_string(), work.join(EVENTS));
        capture.tasks.push(tokio::spawn(async move {
            let opened = std::fs::OpenOptions::new().create(true).append(true).open(&events);
            let Ok(mut file) = opened else {
                log::line(&app, &format!("chat #{login}: can't keep it: {}", events.display()));
                return;
            };
            let mut joined = false;
            let status_app = app.clone();
            let status_login = login.clone();
            irc::listen(
                &login,
                |event| {
                    if let Ok(mut line) = serde_json::to_vec(&event) {
                        line.push(b'\n');
                        let _ = file.write_all(&line);
                    }
                },
                |status| match status {
                    Ok(()) if !joined => {
                        joined = true;
                        log::line(&status_app, &format!("chat #{status_login}: reading it"));
                    }
                    Ok(()) => {}
                    Err(why) => log::line(&status_app, &format!("chat #{status_login}: {why}")),
                },
            )
            .await;
        }));
        capture
    }

    /// Works out when a stretch of the recording aired, in the background.
    pub(super) fn watch_part(&mut self, watch: PartWatch) {
        let (app, work) = (self.app.clone(), self.work.clone());
        self.tasks
            .push(tokio::spawn(async move { find_aired(&app, &work, watch).await }));
    }
}

/// Places the stretch: a guess as soon as its file appears, then the frame match.
async fn find_aired(app: &AppHandle, work: &Path, watch: PartWatch) {
    let target = aired_file(work, watch.number);
    let file = loop {
        if let Some(file) = recording_files(&watch.dir, &watch.needle).into_iter().next() {
            break file;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    };
    let appeared = now();
    // The file's first timestamps, once it has some.
    let start = loop {
        if let Some((start, _)) = file_span(&watch.ffprobe, &file).await {
            break start;
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    };
    let guess = match watch.began {
        // From the start, the file starts where the stream did (see `chat_for_file`).
        Some(began) => Aired {
            at: start,
            aired: began,
            exact: true,
            vod_start: true,
        },
        None => Aired {
            at: start,
            aired: appeared - GUESSED_DELAY,
            exact: false,
            vod_start: false,
        },
    };
    save_aired(&target, &guess).await;
    if guess.exact {
        return;
    }
    let Some(playlist) = watch.playlist else { return };
    let Ok(twitch) = Twitch::new() else { return };
    let started = Instant::now();
    let mut last_try: Option<Instant> = None;
    while started.elapsed() < MATCH_FOR {
        let live = live_segments(twitch.http(), &playlist).await;
        // A few of the stream's own segments: past the ad, and one old enough
        // to be in the recording already.
        if live.len() >= 3 && last_try.is_none_or(|at| at.elapsed() >= MATCH_EVERY) {
            last_try = Some(Instant::now());
            if let Some(found) = match_frame(twitch.http(), &watch.ffmpeg, &watch.ffprobe, work, &live, &file).await {
                log::line(
                    app,
                    &format!(
                        "chat: stretch {} aired {:.2} s after it was guessed to",
                        watch.number,
                        found.aired - found.at - (guess.aired - guess.at)
                    ),
                );
                save_aired(&target, &found).await;
                return;
            }
        }
        tokio::time::sleep(PLAYLIST_EVERY).await;
    }
    log::line(
        app,
        &format!(
            "chat: stretch {} placed by when it began, give or take a few seconds",
            watch.number
        ),
    );
}

async fn save_aired(target: &Path, aired: &Aired) {
    let _ = save_json(target, aired).await;
}

/// Writes `value` to `target` whole: beside it first, then renamed over it,
/// so a crash never leaves half a file.
async fn save_json(target: &Path, value: &impl Serialize) -> std::io::Result<()> {
    let json = serde_json::to_vec(value)?;
    let partial = target.with_extension("part");
    tokio::fs::write(&partial, json).await?;
    tokio::fs::rename(&partial, target).await
}

async fn load_json<T: serde::de::DeserializeOwned>(file: &Path) -> Option<T> {
    serde_json::from_slice(&tokio::fs::read(file).await.ok()?).ok()
}

/// Fetches what the chat said before a recording from the start began, from
/// the VOD's replay, into the work folder, and says what it fetched.
async fn fetch_before(app: &AppHandle, work: &Path, vod: &VodRef) -> Option<Before> {
    let started = Instant::now();
    let fetched = async {
        let twitch = Twitch::new()?;
        // A minute past the recording's start: IRC was on by then, and what
        // both have is kept once.
        twitch
            .comments(&vod.id, 0.0, (vod.until - vod.since + 60.0).max(0.0), |_| {})
            .await
    };
    let replay = match fetched.await {
        Ok(replay) => replay,
        Err(error) => {
            log::line(
                app,
                &format!("chat: the replay of VOD {} couldn't be had: {}", vod.id, error.message),
            );
            return None;
        }
    };
    let Some(origin) = replay.origin else {
        log::line(app, &format!("chat: the replay of VOD {} has nothing yet", vod.id));
        return None;
    };
    let before = Before {
        origin,
        messages: replay
            .messages
            .into_iter()
            .map(|mut message| {
                message.time += origin;
                message
            })
            .collect(),
    };
    log::line(
        app,
        &format!(
            "chat: {} messages from before the recording, from VOD {} ({} gaps), its start {:.2} s after the stream's, {:.1} s",
            before.messages.len(),
            vod.id,
            replay.gaps,
            origin - vod.since,
            started.elapsed().as_secs_f64()
        ),
    );
    save_json(&work.join(BEFORE), &before).await.ok()?;
    Some(before)
}

/// The stream's own segments that `playlist` lists now (none while it shows
/// an ad, or when it can't be read).
async fn live_segments(http: &reqwest::Client, playlist: &str) -> Vec<Segment> {
    let read = async { http.get(playlist).send().await?.error_for_status()?.text().await };
    match read.await {
        Ok(text) => segments(&text, playlist)
            .into_iter()
            .filter(|segment| segment.live)
            .collect(),
        Err(_) => Vec::new(),
    }
}

/// One try at the frame match: a recent live segment's first frame, looked
/// for among the recording's last frames.
async fn match_frame(
    http: &reqwest::Client,
    ffmpeg: &Path,
    ffprobe: &Path,
    work: &Path,
    live: &[Segment],
    file: &Path,
) -> Option<Aired> {
    // The third newest: old enough to be in the recording already.
    let segment = live.iter().rev().nth(2).or_else(|| live.first())?;
    let bytes = http.get(&segment.url).send().await.ok()?.bytes().await.ok()?;
    let local = work.join("frame.ts");
    tokio::fs::write(&local, &bytes).await.ok()?;
    let wanted = first_frame_md5(ffmpeg, &local).await;
    let _ = tokio::fs::remove_file(&local).await;
    let wanted = wanted?;
    let (start, length) = file_span(ffprobe, file).await?;
    let from = (start + length - MATCH_WINDOW).max(start);
    let frames = frame_md5s(ffmpeg, file, from).await;
    let at = frames.iter().find(|(_, md5)| *md5 == wanted)?.0;
    Some(Aired {
        at,
        aired: segment.aired,
        exact: true,
        vod_start: false,
    })
}

/// A segment of a live HLS playlist.
#[derive(Debug, Clone, PartialEq)]
struct Segment {
    /// When it aired (its program date), in seconds since 1970.
    aired: f64,
    url: String,
    /// The stream itself, not one of the ads Twitch stitches in.
    live: bool,
}

/// The segments of a media playlist that say when they aired. Twitch marks
/// the stream's own as `live` (`#EXTINF:2.000,live`); its ads carry the
/// advertiser's name.
fn segments(playlist: &str, base: &str) -> Vec<Segment> {
    let mut segments = Vec::new();
    let mut aired = None;
    let mut live = false;
    for line in playlist.lines().map(str::trim) {
        if let Some(date) = line.strip_prefix("#EXT-X-PROGRAM-DATE-TIME:") {
            aired = parse_timestamp(date);
        } else if let Some(info) = line.strip_prefix("#EXTINF:") {
            live = info.split_once(',').is_some_and(|(_, title)| title == "live");
        } else if !line.is_empty() && !line.starts_with('#') {
            if let Some(aired) = aired.take() {
                let url = if line.starts_with("http") {
                    line.to_string()
                } else {
                    let dir = base.rsplit_once('/').map_or(base, |(dir, _)| dir);
                    format!("{dir}/{line}")
                };
                segments.push(Segment { aired, url, live });
            }
        }
    }
    segments
}

/// Where a file's timestamps start and how long it is, as ffprobe reads it
/// (a recording still growing too).
async fn file_span(ffprobe: &Path, file: &Path) -> Option<(f64, f64)> {
    let mut cmd = process::command(ffprobe);
    cmd.args([
        "-v",
        "error",
        "-show_entries",
        "format=start_time,duration",
        "-of",
        "json",
    ])
    .arg(file);
    let output = process::output_within(cmd, Duration::from_secs(30)).await.ok()??;
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).ok()?;
    let number = |key: &str| value["format"][key].as_str()?.parse::<f64>().ok();
    Some((number("start_time")?, number("duration")?))
}

/// The MD5 of `file`'s first decoded video frame.
async fn first_frame_md5(ffmpeg: &Path, file: &Path) -> Option<String> {
    let mut cmd = process::command(ffmpeg);
    cmd.args(["-v", "error", "-nostdin", "-i"])
        .arg(file)
        .args(["-map", "0:v:0", "-frames:v", "1", "-f", "md5", "-"]);
    let output = process::output_within(cmd, Duration::from_secs(60)).await.ok()??;
    let text = String::from_utf8_lossy(&output.stdout);
    text.trim().strip_prefix("MD5=").map(str::to_string)
}

/// Each video frame of `file` from `from` seconds on: its time in the file's
/// own timestamps, and the MD5 of its picture.
async fn frame_md5s(ffmpeg: &Path, file: &Path, from: f64) -> Vec<(f64, String)> {
    let mut cmd = process::command(ffmpeg);
    cmd.args(["-v", "error", "-nostdin", "-copyts", "-ss", &format!("{from:.3}"), "-i"])
        .arg(file)
        .args(["-map", "0:v:0", "-f", "framemd5", "-"]);
    let Ok(Some(output)) = process::output_within(cmd, Duration::from_secs(120)).await else {
        return Vec::new();
    };
    parse_framemd5(&String::from_utf8_lossy(&output.stdout))
}

/// Reads ffmpeg's framemd5 output: `#tb 0: 1/90000`, then lines of
/// `stream, dts, pts, duration, size, md5`.
fn parse_framemd5(text: &str) -> Vec<(f64, String)> {
    let mut timebase = None;
    let mut frames = Vec::new();
    for line in text.lines() {
        if let Some(base) = line.strip_prefix("#tb 0: ") {
            let (numerator, denominator) = base.trim().split_once('/').unwrap_or(("1", "1"));
            timebase = numerator
                .parse::<f64>()
                .ok()
                .zip(denominator.parse::<f64>().ok())
                .map(|(n, d)| n / d);
            continue;
        }
        if line.starts_with('#') {
            continue;
        }
        let fields: Vec<&str> = line.split(',').map(str::trim).collect();
        if let ([_, _, pts, _, _, md5], Some(timebase)) = (fields.as_slice(), timebase) {
            if let Ok(pts) = pts.parse::<f64>() {
                frames.push((pts * timebase, (*md5).to_string()));
            }
        }
    }
    frames
}

/// The recording's chat placed on the finished file's clock, from what the
/// capture kept in `work`; `None` if it kept no message or no stretch could
/// be placed. Deleted messages and timed-out users' messages are left out.
/// Recorded from the start, what was said before Record comes from the
/// VOD's replay (fetched now if it wasn't while recording), and the stretch
/// that is the VOD from its start is placed as the replay places it.
pub(super) async fn chat_for_file(app: &AppHandle, work: &Path, stretches: &[Stretch]) -> Option<ChatLog> {
    let events = tokio::fs::read_to_string(work.join(EVENTS)).await.unwrap_or_default();
    let before = match load_json::<Before>(&work.join(BEFORE)).await {
        Some(before) => Some(before),
        None => match load_json::<VodRef>(&work.join(VOD)).await {
            Some(vod) => fetch_before(app, work, &vod).await,
            None => None,
        },
    };
    let mut placed: Vec<(Stretch, Aired)> = Vec::new();
    for stretch in stretches {
        let Some(aired) = load_json::<Aired>(&aired_file(work, stretch.number)).await else {
            continue;
        };
        placed.push((
            Stretch {
                number: stretch.number,
                start: stretch.start,
                length: stretch.length,
                offset: stretch.offset,
            },
            aired,
        ));
    }
    if placed.is_empty() {
        return None;
    }
    let snapshot = ChatLog::load(&work.join(SNAPSHOT)).await.unwrap_or_default();
    let messages = arrange(said(&events), before, placed);
    (!messages.is_empty()).then_some(ChatLog { messages, ..snapshot })
}

/// The chat on the finished file's clock: what IRC caught, and the replay's
/// messages from before it, each in the stretch that was airing then. A
/// stretch that is the VOD from its start is placed as the replay places it.
fn arrange(said: Vec<Message>, before: Option<Before>, mut placed: Vec<(Stretch, Aired)>) -> Vec<Message> {
    if let Some(before) = &before {
        for (stretch, aired) in placed.iter_mut().filter(|(_, aired)| aired.vod_start) {
            // The file starts with the VOD's first frame, where the replay
            // counts from: placed `REPLAY_LEAD` later, that is where it shows.
            aired.at = stretch.start;
            aired.aired = before.origin - REPLAY_LEAD;
        }
    }
    let said = merged(said, before.map(|before| before.messages).unwrap_or_default());
    place(said, &placed)
}

/// What IRC caught, and the replay's messages from before IRC's first one:
/// IRC has every message (the replay keeps at most 15 a second) and the
/// notices. A message both have is kept once.
fn merged(mut said: Vec<Message>, before: Vec<Message>) -> Vec<Message> {
    let first = said.iter().map(|message| message.time).fold(f64::INFINITY, f64::min);
    let known: HashSet<String> = said.iter().map(|message| message.id.clone()).collect();
    said.extend(
        before
            .into_iter()
            .filter(|message| message.time < first && !known.contains(&message.id)),
    );
    said
}

/// What the chat said, with deleted messages and timed-out users' recent
/// messages taken away, each message at the moment it was sent.
fn said(events: &str) -> Vec<Message> {
    let mut messages: HashMap<String, Message> = HashMap::new();
    let mut deleted = HashSet::new();
    let mut cleared: Vec<(String, f64)> = Vec::new();
    // A line cut short by a crash is skipped.
    for event in events
        .lines()
        .filter_map(|line| serde_json::from_str::<Event>(line).ok())
    {
        match event {
            Event::Message(message) => {
                messages.entry(message.id.clone()).or_insert(message);
            }
            Event::Delete { id } => {
                deleted.insert(id);
            }
            Event::ClearUser { login, at } => cleared.push((login, at)),
        }
    }
    messages
        .into_values()
        .filter(|message| !deleted.contains(&message.id))
        .filter(|message| {
            !cleared
                .iter()
                .any(|(login, at)| *login == message.login && message.time <= *at && message.time >= at - CLEARED_SPAN)
        })
        .collect()
}

/// Each message moved from when it was sent to where it shows in the
/// finished file ([`REPLAY_LEAD`] before what was airing then), in the
/// stretch that was airing then. Messages sent while nothing was being
/// recorded (the stream had dropped) are left out.
fn place(messages: Vec<Message>, stretches: &[(Stretch, Aired)]) -> Vec<Message> {
    let mut placed: Vec<Message> = messages
        .into_iter()
        .filter_map(|mut message| {
            let time = stretches.iter().find_map(|(stretch, aired)| {
                // When the stretch's file starts, on the clock.
                let began = aired.aired - (aired.at - stretch.start);
                let into = message.time - REPLAY_LEAD - began;
                (0.0..=stretch.length).contains(&into).then_some(stretch.offset + into)
            })?;
            message.time = time;
            Some(message)
        })
        .collect();
    placed.sort_by(|a, b| a.time.total_cmp(&b.time).then_with(|| a.id.cmp(&b.id)));
    placed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat::Part;

    fn message(id: &str, login: &str, time: f64) -> Message {
        Message {
            id: id.into(),
            time,
            name: login.into(),
            login: login.into(),
            color: None,
            badges: Vec::new(),
            parts: vec![Part::Text { text: "hi".into() }],
            notice: false,
        }
    }

    #[test]
    fn reads_when_each_segment_aired_and_which_are_ads() {
        // Shaped as Twitch sent it on 2026-11-21: a stitched ad, then the stream.
        let playlist = "#EXTM3U\n#EXT-X-TARGETDURATION:5\n#EXT-X-DISCONTINUITY\n\
            #EXT-X-PROGRAM-DATE-TIME:2026-11-21T17:55:46.654Z\n#EXTINF:2.000,Amazon|2474283100495.0\n\
            https://cdn.example/ad.ts\n\
            #EXT-X-PROGRAM-DATE-TIME:2026-11-21T17:56:48.889Z\n#EXTINF:2.000,live\n\
            https://cdn.example/a.ts\n\
            #EXT-X-PROGRAM-DATE-TIME:2026-11-21T17:56:50.889Z\n#EXTINF:2.000,live\nb.ts\n";
        let found = segments(playlist, "https://usher.example/v1/playlist/x.m3u8");
        assert_eq!(found.len(), 3);
        assert!(!found[0].live && found[1].live);
        assert!((found[1].aired - 1_795_283_808.889).abs() < 1e-6);
        assert_eq!(
            found[2].url, "https://usher.example/v1/playlist/b.ts",
            "relative to the playlist"
        );
    }

    #[test]
    fn reads_frame_md5s_in_seconds() {
        let text = "#format: frame checksums\n#version: 2\n#hash: MD5\n#tb 0: 1/90000\n\
            #media_type 0: video\n#stream#, dts, pts, duration, size, hash\n\
            0,     900000,     900000,     1500,  3110400, 0c3d2f1e\n\
            0,     901500,     901500,     1500,  3110400, 9a8b7c6d\n";
        assert_eq!(
            parse_framemd5(text),
            vec![
                (10.0, "0c3d2f1e".to_string()),
                (10.016_666_666_666_667, "9a8b7c6d".to_string())
            ]
        );
        assert!(
            parse_framemd5("0, 1, 1, 1, 1, abc").is_empty(),
            "no time base, no times"
        );
    }

    #[test]
    fn deleted_and_timed_out_messages_are_left_out() {
        let lines: Vec<String> = [
            Event::Message(message("1", "kind", 100.0)),
            Event::Message(message("2", "rude", 150.0)),
            Event::Message(message("3", "rude", 20.0)),
            Event::Message(message("4", "other", 160.0)),
            Event::Message(message("4", "other", 160.0)),
            Event::ClearUser {
                login: "rude".into(),
                at: 170.0,
            },
            Event::Delete { id: "4".into() },
        ]
        .iter()
        .map(|event| serde_json::to_string(event).unwrap())
        .collect();
        let text = format!("{}\n{{\"message\":{{\"id\":\"cut sh", lines.join("\n"));
        let mut kept: Vec<String> = said(&text).into_iter().map(|m| m.id).collect();
        kept.sort();
        // "3" was long before the timeout, so it stays, as in Twitch's chat.
        assert_eq!(kept, vec!["1", "3"]);
    }

    #[test]
    fn places_messages_in_the_stretch_that_was_airing() {
        // Two stretches: the stream dropped for a minute between them.
        let stretches = vec![
            (
                Stretch {
                    number: 1,
                    start: 1.4,
                    length: 100.0,
                    offset: 0.0,
                },
                Aired {
                    at: 11.4,
                    aired: 1010.0,
                    exact: true,
                    vod_start: false,
                },
            ),
            (
                Stretch {
                    number: 2,
                    start: 0.0,
                    length: 50.0,
                    offset: 100.0,
                },
                Aired {
                    at: 0.0,
                    aired: 1160.0,
                    exact: false,
                    vod_start: false,
                },
            ),
        ];
        // Each sent a little after what it reacts to aired, as Twitch's replay shows it.
        let sent = |aired: f64| aired + REPLAY_LEAD;
        let placed = place(
            vec![
                message("before", "a", sent(999.0)),
                message("early", "a", sent(1000.5)),
                message("gap", "a", sent(1130.0)),
                message("later", "a", sent(1170.0)),
            ],
            &stretches,
        );
        let times: Vec<(&str, f64)> = placed.iter().map(|m| (m.id.as_str(), m.time)).collect();
        assert_eq!(times, vec![("early", 0.5), ("later", 110.0)]);
    }

    #[test]
    fn from_the_start_the_replay_gives_what_was_said_before_record_and_the_timing() {
        // The stream began at 1000.0; its VOD's replay counts from 1005.7.
        let stretch = || Stretch {
            number: 1,
            start: 64.066,
            length: 1000.0,
            offset: 0.0,
        };
        let aired = Aired {
            at: 64.066,
            aired: 1000.0,
            exact: true,
            vod_start: true,
        };
        let before = Before {
            origin: 1005.7,
            messages: vec![
                message("early", "a", 1010.7),
                // IRC has it too, a moment apart on the clock.
                message("both", "b", 1300.2),
                // After IRC's first message: IRC's own are kept.
                message("late", "c", 1500.7),
            ],
        };
        let said = vec![message("both", "b", 1300.7), message("irc", "d", 1400.7)];
        let placed = arrange(said.clone(), Some(before), vec![(stretch(), aired)]);
        let times: Vec<(&str, f64)> = placed.iter().map(|m| (m.id.as_str(), m.time)).collect();
        // As the replay places them: seconds since its origin.
        let expected = [("early", 5.0), ("both", 295.0), ("irc", 395.0)];
        assert_eq!(times.len(), expected.len(), "{times:?}");
        for ((id, time), (want_id, want_time)) in times.iter().zip(expected) {
            assert_eq!(*id, want_id);
            assert!((time - want_time).abs() < 1e-9, "{id}: {time}");
        }
        // Without the replay, by the stream's start (to the second), as before.
        let placed = arrange(said, None, vec![(stretch(), aired)]);
        assert!((placed[0].time - (1300.7 - REPLAY_LEAD - 1000.0)).abs() < 1e-9);
    }
}
