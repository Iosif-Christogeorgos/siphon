//! Folds yt-dlp's per-stream ticks into one overall, never-decreasing progress value.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::ytdlp::progress::{DownloadTick, PlannedStream};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    /// Waiting for a free slot in the queue.
    Queued,
    /// The site refused the download; yt-dlp is being updated before a second try.
    Updating,
    /// The last try failed in a way that may pass; waiting a moment before the next.
    Retrying,
    Preparing,
    DownloadingVideo,
    DownloadingAudio,
    /// A single stream that already contains both video and audio.
    Downloading,
    Merging,
    Converting,
    /// Fetching subtitles after the video, or adding them to copied streams.
    Subtitles,
    /// Fetching the chat after the video (see `crate::chat`).
    Chat,
    /// Encoding the video with its chat drawn in.
    DrawingChat,
    Finishing,
    /// A live recording waits for its stream to start.
    WaitingLive,
    /// A live recording is being recorded (or, from the start, catching up).
    Recording,
    /// A live stream stopped sending; the recording waits for it to come back.
    Reconnecting,
    /// A live recording has ended and is being made into its file.
    Finalizing,
}

#[derive(Debug, Clone, Serialize)]
pub struct Progress {
    pub stage: Stage,
    /// Overall completion, 0–1.
    pub fraction: f64,
    /// Bytes per second.
    pub speed: Option<f64>,
    /// Seconds remaining for the current stream.
    pub eta: Option<f64>,
    /// A live recording's own readings, in place of a fraction and time left.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub live: Option<LiveProgress>,
    /// While Siphon's ffmpeg makes the file: the size it is heading for, in
    /// bytes, from what it has written so far (see `Tracker::heading_for`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub heading_for: Option<u64>,
}

/// How a live recording is going.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct LiveProgress {
    /// Seconds since recording began.
    pub elapsed: f64,
    /// Bytes recorded so far.
    pub bytes: u64,
    /// The rate it is being recorded at lately, in bits per second.
    pub bitrate: Option<f64>,
    /// Recording from the start: how many seconds behind the live edge it still is.
    pub behind: Option<f64>,
    /// Waiting: when the stream is scheduled to start (Unix seconds).
    pub starts_at: Option<u64>,
    /// Seconds of recording the disk still has room for, when that is getting short.
    pub disk_left: Option<f64>,
    /// Stopped while read from Twitch's VOD, behind live: how many seconds
    /// it still records to get to where Stop was pressed.
    pub stop_in: Option<f64>,
}

/// Share of the bar the downloads get when yt-dlp only has to merge afterwards …
const DOWNLOAD_SHARE: f64 = 0.97;
/// … and when a conversion follows, which usually takes about as long again.
const DOWNLOAD_SHARE_BEFORE_CONVERT: f64 = 0.5;

/// How far back speed is measured. Bytes arrive in bursts (ffmpeg writes a
/// trimmed part in bursts, and yt-dlp fetches YouTube in 10 MB requests), so
/// a reading over a moment swings wildly: yt-dlp's own speed read up to
/// 65.8 MB/s under a 20 MB/s limit while the file grew at 18.8.
const SPEED_WINDOW: Duration = Duration::from_secs(3);

/// Readings needed before a speed is shown at all, or the first burst reads as the speed.
const SPEED_WARMUP: Duration = Duration::from_millis(500);

/// How much of a conversion is done before the size it is heading for is
/// worked out from what it has written: before that, the file's header and
/// the encoder's first frames weigh too much.
const HEADING_FROM: f64 = 0.02;

#[derive(Debug)]
pub struct Tracker {
    plan: Vec<PlannedStream>,
    finished_streams: usize,
    download_share: f64,
    progress: Progress,
    /// Recent byte counts of the stream being fetched, and when they were read.
    samples: VecDeque<(Instant, u64)>,
    /// What Siphon's own ffmpeg pass is called: converting, or only adding subtitles.
    pass_stage: Stage,
}

impl Tracker {
    /// `converts`: whether an ffmpeg conversion follows the download.
    pub fn new(converts: bool) -> Self {
        Self {
            plan: Vec::new(),
            finished_streams: 0,
            download_share: if converts {
                DOWNLOAD_SHARE_BEFORE_CONVERT
            } else {
                DOWNLOAD_SHARE
            },
            progress: Progress {
                stage: Stage::Preparing,
                fraction: 0.0,
                speed: None,
                eta: None,
                live: None,
                heading_for: None,
            },
            samples: VecDeque::new(),
            pass_stage: Stage::Converting,
        }
    }

    /// For converting a file on this PC: nothing is downloaded, so the
    /// conversion is the whole bar.
    pub fn converting() -> Self {
        Self {
            download_share: 0.0,
            ..Self::new(true)
        }
    }

    /// Names Siphon's ffmpeg pass by what it does (see `on_convert`).
    pub fn set_pass_stage(&mut self, stage: Stage) {
        self.pass_stage = stage;
    }

    /// What a job reports while it fetches subtitles after the video.
    pub fn on_subtitles(&mut self) -> Progress {
        self.progress.stage = Stage::Subtitles;
        self.progress.heading_for = None;
        self.progress.fraction = self.progress.fraction.max(self.download_share);
        self.progress.speed = None;
        self.progress.eta = None;
        self.current()
    }

    /// What a job reports while it fetches the chat after the video.
    pub fn on_chat(&mut self) -> Progress {
        self.progress.stage = Stage::Chat;
        self.progress.heading_for = None;
        self.progress.fraction = self.progress.fraction.max(self.download_share);
        self.progress.speed = None;
        self.progress.eta = None;
        self.current()
    }

    /// What a job reports while it waits in the queue.
    pub fn queued() -> Progress {
        Progress {
            stage: Stage::Queued,
            fraction: 0.0,
            speed: None,
            eta: None,
            live: None,
            heading_for: None,
        }
    }

    /// What a job reports while yt-dlp is updated for a second attempt.
    pub fn updating() -> Progress {
        Progress {
            stage: Stage::Updating,
            fraction: 0.0,
            speed: None,
            eta: None,
            live: None,
            heading_for: None,
        }
    }

    /// What a job reports while it waits to try again after a failure.
    pub fn retrying() -> Progress {
        Progress {
            stage: Stage::Retrying,
            fraction: 0.0,
            speed: None,
            eta: None,
            live: None,
            heading_for: None,
        }
    }

    pub fn current(&self) -> Progress {
        self.progress.clone()
    }

    pub fn set_plan(&mut self, plan: Vec<PlannedStream>) {
        self.plan = plan;
    }

    pub fn on_tick(&mut self, tick: &DownloadTick) -> Progress {
        self.on_tick_at(tick, Instant::now())
    }

    fn on_tick_at(&mut self, tick: &DownloadTick, now: Instant) -> Progress {
        let stream_count = self.plan.len().max(self.finished_streams + 1);
        let index = self.finished_streams.min(stream_count - 1);

        let within = if tick.downloading {
            match (tick.downloaded, tick.total) {
                (Some(done), Some(total)) if total > 0 => (done as f64 / total as f64).min(1.0),
                _ => 0.0,
            }
        } else {
            1.0
        };

        let before: f64 = (0..index).map(|i| self.weight(i, stream_count)).sum();
        let overall = (before + self.weight(index, stream_count) * within) * self.download_share;

        if !tick.downloading {
            self.finished_streams += 1;
        }

        self.progress.heading_for = None;
        self.progress.stage = match (tick.has_video, tick.has_audio) {
            (true, false) => Stage::DownloadingVideo,
            (false, true) => Stage::DownloadingAudio,
            _ => Stage::Downloading,
        };
        // Estimates wobble; the bar must never move backwards.
        self.progress.fraction = self.progress.fraction.max(overall);
        if !tick.downloading {
            // The next stream starts from nothing: measure it afresh.
            self.samples.clear();
            self.progress.speed = None;
            self.progress.eta = None;
        } else if let Some(downloaded) = tick.downloaded {
            // Measured from the byte counts rather than taken from yt-dlp,
            // whose speed (and so its time left) is a reading over a moment.
            self.measure(downloaded, tick.total, now);
        }
        // (A tick without a byte count keeps the last reading rather than flicker.)
        self.current()
    }

    /// Progress measured from the size of the file on disk. Section downloads
    /// go through ffmpeg, which reports nothing until it is done, so speed
    /// and time left are worked out here from how fast the file grows.
    pub fn on_bytes(&mut self, written: u64, expected: u64) -> Progress {
        self.on_bytes_at(written, expected, Instant::now())
    }

    fn on_bytes_at(&mut self, written: u64, expected: u64, now: Instant) -> Progress {
        let within = if expected > 0 {
            (written as f64 / expected as f64).min(0.99)
        } else {
            0.0
        };
        self.progress.stage = Stage::Downloading;
        self.progress.heading_for = None;
        self.progress.fraction = self.progress.fraction.max(within * self.download_share);
        self.measure(written, Some(expected), now);
        self.current()
    }

    /// Speed over the last few seconds, from the bytes the current stream has
    /// so far, and the time left at that speed when the total is known.
    fn measure(&mut self, bytes: u64, total: Option<u64>, now: Instant) {
        // Fewer bytes than before: a new stream, or a try that started over.
        if self.samples.back().is_some_and(|(_, last)| bytes < *last) {
            self.samples.clear();
        }
        self.samples.push_back((now, bytes));
        while self
            .samples
            .front()
            .is_some_and(|(at, _)| now.duration_since(*at) > SPEED_WINDOW)
        {
            self.samples.pop_front();
        }
        let (oldest_at, oldest) = self.samples.front().copied().unwrap_or((now, bytes));
        let elapsed = now.duration_since(oldest_at);
        if elapsed < SPEED_WARMUP {
            // Not enough to go on yet: show nothing rather than a burst.
            self.progress.speed = None;
            self.progress.eta = None;
            return;
        }
        let speed = bytes.saturating_sub(oldest) as f64 / elapsed.as_secs_f64();
        self.progress.speed = Some(speed);
        // The total may be an estimate: once it is passed, the time left is unknown.
        self.progress.eta = total
            .filter(|total| speed > 0.0 && bytes < *total)
            .map(|total| (total - bytes) as f64 / speed);
    }

    pub fn on_postprocess(&mut self, name: &str) -> Progress {
        self.progress.stage = match name {
            "Merger" => Stage::Merging,
            "VideoRemuxer" | "VideoConvertor" | "ExtractAudio" => Stage::Converting,
            _ => Stage::Finishing,
        };
        self.progress.heading_for = None;
        self.progress.fraction = self.progress.fraction.max(self.download_share);
        self.progress.speed = None;
        self.progress.eta = None;
        self.current()
    }

    /// Progress of the ffmpeg conversion, 0–1, mapped onto the rest of the bar.
    /// A conversion that starts (again) has written nothing yet.
    pub fn on_convert(&mut self, done: f64) -> Progress {
        let overall = self.download_share + (1.0 - self.download_share) * done.clamp(0.0, 1.0);
        if done <= 0.0 {
            self.progress.heading_for = None;
        }
        self.progress.stage = self.pass_stage;
        self.progress.fraction = self.progress.fraction.max(overall);
        self.progress.speed = None;
        self.progress.eta = None;
        self.current()
    }

    /// The conversion has written `written` bytes of its file with `done`
    /// (0–1) of it made: the size the file is heading for is that scaled to
    /// the whole, once enough of it is done to go by. Exact at the end, and
    /// within a few percent from about a tenth of the way, unless the rest of
    /// the video is much busier or calmer than what came before.
    pub fn heading_for(&mut self, written: u64, done: f64) {
        if done >= HEADING_FROM && written > 0 {
            self.progress.heading_for = Some((written as f64 / done.min(1.0)) as u64);
        }
    }

    /// How much of the download share stream `index` accounts for: by size
    /// when every size is known, otherwise evenly.
    fn weight(&self, index: usize, stream_count: usize) -> f64 {
        let sizes: Option<Vec<u64>> = self.plan.iter().map(|stream| stream.size).collect();
        match sizes {
            Some(sizes) if sizes.len() == stream_count && sizes.iter().sum::<u64>() > 0 => {
                sizes[index] as f64 / sizes.iter().sum::<u64>() as f64
            }
            _ => 1.0 / stream_count as f64,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tick(downloading: bool, downloaded: u64, total: u64, video: bool) -> DownloadTick {
        DownloadTick {
            downloading,
            downloaded: Some(downloaded),
            total: Some(total),
            format_id: String::new(),
            has_video: video,
            has_audio: !video,
            filename: None,
        }
    }

    fn stream(size: u64) -> PlannedStream {
        PlannedStream {
            format_id: String::new(),
            size: Some(size),
        }
    }

    #[test]
    fn weights_streams_by_size() {
        let mut tracker = Tracker::new(false);
        tracker.set_plan(vec![stream(900), stream(100)]);

        let halfway_video = tracker.on_tick(&tick(true, 450, 900, true));
        assert_eq!(halfway_video.stage, Stage::DownloadingVideo);
        assert!((halfway_video.fraction - 0.45 * DOWNLOAD_SHARE).abs() < 1e-9);

        tracker.on_tick(&tick(false, 900, 900, true));
        let halfway_audio = tracker.on_tick(&tick(true, 50, 100, false));
        assert_eq!(halfway_audio.stage, Stage::DownloadingAudio);
        assert!((halfway_audio.fraction - 0.95 * DOWNLOAD_SHARE).abs() < 1e-9);
    }

    #[test]
    fn never_moves_backwards() {
        let mut tracker = Tracker::new(false);
        tracker.set_plan(vec![stream(100)]);
        let first = tracker.on_tick(&tick(true, 60, 100, true)).fraction;
        // The size estimate grew, so the raw ratio dropped.
        let second = tracker.on_tick(&tick(true, 61, 200, true)).fraction;
        assert!(second >= first);
    }

    #[test]
    fn reads_speed_and_time_left_off_a_growing_file() {
        let mut tracker = Tracker::new(false);
        let start = Instant::now();
        let at = |ms: u64| start + Duration::from_millis(ms);

        // Too soon after the first reading to say anything about speed.
        let first = tracker.on_bytes_at(0, 10_000_000, at(0));
        assert_eq!((first.speed, first.eta), (None, None));
        tracker.on_bytes_at(100_000, 10_000_000, at(250));

        // 1 MB/s: 2 MB in two seconds, 8 MB still to come.
        let steady = tracker.on_bytes_at(2_000_000, 10_000_000, at(2000));
        assert!((steady.speed.unwrap() - 1_000_000.0).abs() < 1.0);
        assert!((steady.eta.unwrap() - 8.0).abs() < 0.01);

        // Only the last three seconds count: 4 → 10 MB from 2.5 s to 5.5 s is 2 MB/s.
        tracker.on_bytes_at(4_000_000, 10_000_000, at(2500));
        let faster = tracker.on_bytes_at(10_000_000, 12_000_000, at(5500));
        assert!((faster.speed.unwrap() - 2_000_000.0).abs() < 1.0);
        assert!((faster.eta.unwrap() - 1.0).abs() < 0.01);

        // Past the estimated size, the time left is unknown, not zero.
        let past = tracker.on_bytes_at(13_000_000, 12_000_000, at(6000));
        assert_eq!(past.eta, None);
        assert!(past.speed.is_some());
    }

    #[test]
    fn speed_is_measured_over_seconds_from_the_byte_counts() {
        let mut tracker = Tracker::new(false);
        tracker.set_plan(vec![stream(100_000_000), stream(10_000_000)]);
        let start = Instant::now();
        let at = |ms: u64| start + Duration::from_millis(ms);

        // The first moment of a stream says nothing yet.
        let first = tracker.on_tick_at(&tick(true, 0, 100_000_000, true), at(0));
        assert_eq!((first.speed, first.eta), (None, None));

        // 20 MB/s, arriving in bursts: 2.5 MB at a time, and every fifth
        // tenth of a second nothing.
        let mut last = first;
        let mut downloaded = 0;
        for step in 1..=30 {
            if step % 5 != 0 {
                downloaded += 2_500_000;
            }
            last = tracker.on_tick_at(&tick(true, downloaded, 100_000_000, true), at(step * 100));
        }
        // 60 MB in 3 s.
        assert!(
            (last.speed.unwrap() - 20_000_000.0).abs() < 1_000_000.0,
            "{:?}",
            last.speed
        );
        // 40 MB left at 20 MB/s.
        assert!((last.eta.unwrap() - 2.0).abs() < 0.2, "{:?}", last.eta);

        // The video stream is done; the audio stream starts from nothing.
        tracker.on_tick_at(&tick(false, 100_000_000, 100_000_000, true), at(3100));
        let audio = tracker.on_tick_at(&tick(true, 100_000, 10_000_000, false), at(3200));
        assert_eq!((audio.speed, audio.eta), (None, None));
        let audio = tracker.on_tick_at(&tick(true, 5_100_000, 10_000_000, false), at(4200));
        assert!((audio.speed.unwrap() - 5_000_000.0).abs() < 1.0);
        assert!((audio.eta.unwrap() - 0.98).abs() < 0.01);
    }

    #[test]
    fn a_try_that_starts_over_is_measured_afresh() {
        let mut tracker = Tracker::new(false);
        let start = Instant::now();
        let at = |ms: u64| start + Duration::from_millis(ms);
        tracker.on_tick_at(&tick(true, 0, 10_000_000, true), at(0));
        tracker.on_tick_at(&tick(true, 5_000_000, 10_000_000, true), at(1000));
        // Back to the start of the stream: no negative or stale speed.
        let again = tracker.on_tick_at(&tick(true, 0, 10_000_000, true), at(1100));
        assert_eq!(again.speed, None);
        let again = tracker.on_tick_at(&tick(true, 1_000_000, 10_000_000, true), at(2100));
        assert!((again.speed.unwrap() - 1_000_000.0).abs() < 1.0);
    }

    #[test]
    fn a_conversion_says_what_size_it_is_heading_for() {
        let mut tracker = Tracker::new(true);
        tracker.on_convert(0.0);
        // Too little done to go by.
        tracker.heading_for(200_000, 0.01);
        assert_eq!(tracker.on_convert(0.01).heading_for, None);
        // A quarter of the way, 30 MB written: about 120 MB in all.
        tracker.heading_for(30_000_000, 0.25);
        assert_eq!(tracker.on_convert(0.25).heading_for, Some(120_000_000));
        // A conversion that starts again (Share, a little smaller) has written nothing yet.
        assert_eq!(tracker.on_convert(0.0).heading_for, None);
        tracker.heading_for(50_000_000, 0.5);
        assert_eq!(tracker.current().heading_for, Some(100_000_000));
        // The next stage has no such thing.
        assert_eq!(tracker.on_postprocess("Finishing").heading_for, None);
    }

    #[test]
    fn works_without_a_plan() {
        let mut tracker = Tracker::new(false);
        let progress = tracker.on_tick(&tick(true, 50, 100, true));
        assert!((progress.fraction - 0.5 * DOWNLOAD_SHARE).abs() < 1e-9);
    }
}
