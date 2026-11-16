//! Folds yt-dlp's per-stream ticks into one overall, never-decreasing progress value.

use serde::Serialize;

use crate::ytdlp::progress::{DownloadTick, PlannedStream};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
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

/// Share of the bar reserved for the downloads; the rest is post-processing.
const DOWNLOAD_SHARE: f64 = 0.97;

#[derive(Debug)]
pub struct Tracker {
    plan: Vec<PlannedStream>,
    finished_streams: usize,
    progress: Progress,
}

impl Tracker {
    pub fn new() -> Self {
        Self {
            plan: Vec::new(),
            finished_streams: 0,
            progress: Progress {
                stage: Stage::Preparing,
                fraction: 0.0,
                speed: None,
                eta: None,
            },
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
        let overall = (before + self.weight(index, stream_count) * within) * DOWNLOAD_SHARE;

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

    pub fn on_postprocess(&mut self, name: &str) -> Progress {
        self.progress.stage = match name {
            "Merger" => Stage::Merging,
            "VideoRemuxer" | "VideoConvertor" | "ExtractAudio" => Stage::Converting,
            _ => Stage::Finishing,
        };
        self.progress.fraction = self.progress.fraction.max(DOWNLOAD_SHARE);
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
        let mut tracker = Tracker::new();
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
        let mut tracker = Tracker::new();
        tracker.set_plan(vec![stream(100)]);
        let first = tracker.on_tick(&tick(true, 60, 100, true)).fraction;
        // The size estimate grew, so the raw ratio dropped.
        let second = tracker.on_tick(&tick(true, 61, 200, true)).fraction;
        assert!(second >= first);
    }

    #[test]
    fn works_without_a_plan() {
        let mut tracker = Tracker::new();
        let progress = tracker.on_tick(&tick(true, 50, 100, true));
        assert!((progress.fraction - 0.5 * DOWNLOAD_SHARE).abs() < 1e-9);
    }
}
