//! Measures a video's download size in the background, where the plan can
//! only estimate it: the bytes of a trimmed part ([`parts`]), what a smart
//! cut's re-encoded ends add to them ([`cut_ends`]), and what a conversion
//! comes to, from stretches of the video encoded as it will be
//! ([`samples`]). What it finds
//! is kept with the video (`plan::sizes`), and every plan of it goes by it
//! from then on. See docs/decisions.md#size-estimates.
//!
//! Nothing ever waits for it. It runs while the user is still choosing, one
//! measurement at a time, and stops when they download, choose something
//! else or leave the video; whatever it has found by then is kept.

mod parts;
mod samples;
mod stretch;

use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use futures_util::stream::{self as items, StreamExt};

use tokio::sync::oneshot;

use crate::binaries::BinaryManager;
use crate::error::Result;
use crate::ffmpeg::encoders::Hardware;
use crate::ffmpeg::smart_cut;
use crate::ffmpeg::Pass;
use crate::plan::sizes::{SizeFacts, Span};
use crate::plan::{self, AudioAction, AudioStep, Capabilities, Choice, Recipe, Trim};
use crate::throttle::{Flow, Throttle};
use crate::ytdlp::metadata::{Format, FormatKind, VideoInfo};
use samples::Sums;
use stretch::{Encoding, Stretch};

/// Measuring a conversion starts no more stretches after this long; what
/// was measured by then is used if it is enough.
const SAMPLE_BUDGET: Duration = Duration::from_secs(40);
/// Stretches encoded at once on a graphics card (its encoder takes several
/// at once; a consumer NVIDIA card eight), and on the CPU.
const AT_ONCE_ON_HARDWARE: usize = 3;

/// The measurement running now, if any.
#[derive(Default)]
pub struct Sizing {
    /// Its number, and what ends it: dropping the sender.
    running: Mutex<Option<(u64, oneshot::Sender<()>)>>,
    next: AtomicU64,
}

impl Sizing {
    /// Ends the measurement running now, if any.
    pub fn stop(&self) {
        self.running.lock().unwrap().take();
    }

    /// Runs `work` as the one measurement, ending the one before. `None` if
    /// it was ended itself before it finished.
    async fn alone<T>(&self, work: impl Future<Output = T>) -> Option<T> {
        let (sender, ended) = oneshot::channel::<()>();
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        *self.running.lock().unwrap() = Some((id, sender));
        // Dropping `work` ends whatever it started (see `process::command`).
        let result = tokio::select! {
            result = work => Some(result),
            _ = ended => None,
        };
        let mut running = self.running.lock().unwrap();
        if running.as_ref().is_some_and(|(current, _)| *current == id) {
            *running = None;
        }
        result
    }

    /// Measures what the plan for `choice` (and `trim`) of `video` could only
    /// estimate, handing each finding to `found` as soon as it is made.
    /// Nothing is measured for a stream as it airs, or for a video that
    /// needed signing in (its streams need the sign-in too).
    #[allow(clippy::too_many_arguments)]
    pub async fn measure(
        &self,
        bins: &BinaryManager,
        throttle: &Throttle,
        video: &VideoInfo,
        choice: &Choice,
        trim: Option<Trim>,
        caps: &Capabilities,
        found: impl Fn(&SizeFacts),
        note: impl Fn(&str),
    ) -> Result<Measure> {
        if video.with_account {
            return Ok(Measure::Nothing);
        }
        let recipe = plan::plan(video, choice, trim, caps)?;
        let wanted = video.sizes.wanted(&recipe, video);
        if wanted.is_empty() {
            return Ok(Measure::Nothing);
        }
        let flow = throttle.quiet_flow().await?;
        let Some(duration) = video.duration else {
            return Ok(Measure::Nothing);
        };
        let span = Span::of(recipe.trim);
        let (start, end) = span.seconds(duration);
        let (ffmpeg, ffprobe) = (bins.ffmpeg(), bins.ffprobe());
        let (ffmpeg, ffprobe, flow, found, note) = (&ffmpeg, &ffprobe, &flow, &found, &note);
        let format = recipe
            .video
            .as_ref()
            .and_then(|step| video.formats.iter().find(|format| format.id == step.format_id));
        // The request size the site expects (YouTube's 10 MB), where ffmpeg can be told it.
        let chunk = format
            .and_then(|format| format.http_chunk_size)
            .filter(|_| caps.chunked_requests);
        // The streams' bytes in the part, the cut's ends and the conversion, side by side.
        let streams = async {
            let mut any = false;
            let mut picture = None;
            {
                let reads = wanted.parts.iter().filter_map(|id| {
                    let format = video.formats.iter().find(|format| &format.id == id)?;
                    Some(async move {
                        let bytes = parts::bytes(ffprobe, flow, format, start, end, duration).await;
                        (id, bytes)
                    })
                });
                for (id, bytes) in futures_util::future::join_all(reads).await {
                    note(&format!(
                        "size of {}: stream {id} from {start:.2} to {end:.2} s: {}",
                        video.title,
                        bytes.map_or("couldn't be read".to_string(), |bytes| format!("{bytes} bytes"))
                    ));
                    if let Some(bytes) = bytes {
                        let mut facts = SizeFacts::default();
                        facts.add_part(id, span, bytes);
                        found(&facts);
                        any = true;
                        if format.is_some_and(|format| &format.id == id) {
                            picture = Some(bytes);
                        }
                    }
                }
            }
            (any, picture)
        };
        let ends = async {
            let (true, Some(format)) = (wanted.cut_ends, format) else {
                return false;
            };
            let ends = cut_ends(ffmpeg, flow, format, chunk, &recipe, start, end).await;
            note(&format!(
                "size of {}: the cut's ends {}",
                video.title,
                ends.as_ref()
                    .map_or("couldn't be measured".to_string(), |(_, how)| how.clone())
            ));
            let Some((bytes, _)) = ends else {
                return false;
            };
            let mut facts = SizeFacts::default();
            facts.add_cut_ends(&format.id, span, bytes);
            found(&facts);
            true
        };
        // Not while one of Siphon's own encodes runs: it would slow it, on the same encoder.
        let busy = wanted.encode && crate::downloads::encoding_now();
        if busy {
            note(&format!(
                "size of {}: the conversion waits, as another is running",
                video.title
            ));
        }
        let conversion = async {
            let (true, false, Some(format)) = (wanted.encode, busy, format) else {
                return None;
            };
            let begun = Instant::now();
            let sums = sample(ffmpeg, flow, format, chunk, &recipe, start, end).await;
            Some((sums, begun.elapsed()))
        };
        let work = async {
            // A tool update waits until this is done.
            let _in_use = bins.in_use().await;
            let ((streams, picture), ends, conversion) = tokio::join!(streams, ends, conversion);
            // The picture's bytes in the part: read now, or known from before.
            let picture = picture.or_else(|| format.and_then(|format| video.sizes.part(&format.id, span)));
            let mut converted = false;
            if let (Some((sums, took)), Some(format)) = (conversion, format) {
                let result = sums.and_then(|sums| {
                    let factor = encode_factor(&recipe, format, sums, picture, start, end)
                        .ok_or_else(|| "the rule of thumb has nothing to go by".to_string())?;
                    Ok::<_, String>((sums, factor))
                });
                note(&match &result {
                    Ok((sums, factor)) => format!(
                        "size of {}: the conversion, measured on {} stretches ({:.0} s of video) in {:.1} s, comes to {factor:.2} times the rule of thumb",
                        video.title,
                        sums.count,
                        sums.seconds,
                        took.as_secs_f64()
                    ),
                    Err(error) => format!("size of {}: the conversion couldn't be measured: {error}", video.title),
                });
                if let Ok((_, factor)) = result {
                    let mut facts = SizeFacts::default();
                    facts.add_encode(&recipe, factor);
                    found(&facts);
                    converted = true;
                }
            }
            streams || ends || converted
        };
        Ok(match self.alone(work).await {
            Some(true) => Measure::Found,
            // Nothing found where something was wanted: the encoder was busy,
            // or the site refused (YouTube does, now and then). Tried again
            // when the choice is back.
            Some(false) => Measure::Stopped,
            None => Measure::Stopped,
        })
    }
}

/// Encodes stretches of the part from `start` to `end` as `recipe` encodes
/// the video (see [`samples`]), a few at a time, starting no more once
/// [`SAMPLE_BUDGET`] is spent. The first error, when too few could be measured.
async fn sample(
    ffmpeg: &std::path::Path,
    flow: &Flow,
    format: &Format,
    chunk: Option<u64>,
    recipe: &Recipe,
    start: f64,
    end: f64,
) -> std::result::Result<Sums, String> {
    let (Some(step), Some(encode)) = (recipe.video.as_ref(), recipe.video_encode()) else {
        return Err("nothing is encoded".into());
    };
    let (count, length, at_once) = if encode.hardware == Hardware::Software {
        (samples::ON_CPU, samples::SECONDS_ON_CPU, 1)
    } else {
        (samples::ON_HARDWARE, samples::SECONDS_ON_HARDWARE, AT_ONCE_ON_HARDWARE)
    };
    let filters = crate::ffmpeg::video_filters(step, encode, None, None);
    let encoder = crate::ffmpeg::video_encode_args(step, encode, Pass::Only);
    let encoding = &Encoding {
        filters: &filters,
        encoder: &encoder,
    };
    let spans = samples::spans(start, end, count, length);
    let needed = spans.len().min(3);
    let begun = Instant::now();
    let measured: Vec<std::result::Result<(Stretch, f64, f64), String>> = items::iter(spans)
        .take_while(|_| std::future::ready(begun.elapsed() < SAMPLE_BUDGET))
        .map(|(from, until)| async move {
            let read = stretch::read(ffmpeg, flow, format, chunk, from, until, Some(encoding)).await;
            read.map(|read| (read, from, until))
        })
        .buffer_unordered(at_once)
        .collect()
        .await;
    let mut sums = Sums::default();
    let mut failed = None;
    for result in measured {
        match result {
            Ok((read, from, until)) => sums.add(&read, from, until),
            Err(error) => {
                failed.get_or_insert(error);
            }
        }
    }
    if sums.count >= needed {
        Ok(sums)
    } else {
        Err(failed.unwrap_or_else(|| "too few stretches in time".into()))
    }
}

/// What `recipe`'s encode comes to as measured (`sums`), as a multiple of the
/// rule of thumb's likely size. `picture`: the stream's bytes in the part, if
/// they were measured; otherwise its stated size, scaled to the part.
fn encode_factor(
    recipe: &Recipe,
    format: &Format,
    sums: Sums,
    picture: Option<u64>,
    start: f64,
    end: f64,
) -> Option<f64> {
    let step = recipe.video.as_ref()?;
    let source = picture.or_else(|| step.size.map(|size| (size as f64 * recipe.source_fraction) as u64));
    // A stream with its sound in it, the sound copied: it counts on both sides.
    let keeps_sound = format.kind == FormatKind::Muxed
        && !recipe.strip_audio
        && matches!(
            recipe.audio,
            Some(AudioStep {
                format_id: None,
                action: AudioAction::Copy,
                ..
            })
        );
    // Drawing the chat in, which the stretches leave out, measured to add
    // next to nothing: measured without it, a 2-minute Twitch clip drawn with
    // it came within 0.7% (2026-11-22).
    let whole = sums.whole(source, keeps_sound, end - start)?;
    let rule = recipe.rule_of_thumb_video().filter(|rule| *rule > 0.0)?;
    Some(whole / rule)
}

/// How far past a cut a key frame is looked for: YouTube's come every 2 to
/// 7 seconds, Twitch's every 2.
const KEY_LOOK: f64 = 8.0;

/// What a smart cut's re-encoded ends add to the copied part, in bytes: the
/// frames from `start` to the first key frame inside the part, and from the
/// last key frame to `end`, encoded as the cut encodes them (visually
/// lossless, so larger: measured 2026-11-22, a 30 s 1080p60 YouTube clip's
/// 4.4 s of ends took 7.5 MB more than they had), less what they were. A
/// part with no whole stretch between two key frames is encoded whole, as
/// the cut then does.
async fn cut_ends(
    ffmpeg: &std::path::Path,
    flow: &Flow,
    format: &Format,
    chunk: Option<u64>,
    recipe: &Recipe,
    start: f64,
    end: f64,
) -> Option<(i64, String)> {
    let step = recipe.video.as_ref()?;
    let cut = recipe.cut_encode.as_ref()?;
    let mut edges = smart_cut::edge_encoder(cut.codec, cut.hardware);
    let pixels = if cut.hardware == Hardware::Qsv {
        "nv12"
    } else {
        "yuv420p"
    };
    edges.extend(["-pix_fmt".to_string(), pixels.to_string()]);
    let edge = Encoding {
        filters: &[],
        encoder: &edges,
    };
    let added = |stretch: &Stretch, from: f64, until: f64| {
        stretch::bytes(&stretch.encoded, from, until) as i64 - stretch::bytes(&stretch.video, from, until) as i64
    };
    // Copying starts at the first key frame inside the part that decoding
    // restarts at, and stops before the last key frame of any kind.
    let look_until = (start + KEY_LOOK).min(end);
    let tail_from = (end - KEY_LOOK).max(start);
    let (restarts, around_end) = tokio::join!(
        stretch::restarts(ffmpeg, flow, format, chunk, start, look_until, step.source),
        stretch::read(ffmpeg, flow, format, chunk, tail_from, end, None),
    );
    let first_key = restarts?
        .into_iter()
        .filter(|pts| *pts >= start - 0.0005)
        .reduce(f64::min);
    let last_key = around_end
        .ok()?
        .video
        .iter()
        .filter(|packet| packet.key && packet.pts < end - 0.0005)
        .map(|packet| packet.pts)
        .reduce(f64::max);
    match (first_key, last_key) {
        (Some(first), Some(last)) if first < last => {
            let (head, tail) = tokio::join!(
                stretch::read(ffmpeg, flow, format, chunk, start, first, Some(&edge)),
                stretch::read(ffmpeg, flow, format, chunk, last, end, Some(&edge)),
            );
            let (head, tail) = (added(&head.ok()?, start, first), added(&tail.ok()?, last, end));
            Some((
                head + tail,
                format!("add {head} bytes from {start:.2} to the key frame at {first:.2}, {tail} from {last:.2}"),
            ))
        }
        _ => {
            // Nothing to copy: the whole part is encoded, as a full cut encodes it.
            let full = recipe.with_full_cut();
            let encode = full.video_encode()?;
            let filters = crate::ffmpeg::video_filters(step, encode, None, None);
            let encoder = crate::ffmpeg::video_encode_args(step, encode, Pass::Only);
            let whole = Encoding {
                filters: &filters,
                encoder: &encoder,
            };
            let all = stretch::read(ffmpeg, flow, format, chunk, start, end, Some(&whole))
                .await
                .ok()?;
            let added = added(&all, start, end);
            Some((added, format!("add {added} bytes: no key frames to copy between")))
        }
    }
}

/// How a measurement went.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Measure {
    /// It found something: planned again, the size says more.
    Found,
    /// There was nothing to measure.
    Nothing,
    /// It didn't finish: it was ended (another began, a download started,
    /// the video was left), or what it read was refused. What it found by
    /// then is kept, and the rest is tried again when the choice is back.
    Stopped,
}

/// A plain format for tests.
#[cfg(test)]
pub(crate) fn test_format(id: &str) -> crate::ytdlp::metadata::Format {
    crate::ytdlp::metadata::Format {
        id: id.into(),
        ext: "mp4".into(),
        kind: crate::ytdlp::metadata::FormatKind::Video,
        vcodec: None,
        acodec: None,
        video_codec: None,
        audio_codec: None,
        width: None,
        height: None,
        fps: None,
        tbr: None,
        vbr: None,
        abr: None,
        asr: None,
        audio_channels: None,
        filesize: None,
        filesize_estimated: false,
        dynamic_range: None,
        language: None,
        language_preference: None,
        segmented: false,
        stream_url: None,
        http_chunk_size: None,
        http_headers: Vec::new(),
        note: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_new_measurement_ends_the_one_before() {
        let sizing = std::sync::Arc::new(Sizing::default());
        let first = {
            let sizing = std::sync::Arc::clone(&sizing);
            tokio::spawn(async move { sizing.alone(std::future::pending::<()>()).await })
        };
        tokio::task::yield_now().await;
        while sizing.running.lock().unwrap().is_none() {
            tokio::task::yield_now().await;
        }
        assert_eq!(sizing.alone(async { 7 }).await, Some(7));
        assert_eq!(first.await.unwrap(), None);
        // And stopping ends one.
        let second = {
            let sizing = std::sync::Arc::clone(&sizing);
            tokio::spawn(async move { sizing.alone(std::future::pending::<()>()).await })
        };
        while sizing.running.lock().unwrap().is_none() {
            tokio::task::yield_now().await;
        }
        sizing.stop();
        assert_eq!(second.await.unwrap(), None);
    }

    #[test]
    fn a_span_runs_in_seconds() {
        use crate::plan::sizes::Span;
        let part = Span::of(Some(Trim {
            start: 120.0,
            end: 180.5,
        }));
        assert_eq!(part.seconds(635.0), (120.0, 180.5));
        assert_eq!(Span::Whole.seconds(635.0), (0.0, 635.0));
    }
}
