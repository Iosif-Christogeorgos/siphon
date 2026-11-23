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
//! - for a stretch read from the stream's VOD: `vod-N.json`, which VOD and
//!   what is wanted from its replay, and `replay-N.json`, what it had.
//!
//! A stretch read from the stream's VOD (recorded from the start, which
//! yt-dlp reads from the VOD, or from now, which Siphon reads from it to
//! leave Twitch's ads out: see `twitch_feed`) knows which of the VOD's
//! seconds it starts with, so its chat is placed as the VOD's replay places
//! it. What was said before IRC was read comes from the replay, what was
//! said after from IRC, both on one clock.
//!
//! A stretch read from the stream without its ads knows when its first
//! segment aired, and where it jumps over each ad and when what follows
//! aired: the stream's playlist says when each segment aired.
//!
//! Otherwise (where neither can be had), when a stretch aired is found by
//! matching a frame. The first frame of a recent segment of the stream's
//! playlist is decoded and looked for among the recording's last frames.
//! The playlist is read as often as a player reads it, because Twitch starts
//! it with an ad for viewers without an account that only plays out then.
//! Decoding is exact, so the frame decodes the same in both, whatever ads
//! Twitch stitched into either. Until it is found, a stretch is placed by
//! when its file appeared, which is a few seconds out.

use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tauri::AppHandle;
use tokio::task::JoinHandle;

use super::live::recording_files;
use super::twitch_feed::Mark;
use crate::chat::irc::{self, Event};
use crate::chat::twitch::Twitch;
use crate::chat::{parse_timestamp, ChatLog, Message};
use crate::log;
use crate::process;

/// The channel's emotes and badges, in the work folder.
const SNAPSHOT: &str = "chat.json";
/// When some of the channel's emotes or badges couldn't be had as the
/// recording began, they are looked for again this often, this many times.
const SNAPSHOT_EVERY: Duration = Duration::from_secs(60);
const SNAPSHOT_TRIES: u32 = 10;
/// Everything the chat said, one event a line, in the work folder.
const EVENTS: &str = "chat.jsonl";
/// How long into a stretch read from the VOD its replay is fetched: by then
/// Twitch's replay has the last of what was said before IRC was read (a
/// message showed in the replay within three minutes of being sent,
/// 2026-11-21).
const BEFORE_AFTER: Duration = Duration::from_secs(2 * 60);
/// How many times the replay is fetched, this far apart, for a message that
/// tells its clock: a quiet channel's may have none at first.
const REPLAY_TRIES: u32 = 5;
const REPLAY_EVERY: Duration = Duration::from_secs(2 * 60);
/// How far past the last second it is wanted for a replay is fetched: past
/// the moment Record was pressed, so it meets what IRC caught; past the end
/// of a recording that never got there, so it covers its last seconds.
const REPLAY_PAST: f64 = 60.0;
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
pub(super) const CLEARED_SPAN: f64 = 120.0;
/// A message is shown this long before the moment it was sent aired, as
/// Twitch's own replay of the stream shows it: the replay counts from when
/// the VOD was created, a few seconds after the stream's first frame aired
/// (4.3, 5.0 and 5.8 s on three channels, 2026-11-21), which is also about
/// how far behind viewers watch, so reactions come just after what they
/// react to. Placed by when it aired, chat came 4–6 s later than on Twitch.
pub(super) const REPLAY_LEAD: f64 = 5.0;

/// When a moment of a recording's stretch aired.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(super) struct Aired {
    /// The moment, in the stretch file's own timestamps (seconds).
    pub at: f64,
    /// When it aired, in seconds since 1970.
    pub aired: f64,
    /// Known, rather than guessed from when the file appeared.
    pub exact: bool,
    /// The stretch is read from the stream's VOD, from this second of it:
    /// placed by the VOD's replay where it can be had, rather than by
    /// `aired`, which is about right.
    #[serde(default)]
    pub vod: Option<VodClock>,
    /// Where the stretch jumps over an ad that was left out (see
    /// `twitch_feed`), in order: from each on, it is what aired from then.
    #[serde(default)]
    pub cuts: Vec<Cut>,
}

impl Aired {
    /// Where in the stretch's file (its own timestamps) what aired at `wall`
    /// is, carried on past either end. What aired during an ad that was left
    /// out is where the file jumps over it.
    pub(super) fn file_time(&self, wall: f64) -> f64 {
        let mut points = vec![(self.at, self.aired)];
        points.extend(self.cuts.iter().map(|cut| (cut.at, cut.aired)));
        let index = points.iter().rposition(|(_, aired)| *aired <= wall).unwrap_or(0);
        let (at, aired) = points[index];
        let time = at + (wall - aired);
        points.get(index + 1).map_or(time, |(next, _)| time.min(*next))
    }
}

/// Where a stretch read from a Twitch VOD starts on it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(super) struct VodClock {
    /// The VOD's number.
    pub id: String,
    /// The VOD's second the stretch's file starts with.
    pub at: f64,
}

/// Where a stretch jumps over an ad left out, and when what follows aired.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub(super) struct Cut {
    /// In the stretch file's own timestamps.
    pub at: f64,
    /// In seconds since 1970.
    pub aired: f64,
}

/// The VOD a stretch is read from, and what is wanted from its replay.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct VodRef {
    id: String,
    /// From the stretch's first second of the VOD …
    from: f64,
    /// … to a minute past the second that aired as it began: IRC was on by
    /// then, and what both have is kept once.
    to: f64,
    /// When the replay was thought to count from (see `PartTiming::Vod`),
    /// to say how far out that was.
    guess: f64,
    /// When the stretch began to be recorded, in seconds since 1970: its
    /// replay has what was said before then [`BEFORE_AFTER`] later (see
    /// [`replay_wait`]). 0 in one kept before 0.16.2.
    #[serde(default)]
    asked: f64,
}

impl VodRef {
    /// The replay of VOD `id` for a stretch that starts with its second `at`,
    /// which aired at `aired`, begun to be recorded at `asked` (both in
    /// seconds since 1970): from that second to [`REPLAY_PAST`] past the one
    /// that aired as Record was pressed.
    fn new(id: String, at: f64, aired: f64, asked: f64) -> VodRef {
        VodRef {
            id,
            from: at,
            to: at + (asked - aired) + REPLAY_PAST,
            guess: aired - at + REPLAY_LEAD,
            asked,
        }
    }

    /// What the replay is wanted for once the stretch has been recorded,
    /// `length` seconds of it: as asked, if it got to the moment Record was
    /// pressed, and else only up to [`REPLAY_PAST`] past its last second. A
    /// recording from the start stopped while still catching up keeps only
    /// the stream's first part: in full, the replay of a stream nearly eight
    /// hours in (146,327 messages) took 142 s for a recording that kept its
    /// first 38 minutes, after a wait it didn't need (2026-11-23).
    fn for_recorded(&self, length: f64) -> VodRef {
        VodRef {
            to: self.to.min(self.from + length.max(0.0) + REPLAY_PAST),
            ..self.clone()
        }
    }

    /// When the last second the replay is wanted for aired, in seconds since
    /// 1970: as Record was pressed, unless [`VodRef::for_recorded`] ended it
    /// sooner.
    fn wanted_until(&self) -> f64 {
        self.guess - REPLAY_LEAD + self.to - REPLAY_PAST
    }
}

/// How long to wait before fetching `vod`'s replay at `now`: until it has
/// the last second it is wanted for, [`BEFORE_AFTER`] after that aired. For
/// a stretch that got to the moment Record was pressed, that is two minutes
/// after Record, as the recording itself fetches it. Fetched sooner, the
/// replay hasn't yet got what was said in the seconds before Record
/// (2026-11-22: fetched 102 s after Record, it lacked the last 25 s before
/// it, 5 to 12 messages a second; the same seconds were there later). A
/// stretch that ended before then (from the start, stopped while catching
/// up) waits only until its own last second is two minutes old: at once, if
/// that aired long ago. Timed by when things aired, not by what the replay
/// has, since a quiet chat looks the same as a replay that hasn't caught up.
fn replay_wait(vod: &VodRef, now: f64) -> Duration {
    let until = vod.asked.min(vod.wanted_until()) + BEFORE_AFTER.as_secs_f64();
    Duration::from_secs_f64((until - now).clamp(0.0, BEFORE_AFTER.as_secs_f64()))
}

/// What the VOD's replay had for a stretch read from it.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Before {
    /// When the VOD starts as its replay counts, in seconds since 1970;
    /// `None` if no one said anything to tell it by.
    origin: Option<f64>,
    /// Each at the moment it was sent, on the clock IRC's messages are on.
    messages: Vec<Message>,
}

fn vod_file(work: &Path, number: usize) -> PathBuf {
    work.join(format!("vod-{number}.json"))
}

fn replay_file(work: &Path, number: usize) -> PathBuf {
    work.join(format!("replay-{number}.json"))
}

/// A stretch of the recording as the finished file holds it.
#[derive(Debug, Clone)]
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
    pub timing: PartTiming,
}

/// Where a stretch's timing comes from.
pub(super) enum PartTiming {
    /// It is read from the stream's VOD: where on the VOD it starts, and
    /// when that aired, about (the VOD was created, less [`REPLAY_LEAD`]:
    /// its replay counts from then).
    Vod { clock: VodClock, aired: f64 },
    /// It is read from the stream without the ads: where it starts and goes
    /// on after each ad, and when that aired, as they come (see `twitch_feed`).
    Marks(Arc<Mutex<Vec<Mark>>>),
    /// It is read as yt-dlp reads the stream: by matching a frame with the
    /// stream's playlist, which says when each segment aired.
    Matched(Option<String>),
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

pub(super) fn aired_file(work: &Path, number: usize) -> PathBuf {
    work.join(format!("aired-{number}.json"))
}

impl ChatCapture {
    /// Starts reading `channel`'s chat into `work`, and keeps its emotes and
    /// badges there.
    pub(super) fn start(app: &AppHandle, channel: &str, work: &Path) -> ChatCapture {
        let mut capture = ChatCapture {
            app: app.clone(),
            work: work.to_path_buf(),
            tasks: Vec::new(),
        };
        let _ = std::fs::create_dir_all(work);

        let (app, login, snapshot) = (app.clone(), channel.to_string(), work.join(SNAPSHOT));
        capture.tasks.push(tokio::spawn(
            async move { keep_snapshot(&app, &login, &snapshot).await },
        ));

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

    /// Fetches, in the background, what the replay of the VOD that stretch
    /// `number` is read from has (see `VodRef`): its clock, and what was
    /// said from the stretch's start before IRC was read. `aired`: when the
    /// stretch's first second aired, about (see `PartTiming::Vod`).
    pub(super) fn keep_replay(&mut self, number: usize, clock: &VodClock, aired: f64) {
        let (app, work) = (self.app.clone(), self.work.clone());
        let vod = VodRef::new(clock.id.clone(), clock.at, aired, now());
        self.tasks.push(tokio::spawn(async move {
            if save_json(&vod_file(&work, number), &vod).await.is_err() {
                return;
            }
            tokio::time::sleep(BEFORE_AFTER).await;
            let mut fetched = false;
            for attempt in 0..REPLAY_TRIES {
                if attempt > 0 {
                    tokio::time::sleep(REPLAY_EVERY).await;
                }
                // Wider each time, for a quiet channel.
                let wider = VodRef {
                    to: vod.to + f64::from(attempt) * REPLAY_EVERY.as_secs_f64(),
                    ..vod.clone()
                };
                match fetch_replay(&app, &wider).await {
                    Some(replay) if replay.origin.is_some() => {
                        let _ = save_json(&replay_file(&work, number), &replay).await;
                        return;
                    }
                    Some(_) => fetched = true,
                    None => {}
                }
            }
            if fetched {
                // No one said a thing: placed by when it aired, about.
                let none = Before {
                    origin: None,
                    messages: Vec::new(),
                };
                let _ = save_json(&replay_file(&work, number), &none).await;
            }
        }));
    }

    /// Works out when a stretch of the recording aired, in the background.
    pub(super) fn watch_part(&mut self, watch: PartWatch) {
        let (app, work) = (self.app.clone(), self.work.clone());
        self.tasks
            .push(tokio::spawn(async move { find_aired(&app, &work, watch).await }));
    }

    /// Draws a stretch's chat in while it is recorded (see `live_draw`), in the background.
    pub(super) fn draw_part(&mut self, part: super::live_draw::DrawPart) {
        self.tasks
            .push(tokio::spawn(super::live_draw::draw_while_recording(part)));
    }

    /// The work folder its files are kept in.
    pub(super) fn work(&self) -> &Path {
        &self.work
    }
}

/// Keeps `login`'s emotes and badges in `snapshot`, as the recording begins.
/// Whatever couldn't be had is looked for again (see [`SNAPSHOT_EVERY`]),
/// keeping the most complete set so far, so a moment's failure doesn't cost
/// the whole recording its emotes.
async fn keep_snapshot(app: &AppHandle, login: &str, snapshot: &Path) {
    // Kept from the first time: a stream that came back has the same emotes.
    if snapshot.exists() {
        return;
    }
    let mut fewest_missing = usize::MAX;
    for attempt in 0..SNAPSHOT_TRIES {
        if attempt > 0 {
            tokio::time::sleep(SNAPSHOT_EVERY).await;
        }
        let fetched = async {
            let twitch = Twitch::new()?;
            let channel_id = twitch.channel_id(login).await?;
            Ok::<_, crate::error::AppError>(ChatLog::of_channel(&twitch, &channel_id).await)
        }
        .await;
        let missing = match fetched {
            Ok((log, missing)) => {
                if missing.len() < fewest_missing && log.save(snapshot).await.is_ok() {
                    fewest_missing = missing.len();
                }
                if missing.is_empty() {
                    return;
                }
                missing.join(", ")
            }
            Err(error) => format!("emotes or badges ({})", error.message),
        };
        log::line(app, &format!("chat #{login}: no {missing} yet"));
    }
}

/// Places the stretch once its file has appeared, as its timing allows.
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
    match watch.timing {
        PartTiming::Vod { clock, aired } => {
            let timing = Aired {
                at: start,
                aired,
                exact: true,
                vod: Some(clock),
                cuts: Vec::new(),
            };
            save_aired(&target, &timing).await;
        }
        PartTiming::Marks(marks) => follow_marks(&target, start, &marks).await,
        PartTiming::Matched(playlist) => {
            let guess = Aired {
                at: start,
                aired: appeared - GUESSED_DELAY,
                exact: false,
                vod: None,
                cuts: Vec::new(),
            };
            save_aired(&target, &guess).await;
            if let Some(playlist) = playlist {
                match_aired(
                    app,
                    work,
                    &watch.ffmpeg,
                    &watch.ffprobe,
                    watch.number,
                    &file,
                    &guess,
                    &playlist,
                )
                .await;
            }
        }
    }
}

/// Keeps the timing of a stretch read without the ads as its marks come
/// (see `twitch_feed::Mark`): where it starts, then where it goes on after
/// each ad. Until the recording ends.
async fn follow_marks(target: &Path, start: f64, marks: &Mutex<Vec<Mark>>) {
    let mut known = 0;
    loop {
        let marks: Vec<Mark> = marks.lock().unwrap().clone();
        if let (Some(first), true) = (marks.first(), marks.len() > known) {
            known = marks.len();
            let timing = Aired {
                at: start + first.offset,
                aired: first.aired,
                exact: true,
                vod: None,
                cuts: marks[1..]
                    .iter()
                    .map(|mark| Cut {
                        at: start + mark.offset,
                        aired: mark.aired,
                    })
                    .collect(),
            };
            save_aired(target, &timing).await;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

/// Finds when the stretch aired by matching a frame with the stream's
/// playlist, in place of `guess`.
#[allow(clippy::too_many_arguments)]
async fn match_aired(
    app: &AppHandle,
    work: &Path,
    ffmpeg: &Path,
    ffprobe: &Path,
    number: usize,
    file: &Path,
    guess: &Aired,
    playlist: &str,
) {
    let target = aired_file(work, number);
    let Ok(twitch) = Twitch::new() else { return };
    let started = Instant::now();
    let mut last_try: Option<Instant> = None;
    while started.elapsed() < MATCH_FOR {
        let live = live_segments(twitch.http(), playlist).await;
        // A few of the stream's own segments: past the ad, and one old enough
        // to be in the recording already.
        if live.len() >= 3 && last_try.is_none_or(|at| at.elapsed() >= MATCH_EVERY) {
            last_try = Some(Instant::now());
            if let Some(found) = match_frame(twitch.http(), ffmpeg, ffprobe, work, &live, file).await {
                log::line(
                    app,
                    &format!(
                        "chat: stretch {number} aired {:.2} s after it was guessed to",
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
        &format!("chat: stretch {number} placed by when it began, give or take a few seconds"),
    );
}

async fn save_aired(target: &Path, aired: &Aired) {
    let _ = save_json(target, aired).await;
}

/// Writes `value` to `target` whole: beside it first, then renamed over it,
/// so a crash never leaves half a file.
pub(super) async fn save_json(target: &Path, value: &impl Serialize) -> std::io::Result<()> {
    let json = serde_json::to_vec(value)?;
    let partial = target.with_extension("part");
    tokio::fs::write(&partial, json).await?;
    tokio::fs::rename(&partial, target).await
}

pub(super) async fn load_json<T: serde::de::DeserializeOwned>(file: &Path) -> Option<T> {
    serde_json::from_slice(&tokio::fs::read(file).await.ok()?).ok()
}

/// Fetches what the VOD's replay has for a stretch read from it, and says
/// what it fetched. `None` if it couldn't be had.
async fn fetch_replay(app: &AppHandle, vod: &VodRef) -> Option<Before> {
    let started = Instant::now();
    let fetched = async { Twitch::new()?.comments(&vod.id, vod.from, vod.to, |_| {}).await };
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
        log::line(
            app,
            &format!(
                "chat: the replay of VOD {} has nothing from its second {:.0} to {:.0} yet",
                vod.id, vod.from, vod.to
            ),
        );
        return Some(Before {
            origin: None,
            messages: Vec::new(),
        });
    };
    let messages: Vec<Message> = replay
        .messages
        .into_iter()
        .map(|mut message| {
            message.time += origin;
            message
        })
        .collect();
    log::line(
        app,
        &format!(
            "chat: {} messages from VOD {}'s replay from its second {:.0} to {:.0} ({} gaps), its clock {:+.2} s from the guess, {:.1} s",
            messages.len(),
            vod.id,
            vod.from,
            vod.to,
            replay.gaps,
            origin - vod.guess,
            started.elapsed().as_secs_f64()
        ),
    );
    Some(Before {
        origin: Some(origin),
        messages,
    })
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
    // A fragmented MP4 segment is read after its initialisation segment.
    let fetch = |url: &str| {
        let request = http.get(url);
        async move { request.send().await?.error_for_status()?.bytes().await }
    };
    let mut bytes = Vec::new();
    if let Some(init) = &segment.init {
        bytes.extend_from_slice(&fetch(init).await.ok()?);
    }
    bytes.extend_from_slice(&fetch(&segment.url).await.ok()?);
    let local = work.join(if segment.init.is_some() {
        "frame.mp4"
    } else {
        "frame.ts"
    });
    tokio::fs::write(&local, &bytes).await.ok()?;
    let wanted = first_frame_md5(ffmpeg, &local).await;
    let _ = tokio::fs::remove_file(&local).await;
    let wanted = wanted?;
    let (start, length) = file_span(ffprobe, file).await?;
    let frames = frame_md5s(ffmpeg, file, start, start + length - MATCH_WINDOW).await;
    let at = frames.iter().find(|(_, md5)| *md5 == wanted)?.0;
    Some(Aired {
        at,
        aired: segment.aired,
        exact: true,
        vod: None,
        cuts: Vec::new(),
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
    /// Its initialisation segment, which has to come first for it to be
    /// read: Twitch sends some channels' streams as fragmented MP4 (CMAF),
    /// whose segments are only pieces without it (found 2026-11-22).
    init: Option<String>,
}

/// The segments of a media playlist that say when they aired. Twitch marks
/// the stream's own as `live` (`#EXTINF:2.000,live`); its ads carry the
/// advertiser's name.
fn segments(playlist: &str, base: &str) -> Vec<Segment> {
    let absolute = |link: &str| {
        if link.starts_with("http") {
            link.to_string()
        } else {
            let dir = base.rsplit_once('/').map_or(base, |(dir, _)| dir);
            format!("{dir}/{link}")
        }
    };
    let mut segments = Vec::new();
    let mut aired = None;
    let mut live = false;
    let mut init = None;
    for line in playlist.lines().map(str::trim) {
        if let Some(date) = line.strip_prefix("#EXT-X-PROGRAM-DATE-TIME:") {
            aired = parse_timestamp(date);
        } else if let Some(info) = line.strip_prefix("#EXTINF:") {
            live = info.split_once(',').is_some_and(|(_, title)| title == "live");
        } else if let Some(map) = line.strip_prefix("#EXT-X-MAP:") {
            // `URI="…"`, perhaps with a byte range after it.
            init = map
                .split_once("URI=\"")
                .and_then(|(_, rest)| rest.split_once('"'))
                .map(|(link, _)| absolute(link));
        } else if !line.is_empty() && !line.starts_with('#') {
            if let Some(aired) = aired.take() {
                segments.push(Segment {
                    aired,
                    url: absolute(line),
                    live,
                    init: init.clone(),
                });
            }
        }
    }
    segments
}

/// Where a file's timestamps start and how long it is, as ffprobe reads it
/// (a recording still growing too).
pub(super) async fn file_span(ffprobe: &Path, file: &Path) -> Option<(f64, f64)> {
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

/// Each video frame of `file` (whose timestamps start at `start`) from its
/// own timestamp `from` on: its time in the file's own timestamps, and the
/// MD5 of its picture.
async fn frame_md5s(ffmpeg: &Path, file: &Path, start: f64, from: f64) -> Vec<(f64, String)> {
    let mut cmd = process::command(ffmpeg);
    cmd.args([
        "-v",
        "error",
        "-nostdin",
        "-copyts",
        "-ss",
        &crate::ffmpeg::seek_from(start, from),
        "-i",
    ])
    .arg(file)
    // Every frame, at its own time. Otherwise ffmpeg makes the frames even
    // at the rate the recording starts with, which is that of Twitch's ad
    // (30 a second): every other frame of a 60-frame stream was left out,
    // the one looked for among them, and its time was rounded to 1/30 s
    // (found 2026-11-22: no match in ten minutes).
    .args([
        "-map",
        "0:v:0",
        "-fps_mode",
        "passthrough",
        "-enc_time_base",
        "demux",
        "-f",
        "framemd5",
        "-",
    ]);
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
/// A stretch read from the VOD is placed as the VOD's replay places it
/// (fetched now if it wasn't while recording: see [`replay_for`]), which
/// also has what was said before IRC was read. `fetching` is told when the
/// replay has to be fetched, which can take up to [`BEFORE_AFTER`].
pub(super) async fn chat_for_file(
    app: &AppHandle,
    work: &Path,
    stretches: &[Stretch],
    fetching: impl Fn(),
) -> Option<ChatLog> {
    let events = tokio::fs::read_to_string(work.join(EVENTS)).await.unwrap_or_default();
    let mut placed: Vec<(Stretch, Aired)> = Vec::new();
    let mut replays = HashMap::new();
    let fetching = &fetching;
    for stretch in stretches {
        let Some(aired) = load_json::<Aired>(&aired_file(work, stretch.number)).await else {
            continue;
        };
        if aired.vod.is_some() {
            let length = stretch.length;
            let fetch = |vod: VodRef| async move {
                let vod = vod.for_recorded(length);
                fetching();
                let wait = replay_wait(&vod, now());
                if !wait.is_zero() {
                    log::line(
                        app,
                        &format!(
                            "chat: waiting {:.0} s for the replay of VOD {} to have what was said before Record",
                            wait.as_secs_f64(),
                            vod.id
                        ),
                    );
                    tokio::time::sleep(wait).await;
                }
                fetch_replay(app, &vod).await
            };
            if let Some(replay) = replay_for(work, stretch.number, fetch).await {
                replays.insert(stretch.number, replay);
            }
        }
        placed.push((stretch.clone(), aired));
    }
    if placed.is_empty() {
        return None;
    }
    let snapshot = ChatLog::load(&work.join(SNAPSHOT)).await.unwrap_or_default();
    let messages = arrange(said(&events), &replays, placed);
    (!messages.is_empty()).then_some(ChatLog { messages, ..snapshot })
}

/// The replay of the VOD stretch `number` is read from (see `VodRef`): as
/// the recording kept it, or else fetched now with `fetch` and kept in
/// `work` beside it. The recording fetches it [`BEFORE_AFTER`] in, and a
/// recording stopped sooner ends that fetch, so `fetch` waits for that
/// moment first (see [`replay_wait`]). Drawing the stretch reads the replay
/// only from `work` (`chat_for_stretch`): until 0.16.2 the one fetched here
/// wasn't kept, and a recording from the start stopped within two minutes
/// had none of the chat from before Record drawn in (2026-11-22). A replay
/// with nothing to tell its clock by isn't kept: finishing the recording
/// again looks again.
async fn replay_for<F, Fut>(work: &Path, number: usize, fetch: F) -> Option<Before>
where
    F: FnOnce(VodRef) -> Fut,
    Fut: std::future::Future<Output = Option<Before>>,
{
    if let Some(replay) = load_json::<Before>(&replay_file(work, number)).await {
        return Some(replay);
    }
    let vod = load_json::<VodRef>(&vod_file(work, number)).await?;
    let replay = fetch(vod).await?;
    if replay.origin.is_some() {
        let _ = save_json(&replay_file(work, number), &replay).await;
    }
    Some(replay)
}

/// The chat of stretch `number` of a recording that is still going on, on
/// the stretch file's own clock (its first timestamp `start`), for drawing it
/// in as it records (see `live_draw`); `None` without its timing or any
/// message. As `chat_for_file` places it, but each stretch on its own.
pub(super) async fn chat_for_stretch(work: &Path, number: usize, start: f64) -> Option<ChatLog> {
    let events = tokio::fs::read_to_string(work.join(EVENTS)).await.unwrap_or_default();
    let aired = load_json::<Aired>(&aired_file(work, number)).await?;
    let mut replays = HashMap::new();
    if let Some(replay) = load_json::<Before>(&replay_file(work, number)).await {
        replays.insert(number, replay);
    }
    let stretch = Stretch {
        number,
        start,
        // Not over yet: everything after its start.
        length: f64::INFINITY,
        // On its own clock.
        offset: start,
    };
    let messages = arrange(said(&events), &replays, vec![(stretch, aired)]);
    let snapshot = ChatLog::load(&work.join(SNAPSHOT)).await.unwrap_or_default();
    (!messages.is_empty()).then_some(ChatLog { messages, ..snapshot })
}

/// When stretch `number` aired, once that is as known as it will be: for a
/// stretch read from the VOD, once its replay is in (see `arrange`);
/// otherwise once the frame match found it, or gave up. A stretch read
/// without the ads gets more of it as the ads come.
pub(super) async fn timing_settled(work: &Path, number: usize) -> Option<Aired> {
    let aired = load_json::<Aired>(&aired_file(work, number)).await?;
    let settled = if aired.vod.is_some() {
        replay_file(work, number).exists()
    } else {
        aired.exact
    };
    settled.then_some(aired)
}

/// The chat on the finished file's clock: what IRC caught, and the replays'
/// messages from before it, each in the stretch that was airing then. A
/// stretch read from the VOD is placed as its replay places it.
fn arrange(said: Vec<Message>, replays: &HashMap<usize, Before>, mut placed: Vec<(Stretch, Aired)>) -> Vec<Message> {
    for (stretch, aired) in &mut placed {
        let origin = replays.get(&stretch.number).and_then(|replay| replay.origin);
        if let (Some(clock), Some(origin)) = (&aired.vod, origin) {
            // The file starts with the VOD's second `clock.at`, which the
            // replay shows at `origin` and that many seconds: placed
            // `REPLAY_LEAD` later, that is where it shows.
            aired.at = stretch.start;
            aired.aired = origin - REPLAY_LEAD + clock.at;
        }
    }
    let before = replays
        .values()
        .flat_map(|replay| replay.messages.iter().cloned())
        .collect();
    place(merged(said, before), &placed)
}

/// What IRC caught, and the replay's messages from before IRC's first one:
/// IRC has every message (the replay keeps at most 15 a second) and the
/// notices. A message both have is kept once.
fn merged(mut said: Vec<Message>, before: Vec<Message>) -> Vec<Message> {
    let first = said.iter().map(|message| message.time).fold(f64::INFINITY, f64::min);
    let mut known: HashSet<String> = said.iter().map(|message| message.id.clone()).collect();
    for message in before {
        if message.time < first && known.insert(message.id.clone()) {
            said.push(message);
        }
    }
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
/// recorded (the stream had dropped) are left out; those sent during an ad
/// that was left out show where the recording jumps over it.
fn place(messages: Vec<Message>, stretches: &[(Stretch, Aired)]) -> Vec<Message> {
    let mut placed: Vec<Message> = messages
        .into_iter()
        .filter_map(|mut message| {
            let time = stretches.iter().find_map(|(stretch, aired)| {
                let into = aired.file_time(message.time - REPLAY_LEAD) - stretch.start;
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
        assert!(found.iter().all(|segment| segment.init.is_none()));

        // Fragmented MP4, as Twitch sent some channels on 2026-11-22: each
        // segment goes with the initialisation segment before it.
        let cmaf = "#EXTM3U
#EXT-X-VERSION:6
#EXT-X-MAP:URI=\"https://cdn.example/ad-init.mp4\"
            #EXT-X-PROGRAM-DATE-TIME:2026-11-21T23:05:08.017Z
#EXTINF:2.000,Amazon|2474283100495.0
            https://cdn.example/ad.mp4
#EXT-X-DISCONTINUITY
#EXT-X-MAP:URI=\"init.mp4\",BYTERANGE=\"720@0\"
            #EXT-X-PROGRAM-DATE-TIME:2026-11-21T23:05:40.017Z
#EXTINF:2.000,live
live.mp4
";
        let found = segments(cmaf, "https://usher.example/v1/playlist/x.m3u8");
        assert_eq!(found[0].init.as_deref(), Some("https://cdn.example/ad-init.mp4"));
        assert_eq!(
            found[1].init.as_deref(),
            Some("https://usher.example/v1/playlist/init.mp4")
        );
        assert!(found[1].live);
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

    fn timing(at: f64, aired: f64) -> Aired {
        Aired {
            at,
            aired,
            exact: true,
            vod: None,
            cuts: Vec::new(),
        }
    }

    fn times(placed: &[Message]) -> Vec<(&str, f64)> {
        placed.iter().map(|m| (m.id.as_str(), m.time)).collect()
    }

    fn assert_times(placed: &[Message], expected: &[(&str, f64)]) {
        let times = times(placed);
        assert_eq!(times.len(), expected.len(), "{times:?}");
        for ((id, time), (want_id, want_time)) in times.iter().zip(expected) {
            assert_eq!(id, want_id, "{times:?}");
            assert!((time - want_time).abs() < 1e-9, "{id}: {time}");
        }
    }

    /// Sent a little after what it reacts to aired, as Twitch's replay shows it.
    fn sent(aired: f64) -> f64 {
        aired + REPLAY_LEAD
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
                timing(11.4, 1010.0),
            ),
            (
                Stretch {
                    number: 2,
                    start: 0.0,
                    length: 50.0,
                    offset: 100.0,
                },
                Aired {
                    exact: false,
                    ..timing(0.0, 1160.0)
                },
            ),
        ];
        let placed = place(
            vec![
                message("before", "a", sent(999.0)),
                message("early", "a", sent(1000.5)),
                message("gap", "a", sent(1130.0)),
                message("later", "a", sent(1170.0)),
            ],
            &stretches,
        );
        assert_times(&placed, &[("early", 0.5), ("later", 110.0)]);
    }

    #[test]
    fn where_an_ad_was_left_out_the_chat_jumps_with_the_picture() {
        // From 1.4 the file is what aired from 1000; at 31.4 it jumps over a
        // 60-second ad to what aired from 1090.
        let stretch = Stretch {
            number: 1,
            start: 1.4,
            length: 100.0,
            offset: 0.0,
        };
        let aired = Aired {
            cuts: vec![Cut {
                at: 31.4,
                aired: 1090.0,
            }],
            ..timing(1.4, 1000.0)
        };
        let placed = place(
            vec![
                message("before", "a", sent(1010.0)),
                message("during", "a", sent(1050.0)),
                message("after", "a", sent(1100.0)),
            ],
            &[(stretch, aired.clone())],
        );
        // Said during the ad: where the file jumps over it.
        assert_times(&placed, &[("before", 10.0), ("during", 30.0), ("after", 40.0)]);
        // And back: what aired at a moment of the file.
        assert!((aired.file_time(1095.0) - 36.4).abs() < 1e-9);
    }

    fn from_vod(start: f64, at: f64, aired: f64) -> (Stretch, Aired) {
        (
            Stretch {
                number: 1,
                start,
                length: 1000.0,
                offset: 0.0,
            },
            Aired {
                vod: Some(VodClock { id: "1".into(), at }),
                ..timing(start, aired)
            },
        )
    }

    #[test]
    fn from_the_start_the_replay_gives_what_was_said_before_record_and_the_timing() {
        // The stream began at 1000.0; its VOD's replay counts from 1005.7.
        let replay = Before {
            origin: Some(1005.7),
            messages: vec![
                message("early", "a", 1010.7),
                // IRC has it too, a moment apart on the clock.
                message("both", "b", 1300.2),
                // After IRC's first message: IRC's own are kept.
                message("late", "c", 1500.7),
            ],
        };
        let said = vec![message("both", "b", 1300.7), message("irc", "d", 1400.7)];
        let replays = HashMap::from([(1, replay)]);
        let placed = arrange(said.clone(), &replays, vec![from_vod(64.066, 0.0, 1000.0)]);
        // As the replay places them: seconds since its origin.
        assert_times(&placed, &[("early", 5.0), ("both", 295.0), ("irc", 395.0)]);
        // Without the replay, by when it aired, about.
        let placed = arrange(said, &HashMap::new(), vec![from_vod(64.066, 0.0, 1000.0)]);
        assert!((placed[0].time - (1300.7 - REPLAY_LEAD - 1000.0)).abs() < 1e-9);
    }

    #[test]
    fn from_now_from_the_vod_the_replay_places_it_from_the_second_it_starts_with() {
        // Read from the VOD's second 3600: the replay shows that at
        // origin + 3600, so a message sent then shows at the file's start.
        let replays = HashMap::from([(
            1,
            Before {
                origin: Some(1005.7),
                messages: vec![message("replay", "a", 1005.7 + 3610.0)],
            },
        )]);
        let said = vec![message("irc", "b", 1005.7 + 3700.0)];
        let placed = arrange(said, &replays, vec![from_vod(1.4, 3600.0, 4601.0)]);
        assert_times(&placed, &[("replay", 10.0), ("irc", 100.0)]);
    }

    #[tokio::test]
    async fn a_replay_fetched_as_the_recording_is_finished_is_drawn_in_too() {
        // A recording from the start stopped before its replay was fetched
        // (2026-11-22): the work folder says which VOD, when it aired, and
        // what IRC caught after Record, but has no replay.
        let work = std::env::temp_dir().join(format!("siphon-replay-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&work);
        std::fs::create_dir_all(&work).unwrap();
        let vod = |number: usize| VodRef {
            id: format!("{number}"),
            from: 0.0,
            to: 900.0,
            guess: 1005.0,
            asked: 1000.0,
        };
        save_json(&vod_file(&work, 1), &vod(1)).await.unwrap();
        save_json(&aired_file(&work, 1), &from_vod(1.4, 0.0, 1000.0).1)
            .await
            .unwrap();
        let irc = serde_json::to_string(&Event::Message(message("irc", "b", 1800.0))).unwrap();
        std::fs::write(work.join(EVENTS), format!("{irc}\n")).unwrap();
        // Without the replay, only what IRC caught would be drawn.
        let ids = |log: ChatLog| log.messages.into_iter().map(|m| m.id).collect::<Vec<_>>();
        assert_eq!(ids(chat_for_stretch(&work, 1, 1.4).await.unwrap()), ["irc"]);

        // Finishing fetches it …
        let replay = Before {
            origin: Some(1005.7),
            messages: vec![message("before record", "a", 1010.7)],
        };
        let fetched = replay_for(&work, 1, |asked| {
            assert_eq!(asked.id, "1");
            async move { Some(replay) }
        })
        .await;
        assert!(fetched.is_some());
        // … and drawing the stretch, which reads it from the work folder, has it.
        assert_eq!(
            ids(chat_for_stretch(&work, 1, 1.4).await.unwrap()),
            ["before record", "irc"]
        );
        // Kept: finishing again doesn't fetch it again.
        let mut asked = false;
        let again = replay_for(&work, 1, |_| {
            asked = true;
            async { None }
        })
        .await;
        assert!(again.is_some() && !asked);

        // A replay with nothing to tell its clock by isn't kept, so it is looked for again.
        save_json(&vod_file(&work, 2), &vod(2)).await.unwrap();
        let quiet = replay_for(&work, 2, |_| async {
            Some(Before {
                origin: None,
                messages: Vec::new(),
            })
        })
        .await;
        assert!(quiet.is_some() && !replay_file(&work, 2).exists());
        std::fs::remove_dir_all(&work).unwrap();
    }

    #[test]
    fn a_replay_is_fetched_no_sooner_than_two_minutes_after_record() {
        let vod = |asked: f64| VodRef {
            id: "1".into(),
            from: 0.0,
            to: 900.0,
            guess: 1005.0,
            asked,
        };
        // Stopped 30 s after Record: the rest of the two minutes.
        assert_eq!(replay_wait(&vod(1000.0), 1030.0), Duration::from_secs(90));
        // Two minutes on, or finished after a restart: at once.
        assert_eq!(replay_wait(&vod(1000.0), 1120.0), Duration::ZERO);
        assert_eq!(replay_wait(&vod(1000.0), 5000.0), Duration::ZERO);
        // Kept before 0.16.2, without the time: at once.
        let old: VodRef = serde_json::from_str(r#"{"id":"1","from":0.0,"to":900.0,"guess":1005.0}"#).unwrap();
        assert_eq!(replay_wait(&old, 1030.0), Duration::ZERO);
        // A clock set back since: never longer than the two minutes.
        assert_eq!(replay_wait(&vod(5000.0), 1000.0), BEFORE_AFTER);
    }

    #[test]
    fn a_recording_stopped_while_catching_up_wants_only_the_replay_of_what_it_kept() {
        let record = 1_000_000.0;
        // From the start of a stream 28,000 s (7.8 h) in, whose first second aired then.
        let vod = VodRef::new("1".into(), 0.0, record - 28_000.0, record);
        assert_eq!(vod.to, 28_060.0);
        // As the recording itself fetches it: all of it, two minutes after Record.
        assert_eq!(replay_wait(&vod, record + 68.0), Duration::from_secs(52));

        // Stopped 68 s after Record with its first 1,430 s (2026-11-23): only
        // those, and at once, since they aired hours ago.
        let kept = vod.for_recorded(1430.0);
        assert_eq!((kept.from, kept.to), (0.0, 1490.0));
        assert_eq!(replay_wait(&kept, record + 68.0), Duration::ZERO);

        // Caught up to Record and past it: as asked, so it meets what IRC
        // caught, and two minutes after Record, as since 0.16.2.
        let caught_up = vod.for_recorded(28_090.0);
        assert_eq!(caught_up.to, vod.to);
        assert_eq!(replay_wait(&caught_up, record + 68.0), Duration::from_secs(52));

        // A stream 15 minutes in, stopped 50 s short of the moment of Record:
        // to a minute past its last second, once that is two minutes old.
        let short = VodRef::new("2".into(), 0.0, record - 900.0, record).for_recorded(850.0);
        assert_eq!(short.to, 910.0);
        assert_eq!(replay_wait(&short, record + 60.0), Duration::from_secs(10));
    }
}
