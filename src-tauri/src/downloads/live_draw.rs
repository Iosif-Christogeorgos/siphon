//! A recording's chat drawn in while it records (see `live_chat`), so that
//! when it ends only its last few minutes are left to draw rather than all
//! of it: drawing runs at about 14 times real time, so a three-hour stream
//! took about 13 minutes after Stop (docs/chat.md).
//!
//! Each stretch of the recording is drawn in pieces of [`PIECE`], each once
//! its chat has settled, [`SETTLE`] after the last of it aired: by then a
//! moderator's deletion or a timeout has come, and the finished file leaves
//! those messages out. A piece is the stretch's picture from where the last
//! one ended, encoded with the chat drawn in, and its audio packets copied as
//! recorded, all on the recording's own timestamps
//! (`ffmpeg::draw_piece_args`). It is appended to one drawn file per
//! stretch, beside the recorded one, which piece after piece is the
//! stretch, drawn. How far each is drawn is kept in the work folder
//! (`drawn-N.json`), with the size the drawn file had then, so a piece cut
//! short by a crash is cut off again. When the recording ends, the rest is
//! drawn ([`finish`]) and the drawn files stand in for the recorded ones.
//!
//! Anything that goes wrong only means the drawn files aren't used: the
//! recording is then finished from what was recorded, with its chat drawn
//! in as it is finished, as it would have been without this.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tauri::AppHandle;
use tokio::io::{AsyncBufReadExt, AsyncSeekExt, AsyncWriteExt, BufReader};

use super::live::recording_files;
use super::live_chat::{self, CLEARED_SPAN, REPLAY_LEAD};
use crate::chat::render::{layer_fps, Layer, Style};
use crate::chat::twitch::Twitch;
use crate::chat::{images, Placement};
use crate::ffmpeg::{self, ChatLayer};
use crate::log;
use crate::plan::Recipe;
use crate::process;

/// How much of a stretch is drawn at a time, in seconds. Shorter pieces
/// leave less to draw after Stop; each costs ffmpeg a second or so to start.
const PIECE: f64 = 120.0;
/// A piece is drawn once this long has passed since the last of it aired
/// (and its chat, [`REPLAY_LEAD`] later, was sent): a timeout takes away
/// the user's messages from the two minutes before it, and the finished
/// file leaves them out too.
const SETTLE: f64 = REPLAY_LEAD + CLEARED_SPAN + 10.0;
/// Nor closer to the end of what has been recorded than this: the last
/// segment may still be being written.
const BEHIND_END: f64 = 10.0;
/// How often a stretch is looked at for a piece to draw.
const LOOK_EVERY: Duration = Duration::from_secs(10);
/// How long a stretch's timing may take to settle (the frame match, or the
/// VOD's replay) before drawing goes ahead with what there is.
const TIMING_WAIT: Duration = Duration::from_secs(15 * 60);

/// How far a stretch has been drawn, as kept in the work folder.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Drawn {
    /// Drawn up to here, on the stretch file's own clock.
    until: f64,
    /// The drawn file's size then: whatever is after it is a piece that was
    /// being added when Siphon closed, and is cut off.
    bytes: u64,
}

fn state_file(work: &Path, number: usize) -> PathBuf {
    work.join(format!("drawn-{number}.json"))
}

/// The drawn file of stretch `number`, beside its recorded file:
/// `Title.siphon-1a2b3c4d.rec1.ts` → `Title.siphon-1a2b3c4d.drawn1.ts`.
pub(super) fn drawn_path(raw: &Path, number: usize) -> Option<PathBuf> {
    let name = raw.file_name()?.to_str()?;
    let marker = format!(".rec{number}.");
    let at = name.find(&marker)?;
    Some(raw.with_file_name(format!("{}.drawn{number}.ts", &name[..at])))
}

/// Where a piece is put together before it is added to the drawn file.
fn piece_path(drawn: &Path) -> PathBuf {
    drawn.with_extension("piece.ts")
}

/// Removes what drawing stretch `number` left beside `raw`.
pub(super) async fn remove(raw: &Path, number: usize) {
    if let Some(drawn) = drawn_path(raw, number) {
        let _ = tokio::fs::remove_file(piece_path(&drawn)).await;
        let _ = tokio::fs::remove_file(&drawn).await;
    }
}

/// What a stretch's drawing needs.
pub(super) struct DrawPart {
    pub app: AppHandle,
    pub work: PathBuf,
    pub number: usize,
    /// Where the stretch's file is, and what its name carries.
    pub dir: PathBuf,
    pub needle: String,
    pub ffmpeg: PathBuf,
    pub ffprobe: PathBuf,
    pub recipe: Recipe,
    pub placement: Placement,
    /// The emote and badge images' cache.
    pub images: PathBuf,
}

fn now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |since| since.as_secs_f64())
}

/// How far a stretch may be drawn now, on its file's clock: what has
/// settled ([`SETTLE`]) and is far enough from the end of the file. `aired`
/// says when each moment of the file aired, `end` where what has been
/// recorded ends.
fn settled_until(aired: &live_chat::Aired, end: f64, now: f64) -> f64 {
    aired.file_time(now - SETTLE).min(end - BEHIND_END)
}

/// Draws stretch `part` piece by piece while it is recorded, until the
/// recording ends (the task is dropped with the recording's chat capture).
pub(super) async fn draw_while_recording(part: DrawPart) {
    let raw = loop {
        if let Some(file) = recording_files(&part.dir, &part.needle).into_iter().next() {
            break file;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    };
    let Some(drawn) = drawn_path(&raw, part.number) else {
        return;
    };
    let appeared = Instant::now();
    let start = loop {
        if let Some((start, _)) = live_chat::file_span(&part.ffprobe, &raw).await {
            break start;
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    };
    // When it aired, as well as that will be known.
    let mut aired = loop {
        if let Some(aired) = live_chat::timing_settled(&part.work, part.number).await {
            break aired;
        }
        if appeared.elapsed() > TIMING_WAIT {
            match live_chat::load_json::<live_chat::Aired>(&live_chat::aired_file(&part.work, part.number)).await {
                // The frame match gave up: its guess stands, as it will when finished.
                Some(aired) if aired.vod.is_none() => break aired,
                // Read from the VOD without its replay: left to when it is
                // finished, which looks for the replay again.
                _ => return,
            }
        }
        tokio::time::sleep(LOOK_EVERY).await;
    };
    let state = state_file(&part.work, part.number);
    let mut done = match live_chat::load_json::<Drawn>(&state).await {
        Some(done) => done,
        None => {
            if tokio::fs::write(&drawn, b"").await.is_err() {
                return;
            }
            Drawn { until: start, bytes: 0 }
        }
    };
    loop {
        // Read without the ads, it learns where each ad was left out as it comes.
        if let Some(latest) = live_chat::timing_settled(&part.work, part.number).await {
            aired = latest;
        }
        if let Some((now_start, length)) = live_chat::file_span(&part.ffprobe, &raw).await {
            // (A stretch read from the VOD is placed by its replay, which
            // `live_chat` does; drawing only needs to know when its chat has settled.)
            let until = settled_until(&aired, now_start + length, now());
            if until - done.until >= PIECE {
                let to = done.until + PIECE;
                let started = Instant::now();
                match draw_piece(&part, &raw, &drawn, start, done.until, Some(to), &mut |_| {}, &|| false).await {
                    Ok(()) => match append(&drawn, done.bytes).await {
                        Ok(bytes) => {
                            done = Drawn { until: to, bytes };
                            if live_chat::save_json(&state, &done).await.is_err() {
                                return;
                            }
                            log::line(
                                &part.app,
                                &format!(
                                    "chat: drew stretch {} to {to:.0} s in {:.1} s",
                                    part.number,
                                    started.elapsed().as_secs_f64()
                                ),
                            );
                        }
                        Err(why) => return give_up(&part, &state, &why).await,
                    },
                    Err(why) => return give_up(&part, &state, &why).await,
                }
                // Straight on to the next, if it is ready (catching up from the start).
                continue;
            }
        }
        tokio::time::sleep(LOOK_EVERY).await;
    }
}

/// Stops drawing a stretch: it is finished from what was recorded.
async fn give_up(part: &DrawPart, state: &Path, why: &str) {
    let _ = tokio::fs::remove_file(state).await;
    log::line(
        &part.app,
        &format!(
            "chat: drawing stretch {} as it records stopped ({why}); it is drawn when the recording is finished",
            part.number
        ),
    );
}

/// Adds the piece beside `drawn` to it, after cutting it back to `bytes`
/// (a piece added halfway when Siphon closed). Returns the new size.
async fn append(drawn: &Path, bytes: u64) -> Result<u64, String> {
    let piece = piece_path(drawn);
    let appended = async {
        let mut file = tokio::fs::OpenOptions::new().write(true).open(drawn).await?;
        file.set_len(bytes).await?;
        file.seek(std::io::SeekFrom::End(0)).await?;
        let mut from = tokio::fs::File::open(&piece).await?;
        let added = tokio::io::copy(&mut from, &mut file).await?;
        file.flush().await?;
        file.sync_all().await?;
        Ok::<u64, std::io::Error>(bytes + added)
    };
    let result = appended.await.map_err(|err| format!("adding a piece failed: {err}"));
    let _ = tokio::fs::remove_file(&piece).await;
    result
}

/// Draws the chat into stretch `raw` from `from` to `until` (its end, if
/// `None`), into the piece file beside `drawn`. `on_progress` hears how far
/// it has got, in the stretch's seconds; `stopped` says to stop.
#[allow(clippy::too_many_arguments)]
async fn draw_piece(
    part: &DrawPart,
    raw: &Path,
    drawn: &Path,
    start: f64,
    from: f64,
    until: Option<f64>,
    on_progress: &mut impl FnMut(f64),
    stopped: &(dyn Fn() -> bool + Sync),
) -> Result<(), String> {
    let log = live_chat::chat_for_stretch(&part.work, part.number, start)
        .await
        .ok_or("there's no chat to draw")?;
    let picture = picture(&part.ffprobe, raw)
        .await
        .ok_or("the recording couldn't be read")?;
    let fps = part.recipe.output_fps().or(picture.fps).unwrap_or(30.0);
    let layer_fps = layer_fps(fps);
    // The images of what this piece shows, from a minute before (still in the box).
    let geometry = crate::chat::render::Geometry::new(&part.placement, picture.width, picture.height);
    let style = Style::for_width(geometry.width);
    if let Ok(twitch) = Twitch::new() {
        let urls = log.images_used(from - 60.0, until.unwrap_or(f64::MAX), style.emote(), style.badge());
        images::fetch(twitch.http(), &part.images, urls).await;
    }
    let layer = Layer::new(log, &part.placement, picture.width, picture.height, &part.images);
    let geometry = layer.geometry();
    let chat = ChatLayer {
        width: geometry.width,
        height: geometry.height,
        fps: layer_fps,
        x: geometry.x,
        y: geometry.y,
        start: from,
        hd: picture.height >= 720,
        // A piece ends exactly where the next begins, at the stream's own rate.
        end: until,
        rate: Some(fps),
    };
    let piece = piece_path(drawn);
    let args = ffmpeg::draw_piece_args(&part.recipe, raw, start, from, until, &chat, &piece);
    // The layer for no longer than the piece (see `ffmpeg::ChatLayer::end`);
    // the last piece's length is what is there.
    let length = match until {
        Some(until) => until - from,
        None => picture
            .duration
            .map_or(f64::MAX / 4.0, |duration| duration - (from - start)),
    };
    run(&part.ffmpeg, args, layer, from, layer_fps, length, on_progress, stopped).await?;
    // Pieces are joined as they are: one that starts late or overlaps the
    // next would show in the finished file, so it isn't used.
    let times = frame_times(&part.ffprobe, &piece).await;
    piece_fits(&times, from, until, fps)
}

/// When each frame of `file` is shown, in order.
async fn frame_times(ffprobe: &Path, file: &Path) -> Vec<f64> {
    let mut cmd = process::command(ffprobe);
    cmd.args([
        "-v",
        "error",
        "-select_streams",
        "v:0",
        "-show_entries",
        "packet=pts_time",
        "-of",
        "csv=p=0",
    ])
    .arg(file);
    let Ok(Some(output)) = process::output_within(cmd, Duration::from_secs(120)).await else {
        return Vec::new();
    };
    let mut times: Vec<f64> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| line.trim().trim_end_matches(',').parse().ok())
        .collect();
    times.sort_by(f64::total_cmp);
    times
}

/// Whether a piece's frames (`times`) are those from `from` to `until`, at
/// `fps` a second: starting with the first, ending just before the next
/// piece's, none missing. The frames are on the rate's grid (see
/// `ffmpeg::ChatLayer::rate`) and each cut keeps the frame nearest it, so
/// the first is within half a frame of `from` and the last within half a
/// frame of one frame before `until`: a frame lost or doubled at either end
/// is a whole frame out, and fails.
fn piece_fits(times: &[f64], from: f64, until: Option<f64>, fps: f64) -> Result<(), String> {
    /// Leeway for times printed to the microsecond.
    const PRINTED: f64 = 1e-4;
    let frame = 1.0 / fps;
    let near = |time: f64, at: f64| (time - at).abs() <= frame / 2.0 + PRINTED;
    let (Some(&first), Some(&last)) = (times.first(), times.last()) else {
        return Err("the piece has no picture".to_string());
    };
    if !near(first, from) {
        return Err(format!("the piece starts at {first:.4} s, not {from:.4} s"));
    }
    if let Some(until) = until {
        let wanted = ((until - from) * fps).round() as usize;
        if !near(last, until - frame) || times.len().abs_diff(wanted) > 1 {
            return Err(format!(
                "the piece has {} frames to {last:.4} s, not {wanted} to just before {until:.4} s",
                times.len()
            ));
        }
    }
    Ok(())
}

/// The size, frame rate and length of `file`'s picture.
async fn picture(ffprobe: &Path, file: &Path) -> Option<ffmpeg::Picture> {
    let mut cmd = process::command(ffprobe);
    cmd.args(ffmpeg::picture_probe_args(file));
    let output = process::output_within(cmd, Duration::from_secs(60)).await.ok()??;
    ffmpeg::Picture::parse(&String::from_utf8_lossy(&output.stdout))
}

/// Runs ffmpeg with `args` while a thread feeds it `layer`'s frames on its
/// standard input, from `from` seconds, `fps` a second, for `length`.
/// `on_progress` hears how far ffmpeg has got, on the
/// recording's clock. Killed if dropped (the recording ended), or once
/// `stopped` says so (Siphon is closing).
#[allow(clippy::too_many_arguments)]
async fn run(
    ffmpeg_path: &Path,
    args: Vec<std::ffi::OsString>,
    mut layer: Layer,
    from: f64,
    fps: f64,
    length: f64,
    on_progress: &mut impl FnMut(f64),
    stopped: &(dyn Fn() -> bool + Sync),
) -> Result<(), String> {
    let (reader, mut writer) = std::io::pipe().map_err(|err| err.to_string())?;
    let mut cmd = process::command(ffmpeg_path);
    cmd.args(args)
        .stdin(Stdio::from(reader))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|err| format!("ffmpeg didn't start: {err}"))?;
    process::end_with_siphon(&child);
    let pid = child.id();
    let frames = (length * fps).floor().clamp(1.0, u64::MAX as f64) as u64;
    let drawing = std::thread::spawn(move || {
        for index in 0..frames {
            if writer.write_all(layer.frame(from + index as f64 / fps)).is_err() {
                break;
            }
        }
    });
    let (stdout, stderr) = (child.stdout.take(), child.stderr.take());
    let errors = tokio::spawn(async move {
        let mut text = String::new();
        if let Some(stderr) = stderr {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                text.push_str(&line);
                text.push('\n');
            }
        }
        text
    });
    let (sender, mut progress) = tokio::sync::mpsc::unbounded_channel();
    let reading = tokio::spawn(async move {
        if let Some(stdout) = stdout {
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if let Some(done) = ffmpeg::parse_progress(&line) {
                    let _ = sender.send(done);
                }
            }
        }
    });
    let mut killed = false;
    let wait = child.wait();
    tokio::pin!(wait);
    let status = loop {
        tokio::select! {
            status = &mut wait => break status.map_err(|err| err.to_string())?,
            Some(done) = progress.recv() => on_progress(done),
            () = tokio::time::sleep(Duration::from_millis(500)), if !killed => {
                if stopped() {
                    killed = true;
                    if let Some(pid) = pid {
                        process::kill_tree(pid).await;
                    }
                }
            }
        }
    };
    let _ = reading.await;
    while let Ok(done) = progress.try_recv() {
        on_progress(done);
    }
    let errors = errors.await.unwrap_or_default();
    let _ = tokio::task::spawn_blocking(move || drawing.join()).await;
    if killed {
        Err("stopped".to_string())
    } else if status.success() {
        Ok(())
    } else {
        let last = errors.lines().rev().take(3).collect::<Vec<_>>().join(" / ");
        Err(format!("ffmpeg failed: {last}"))
    }
}

/// Draws what is left of each stretch once the recording has ended (all of
/// a stretch that wasn't drawn while recording: a short one, or after
/// Siphon restarted), and returns the drawn files in the order of `raws`;
/// `None` if any couldn't be drawn. `on_progress` hears how far it has got (0–1).
#[allow(clippy::too_many_arguments)]
pub(super) async fn finish(
    app: &AppHandle,
    work: &Path,
    numbers: &[usize],
    raws: &[PathBuf],
    ffmpeg_path: &Path,
    ffprobe: &Path,
    recipe: &Recipe,
    placement: Placement,
    images: &Path,
    on_progress: &mut impl FnMut(f64),
    stopped: &(dyn Fn() -> bool + Sync),
) -> Option<Vec<PathBuf>> {
    // Every stretch, with how far it was drawn.
    let mut drawing = Vec::new();
    for (&number, raw) in numbers.iter().zip(raws) {
        let drawn = drawn_path(raw, number)?;
        let (start, length) = live_chat::file_span(ffprobe, raw).await?;
        let done = match live_chat::load_json::<Drawn>(&state_file(work, number)).await {
            Some(done) if drawn.exists() => done,
            _ => {
                tokio::fs::write(&drawn, b"").await.ok()?;
                Drawn { until: start, bytes: 0 }
            }
        };
        drawing.push((number, raw.clone(), drawn, start, start + length, done));
    }
    let left: f64 = drawing
        .iter()
        .map(|(.., start, end, done)| (end - done.until.max(*start)).max(0.0))
        .sum();
    let started = Instant::now();
    let mut drawn_before = 0.0;
    let mut files = Vec::new();
    for (number, raw, drawn, start, end, done) in drawing {
        let part = DrawPart {
            app: app.clone(),
            work: work.to_path_buf(),
            number,
            dir: PathBuf::new(),
            needle: String::new(),
            ffmpeg: ffmpeg_path.to_path_buf(),
            ffprobe: ffprobe.to_path_buf(),
            recipe: recipe.clone(),
            placement,
            images: images.to_path_buf(),
        };
        if end - done.until > 0.1 {
            let from = done.until;
            let drew = draw_piece(
                &part,
                &raw,
                &drawn,
                start,
                from,
                None,
                &mut |at| {
                    if left > 0.0 {
                        on_progress(((drawn_before + (at - from).max(0.0)) / left).min(1.0));
                    }
                },
                stopped,
            )
            .await;
            if let Err(why) = drew {
                log::line(
                    app,
                    &format!("chat: the rest of stretch {number} couldn't be drawn ({why})"),
                );
                return None;
            }
            if let Err(why) = append(&drawn, done.bytes).await {
                log::line(
                    app,
                    &format!("chat: the rest of stretch {number} couldn't be added ({why})"),
                );
                return None;
            }
            drawn_before += end - from;
        } else {
            // Nothing left: only a piece cut short to cut off.
            let file = tokio::fs::OpenOptions::new().write(true).open(&drawn).await.ok()?;
            file.set_len(done.bytes).await.ok()?;
        }
        files.push(drawn);
    }
    log::line(
        app,
        &format!(
            "chat: drew the last {left:.0} s after the recording ended, in {:.1} s",
            started.elapsed().as_secs_f64()
        ),
    );
    Some(files)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_drawn_file_sits_beside_the_recorded_one() {
        let dir = Path::new(r"C:\Videos");
        assert_eq!(
            drawn_path(&dir.join("Show.siphon-1a2b3c4d.rec1.ts"), 1),
            Some(dir.join("Show.siphon-1a2b3c4d.drawn1.ts"))
        );
        // From the start yt-dlp names the extension; and `rec1` is not `rec12`.
        assert_eq!(
            drawn_path(&dir.join("Show.siphon-1a2b3c4d.rec12.mp4"), 12),
            Some(dir.join("Show.siphon-1a2b3c4d.drawn12.ts"))
        );
        assert_eq!(drawn_path(&dir.join("Show.siphon-1a2b3c4d.rec12.mp4"), 1), None);
        // A pair joined in the work folder isn't drawn as it records.
        assert_eq!(drawn_path(&dir.join("part1.mkv"), 1), None);
    }

    #[test]
    fn a_stretch_is_drawn_once_its_chat_has_settled_and_short_of_its_end() {
        // The file's first timestamp, 1.4, aired at 1000.
        let start = 1.4;
        let mut aired = live_chat::Aired {
            at: start,
            aired: 1000.0,
            exact: true,
            vod: None,
            cuts: Vec::new(),
        };
        // Five minutes on: what aired over two minutes and a bit ago.
        let until = settled_until(&aired, 1000.0, 1300.0);
        assert!((until - (start + 300.0 - SETTLE)).abs() < 1e-9, "{until}");
        const { assert!(SETTLE >= REPLAY_LEAD + CLEARED_SPAN) };
        // Recorded from the start, long settled: as far as the file goes, less a margin.
        assert!((settled_until(&aired, 500.0, 99_999.0) - (500.0 - BEHIND_END)).abs() < 1e-9);
        // A minute's ad left out at 100.0: what follows aired a minute later,
        // and settles a minute later.
        aired.cuts.push(live_chat::Cut {
            at: 100.0,
            aired: 1158.6,
        });
        let until = settled_until(&aired, 1000.0, 1158.6 + SETTLE + 20.0);
        assert!((until - 120.0).abs() < 1e-9, "{until}");
    }

    #[test]
    fn a_piece_is_only_used_if_it_is_exactly_its_frames() {
        let frames = |from: f64, count: usize| -> Vec<f64> { (0..count).map(|i| from + i as f64 / 60.0).collect() };
        // Two minutes at 60 frames a second, from 121.467 s.
        assert_eq!(
            piece_fits(&frames(121.466_667, 7200), 121.467, Some(241.467), 60.0),
            Ok(())
        );
        // Ten frames late (a seek past a key frame), or two frames too many (the overlap at a join).
        assert!(piece_fits(&frames(121.633_333, 7190), 121.467, Some(241.467), 60.0).is_err());
        assert!(piece_fits(&frames(121.466_667, 7202), 121.467, Some(241.467), 60.0).is_err());
        // One frame lost at the start, or at the end, as a cut after the
        // overlay lost it at every join of a Twitch recording from the start.
        assert!(piece_fits(&frames(121.483_333, 7199), 121.467, Some(241.467), 60.0).is_err());
        assert!(piece_fits(&frames(121.466_667, 7199), 121.467, Some(241.467), 60.0).is_err());
        // Cuts off the rate's grid keep the frame nearest them.
        let off_grid = 121.467 + 0.4 / 60.0;
        assert_eq!(
            piece_fits(&frames(121.466_667, 7200), off_grid, Some(off_grid + 120.0), 60.0),
            Ok(())
        );
        // The last piece runs to the end of what was recorded.
        assert_eq!(piece_fits(&frames(241.466_667, 500), 241.467, None, 60.0), Ok(()));
        assert!(piece_fits(&[], 1.0, None, 60.0).is_err());
    }

    #[tokio::test]
    async fn a_piece_is_added_after_cutting_off_one_that_was_cut_short() {
        let dir = std::env::temp_dir().join(format!("siphon-draw-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let drawn = dir.join("Show.siphon-ab.drawn1.ts");
        // Two good pieces, then half of a third when Siphon closed.
        std::fs::write(&drawn, b"aaaabbbbcc").unwrap();
        std::fs::write(piece_path(&drawn), b"cccc").unwrap();
        assert_eq!(append(&drawn, 8).await, Ok(12));
        assert_eq!(std::fs::read(&drawn).unwrap(), b"aaaabbbbcccc");
        assert!(!piece_path(&drawn).exists(), "the piece is removed once added");
        // Nothing to add: said so, and the drawn file is left alone.
        assert!(append(&drawn, 12).await.is_err());
        assert_eq!(std::fs::read(&drawn).unwrap().len(), 12);
        let _ = std::fs::remove_dir_all(dir);
    }
}
