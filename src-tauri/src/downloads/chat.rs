//! A download with its chat drawn in: the chat is fetched beside the video
//! (its emotes, badges and their images too), then drawn into the picture
//! while it is encoded, by a thread that feeds ffmpeg the chat layer frame
//! by frame (see `crate::chat`).

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use tauri::Manager;

use super::{ChildEvent, DownloadManager, Failure, JobContext};
use crate::chat::render::{Geometry, Layer, Style};
use crate::chat::twitch::Twitch;
use crate::chat::{emotes, images, ChatLog, ChatSource, ChatStep};
use crate::error::{AppError, Result};
use crate::ffmpeg::{self, ChatLayer, Pass};
use crate::log;
use crate::plan::Recipe;
use crate::process;

/// The fetched chat, in the download's work folder.
const CHAT_FILE: &str = "chat.json";
/// Chat from this long before a clip starts is fetched too, so its first
/// frame shows the chat as it was then rather than an empty box.
const LEAD_IN: f64 = 60.0;
/// The chat layer never needs more frames a second than this: messages come
/// in and emotes move no faster, and every frame is a copy of the box.
const MAX_LAYER_FPS: f64 = 31.0;

/// Where emote and badge images are kept, for every download.
pub fn image_folder(app: &tauri::AppHandle) -> Result<PathBuf> {
    app.path()
        .app_cache_dir()
        .map(|dir| dir.join("chat-images"))
        .map_err(|err| AppError::internal("Siphon couldn't find its cache folder.").with_details(err.to_string()))
}

/// The seconds of the video a recipe covers: its cut, or all of it.
fn span(recipe: &Recipe, length: f64) -> (f64, f64) {
    match recipe.trim {
        Some(trim) => (trim.start, trim.end),
        None => (0.0, recipe.duration.unwrap_or(length).max(0.0)),
    }
}

impl DownloadManager {
    /// Fetches the chat `step` draws in, with its emotes, badges and their
    /// images, into the work folder `work`. Ok(Err(why)) when it can't be had:
    /// the video is saved without it. What an earlier try fetched is used again.
    pub(super) async fn fetch_chat(
        &self,
        job: &JobContext<'_>,
        recipe: &Recipe,
        step: &ChatStep,
        work: &Path,
    ) -> Result<std::result::Result<PathBuf, String>> {
        let file = work.join(CHAT_FILE);
        if ChatLog::load(&file).await.is_some() {
            return Ok(Ok(file));
        }
        let ChatSource::Vod { id, .. } = &step.source else {
            return Ok(Err("Siphon draws in the chat of Twitch videos only.".into()));
        };
        match self.fetch_vod_chat(job, recipe, step, id, &file).await {
            Ok(()) => Ok(Ok(file)),
            Err(error) => {
                log::line(
                    job.app,
                    &format!(
                        "chat {id}: couldn't be had: {} {}",
                        error.message,
                        error.details.as_deref().unwrap_or("")
                    ),
                );
                Ok(Err(error.message))
            }
        }
    }

    async fn fetch_vod_chat(
        &self,
        job: &JobContext<'_>,
        recipe: &Recipe,
        step: &ChatStep,
        id: &str,
        file: &Path,
    ) -> Result<()> {
        let started = std::time::Instant::now();
        let twitch = Twitch::new()?;
        let vod = twitch.vod(id).await?;
        let (from, to) = span(recipe, vod.length);
        let (replay, (emote_list, missing), badges) = tokio::join!(
            twitch.comments(id, (from - LEAD_IN).max(0.0), to, |_| {}),
            emotes::fetch(twitch.http(), &vod.channel.id),
            twitch.badges(&vod.channel.id),
        );
        let replay = replay?;
        if replay.messages.is_empty() {
            let why = if recipe.trim.is_some() {
                "Nobody wrote in the chat during this part."
            } else {
                "Twitch has no chat for this video."
            };
            return Err(AppError::new(crate::error::ErrorKind::Unavailable, why));
        }
        let log = ChatLog {
            channel: vod.channel,
            messages: replay.messages,
            emotes: emote_list,
            // Without badges the chat is still the chat.
            badges: badges.unwrap_or_default(),
        };
        // The images, at the sizes the box will draw them (the frame's size as
        // the lookup gave it; the layer makes do with another size if it differs).
        let (width, height) = recipe
            .video
            .as_ref()
            .and_then(|video| video.width.zip(video.height))
            .unwrap_or((1920, 1080));
        let geometry = Geometry::new(&step.placement, width, height);
        let style = Style::for_width(geometry.width);
        let urls = log.images_used((from - LEAD_IN).max(0.0), to, style.emote(), style.badge());
        let wanted = urls.len();
        let failed = images::fetch(twitch.http(), &image_folder(job.app)?, urls).await;
        log.save(file).await?;
        crate::log::line(
            job.app,
            &format!(
                "chat {id}: {} messages ({} gaps), {} emotes (not had: {}), {} badges, {wanted} images ({failed} failed), {:.1} s",
                log.messages.len(),
                replay.gaps,
                log.emotes.len(),
                if missing.is_empty() { "none".to_string() } else { missing.join(", ") },
                log.badges.len(),
                started.elapsed().as_secs_f64()
            ),
        );
        Ok(())
    }

    /// Encodes `input` into `output` as `recipe` says, with its chat drawn in:
    /// a thread draws the chat layer frame by frame and feeds it to ffmpeg's
    /// standard input, which lays it over the picture as it encodes.
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn draw_chat(
        &self,
        job: &JobContext<'_>,
        recipe: &Recipe,
        step: &ChatStep,
        input: &Path,
        output: &Path,
        work: &Path,
        audio_shift: Option<f64>,
        subtitles: Option<&Path>,
        mut on_progress: impl FnMut(f64),
    ) -> Result<()> {
        let log = ChatLog::load(&work.join(CHAT_FILE))
            .await
            .ok_or_else(|| AppError::internal("The chat Siphon fetched for this video is missing."))?;
        let picture = self.picture(job, input).await?;
        // The frame the chat is drawn on: the picture after any scaling.
        let (width, height) = match (recipe.video_encode().and_then(|e| e.scale_lines), recipe.video.as_ref()) {
            (Some(lines), Some(video)) if lines < video.lines => {
                let scale = f64::from(lines) / f64::from(picture.width.min(picture.height));
                (
                    ((f64::from(picture.width) * scale / 2.0).round() as u32) * 2,
                    ((f64::from(picture.height) * scale / 2.0).round() as u32) * 2,
                )
            }
            _ => (picture.width, picture.height),
        };
        let fps = picture.fps.or_else(|| recipe.output_fps()).unwrap_or(30.0);
        let layer_fps = if fps > MAX_LAYER_FPS { fps / 2.0 } else { fps };
        let (start, end) = span(recipe, picture.duration.unwrap_or(0.0));
        let length = (end - start).max(0.0);
        // A cut keeps the video's own clock (see `ffmpeg::convert_args_shifted`);
        // a whole video starts at zero.
        let layer_start = if recipe.cut().is_some() { start } else { 0.0 };

        let cache = image_folder(job.app)?;
        let placement = step.placement;
        let chat_layer = Layer::new(log, &placement, width, height, &cache);
        let geometry = chat_layer.geometry();
        let args = ChatLayer {
            width: geometry.width,
            height: geometry.height,
            fps: layer_fps,
            x: geometry.x,
            y: geometry.y,
            start: layer_start,
            hd: height >= 720,
            // A clip ends exactly where it was cut; a whole video, where its picture does.
            end: recipe.cut().is_some().then_some(end),
            rate: None,
        };
        let mut cmd = process::command(job.bins.ffmpeg());
        cmd.current_dir(work);
        cmd.args(ffmpeg::convert_args_shifted(
            recipe,
            input,
            output,
            Pass::Only,
            audio_shift,
            subtitles,
            Some(&args),
        ));
        self.run_with_layer(job, cmd, chat_layer, start, layer_fps, length, &mut on_progress)
            .await
    }

    /// Runs ffmpeg (`cmd`, set to read the chat layer from its standard input)
    /// while a thread draws `layer`'s frames into it: from `start` seconds of
    /// the chat's clock, `fps` a second, for `length` seconds.
    /// `on_progress` hears how far ffmpeg has got (0–1).
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn run_with_layer(
        &self,
        job: &JobContext<'_>,
        mut cmd: tokio::process::Command,
        mut layer: Layer,
        start: f64,
        fps: f64,
        length: f64,
        on_progress: &mut impl FnMut(f64),
    ) -> Result<()> {
        let (reader, mut writer) = std::io::pipe()
            .map_err(|err| AppError::internal("Couldn't start drawing the chat.").with_details(err.to_string()))?;
        cmd.stdin(Stdio::from(reader));
        // Frames for no longer than the picture lasts: the overlay goes on for
        // as long as the layer does (see `ffmpeg::ChatLayer::end`), and should
        // the layer end a frame early, its last frame stays. The thread stops
        // when it has drawn them all, or ffmpeg ends first.
        let frames = (length * fps).floor().max(1.0) as u64;
        let drawing = std::thread::spawn(move || {
            for index in 0..frames {
                let at = start + index as f64 / fps;
                if writer.write_all(layer.frame(at)).is_err() {
                    break;
                }
            }
        });
        let result = self
            .run_child(job.bins, job.id, cmd, |event| {
                if let ChildEvent::Line(line) = event {
                    if let Some(done) = ffmpeg::parse_progress(line) {
                        if length > 0.0 {
                            on_progress((done / length).clamp(0.0, 1.0));
                        }
                    }
                }
            })
            .await;
        // ffmpeg has ended: the thread's next write fails, if it hasn't finished.
        let drew = tokio::task::spawn_blocking(move || drawing.join().is_ok())
            .await
            .unwrap_or(false);
        match result {
            Ok(_) if drew => Ok(()),
            Ok(_) => Err(AppError::internal("Drawing the chat failed.")),
            Err(Failure::Stopped(_)) => Err(AppError::cancelled()),
            Err(Failure::Exited(log)) => {
                Err(AppError::internal("Drawing the chat into the video failed.").with_details(log))
            }
            Err(Failure::Spawn(error)) => Err(error),
        }
    }

    /// The size, frame rate and length of `file`'s picture.
    pub(super) async fn picture(&self, job: &JobContext<'_>, file: &Path) -> Result<ffmpeg::Picture> {
        let mut json = String::new();
        let mut cmd = process::command(job.bins.ffprobe());
        cmd.args(ffmpeg::picture_probe_args(file));
        match self
            .run_child(job.bins, job.id, cmd, |event| {
                if let ChildEvent::Line(line) = event {
                    json.push_str(line);
                }
            })
            .await
        {
            Ok(_) => {}
            Err(Failure::Stopped(_)) => return Err(AppError::cancelled()),
            Err(Failure::Exited(log)) => {
                return Err(AppError::internal("Siphon couldn't read the downloaded video.").with_details(log))
            }
            Err(Failure::Spawn(error)) => return Err(error),
        }
        ffmpeg::Picture::parse(&json)
            .ok_or_else(|| AppError::internal("Siphon couldn't read the downloaded video.").with_details(json))
    }
}
