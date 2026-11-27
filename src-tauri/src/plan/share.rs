//! Share: the best quality that fits under an upload limit.
//!
//! 1. If an original H.264 + AAC stream already fits, and is at least as sharp
//!    as an encode would be, it is copied: no re-encoding at all.
//! 2. Otherwise the limit (with a safety margin) divided by the length gives a
//!    bitrate. Audio gets a small share of it, video the rest.
//! 3. The resolution and frame rate are picked by bits per pixel: at the same
//!    size, a clean 720p30 looks better than a blocky 1080p60. Never upscaled.
//! 4. The file is H.264 + AAC in MP4 with the index at the front, so it plays
//!    inline in the chat. The download checks the result against the limit
//!    and encodes again, a little smaller, if it came out over.
//! 5. When even the smallest picture would look broken, the plan is blocked
//!    with a suggestion to trim, rather than making a pixelated file.
//!
//! Every figure uses the trimmed length when a part is selected.

use super::codecs::{AudioCodec, Container, VideoCodec};
use super::registry::{Target, Tier};
use super::{max_by_key, scaled_to, warning, AudioAction, AudioStep, Planner, Recipe, Video, VideoAction, VideoEncode};

/// Aim this far under the limit: services count megabytes differently, and
/// an encoder's bitrate is an average, not a promise.
pub const SAFETY: f64 = 0.95;
/// About 2% of an MP4 at these sizes is container overhead.
const OVERHEAD: f64 = 0.98;
/// Below this many bits per pixel H.264 turns visibly blocky, and a smaller
/// picture at the same size looks better.
const GOOD_BPP: f64 = 0.07;
/// Below this even the smallest picture falls apart.
const MIN_BPP: f64 = 0.03;
/// More than this doesn't show, so a generous limit isn't filled for nothing.
const MAX_BPP: f64 = 0.2;
/// Resolutions to choose from, largest first (named as `lines_for` names them).
const LADDER: [u32; 8] = [2160, 1440, 1080, 720, 540, 480, 360, 240];

/// A resolution and frame rate to encode at.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Rung {
    pub lines: u32,
    pub fps: f64,
    /// Bits per pixel the budget allows at this size.
    pub bpp: f64,
}

/// The audio bitrate for a total budget: generous when there is room, lean when tight.
pub(super) fn audio_kbps(total_kbps: f64) -> u32 {
    match total_kbps {
        t if t >= 1500.0 => 128,
        t if t >= 600.0 => 96,
        t if t >= 250.0 => 64,
        _ => 48,
    }
}

/// The source's frame, width by height, when known; else a 16:9 one of its resolution.
fn frame_of(video: &Video) -> (u32, u32) {
    match (video.format.width, video.format.height) {
        (Some(w), Some(h)) if w > 0 && h > 0 => (w, h),
        _ => ((f64::from(video.lines) * 16.0 / 9.0).round() as u32, video.lines),
    }
}

/// Pixels in a frame of `frame` scaled down to `lines`, as the encode makes it.
fn pixels_at(frame: (u32, u32), lines: u32) -> f64 {
    let (w, h) = scaled_to(frame.0, frame.1, lines);
    f64::from(w) * f64::from(h)
}

/// The best-looking size and frame rate for `video_kbps`, never above the
/// source (`source_lines`, `source_fps`, its `frame`) or the service's own
/// cap. `None` when even the smallest would look broken.
pub(super) fn pick_rung(
    video_kbps: f64,
    source_lines: u32,
    source_fps: f64,
    frame: (u32, u32),
    cap: u32,
) -> Option<Rung> {
    let top = source_lines.min(cap).max(1);
    let mut lines: Vec<u32> = LADDER.iter().copied().filter(|lines| *lines < top).collect();
    lines.insert(0, top);
    let frame_rates: Vec<f64> = if source_fps > 30.5 {
        vec![source_fps, source_fps / 2.0]
    } else {
        vec![source_fps.max(1.0)]
    };
    let mut rungs: Vec<Rung> = lines
        .iter()
        .flat_map(|&lines| {
            frame_rates.iter().map(move |&fps| {
                let pixels = pixels_at(frame, lines);
                Rung {
                    lines,
                    fps,
                    bpp: video_kbps * 1000.0 / (pixels * fps),
                }
            })
        })
        .collect();
    // Sharpness first: a chat window shows detail before smoothness, so a
    // larger picture at half the frame rate goes before a smaller one at full.
    rungs.sort_by(|a, b| b.lines.cmp(&a.lines).then(b.fps.total_cmp(&a.fps)));
    rungs
        .iter()
        .copied()
        .find(|rung| rung.bpp >= GOOD_BPP)
        .or_else(|| rungs.last().copied().filter(|rung| rung.bpp >= MIN_BPP))
}

/// "10 MB", "2 GB".
fn megabytes(mb: u32) -> String {
    if mb >= 1000 && mb.is_multiple_of(1000) {
        format!("{} GB", mb / 1000)
    } else {
        format!("{mb} MB")
    }
}

impl Planner<'_> {
    pub(super) fn share(&self, target: &Target, tier: &Tier) -> Recipe {
        let limit = tier.limit_bytes();
        let budget = limit as f64 * SAFETY;
        let promise = if tier.inline {
            format!("Plays right in {}", target.name)
        } else {
            format!("Sent as a document on {}", target.name)
        };
        let mut recipe = self.recipe(Container::Mp4, promise);
        recipe.size_limit = Some(limit);
        recipe.fits = Some(format!("fits {} ({})", target.name, megabytes(tier.limit_mb)));
        let pool = self.pool(tier.max_lines, 60.0);
        let aac = max_by_key(
            self.audios
                .iter()
                .copied()
                .filter(|audio| audio.codec == AudioCodec::Aac && audio.kbps <= 160.0),
            |audio| audio.rank(),
        );

        let Some(duration) = self.duration else {
            // Without a length there is no budget: take a small original and say so. One that comes
            // out over the limit is converted to fit, planned then from the file (`fit_downloaded`).
            let video = max_by_key(Self::at_resolution(&pool, 480), |video| video.bitrate_rank())
                .expect("plan() checks that videos exist");
            recipe.audio = self.audio_for(&video, &[AudioCodec::Aac], AudioCodec::Aac);
            recipe.video = Some(Self::video_step(&video, VideoAction::Copy));
            recipe.hints.push(warning(
                "Length unknown",
                format!(
                    "The site didn’t say how long this video is. If the original comes out over {}, Siphon converts it to fit.",
                    megabytes(tier.limit_mb)
                ),
            ));
            return recipe;
        };

        let total_kbps = budget * 8.0 / duration / 1000.0;
        let audio_kbps = audio_kbps(total_kbps);
        let video_kbps = (total_kbps * OVERHEAD - f64::from(audio_kbps)).max(1.0);
        let best = max_by_key(pool.iter().copied(), |video| (video.lines, video.fps as u32))
            .expect("plan() checks that videos exist");
        let frame = frame_of(&best);
        let rung = pick_rung(video_kbps, best.lines, best.fps, frame, tier.max_lines);

        // First choice: an original that already fits, and is no less sharp than the encode.
        // A video with no sound at all (a screen recording, a clip posted as a GIF) needs no AAC.
        let silent = self.audios.is_empty();
        let fits = |video: &Video| {
            let audio = if video.has_audio() { None } else { aac.as_ref() };
            let carries_aac = if video.has_audio() {
                AudioCodec::of(video.format) == AudioCodec::Aac
            } else {
                aac.is_some() || silent
            };
            carries_aac
                && video
                    .size_with(audio)
                    .is_some_and(|size| size as f64 * self.source_fraction <= budget)
        };
        let copyable = pool
            .iter()
            .copied()
            .filter(|video| video.codec == VideoCodec::H264)
            .filter(|video| rung.is_none_or(|rung| video.lines >= rung.lines))
            .filter(fits);
        let copied = max_by_key(copyable, |video| (video.lines, video.fps as u32, video.bitrate_rank()));

        let Some(rung) = rung else {
            // Too long to look decent: plan the smallest version so the summary
            // can say what it would be, but don't offer to download it.
            let smallest = pick_rung(video_kbps, best.lines, best.fps, frame, 240).unwrap_or(Rung {
                lines: best.lines.min(240),
                fps: best.fps.min(30.0),
                bpp: 0.0,
            });
            self.encode_for_share(&mut recipe, &pool, smallest, video_kbps, audio_kbps);
            recipe.blocked = Some(format!(
                "This video is too long to look decent under {}. Trim it to the part you want, or pick a larger limit.",
                megabytes(tier.limit_mb)
            ));
            return recipe;
        };

        if let Some(video) = copied {
            let mut fallback = recipe.clone();
            self.encode_for_share(&mut fallback, &[video], rung, video_kbps, audio_kbps);
            recipe.audio = self.audio_for(&video, &[AudioCodec::Aac], AudioCodec::Aac);
            if let (Some(step), Some(track)) = (recipe.audio.as_mut(), aac.filter(|_| !video.has_audio())) {
                step.format_id = Some(track.format.id.clone());
                step.source_kbps = Some(track.kbps);
                step.size = track.format.filesize;
            }
            recipe.video = Some(Self::video_step(&video, VideoAction::Copy));
            recipe.fallback = Some(Box::new(fallback));
            return recipe;
        }

        self.encode_for_share(&mut recipe, &pool, rung, video_kbps, audio_kbps);
        recipe.reason = Some(format!("no original stream fits under {}", megabytes(tier.limit_mb)));
        if rung.bpp < GOOD_BPP {
            recipe.hints.push(warning(
                "Will look soft",
                format!(
                    "This is long for {}, so it will look soft. Trimming it, or a larger limit, will help.",
                    megabytes(tier.limit_mb)
                ),
            ));
        }
        recipe
    }

    /// Fills `recipe` with an H.264 + AAC encode at `rung`, from the smallest
    /// stream in `pool` that still covers it.
    fn encode_for_share(&self, recipe: &mut Recipe, pool: &[Video], rung: Rung, video_kbps: f64, audio_kbps: u32) {
        let covering = pool.iter().map(|v| v.lines).filter(|lines| *lines >= rung.lines).min();
        let source_lines = covering.or_else(|| pool.iter().map(|v| v.lines).max()).unwrap_or(0);
        let video = max_by_key(pool.iter().copied().filter(|v| v.lines == source_lines), |video| {
            (
                video.fps >= rung.fps - 0.5,
                video.codec == VideoCodec::H264,
                video.bitrate_rank(),
            )
        })
        .expect("the pool is not empty");

        let pixels = pixels_at(frame_of(&video), rung.lines);
        let ceiling = MAX_BPP * pixels * rung.fps / 1000.0;
        let encode = VideoEncode {
            bitrate_kbps: Some(video_kbps.min(ceiling).max(40.0) as u32),
            scale_lines: (video.lines > rung.lines).then_some(rung.lines),
            max_fps: (video.fps > rung.fps + 0.5).then_some(rung.fps),
            ..self.encode(VideoCodec::H264)
        };
        recipe.audio = self.audio_for(&video, &[], AudioCodec::Aac).map(|step| AudioStep {
            action: AudioAction::Encode {
                codec: AudioCodec::Aac,
                bitrate_kbps: Some(audio_kbps),
                stereo: true,
            },
            ..step
        });
        recipe.video = Some(Self::video_step(&video, VideoAction::Encode(encode)));
    }
}
