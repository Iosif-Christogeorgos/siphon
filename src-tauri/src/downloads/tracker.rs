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
    Finishing,
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
}

/// Share of the bar the downloads get when yt-dlp only has to merge afterwards …
const DOWNLOAD_SHARE: f64 = 0.97;
/// … and when a conversion follows, which usually takes about as long again.
const DOWNLOAD_SHARE_BEFORE_CONVERT: f64 = 0.5;

/// How far back speed is measured when it is read off a growing file.
/// ffmpeg writes in bursts, so a single 250 ms step swings wildly.
const SPEED_WINDOW: Duration = Duration::from_secs(3);

#[derive(Debug)]
pub struct Tracker {
    plan: Vec<PlannedStream>,
    finished_streams: usize,
    download_share: f64,
    progress: Progress,
    /// Recent file sizes and when they were read (see `on_bytes`).
    samples: VecDeque<(Instant, u64)>,
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
            },
            samples: VecDeque::new(),
        }
    }

    /// What a job reports while it waits in the queue.
    pub fn queued() -> Progress {
        Progress {
            stage: Stage::Queued,
            fraction: 0.0,
            speed: None,
            eta: None,
        }
    }

    /// What a job reports while yt-dlp is updated for a second attempt.
    pub fn updating() -> Progress {
        Progress {
            stage: Stage::Updating,
            fraction: 0.0,
            speed: None,
            eta: None,
        }
    }

    /// What a job reports while it waits to try again after a failure.
    pub fn retrying() -> Progress {
        Progress {
            stage: Stage::Retrying,
            fraction: 0.0,
            speed: None,
            eta: None,
        }
    }

    pub fn current(&self) -> Progress {
        self.progress.clone()
    }

    /// More than one stream means the per-stream files are temporary.
    pub fn is_multi_stream(&self) -> bool {
        self.plan.len() > 1
    }

    pub fn set_plan(&mut self, plan: Vec<PlannedStream>) {
        self.plan = plan;
    }

    pub fn on_tick(&mut self, tick: &DownloadTick) -> Progress {
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

        self.progress.stage = match (tick.has_video, tick.has_audio) {
            (true, false) => Stage::DownloadingVideo,
            (false, true) => Stage::DownloadingAudio,
            _ => Stage::Downloading,
        };
        // Estimates wobble; the bar must never move backwards.
        self.progress.fraction = self.progress.fraction.max(overall);
        if tick.downloading {
            // Some ticks omit speed/ETA; keep the last reading rather than flicker.
            self.progress.speed = tick.speed.or(self.progress.speed);
            self.progress.eta = tick.eta.or(self.progress.eta);
        } else {
            self.progress.speed = None;
            self.progress.eta = None;
        }
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
        self.progress.fraction = self.progress.fraction.max(within * self.download_share);

        self.samples.push_back((now, written));
        while self
            .samples
            .front()
            .is_some_and(|(at, _)| now.duration_since(*at) > SPEED_WINDOW)
        {
            self.samples.pop_front();
        }
        let (oldest_at, oldest) = self.samples.front().copied().unwrap_or((now, written));
        let elapsed = now.duration_since(oldest_at).as_secs_f64();
        // Half a second of readings at least, or the first burst reads as the speed.
        if elapsed >= 0.5 {
            let speed = written.saturating_sub(oldest) as f64 / elapsed;
            self.progress.speed = Some(speed);
            // The expected size is an estimate: once it is passed, the time left is unknown.
            self.progress.eta = (speed > 0.0 && written < expected)
                .then(|| (expected - written) as f64 / speed);
        }
        self.current()
    }

    pub fn on_postprocess(&mut self, name: &str) -> Progress {
        self.progress.stage = match name {
            "Merger" => Stage::Merging,
            "VideoRemuxer" | "VideoConvertor" | "ExtractAudio" => Stage::Converting,
            _ => Stage::Finishing,
        };
        self.progress.fraction = self.progress.fraction.max(self.download_share);
        self.progress.speed = None;
        self.progress.eta = None;
        self.current()
    }

    /// Progress of the ffmpeg conversion, 0–1, mapped onto the rest of the bar.
    pub fn on_convert(&mut self, done: f64) -> Progress {
        let overall = self.download_share + (1.0 - self.download_share) * done.clamp(0.0, 1.0);
        self.progress.stage = Stage::Converting;
        self.progress.fraction = self.progress.fraction.max(overall);
        self.progress.speed = None;
        self.progress.eta = None;
        self.current()
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
            speed: Some(1.0),
            eta: Some(1.0),
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
    fn works_without_a_plan() {
        let mut tracker = Tracker::new(false);
        let progress = tracker.on_tick(&tick(true, 50, 100, true));
        assert!((progress.fraction - 0.5 * DOWNLOAD_SHARE).abs() < 1e-9);
    }
}
