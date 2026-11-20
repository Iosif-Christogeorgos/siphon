//! Recording live streams.
//!
//! A recording waits for its stream to go on air if it hasn't yet, records
//! it as it airs into MPEG-TS files in the download folder, and once the
//! stream ends or the user stops it, makes those into the preset's file.
//!
//! Nothing recorded is ever lost to the way it ends:
//! - MPEG-TS has no index to write at the end, so a recording cut off by a
//!   crash or a power cut plays up to its last second.
//! - Stop asks yt-dlp's ffmpeg to finish (Ctrl+Break), and waits for it,
//!   rather than killing it (see `process::interrupt_tree`).
//! - Which files a recording has is written to `downloads.json` as they
//!   appear. One that Siphon's closing or a crash cut short is finished when
//!   Siphon starts again ([`DownloadManager::recover`]).
//! - The recorded files are only deleted once the finished file has been
//!   checked: as long as what was recorded, with picture and sound in step,
//!   and decoding at both ends. If it isn't, or there's no room to make it,
//!   the recording is kept as it was recorded (`.ts`, which plays too).
//!
//! A stream that stops sending (the streamer's connection dropped) is looked
//! at again for a few minutes; if it comes back, the recording carries on in
//! a new file, and the files are joined at the end.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};

use super::tracker::{LiveProgress, Progress, Stage};
use super::{
    file_size, move_file, partial_path, work_dir, ChildEvent, Done, DownloadEvent, DownloadManager, Failure, Job,
    JobContext, Lookup, Stop, EVENT,
};
use crate::binaries::BinaryManager;
use crate::error::{AppError, ErrorKind, Result, Retry};
use crate::ffmpeg::{self, encoders::Hardware, Probed, Recorded};
use crate::log;
use crate::plan::{self, Choice, Recipe, VideoAction};
use crate::process;
use crate::sessions::{Account, Sessions};
use crate::settings::SettingsStore;
use crate::state::CapabilityCache;
use crate::ytdlp::metadata::{self, LiveStatus, VideoInfo};
use crate::ytdlp::{self, args, errors, progress};

/// How long a stopped stream is given to come back before the recording is
/// finished: long enough for a streamer to reconnect.
const RECONNECT_WINDOW: Duration = Duration::from_secs(3 * 60);

/// How often a stopped stream is looked at in that time. The first look
/// waits too: a stream that has just ended may still say it is live.
const RECONNECT_CHECK: Duration = Duration::from_secs(20);

/// How long Stop waits for the recording's processes to finish their file
/// before they are killed. ffmpeg takes about a second.
pub const STOP_GRACE: Duration = Duration::from_secs(15);

/// A recording stops, and is finished, before the disk has less than this free.
const DISK_RESERVE: u64 = 1024 * 1024 * 1024;

/// How often the free space is looked at while recording.
const DISK_CHECK: Duration = Duration::from_secs(5);

/// Below this many seconds of room on the disk, the recording says so.
const DISK_WARNING: f64 = 2.0 * 3600.0;

/// How often a recording reports how it is going.
const REPORT_EVERY: Duration = Duration::from_millis(500);

/// A recording from the start counts as caught up within this many seconds of live.
const CAUGHT_UP: f64 = 30.0;

/// Parts in a row that recorded nothing before the recording gives up.
const EMPTY_PARTS: usize = 3;

/// What the frontend asks for when it starts a recording.
#[derive(Debug, Clone, Copy, Default, Deserialize)]
pub struct RecordRequest {
    /// Record what already aired too, where the site keeps it.
    #[serde(default)]
    pub from_start: bool,
}

/// A recording that isn't finished yet, as it is kept in `downloads.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Recording {
    pub url: String,
    pub title: String,
    pub thumbnail: Option<String>,
    /// What it is made into once it ends.
    pub recipe: Recipe,
    pub output_dir: PathBuf,
    /// The files recorded, one list for each stretch of the stream: a new
    /// stretch begins when the stream comes back after dropping out. A stretch
    /// recorded from now on is one MPEG-TS file; from the start, a video and
    /// an audio file until yt-dlp joins them.
    pub parts: Vec<Vec<PathBuf>>,
}

impl Recording {
    fn files(&self) -> impl Iterator<Item = &PathBuf> {
        self.parts.iter().flatten()
    }

    fn bytes(&self) -> u64 {
        self.files().map(|file| file_size(file)).sum()
    }

    /// The name its file gets, without the extension: the recorded files'
    /// name without what Siphon added (see `args::recording_stem`).
    fn stem(&self) -> String {
        self.files()
            .next()
            .and_then(|file| file.file_stem())
            .and_then(|stem| stem.to_str())
            .map(|stem| args::recording_stem(stem).to_string())
            .unwrap_or_else(|| "Recording".to_string())
    }
}

/// A recording finished after Siphon started again, for the frontend's list.
#[derive(Debug, Clone, Serialize)]
pub struct Recovered {
    pub id: String,
    pub url: String,
    pub title: String,
    pub thumbnail: Option<String>,
}

/// How one part of a recording ended on its own.
enum PartEnd {
    /// The stream ended: its playlist said so, and nothing failed.
    Over,
    /// yt-dlp finished, but the stream stopped sending: it may come back.
    Dropped,
    /// The disk is nearly full: the recording was stopped there.
    DiskFull,
}

/// The readings a recording reports, worked out from its byte count over time.
pub(super) struct Readings {
    started: Instant,
    /// When the stream went on air (Unix seconds), for how far behind a recording from the start is.
    since: Option<u64>,
    /// The stream's rate in bytes per second, when known.
    rate: Option<u64>,
    from_start: bool,
    caught_up: bool,
    samples: VecDeque<(Instant, u64)>,
}

/// How far back the bitrate is measured: streams arrive a segment at a time.
const BITRATE_WINDOW: Duration = Duration::from_secs(10);

impl Readings {
    pub(super) fn new(started: Instant, since: Option<u64>, rate: Option<u64>, from_start: bool) -> Self {
        Self {
            started,
            since,
            rate,
            from_start,
            caught_up: !from_start,
            samples: VecDeque::new(),
        }
    }

    /// What to report with `bytes` recorded at `now` (`unix` is the same moment in Unix seconds).
    pub(super) fn at(&mut self, bytes: u64, now: Instant, unix: u64) -> LiveProgress {
        self.samples.push_back((now, bytes));
        while self
            .samples
            .front()
            .is_some_and(|(at, _)| now.duration_since(*at) > BITRATE_WINDOW)
        {
            self.samples.pop_front();
        }
        let bitrate = self.samples.front().and_then(|(at, first)| {
            let seconds = now.duration_since(*at).as_secs_f64();
            (seconds >= 2.0).then(|| bytes.saturating_sub(*first) as f64 * 8.0 / seconds)
        });
        // From the start: the stream's age, less what has been recorded of it at its own rate.
        let behind = match (self.from_start && !self.caught_up, self.since, self.rate) {
            (true, Some(since), Some(rate)) if rate > 0 => {
                let behind = unix.saturating_sub(since) as f64 - bytes as f64 / rate as f64;
                if behind < CAUGHT_UP {
                    self.caught_up = true;
                    None
                } else {
                    Some(behind)
                }
            }
            _ => None,
        };
        LiveProgress {
            elapsed: now.duration_since(self.started).as_secs_f64(),
            bytes,
            bitrate,
            behind,
            ..LiveProgress::default()
        }
    }

    /// Seconds the disk still has room for at the rate recorded lately, when that is getting short.
    pub(super) fn disk_left(&self, free: u64, reading: &LiveProgress) -> Option<f64> {
        let rate = reading
            .bitrate
            .map(|bits| bits / 8.0)
            .or(self.rate.map(|rate| rate as f64))?;
        let left = free.saturating_sub(DISK_RESERVE) as f64 / rate.max(1.0);
        (left < DISK_WARNING).then_some(left)
    }
}

/// How often a recording's folder is looked at for files yt-dlp didn't name.
const FILE_LOOK: Duration = Duration::from_secs(2);

/// The files in `dir` whose names carry `needle` (see `args::recording_needle`):
/// a recording part's own, without what yt-dlp keeps beside them.
fn recording_files(dir: &Path, needle: &str) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = entries
        .flatten()
        .filter(|entry| {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            name.contains(needle) && !name.ends_with(".ytdl") && !name.contains("-Frag") && !name.ends_with(".part")
        })
        .map(|entry| entry.path())
        .collect();
    files.sort();
    files
}

/// Removes a recording's files, with what yt-dlp kept beside them (a piece
/// being fetched, its notes on a recording from the start).
async fn remove_recorded(recording: &Recording) {
    let files: Vec<PathBuf> = recording.files().cloned().collect();
    super::remove_partial_files(&files).await;
}

fn unix_now() -> u64 {
    crate::sessions::now()
}

fn report(job: &JobContext<'_>, stage: Stage, fraction: f64, live: LiveProgress) {
    job.emit(Progress {
        stage,
        fraction,
        speed: None,
        eta: None,
        live: Some(live),
    });
}

impl DownloadManager {
    /// Records the live stream at `url`: waits for it to go on air, records
    /// it until it ends or the user stops it, and makes it into its file.
    pub(super) async fn record(
        &self,
        job: &JobContext<'_>,
        url: &str,
        choice: &Choice,
        request: RecordRequest,
        account: Account,
    ) -> Result<Done> {
        self.set_graceful(job.id);
        let output_dir = job.output_dir().await?;
        let mut video = self.until_live(job, url, account).await?;
        let account = if video.with_account { Account::Use } else { account };

        // From the start where it was asked for and the site still keeps it.
        let mut from_start = false;
        if request.from_start {
            match self.formats_from_start(job, &video).await {
                Ok(formats) if !formats.is_empty() => {
                    let mut with = (*video).clone();
                    if let Some(live) = with.live.as_mut() {
                        live.from_start = Some(formats);
                    }
                    video = Arc::new(with);
                    from_start = true;
                }
                result => log::line(
                    job.app,
                    &format!(
                        "record {}: can't record from the start ({:?}); from now on instead",
                        video.title,
                        result.err().map(|e| e.message)
                    ),
                ),
            }
        }
        let caps = job.app.state::<CapabilityCache>().get(job.bins).await;
        let recipe = plan::live::plan(&video, choice, from_start, &caps)?;
        // A recording has to keep up with its stream: it goes first within the speed limit.
        if let Some(flow) = self.jobs.lock().unwrap().get(job.id).and_then(|job| job.flow.as_ref()) {
            flow.reserve(plan::live::reserve(&recipe));
        }

        let mut recording = Recording {
            url: video.url.clone(),
            title: video.title.clone(),
            thumbnail: video.thumbnail.clone(),
            recipe,
            output_dir,
            parts: Vec::new(),
        };
        self.keep_recording(job.id, &recording);
        let since = video.live.as_ref().and_then(|live| live.since);
        let mut readings = Readings::new(
            Instant::now(),
            since,
            plan::live::stream_rate(&recording.recipe),
            from_start,
        );
        log::line(
            job.app,
            &format!(
                "record {}: {} as {}",
                recording.title,
                if from_start { "from the start" } else { "from now" },
                recording.recipe.format_selector()
            ),
        );

        let mut last_error: Option<AppError> = None;
        let mut empty_parts = 0;
        let mut disk_full = false;
        let stop = loop {
            let before = recording.bytes();
            let first = recording.parts.is_empty();
            let ended = self
                .record_part(job, &video, from_start && first, &mut recording, &mut readings)
                .await;
            let recorded_some = recording.bytes() > before;
            empty_parts = if recorded_some { 0 } else { empty_parts + 1 };
            match ended {
                // Nothing to wait for. Looking again could find it still "live" for
                // a moment, and a new part would read the ended stream from its start.
                Ok(PartEnd::Over) => break Stop::Finish,
                Ok(PartEnd::Dropped) => {}
                Ok(PartEnd::DiskFull) => {
                    disk_full = true;
                    break Stop::Finish;
                }
                Err(Failure::Stopped(stop)) => break stop,
                Err(Failure::Exited(log)) => {
                    let sessions = Arc::clone(&job.app.state::<Arc<Sessions>>());
                    let error = sessions.explain(errors::classify(&log), url, video.with_account);
                    log::line(
                        job.app,
                        &format!("record {}: part ended with {}", recording.title, error.message),
                    );
                    // A refusal that won't pass (a sign-in, the bot check) isn't waited out.
                    let lasting = error.retry == Retry::No && error.kind != ErrorKind::Unavailable;
                    last_error = Some(error);
                    if lasting && !recorded_some {
                        break Stop::Finish;
                    }
                }
                Err(Failure::Spawn(error)) => {
                    last_error = Some(error);
                    break Stop::Finish;
                }
            }
            if empty_parts >= EMPTY_PARTS {
                break Stop::Finish;
            }
            match self.until_back(job, &video, account, &mut readings, &recording).await {
                Some(again) => video = again,
                None => break self.stop_of(job.id).unwrap_or(Stop::Finish),
            }
        };

        match stop {
            Stop::Cancel => {
                self.forget_recording(job.id, true).await;
                Err(AppError::cancelled())
            }
            // Siphon is closing: the recording is finished when it starts again.
            Stop::Pause => Err(AppError::cancelled()),
            Stop::Finish if recording.bytes() == 0 => {
                self.forget_recording(job.id, true).await;
                Err(last_error.unwrap_or_else(|| {
                    AppError::new(ErrorKind::Unavailable, "The stream ended before anything was recorded.")
                }))
            }
            Stop::Finish => {
                // Stop ended the recording, not the job: its finishing steps run now.
                self.clear_stop(job.id);
                let mut done = self.finalize(job, &recording).await?;
                if disk_full {
                    let saved = "The disk was nearly full, so the recording stopped there.";
                    done.note = Some(match done.note {
                        Some(note) => format!("{saved} {note}"),
                        None => saved.to_string(),
                    });
                }
                Ok(done)
            }
        }
    }

    /// The video once it is on air: at once if it is, otherwise after
    /// waiting for it, looking again from time to time.
    async fn until_live(&self, job: &JobContext<'_>, url: &str, account: Account) -> Result<Arc<VideoInfo>> {
        // What was looked up when the video was opened will do the first time.
        let mut refresh = false;
        loop {
            let lookup = Lookup { refresh, account };
            refresh = true;
            let delay = match self.video_info(job, url, lookup).await {
                Ok(video) => match video.live.as_ref().map(|live| (live.status, live.since)) {
                    Some((LiveStatus::Live, _)) => return Ok(video),
                    Some((status, since)) => {
                        let starts_at = (status == LiveStatus::Upcoming).then_some(since).flatten();
                        report(
                            job,
                            Stage::WaitingLive,
                            0.0,
                            LiveProgress {
                                starts_at,
                                ..LiveProgress::default()
                            },
                        );
                        plan::live::next_check(status, since, unix_now())
                    }
                    None => {
                        return Err(AppError::new(
                            ErrorKind::Unsupported,
                            "This isn't a live stream (any more). Download it instead.",
                        ))
                    }
                },
                // Offline for a moment: keep waiting.
                Err(error) if error.kind == ErrorKind::Network => Duration::from_secs(60),
                Err(error) => return Err(error),
            };
            if !self.wait_unless_stopped(job.id, delay).await {
                return Err(AppError::cancelled());
            }
        }
    }

    /// The formats to record `video` from its start with, looked up now.
    async fn formats_from_start(&self, job: &JobContext<'_>, video: &VideoInfo) -> Result<Vec<metadata::Format>> {
        let sessions = Arc::clone(&job.app.state::<Arc<Sessions>>());
        let cookies = if video.with_account {
            sessions.cookie_file(&video.url)
        } else {
            None
        };
        metadata::fetch_from_start(job.bins, &video.url, cookies.as_ref().map(|file| file.path())).await
    }

    /// Records one stretch of the stream, until yt-dlp ends (the stream ended
    /// or stopped sending), the user stops it, or the disk is nearly full.
    async fn record_part(
        &self,
        job: &JobContext<'_>,
        video: &VideoInfo,
        from_start: bool,
        recording: &mut Recording,
        readings: &mut Readings,
    ) -> std::result::Result<PartEnd, Failure> {
        let part = recording.parts.len() + 1;
        let mut cmd = ytdlp::command(job.bins);
        // Its own process group: Stop asks it to finish, rather than killing it.
        process::in_own_group(&mut cmd);
        cmd.args(progress::template_args())
            .args(args::record_args(&recording.recipe, from_start))
            .args(["--no-playlist", "--no-mtime"]);
        self.route(job.id, &mut cmd).map_err(Failure::Spawn)?;
        let sessions = Arc::clone(&job.app.state::<Arc<Sessions>>());
        let cookies = if video.with_account {
            sessions.cookie_file(&video.url)
        } else {
            None
        };
        self.attach_cookies(job.id, cookies);
        self.add_cookies(job.id, &mut cmd);
        let name = format!("{}{}", job.place.prefix(), job.settings.filename_template());
        cmd.arg("--paths")
            .arg(&recording.output_dir)
            .args([
                "--output",
                &args::recording_template(&name, &args::own_marker(job.id), part, from_start),
                "--",
            ])
            .arg(&video.url);

        recording.parts.push(Vec::new());
        let mut disk_full = false;
        let mut last_report: Option<Instant> = None;
        let mut last_disk_check: Option<Instant> = None;
        let mut disk_left = None;
        let output_dir = recording.output_dir.clone();
        // What this part's files have in their names, to find them on disk.
        let needle = args::recording_needle(&args::own_marker(job.id), part);
        let mut last_look: Option<Instant> = None;
        let result = self
            .run_child(job.bins, job.id, cmd, |event| match event {
                ChildEvent::Line(line) => {
                    let file = match progress::parse(line) {
                        Some(progress::Line::Target(path)) => Some(PathBuf::from(path)),
                        Some(progress::Line::Download(tick)) => tick.filename.map(PathBuf::from),
                        Some(progress::Line::File(path)) => Some(PathBuf::from(path)),
                        _ => None,
                    };
                    if let (Some(file), Some(current)) = (file, recording.parts.last_mut()) {
                        if !current.contains(&file) {
                            current.push(file);
                            // Known from its first byte, should Siphon close or crash.
                            self.keep_recording(job.id, recording);
                        }
                    }
                }
                ChildEvent::Tick => {
                    let now = Instant::now();
                    if last_report.is_some_and(|at| now.duration_since(at) < REPORT_EVERY) {
                        return;
                    }
                    last_report = Some(now);
                    // yt-dlp doesn't always say which files it writes (fetching two
                    // streams at once, it reports them on stderr): look for them too.
                    if last_look.is_none_or(|at| now.duration_since(at) >= FILE_LOOK) {
                        last_look = Some(now);
                        let found = recording_files(&output_dir, &needle);
                        if let Some(current) = recording.parts.last_mut() {
                            let new: Vec<PathBuf> = found.into_iter().filter(|file| !current.contains(file)).collect();
                            if !new.is_empty() {
                                current.extend(new);
                                self.keep_recording(job.id, recording);
                            }
                        }
                    }
                    let mut reading = readings.at(recording.bytes(), now, unix_now());
                    if last_disk_check.is_none_or(|at| now.duration_since(at) >= DISK_CHECK) {
                        last_disk_check = Some(now);
                        if let Some(free) = super::free_space(&output_dir) {
                            disk_left = readings.disk_left(free, &reading);
                            if free < DISK_RESERVE && !disk_full {
                                disk_full = true;
                                self.finish_soon(job.id);
                            }
                        }
                    }
                    reading.disk_left = disk_left;
                    report(job, Stage::Recording, 0.0, reading);
                }
            })
            .await;
        if let Some(current) = recording.parts.last_mut() {
            // One last look: a file yt-dlp started since the last one (they are
            // two seconds apart) would otherwise be left out, and left behind.
            for file in recording_files(&output_dir, &needle) {
                if !current.contains(&file) {
                    current.push(file);
                }
            }
            // A file that never got a byte isn't part of the recording.
            current.retain(|file| file_size(file) > 0);
        }
        self.keep_recording(job.id, recording);
        match result {
            Ok(log) if errors::stream_over(&log) => Ok(PartEnd::Over),
            Ok(_) => Ok(PartEnd::Dropped),
            Err(Failure::Stopped(Stop::Finish)) if disk_full => Ok(PartEnd::DiskFull),
            Err(failure) => Err(failure),
        }
    }

    /// After the stream stopped sending: waits a few minutes for it to come
    /// back. Returns the video if it is on air again, `None` if it ended (or
    /// the recording was stopped meanwhile).
    async fn until_back(
        &self,
        job: &JobContext<'_>,
        video: &VideoInfo,
        account: Account,
        readings: &mut Readings,
        recording: &Recording,
    ) -> Option<Arc<VideoInfo>> {
        let deadline = Instant::now() + RECONNECT_WINDOW;
        let reading = readings.at(recording.bytes(), Instant::now(), unix_now());
        report(job, Stage::Reconnecting, 0.0, reading);
        log::line(
            job.app,
            &format!("record {}: the stream stopped; waiting for it", video.title),
        );
        while Instant::now() < deadline {
            if !self.wait_unless_stopped(job.id, RECONNECT_CHECK).await {
                return None;
            }
            let lookup = Lookup { refresh: true, account };
            // A lookup that fails says nothing: the stream may still come back.
            if let Ok(again) = self.video_info(job, &video.url, lookup).await {
                match again.live.as_ref().map(|live| live.status) {
                    Some(LiveStatus::Live) => {
                        log::line(job.app, &format!("record {}: the stream is back", video.title));
                        return Some(again);
                    }
                    // It has ended for good: the site says it was live.
                    None => return None,
                    Some(_) => {}
                }
            }
        }
        None
    }

    /// Makes a recording into its file: joins its parts, copies or converts
    /// them as its recipe says, checks the result, and only then removes what
    /// was recorded. If there's no room to make it, it doesn't pass its
    /// checks, or anything else goes wrong, the recording is kept as it was
    /// recorded. Only Siphon closing leaves it to be finished next time.
    async fn finalize(&self, job: &JobContext<'_>, recording: &Recording) -> Result<Done> {
        report(job, Stage::Finalizing, 0.0, LiveProgress::default());
        let result = match work_dir(job.app, job.id) {
            Ok(work) => {
                let result = match tokio::fs::create_dir_all(&work).await {
                    Ok(()) => self.finalize_in(job, recording, &work).await,
                    Err(error) => Err(error.into()),
                };
                let _ = tokio::fs::remove_dir_all(&work).await;
                result
            }
            Err(error) => Err(error),
        };
        match (result, self.stop_of(job.id)) {
            (Ok(done), _) => {
                self.forget_recording(job.id, false).await;
                Ok(done)
            }
            // Cancelled: thrown away, as while recording.
            (Err(error), Some(Stop::Cancel)) => {
                self.forget_recording(job.id, true).await;
                Err(error)
            }
            // Siphon is closing: the recording stays as it was recorded, and
            // is finished when Siphon starts again.
            (Err(error), Some(_)) => Err(error),
            // A step failed in a way that would fail again (joining a recording
            // from the start, the disk): the recording is kept as it was
            // recorded, rather than tried again at every start.
            (Err(error), None) => self.keep_after_failure(job, recording, error).await,
        }
    }

    /// Keeps a recording whose finishing failed as it was recorded, file by
    /// file. If even that fails, it is forgotten (its files stay where they
    /// are), so it isn't tried again at every start.
    async fn keep_after_failure(&self, job: &JobContext<'_>, recording: &Recording, error: AppError) -> Result<Done> {
        log::line(
            job.app,
            &format!(
                "record {}: couldn't be finished ({}); keeping it as recorded",
                recording.title,
                error.details.as_deref().unwrap_or(&error.message)
            ),
        );
        let files: Vec<PathBuf> = recording.files().filter(|file| file_size(file) > 0).cloned().collect();
        if files.is_empty() {
            self.forget_recording(job.id, true).await;
            return Err(error);
        }
        match self
            .keep_as_recorded(
                job,
                recording,
                &recording.stem(),
                &files,
                "Making it into its file didn't work out.",
            )
            .await
        {
            Ok(done) => Ok(done),
            Err(_) => {
                self.forget_recording(job.id, false).await;
                Err(error)
            }
        }
    }

    async fn finalize_in(&self, job: &JobContext<'_>, recording: &Recording, work: &Path) -> Result<Done> {
        let title = &recording.title;
        // One file for each stretch of the stream.
        let mut inputs: Vec<PathBuf> = Vec::new();
        for (index, files) in recording.parts.iter().enumerate() {
            let files: Vec<PathBuf> = files.iter().filter(|file| file_size(file) > 0).cloned().collect();
            match files.len() {
                0 => {}
                1 => inputs.extend(files),
                // From the start, cut short before yt-dlp joined the video and audio.
                _ => {
                    let joined = work.join(format!("part{}.mkv", index + 1));
                    let mut cmd = process::command(job.bins.ffmpeg());
                    cmd.args(ffmpeg::join_parts_args(&files, &joined));
                    self.run_tool(job, cmd, |_| {}).await?;
                    inputs.push(joined);
                }
            }
        }
        let Some(first) = inputs.first() else {
            return Err(AppError::new(ErrorKind::Unavailable, "Nothing was recorded."));
        };

        let mut probes = Vec::new();
        for input in &inputs {
            probes.push(self.probe(job, input).await);
        }
        // How long it is, for the check and the progress: what has picture and
        // sound both (from the start, stopped while catching up, the sound may
        // have run far ahead). Several parts are each ended there (see
        // `ffmpeg::concat_list`), which also says how long they are together.
        let parts: Vec<(&Path, Option<&Probed>)> = inputs
            .iter()
            .zip(&probes)
            .map(|(input, probed)| (input.as_path(), probed.as_ref()))
            .collect();
        let (list_text, joined_length) = ffmpeg::concat_list(&parts);
        let recorded = if inputs.len() == 1 {
            probes[0].as_ref().and_then(Probed::common_length).unwrap_or(0.0)
        } else {
            joined_length
        };
        let raw_bytes: u64 = inputs.iter().map(|input| file_size(input)).sum();
        let stem = recording.stem();
        let mut recipe = recording.recipe.clone();
        recipe.duration = Some(recorded);

        // A copy needs about as much room again as the recording; a conversion usually less.
        let copies = !(recipe.encodes_video() || recipe.encodes_audio());
        let needed = if copies {
            raw_bytes + raw_bytes / 50
        } else {
            raw_bytes / 2
        };
        if super::free_space(&recording.output_dir).is_some_and(|free| free < needed + DISK_RESERVE / 4) {
            return self
                .keep_as_recorded(
                    job,
                    recording,
                    &stem,
                    &inputs,
                    "There wasn't enough room on the disk to make it into its file.",
                )
                .await;
        }

        let list = work.join("parts.txt");
        let input = if inputs.len() == 1 {
            Recorded::File(first)
        } else {
            tokio::fs::write(&list, list_text).await?;
            Recorded::List(&list)
        };
        let output = self.unique_output(
            recording.output_dir.join(format!("{stem}.{}", recipe.container.ext())),
            job.id,
        );
        let partial = partial_path(&output);
        let mut made = self.make_file(job, &recipe, input, &partial, recorded).await;
        let on_hardware = matches!(&recipe.video, Some(video) if matches!(&video.action, VideoAction::Encode(encode) if encode.hardware != Hardware::Software));
        if made.is_err() && on_hardware && self.stop_of(job.id).is_none() {
            made = self
                .make_file(job, &recipe.on_software(), input, &partial, recorded)
                .await;
        }
        let checked = match made {
            Ok(()) => self.check_finished(job, &recipe, &partial, recorded).await,
            Err(error) => Err(error.details.unwrap_or(error.message)),
        };
        // Stopped while it was being made or checked (Siphon is closing):
        // that isn't the recording's fault, so it isn't kept as recorded.
        // `finalize` leaves it to be made next time.
        if self.stop_of(job.id).is_some() {
            let _ = tokio::fs::remove_file(&partial).await;
            return Err(AppError::cancelled());
        }
        match checked {
            Ok(()) => {
                tokio::fs::rename(&partial, &output).await?;
                remove_recorded(recording).await;
                log::line(job.app, &format!("record {title}: finished, {recorded:.0} s"));
                Ok(Done {
                    path: output,
                    note: None,
                    as_recorded: false,
                    note_label: None,
                })
            }
            Err(why) => {
                let _ = tokio::fs::remove_file(&partial).await;
                log::line(job.app, &format!("record {title}: kept as recorded, because {why}"));
                self.keep_as_recorded(
                    job,
                    recording,
                    &stem,
                    &inputs,
                    "Making it into its file didn't work out.",
                )
                .await
            }
        }
    }

    /// Runs the finishing ffmpeg pass, reporting its progress.
    async fn make_file(
        &self,
        job: &JobContext<'_>,
        recipe: &Recipe,
        input: Recorded<'_>,
        output: &Path,
        recorded: f64,
    ) -> Result<()> {
        let mut cmd = process::command(job.bins.ffmpeg());
        cmd.args(ffmpeg::finalize_args(recipe, input, output));
        self.run_tool(job, cmd, |line| {
            if let Some(done) = ffmpeg::parse_progress(line) {
                let fraction = if recorded > 0.0 {
                    (done / recorded).min(1.0)
                } else {
                    0.0
                };
                report(job, Stage::Finalizing, fraction, LiveProgress::default());
            }
        })
        .await
    }

    /// Whether the made file holds what was recorded (see `Probed::matches`)
    /// and decodes at both ends. Says what is wrong if not.
    async fn check_finished(
        &self,
        job: &JobContext<'_>,
        recipe: &Recipe,
        file: &Path,
        recorded: f64,
    ) -> std::result::Result<(), String> {
        let probed = self.probe(job, file).await.ok_or("ffprobe couldn't read it")?;
        let video = recipe.video.is_some();
        let audio = recipe.audio.is_some() && !recipe.strip_audio;
        probed.matches(recorded, video, audio)?;
        for at_end in [false, true] {
            let mut cmd = process::command(job.bins.ffmpeg());
            cmd.args(ffmpeg::decode_check_args(file, at_end));
            if let Err(error) = self.run_tool(job, cmd, |_| {}).await {
                let end = if at_end { "end" } else { "start" };
                return Err(format!(
                    "its {end} doesn't decode: {}",
                    error.details.unwrap_or(error.message)
                ));
            }
        }
        Ok(())
    }

    /// Keeps a recording as it was recorded: each stretch under the finished
    /// name, as `.ts`, which plays in VLC and in Windows' own players.
    async fn keep_as_recorded(
        &self,
        job: &JobContext<'_>,
        recording: &Recording,
        stem: &str,
        inputs: &[PathBuf],
        why: &str,
    ) -> Result<Done> {
        let mut kept = Vec::new();
        // One file for each stretch of the stream, or (when even joining a
        // recording from the start failed) its picture and sound apart.
        let stretches = recording
            .parts
            .iter()
            .filter(|part| part.iter().any(|file| file_size(file) > 0))
            .count();
        let one_per_stretch = inputs.len() == stretches;
        // MPEG-TS as recorded from now on; Matroska for video and audio a recording from the start joined.
        let ext_of = |input: &PathBuf| {
            input
                .extension()
                .and_then(|ext| ext.to_str())
                .unwrap_or("ts")
                .to_string()
        };
        for (index, input) in inputs.iter().enumerate() {
            let ext = ext_of(input);
            let name = if inputs.len() == 1 {
                format!("{stem}.{ext}")
            } else {
                format!("{stem} (part {}).{ext}", index + 1)
            };
            let target = self.unique_output(recording.output_dir.join(name), job.id);
            move_file(input, &target).await?;
            kept.push(target);
        }
        // Whatever is left (a video and audio pair that was joined) has a joined copy now.
        remove_recorded(recording).await;
        self.forget_recording(job.id, false).await;
        let parts = match kept.len() {
            1 => String::new(),
            count if one_per_stretch => format!(" in {count} parts, one for each stretch of the stream"),
            count => format!(" in {count} files"),
        };
        let first = kept.remove(0);
        let ext = ext_of(&first);
        let players = if ext == "ts" {
            "VLC and Windows' own players"
        } else {
            "VLC"
        };
        Ok(Done {
            path: first,
            as_recorded: true,
            note_label: None,
            note: Some(format!(
                "{why} It's saved as it was recorded (.{ext}){parts}, which plays in {players}."
            )),
        })
    }

    /// ffprobe's reading of a file.
    async fn probe(&self, job: &JobContext<'_>, file: &Path) -> Option<Probed> {
        let mut cmd = process::command(job.bins.ffprobe());
        cmd.args(ffmpeg::probe_args(file));
        let mut json = String::new();
        self.run_tool(job, cmd, |line| json.push_str(line)).await.ok()?;
        Probed::parse(&json)
    }

    /// Runs ffmpeg or ffprobe for a recording, handing each line it prints to `on_line`.
    async fn run_tool(
        &self,
        job: &JobContext<'_>,
        cmd: tokio::process::Command,
        mut on_line: impl FnMut(&str),
    ) -> Result<()> {
        match self
            .run_child(job.bins, job.id, cmd, |event| {
                if let ChildEvent::Line(line) = event {
                    on_line(line);
                }
            })
            .await
        {
            Ok(_) => Ok(()),
            Err(Failure::Stopped(_)) => Err(AppError::cancelled()),
            Err(Failure::Exited(log)) => Err(AppError::internal("ffmpeg failed.").with_details(log)),
            Err(Failure::Spawn(error)) => Err(error),
        }
    }

    /// Marks a job as one whose processes are asked to stop rather than killed.
    fn set_graceful(&self, id: &str) {
        if let Some(job) = self.jobs.lock().unwrap().get_mut(id) {
            job.graceful = true;
        }
    }

    /// Forgets a Stop once the recording has ended, so the steps that finish it
    /// can run (`run_child` starts nothing for a job that was stopped).
    fn clear_stop(&self, id: &str) {
        if let Some(job) = self.jobs.lock().unwrap().get_mut(id) {
            if job.stop == Some(Stop::Finish) {
                job.stop = None;
            }
        }
    }

    /// Stops a recording from within (the disk is nearly full): like Stop.
    fn finish_soon(&self, id: &str) {
        let mut jobs = self.jobs.lock().unwrap();
        let Some(job) = jobs.get_mut(id) else { return };
        if job.stop.is_none() {
            job.stop = Some(Stop::Finish);
            if let Some(pid) = job.pid {
                job.interrupting = Some(super::interrupt(pid));
            }
        }
    }

    /// Writes down what a recording has so far, for after a crash.
    fn keep_recording(&self, id: &str, recording: &Recording) {
        self.recordings
            .lock()
            .unwrap()
            .insert(id.to_string(), recording.clone());
        self.save();
    }

    /// Forgets a recording that is finished, or cancelled (`delete`: its files go too).
    async fn forget_recording(&self, id: &str, delete: bool) {
        let recording = self.recordings.lock().unwrap().remove(id);
        self.save();
        if let (true, Some(recording)) = (delete, recording) {
            remove_recorded(&recording).await;
        }
    }

    /// Finishes the recordings that Siphon's closing or a crash cut short,
    /// each as a job of its own, and says which they are. Called once the
    /// frontend has restored its list, so it can show them.
    pub fn recover(self: &Arc<Self>, app: &AppHandle) -> Vec<Recovered> {
        let unfinished: Vec<(String, Recording)> = {
            let jobs = self.jobs.lock().unwrap();
            self.recordings
                .lock()
                .unwrap()
                .iter()
                .filter(|(id, _)| !jobs.contains_key(*id))
                .map(|(id, recording)| (id.clone(), recording.clone()))
                .collect()
        };
        let mut recovered = Vec::new();
        for (id, recording) in unfinished {
            recovered.push(Recovered {
                id: id.clone(),
                url: recording.url.clone(),
                title: recording.title.clone(),
                thumbnail: recording.thumbnail.clone(),
            });
            self.jobs.lock().unwrap().insert(id.clone(), Job::default());
            let manager = Arc::clone(self);
            let app = app.clone();
            tauri::async_runtime::spawn(async move {
                let bins = Arc::clone(&app.state::<Arc<BinaryManager>>());
                let job = JobContext {
                    app: &app,
                    bins: &bins,
                    id: &id,
                    settings: app.state::<SettingsStore>().get(),
                    place: Default::default(),
                };
                log::line(
                    &app,
                    &format!(
                        "record {}: finishing what was recorded before Siphon closed",
                        recording.title
                    ),
                );
                let outcome = if recording.bytes() == 0 {
                    manager.forget_recording(&id, true).await;
                    Err(AppError::new(
                        ErrorKind::Unavailable,
                        "Nothing was recorded before Siphon closed.",
                    ))
                } else {
                    manager.finalize(&job, &recording).await
                };
                let stop = manager.end_job(&id);
                let event = match (stop, outcome) {
                    (Some(Stop::Cancel), _) => DownloadEvent::Cancelled { id },
                    (_, Ok(done)) => manager.finished_event(id, done, true),
                    (_, Err(error)) => DownloadEvent::Failed { id, error },
                };
                let _ = app.emit(EVENT, event);
            });
        }
        recovered
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_the_bitrate_over_the_last_seconds() {
        let start = Instant::now();
        let mut readings = Readings::new(start, None, None, false);
        let first = readings.at(0, start, 1000);
        assert_eq!(first.bitrate, None);
        // 1 MB a second for four seconds is 8 Mbit/s.
        let later = readings.at(4_000_000, start + Duration::from_secs(4), 1004);
        assert_eq!(later.bitrate, Some(8_000_000.0));
        assert_eq!(later.elapsed, 4.0);
        assert_eq!(later.behind, None);
    }

    #[test]
    fn says_how_far_behind_a_recording_from_the_start_is_until_it_catches_up() {
        let start = Instant::now();
        // On air for an hour, at 1 MB a second.
        let mut readings = Readings::new(start, Some(10_000), Some(1_000_000), true);
        let early = readings.at(600_000_000, start, 13_600);
        // Ten minutes recorded of an hour: 50 minutes behind.
        assert_eq!(early.behind, Some(3000.0));
        let close = readings.at(3_590_000_000, start + Duration::from_secs(10), 13_610);
        assert_eq!(close.behind, None);
        // Once caught up, it stays so.
        let after = readings.at(3_600_000_000, start + Duration::from_secs(70), 13_900);
        assert_eq!(after.behind, None);
    }

    #[test]
    fn warns_when_the_disk_has_room_for_less_than_two_hours() {
        let start = Instant::now();
        let readings = Readings::new(start, None, Some(1_000_000), false);
        let reading = LiveProgress::default();
        // 1 GB kept free, plus an hour at 1 MB/s.
        let left = readings.disk_left(DISK_RESERVE + 3_600_000_000, &reading);
        assert_eq!(left, Some(3600.0));
        assert_eq!(readings.disk_left(DISK_RESERVE + 10_000_000_000, &reading), None);
    }
}

#[cfg(test)]
mod cleanup_tests {
    use super::*;

    #[tokio::test]
    async fn removing_a_recording_takes_what_yt_dlp_kept_beside_it_and_nothing_else() {
        let dir = std::env::temp_dir().join(format!("siphon-cleanup-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let video = dir.join("Show.siphon-ab12.rec1.f302.mkv");
        let kept = [
            "Show.siphon-ab12.rec1.f302.mkv.ytdl",
            "Show.siphon-ab12.rec1.f302.mkv-Frag7",
            "Show.siphon-ab12.rec1.f302.mkv.part",
        ];
        let others = [
            "Show.mkv",
            "Show.siphon-ab12.rec2.ts",
            "Another.siphon-ab12.rec1.f302.mkv-Frag7",
        ];
        for name in kept.iter().chain(&others) {
            std::fs::write(dir.join(name), b"x").unwrap();
        }
        std::fs::write(&video, b"recorded").unwrap();
        let recording = Recording {
            url: "https://example.com/live".into(),
            title: "Show".into(),
            thumbnail: None,
            recipe: serde_json::from_value(serde_json::json!({
                "video": null, "audio": null, "container": "mkv", "strip_audio": false, "reason": null,
                "promise": "", "hints": [], "duration": null, "trim": null, "source_fraction": 1.0,
                "badge": "lossless", "size_limit": null, "fits": null, "blocked": null,
                "section_request_size": null, "fallback": null, "cut_encode": null
            }))
            .unwrap(),
            output_dir: dir.clone(),
            parts: vec![vec![video.clone()]],
        };
        remove_recorded(&recording).await;
        assert!(!video.exists());
        for name in kept {
            assert!(!dir.join(name).exists(), "{name} was left");
        }
        for name in others {
            assert!(dir.join(name).exists(), "{name} was removed");
        }
        let _ = std::fs::remove_dir_all(dir);
    }
}
