//! Converting a file on this PC (see `crate::local`): planned as a link's
//! video is, made by Siphon's own ffmpeg pass straight from the file, and
//! saved beside it under a name of its own.
//!
//! The file is only ever read. What is made is written under the job's own
//! marker (`local::partial_name`) and given a name no file has
//! (`unique_output`) once it is checked; nothing that moves, renames or
//! deletes a file is ever handed the original (see [`super::Made`]).

use std::collections::{HashMap, VecDeque};
use std::path::Path;

use tauri::Manager;

use super::tracker::Tracker;
use super::{free_space, work_dir, ChildEvent, Done, DownloadManager, Failure, JobContext, Note};
use crate::error::{AppError, ErrorKind, Result};
use crate::ffmpeg::{self, encoders::Hardware};
use crate::local::{self, LocalFile, Original};
use crate::plan::{self, Choice, ReadFrom, Recipe, Trim};
use crate::process;
use crate::state::CapabilityCache;

/// How far before a part of a file reading starts: well before the key frame
/// before it, however far apart a file's key frames are (see `plan::ReadFrom`).
const READ_LEAD: f64 = 30.0;

/// What converting a file mostly takes. Each kind has its own number of
/// turns, so a batch of files never runs more at once than the PC takes
/// well; the rest wait their turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum Load {
    /// Encoding video on the processor, which takes every core.
    Processor,
    /// Encoding video on the graphics card. Consumer cards take only a few
    /// encodes at once, and Siphon's downloads may be using it too.
    GraphicsCard,
    /// Encoding sound alone: light.
    Sound,
    /// Copying streams into another file type, or cutting a copy: the disk.
    Disk,
}

impl Load {
    fn of(recipe: &Recipe) -> Load {
        match recipe.video_encode() {
            Some(encode) if encode.hardware == Hardware::Software => Load::Processor,
            Some(_) => Load::GraphicsCard,
            None if recipe.encodes_audio() => Load::Sound,
            None => Load::Disk,
        }
    }

    /// How many of this kind run at once.
    fn turns(self) -> usize {
        match self {
            Load::Processor => 1,
            Load::GraphicsCard | Load::Disk => 2,
            Load::Sound => 4,
        }
    }
}

/// Who may convert a file now: each kind of load takes turns in the order
/// its conversions were started.
#[derive(Default)]
pub(super) struct FilesGate {
    waiting: VecDeque<(String, Load)>,
    running: HashMap<Load, usize>,
}

impl DownloadManager {
    /// Converts a file on this PC as `choice` says (`trim`: the part to
    /// keep, from the file's start) and returns what was made. Nothing is
    /// made when the file already is what was asked for.
    pub(super) async fn run_file(
        &self,
        job: &JobContext<'_>,
        original: &Original,
        choice: &Choice,
        trim: Option<Trim>,
    ) -> Result<Done> {
        job.emit(Tracker::queued());
        let video = local::read(job.app, job.bins, &original.key()).await?;
        let file = video
            .file
            .clone()
            .ok_or_else(|| AppError::internal("Siphon lost track of this file. Drop it on the window again."))?;
        let caps = job.app.state::<CapabilityCache>().get(job.bins).await;
        // A part is chosen from the file's start, and cut by its timestamps.
        let on_its_clock = trim.map(|trim| Trim {
            start: trim.start + file.start,
            end: trim.end + file.start,
        });
        let mut recipe = plan::plan(&video, choice, on_its_clock, &caps)?;
        // A part of the file is read from a little before it, not from the file's start.
        if let Some(cut) = recipe.cut() {
            let seek = (cut.start - file.start - READ_LEAD).max(0.0);
            if seek > 0.0 {
                let from = ReadFrom {
                    seek,
                    at: file.start + seek,
                };
                recipe.read_from = Some(from);
                if let Some(fallback) = recipe.fallback.as_deref_mut() {
                    fallback.read_from = Some(from);
                }
            }
        }
        if let Some(reason) = &recipe.blocked {
            return Err(AppError::new(ErrorKind::TooLong, reason.clone()));
        }
        // The window doesn't start such a conversion; should one come, it makes nothing.
        if let Some(why) = local::nothing_to_do(&video, choice, &recipe) {
            return Err(AppError::new(ErrorKind::Unsupported, why));
        }
        let load = Load::of(&recipe);
        if !self.wait_for_turn(job.id, load).await {
            return Err(AppError::cancelled());
        }
        let made = self.convert_file(job, original, &file, choice, &recipe, trim).await;
        self.end_turn(load);
        made
    }

    /// Waits until it is this conversion's turn for its kind of load.
    /// False if it was stopped while it waited.
    async fn wait_for_turn(&self, id: &str, load: Load) -> bool {
        self.files_gate
            .lock()
            .unwrap()
            .waiting
            .push_back((id.to_string(), load));
        loop {
            // Created before checking, so a signal between check and await isn't missed.
            let changed = self.gate_changed.notified();
            {
                let mut gate = self.files_gate.lock().unwrap();
                if self.stop_of(id).is_some() {
                    gate.waiting.retain(|(waiting, _)| waiting != id);
                    drop(gate);
                    self.gate_changed.notify_waiters();
                    return false;
                }
                let first = gate
                    .waiting
                    .iter()
                    .find(|(_, kind)| *kind == load)
                    .map(|(first, _)| first);
                let running = gate.running.get(&load).copied().unwrap_or(0);
                if first.is_some_and(|first| first == id) && running < load.turns() {
                    gate.waiting.retain(|(waiting, _)| waiting != id);
                    *gate.running.entry(load).or_default() += 1;
                    drop(gate);
                    self.gate_changed.notify_waiters();
                    return true;
                }
            }
            changed.await;
        }
    }

    fn end_turn(&self, load: Load) {
        if let Some(running) = self.files_gate.lock().unwrap().running.get_mut(&load) {
            *running = running.saturating_sub(1);
        }
        self.gate_changed.notify_waiters();
    }

    /// Makes the file beside the original (or in the download folder, if its
    /// own folder can't be written to), checked before it gets its name.
    async fn convert_file(
        &self,
        job: &JobContext<'_>,
        original: &Original,
        file: &LocalFile,
        choice: &Choice,
        recipe: &Recipe,
        trim: Option<Trim>,
    ) -> Result<Done> {
        let mut tracker = Tracker::converting();
        job.emit(tracker.on_convert(0.0));
        let (folder, note) = match original.folder().filter(|folder| local::writable(folder, job.id)) {
            Some(folder) => (folder.to_path_buf(), None),
            None => (
                job.output_dir().await?,
                Some(Note {
                    text: "Saved in your download folder, because the file's own folder can't be written to."
                        .to_string(),
                    label: "In download folder",
                }),
            ),
        };
        // A disk filled to the brim fails a conversion halfway, after minutes of it.
        if let (Some(free), Some(needs)) = (free_space(&folder), room_needed(recipe)) {
            if free < needs {
                return Err(AppError::new(
                    ErrorKind::DiskFull,
                    format!(
                        "There isn't room for it on the disk: it needs about {}, and {} is free.",
                        size_text(needs),
                        size_text(free)
                    ),
                ));
            }
        }
        let name = local::output_name(&original.stem(), choice, recipe, trim);
        let output = self.unique_output(folder.join(name), job.id);
        let partial = local::partial_name(&output, job.id);
        let work = work_dir(job.app, job.id)?;
        tokio::fs::create_dir_all(&work).await?;
        let made = if recipe.video.is_some() {
            self.make_video(job, recipe, original.path(), &output, &partial, &work, &mut tracker)
                .await
        } else {
            self.make_sound(job, recipe, original.path(), &output, &partial, file, &mut tracker)
                .await
        };
        // Only Siphon's own: the two-pass log, a smart cut's pieces.
        let _ = tokio::fs::remove_dir_all(&work).await;
        made?;
        Ok(Done::file(output, note))
    }

    /// Video: copied into another file type, cut, or converted, by the steps
    /// a download's conversion takes. A copy for Share that comes out over
    /// the limit after all is converted.
    #[allow(clippy::too_many_arguments)]
    async fn make_video(
        &self,
        job: &JobContext<'_>,
        recipe: &Recipe,
        source: &Path,
        output: &Path,
        partial: &Path,
        work: &Path,
        tracker: &mut Tracker,
    ) -> Result<()> {
        let made = self
            .convert_checked(job, recipe, source, output, partial, work, tracker, None)
            .await;
        match (made, recipe.fallback.as_deref()) {
            (Err(error), Some(fallback))
                if error.kind == ErrorKind::TooLong && !recipe.encodes_video() && self.stop_of(job.id).is_none() =>
            {
                self.convert_checked(job, fallback, source, output, partial, work, tracker, None)
                    .await
            }
            (made, _) => made,
        }
    }

    /// Sound alone: copied or converted, with the file's tags, and its cover
    /// where the file type holds one.
    #[allow(clippy::too_many_arguments)]
    async fn make_sound(
        &self,
        job: &JobContext<'_>,
        recipe: &Recipe,
        source: &Path,
        output: &Path,
        partial: &Path,
        file: &LocalFile,
        tracker: &mut Tracker,
    ) -> Result<()> {
        // The part, from where it starts in the file.
        let cut = recipe.trim.map(|trim| (trim.start - file.start, trim.duration()));
        let mut cmd = process::command(job.bins.ffmpeg());
        cmd.args(ffmpeg::sound_args(recipe, source, partial, cut, file.cover));
        let mut written = None;
        let ran = self
            .run_child(job.bins, job.id, cmd, |event| {
                let ChildEvent::Line(line) = event else { return };
                if let Some(bytes) = ffmpeg::parse_written(line) {
                    written = Some(bytes);
                }
                if let (Some(done), Some(total)) = (ffmpeg::parse_progress(line), recipe.duration) {
                    let share = done / total;
                    if let Some(bytes) = written {
                        tracker.heading_for(bytes, share);
                    }
                    job.emit(tracker.on_convert(share));
                }
            })
            .await;
        let made = match ran {
            Ok(_) => self.check_converted(job, recipe, partial).await,
            Err(Failure::Stopped(_)) => Err(AppError::cancelled()),
            Err(Failure::Exited(log)) => Err(AppError::internal("Converting the sound failed.").with_details(log)),
            Err(Failure::Spawn(error)) => Err(error),
        };
        if let Err(error) = made {
            let _ = tokio::fs::remove_file(partial).await;
            return Err(error);
        }
        tokio::fs::rename(partial, output).await?;
        Ok(())
    }
}

/// The room a conversion needs on its disk: what it is likely to make, a
/// fifth more for a picture busier than expected, and a quarter of a
/// gigabyte to spare. None when the size can't be told.
fn room_needed(recipe: &Recipe) -> Option<u64> {
    let likely = recipe.estimated_size()?;
    Some(likely + likely / 5 + 256 * 1024 * 1024)
}

/// "1.2 GB", "350 MB", in the units the window shows sizes in.
fn size_text(bytes: u64) -> String {
    const MB: f64 = 1024.0 * 1024.0;
    let mb = bytes as f64 / MB;
    if mb >= 1000.0 {
        format!("{:.1} GB", mb / 1024.0)
    } else {
        format!("{} MB", mb.round().max(1.0))
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::plan::codecs::{AudioCodec, Container, VideoCodec};
    use crate::plan::{AudioAction, AudioStep, Badge, VideoAction, VideoEncode, VideoStep};

    /// Where a conversion of the file at `original` writes until it is checked.
    fn partial_beside(original: &Path, output_name: &str, job_id: &str) -> PathBuf {
        local::partial_name(&original.with_file_name(output_name), job_id)
    }

    fn recipe(video: Option<VideoAction>, audio: Option<AudioAction>) -> Recipe {
        Recipe {
            video: video.map(|action| VideoStep {
                format_id: "file".into(),
                source: VideoCodec::H264,
                lines: 1080,
                width: Some(1920),
                height: Some(1080),
                fps: Some(30.0),
                hdr: false,
                size: None,
                kbps: None,
                action,
            }),
            audio: audio.map(|action| AudioStep {
                format_id: Some("file".into()),
                source: AudioCodec::Aac,
                source_kbps: Some(128.0),
                size: None,
                action,
            }),
            container: Container::Mp4,
            strip_audio: false,
            reason: None,
            promise: String::new(),
            hints: Vec::new(),
            duration: Some(60.0),
            trim: None,
            source_fraction: 1.0,
            badge: Badge::Lossless,
            size_limit: None,
            fits: None,
            blocked: None,
            section_request_size: None,
            fallback: None,
            cut_encode: None,
            subtitles: None,
            chat: None,
            measured: Default::default(),
            read_from: None,
        }
    }

    #[test]
    fn each_kind_of_conversion_waits_for_its_own_kind_of_turn() {
        let encode = |hardware| {
            VideoAction::Encode(VideoEncode {
                codec: VideoCodec::H264,
                hardware,
                bitrate_kbps: None,
                scale_lines: None,
                max_fps: None,
                fast: false,
                max_kbps: None,
            })
        };
        let mp3 = AudioAction::Encode {
            codec: AudioCodec::Mp3,
            bitrate_kbps: Some(192),
            stereo: false,
        };
        let copy = Some(AudioAction::Copy);
        assert_eq!(
            Load::of(&recipe(Some(encode(Hardware::Software)), copy.clone())),
            Load::Processor
        );
        assert_eq!(
            Load::of(&recipe(Some(encode(Hardware::Nvenc)), copy.clone())),
            Load::GraphicsCard
        );
        assert_eq!(Load::of(&recipe(None, Some(mp3.clone()))), Load::Sound);
        assert_eq!(Load::of(&recipe(Some(VideoAction::Copy), Some(mp3))), Load::Sound);
        assert_eq!(Load::of(&recipe(Some(VideoAction::Copy), copy)), Load::Disk);
        assert_eq!(Load::Processor.turns(), 1);
    }

    #[tokio::test]
    async fn conversions_of_one_kind_take_turns_in_order_while_others_go_by() {
        let manager = std::sync::Arc::new(DownloadManager::default());
        for id in ["a", "b", "c", "d"] {
            manager.register_file(id);
        }
        // The processor's one turn is taken by "a"; "b" waits for it, but
        // "c", a copy, goes ahead of it.
        assert!(manager.wait_for_turn("a", Load::Processor).await);
        let waiting = {
            let manager = std::sync::Arc::clone(&manager);
            tokio::spawn(async move { manager.wait_for_turn("b", Load::Processor).await })
        };
        tokio::task::yield_now().await;
        assert!(manager.wait_for_turn("c", Load::Disk).await);
        assert!(!waiting.is_finished());
        // A conversion stopped while it waits gives up its place.
        let stopped = {
            let manager = std::sync::Arc::clone(&manager);
            tokio::spawn(async move { manager.wait_for_turn("d", Load::Processor).await })
        };
        tokio::task::yield_now().await;
        manager.cancel("d").await;
        assert!(!stopped.await.unwrap());
        // "a" is done: "b" has its turn.
        manager.end_turn(Load::Processor);
        assert!(waiting.await.unwrap());
        assert!(manager.files_gate.lock().unwrap().waiting.is_empty());
    }

    #[test]
    fn a_conversion_needs_room_for_what_it_makes_and_some_to_spare() {
        let mut copy = recipe(Some(VideoAction::Copy), Some(AudioAction::Copy));
        copy.video.as_mut().unwrap().size = Some(1024 * 1024 * 1024);
        // A file's sound is in its one format, counted with the picture.
        copy.audio.as_mut().unwrap().format_id = None;
        assert_eq!(room_needed(&copy), Some(1024 * 1024 * 1024 * 6 / 5 + 256 * 1024 * 1024));
        assert_eq!(size_text(1024 * 1024 * 1024 * 6 / 5), "1.2 GB");
        assert_eq!(size_text(350 * 1024 * 1024), "350 MB");
        assert_eq!(size_text(10), "1 MB");
    }

    #[test]
    fn what_a_conversion_writes_is_never_the_file_it_reads() {
        let original = Path::new(r"C:\Videos\Clip.mp4");
        // Even made for Max quality, into its own file type, under its own name:
        // the finished name is free (see `unique_output`), and the one it is
        // written under meanwhile carries the job's marker.
        let partial = partial_beside(original, "Clip.mp4", "1a2b3c4d");
        assert_ne!(partial, original);
        assert_eq!(partial, Path::new(r"C:\Videos\Clip.siphon-1a2b3c4d.mp4"));
        let manager = DownloadManager::default();
        let dir = std::env::temp_dir().join(format!("siphon-files-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let taken = dir.join("Clip.mp4");
        std::fs::write(&taken, b"the user's").unwrap();
        assert_eq!(manager.unique_output(taken.clone(), "job"), dir.join("Clip (2).mp4"));
        assert_eq!(std::fs::read(&taken).unwrap(), b"the user's");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
