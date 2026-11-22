//! Planning a live recording. Pure: no I/O.
//!
//! A live stream has no length, so nothing can be sized in total. Each
//! stream is given the size of one hour at its bitrate, and the ordinary
//! planner does the rest: the estimate it makes is then a size per hour.

use std::time::Duration;

use super::codecs::AudioCodec;
use super::explain::{self, PlanSummary};
use super::registry::Group;
use super::{plan as plan_file, Capabilities, Choice, Hint, HintLevel, Recipe};
use crate::error::{AppError, ErrorKind, Result};
use crate::ytdlp::metadata::{Format, FormatKind, LiveStatus, VideoInfo};

/// The length every estimate is made for: sizes come out per hour.
const HOUR: f64 = 3600.0;

/// A stream's audio whose bitrate the site doesn't state (YouTube's live
/// audio doesn't), in kbit/s: what the usual AAC track carries.
const AUDIO_KBPS: f64 = 128.0;

/// A stream whose bitrate isn't known at all is taken to need this much,
/// in bytes per second, so its recording still goes first (8 Mbit/s).
const UNKNOWN_RATE: u64 = 1_000_000;

/// Recordings go first for this much more than their stream's bitrate:
/// segments arrive in bursts, and a recording that fell behind catches up.
const HEADROOM: f64 = 1.25;

/// What makes a recording of `video` with `choice`: from the formats on air
/// now, or from the start when that was asked for and the site keeps it.
pub fn plan(video: &VideoInfo, choice: &Choice, from_start: bool, caps: &Capabilities) -> Result<Recipe> {
    refuse_file_only(choice)?;
    plan_file(&hourly(video, from_start)?, choice, None, caps)
}

/// The summary line for a recording: the plan, sized per hour.
pub fn summarize(video: &VideoInfo, choice: &Choice, from_start: bool, caps: &Capabilities) -> Result<PlanSummary> {
    refuse_file_only(choice)?;
    let mut summary = explain::summarize(&hourly(video, from_start)?, choice, None, caps)?;
    let start = if from_start {
        "Records from the start: what already aired comes first at full speed, then it carries on in real time"
    } else {
        "Records from now on, as it airs"
    };
    summary.detail = format!("{start}. {}", summary.detail);
    if plan(video, choice, from_start, caps).is_ok_and(|recipe| recipe.encodes_video() || recipe.encodes_audio()) {
        summary.hints.push(info(
            "It's recorded as it comes, and converted once the recording ends.",
        ));
    }
    // Sizes are per hour, and a conversion's time can't be known ahead.
    summary.convert_seconds = None;
    Ok(summary)
}

/// Share makes a file from the whole video: it is for the finished
/// recording, not for a stream as it airs. The interface doesn't offer it.
fn refuse_file_only(choice: &Choice) -> Result<()> {
    match choice {
        Choice::Preset(preset) if preset.target.group() == Group::Share => Err(AppError::new(
            ErrorKind::Unsupported,
            "Share is for the finished recording. Record it with Max quality or Watch first.",
        )),
        _ => Ok(()),
    }
}

/// `video` as the planner should see it: the formats to record from, each
/// sized for an hour, and an hour long.
fn hourly(video: &VideoInfo, from_start: bool) -> Result<VideoInfo> {
    let live = video.live.as_ref().filter(|live| live.status == LiveStatus::Live);
    let formats = match (live, from_start) {
        (None, _) => {
            return Err(AppError::new(
                ErrorKind::Unavailable,
                "This stream isn't live yet. Siphon can wait for it and record it when it starts.",
            ))
        }
        (Some(live), true) => live
            .from_start
            .clone()
            .filter(|formats| !formats.is_empty())
            .ok_or_else(|| {
                AppError::new(
                    ErrorKind::Unsupported,
                    "This stream can't be recorded from the start: the site doesn't keep what already aired.",
                )
            })?,
        (Some(_), false) => video.formats.clone(),
    };
    Ok(VideoInfo {
        formats: formats.into_iter().map(sized_for_an_hour).collect(),
        duration: Some(HOUR),
        ..video.clone()
    })
}

fn sized_for_an_hour(mut format: Format) -> Format {
    // YouTube's live audio states no codec either; in an MP4 it is AAC.
    // Unknown, it would be converted, for nothing.
    let unnamed = format.acodec.is_none() && format.kind == FormatKind::Audio;
    if unnamed && matches!(format.ext.as_str(), "mp4" | "m4a") {
        format.acodec = Some("mp4a.40.2".into());
        format.audio_codec = Some(AudioCodec::Aac);
    }
    if format.kind == FormatKind::Audio && format.abr.or(format.tbr).is_none() {
        // YouTube lists its live audio as "low" (48 kbit/s) and "high" (128) without numbers.
        let low = format
            .note
            .as_deref()
            .is_some_and(|note| note.to_lowercase().contains("low"));
        format.abr = Some(if low { 48.0 } else { AUDIO_KBPS });
    }
    format.filesize = kbps(&format).map(|kbps| (kbps * 1000.0 / 8.0 * HOUR) as u64);
    format.filesize_estimated = true;
    format
}

fn kbps(format: &Format) -> Option<f64> {
    format.tbr.or_else(|| match (format.vbr, format.abr) {
        (None, None) => None,
        (video, audio) => Some(video.unwrap_or(0.0) + audio.unwrap_or(0.0)),
    })
}

/// What the recording's streams carry, in bytes per second.
pub fn stream_rate(recipe: &Recipe) -> Option<u64> {
    let video = recipe.video.as_ref().and_then(|video| video.kbps);
    let audio = recipe
        .audio
        .as_ref()
        .filter(|audio| audio.format_id.is_some())
        .and_then(|audio| audio.source_kbps);
    match (video, audio) {
        (None, None) => None,
        (video, audio) => Some(((video.unwrap_or(0.0) + audio.unwrap_or(0.0)) * 1000.0 / 8.0) as u64),
    }
}

/// How much of the speed limit a recording goes first for, in bytes per
/// second (see `throttle::limiter`): its stream's rate and some room.
pub fn reserve(recipe: &Recipe) -> u64 {
    stream_rate(recipe).map_or(UNKNOWN_RATE, |rate| (rate as f64 * HEADROOM) as u64)
}

/// Notes for a recording about what it shares with: the speed limit, the
/// disk, and the site. `limit` is the speed limit in bytes per second,
/// `free` the space left where it is saved, `per_hour` its estimated size.
pub fn hints(recipe: &Recipe, extractor: Option<&str>, limit: Option<u64>, free: Option<u64>) -> Vec<Hint> {
    let mut hints = Vec::new();
    let needs = stream_rate(recipe);
    match (limit, needs) {
        (Some(limit), Some(needs)) if limit < needs => hints.push(warning(format!(
            "Your speed limit ({}) is lower than this stream needs ({}). The recording will fall behind, and parts of the stream will be missing from it. Raise the limit in Settings first.",
            rate(limit),
            rate(needs)
        ))),
        (Some(limit), _) => hints.push(info(format!(
            "Recordings get the part of your speed limit ({}) they need to keep up first. Other downloads share what's left.",
            rate(limit)
        ))),
        (None, _) => {}
    }
    if let (Some(free), Some(per_hour)) = (free, recipe.estimated_size().filter(|size| *size > 0)) {
        let hours = free as f64 / per_hour as f64;
        let text = format!("about {} of this stream", span(hours * HOUR));
        if hours < 2.0 {
            hints.push(warning(format!(
                "The disk only has room for {text}. Siphon stops and saves the recording before it fills up."
            )));
        } else {
            hints.push(info(format!("The disk has room for {text}.")));
        }
    }
    if extractor.is_some_and(|key| key.to_lowercase().starts_with("twitch")) {
        hints.push(info(
            "Twitch's ads are left out. Where the channel keeps its past broadcasts, the recording is read from them, so nothing is missed during an ad, and Stop takes a few seconds to catch up. Otherwise it skips what aired during each ad.",
        ));
    }
    hints
}

/// How long to wait before looking again whether a stream has started.
/// `since` is when it is scheduled to (Unix seconds), `now` the time now.
pub fn next_check(status: LiveStatus, since: Option<u64>, now: u64) -> Duration {
    const NEAR: u64 = 5 * 60;
    match (status, since) {
        // Far off: sleep until shortly before (in steps, so a moved start is noticed).
        (LiveStatus::Upcoming, Some(at)) if at > now + 2 * NEAR => Duration::from_secs((at - now - NEAR).min(30 * 60)),
        // Around the time it should start: often, since streams start a little late.
        (LiveStatus::Upcoming, Some(at)) if now < at + 30 * 60 => Duration::from_secs(30),
        // An offline channel, or a stream long past its time: without
        // asking the site so often that it grows suspicious.
        _ => Duration::from_secs(60),
    }
}

fn info(text: impl Into<String>) -> Hint {
    Hint {
        level: HintLevel::Info,
        text: text.into(),
    }
}

fn warning(text: impl Into<String>) -> Hint {
    Hint {
        level: HintLevel::Warning,
        text: text.into(),
    }
}

/// Bytes per second as the interface shows speeds: "2.4 MB/s", "500 KB/s".
fn rate(bytes_per_second: u64) -> String {
    let mb = bytes_per_second as f64 / (1024.0 * 1024.0);
    if mb < 1.0 {
        format!("{} KB/s", (bytes_per_second as f64 / 1024.0).round())
    } else {
        format!("{} MB/s", (mb * 10.0).round() / 10.0)
    }
}

/// "45 min", "3 h", "1 h 20 min".
fn span(seconds: f64) -> String {
    let minutes = (seconds / 60.0).round() as u64;
    match (minutes / 60, minutes % 60) {
        (0, minutes) => format!("{} min", minutes.max(1)),
        (hours, 0) => format!("{hours} h"),
        (hours, _) if hours >= 10 => format!("{hours} h"),
        (hours, minutes) => format!("{hours} h {minutes} min"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::codecs::VideoCodec;
    use crate::plan::registry::TargetId;
    use crate::plan::{PresetChoice, VideoAction};
    use crate::ytdlp::metadata::Live;

    fn format(id: &str, kind: FormatKind, height: Option<u32>, tbr: Option<f64>, note: Option<&str>) -> Format {
        Format {
            id: id.into(),
            ext: "mp4".into(),
            kind,
            vcodec: (kind != FormatKind::Audio).then(|| "avc1.640028".into()),
            // YouTube's live audio names no codec.
            acodec: (kind == FormatKind::Muxed).then(|| "mp4a.40.2".into()),
            video_codec: (kind != FormatKind::Audio).then_some(VideoCodec::H264),
            audio_codec: (kind == FormatKind::Muxed).then_some(AudioCodec::Aac),
            width: height.map(|h| h * 16 / 9),
            height,
            fps: height.map(|_| 30.0),
            tbr,
            vbr: None,
            abr: None,
            asr: None,
            audio_channels: None,
            filesize: None,
            filesize_estimated: false,
            dynamic_range: None,
            language: None,
            language_preference: None,
            segmented: true,
            stream_url: None,
            http_chunk_size: None,
            note: note.map(str::to_string),
        }
    }

    /// What YouTube offered for a live stream on 2026-11-19: H.264 video over
    /// HLS and two audio tracks without a stated bitrate.
    fn youtube_live() -> VideoInfo {
        VideoInfo {
            id: "4t4AQ15yj1M".into(),
            title: "lofi radio".into(),
            url: "https://www.youtube.com/watch?v=4t4AQ15yj1M".into(),
            thumbnail: None,
            duration: None,
            channel: None,
            view_count: None,
            extractor: Some("Youtube".into()),
            formats: vec![
                format("233", FormatKind::Audio, None, None, Some("Default, low")),
                format("234", FormatKind::Audio, None, None, Some("Default, high")),
                format("232", FormatKind::Video, Some(720), Some(2970.0), None),
                format("270", FormatKind::Video, Some(1080), Some(5421.0), None),
            ],
            with_account: false,
            live: Some(Live {
                status: LiveStatus::Live,
                since: Some(1_793_728_839),
                viewers: Some(4298),
                from_start: None,
            }),
            subtitles: Vec::new(),
            chat: None,
            saved: None,
        }
    }

    fn preset(target: &str) -> Choice {
        Choice::Preset(PresetChoice {
            target: serde_json::from_value::<TargetId>(serde_json::json!(target)).unwrap(),
            quality: None,
            tier: None,
            music_format: Default::default(),
            subtitles: None,
            chat: None,
        })
    }

    #[test]
    fn a_twitch_recording_can_draw_its_chat_in() {
        let mut stream = youtube_live();
        stream.chat = Some(crate::chat::ChatSource::Live {
            channel: "auronplay".into(),
        });
        let Choice::Preset(preset) = preset("windows") else {
            unreachable!()
        };
        let with_chat = Choice::Preset(crate::plan::PresetChoice {
            chat: Some(crate::chat::Placement::DEFAULT),
            ..preset
        });
        let recipe = plan(&stream, &with_chat, false, &Capabilities::BASIC).unwrap();
        assert!(recipe.chat.is_some());
        assert!(recipe.video_encode().is_some_and(|encode| encode.fast));
        // A VOD's replay never goes with a stream as it airs.
        stream.chat = Some(crate::chat::ChatSource::Vod {
            id: "1".into(),
            channel: "auronplay".into(),
        });
        assert!(plan(&stream, &with_chat, false, &Capabilities::BASIC)
            .unwrap()
            .chat
            .is_none());
    }

    #[test]
    fn a_recording_is_planned_per_hour_from_the_streams_on_air() {
        let recipe = plan(&youtube_live(), &preset("max"), false, &Capabilities::BASIC).unwrap();
        assert_eq!(recipe.format_selector(), "270+234");
        assert_eq!(recipe.video.as_ref().unwrap().action, VideoAction::Copy);
        // Watch on Windows copies both: the unnamed audio is AAC.
        let watch = plan(&youtube_live(), &preset("windows"), false, &Capabilities::BASIC).unwrap();
        assert!(!watch.encodes_video() && !watch.encodes_audio(), "{watch:?}");
        // 5421 + 128 kbit/s for an hour: about 2.5 GB.
        let per_hour = recipe.estimated_size().unwrap();
        assert!((2_400_000_000..2_600_000_000).contains(&per_hour), "{per_hour}");
        assert_eq!(stream_rate(&recipe), Some(693_625));
        assert_eq!(reserve(&recipe), 867_031);
    }

    #[test]
    fn share_waits_for_the_finished_recording() {
        for target in ["discord", "social"] {
            let error = plan(&youtube_live(), &preset(target), false, &Capabilities::BASIC).unwrap_err();
            assert_eq!(error.kind, ErrorKind::Unsupported);
        }
        assert!(plan(&youtube_live(), &preset("music"), false, &Capabilities::BASIC).is_ok());
    }

    #[test]
    fn from_the_start_needs_the_formats_the_site_keeps() {
        let mut video = youtube_live();
        assert!(plan(&video, &preset("max"), true, &Capabilities::BASIC).is_err());
        video.live.as_mut().unwrap().from_start = Some(vec![
            format("137", FormatKind::Video, Some(1080), Some(5019.0), None),
            format("140", FormatKind::Audio, None, Some(144.0), None),
        ]);
        let recipe = plan(&video, &preset("max"), true, &Capabilities::BASIC).unwrap();
        assert_eq!(recipe.format_selector(), "137+140");
    }

    #[test]
    fn a_stream_that_isnt_on_air_has_nothing_to_plan() {
        let mut video = youtube_live();
        video.live.as_mut().unwrap().status = LiveStatus::Upcoming;
        assert!(plan(&video, &preset("max"), false, &Capabilities::BASIC).is_err());
    }

    #[test]
    fn warns_when_the_speed_limit_or_the_disk_is_too_small() {
        let recipe = plan(&youtube_live(), &preset("max"), false, &Capabilities::BASIC).unwrap();
        let warnings = |hints: Vec<Hint>| {
            hints
                .into_iter()
                .filter(|hint| hint.level == HintLevel::Warning)
                .count()
        };
        // 0.5 MB/s is below the stream's 0.66 MB/s; 1 GB is under an hour of it.
        assert_eq!(warnings(hints(&recipe, None, Some(512 * 1024), Some(1_000_000_000))), 2);
        let roomy = hints(&recipe, None, Some(20 * 1024 * 1024), Some(100_000_000_000));
        assert_eq!(warnings(roomy.clone()), 0);
        assert!(
            roomy.iter().any(|hint| hint.text.contains("room for about 40 h")),
            "{roomy:?}"
        );
        assert!(hints(&recipe, Some("TwitchStream"), None, None)[0]
            .text
            .contains("ads are left out"));
        assert!(hints(&recipe, None, None, None).is_empty());
    }

    #[test]
    fn waits_for_a_scheduled_stream_without_asking_too_often() {
        let now = 1_000_000;
        // Two hours off: sleeps 30 minutes at a time.
        assert_eq!(
            next_check(LiveStatus::Upcoming, Some(now + 7200), now),
            Duration::from_secs(1800)
        );
        // Twelve minutes off: until five minutes before.
        assert_eq!(
            next_check(LiveStatus::Upcoming, Some(now + 720), now),
            Duration::from_secs(420)
        );
        // About time, or a little late: every 30 seconds.
        assert_eq!(
            next_check(LiveStatus::Upcoming, Some(now + 60), now),
            Duration::from_secs(30)
        );
        assert_eq!(
            next_check(LiveStatus::Upcoming, Some(now - 600), now),
            Duration::from_secs(30)
        );
        // Long overdue, unscheduled, or an offline channel: every minute.
        assert_eq!(
            next_check(LiveStatus::Upcoming, Some(now - 3600), now),
            Duration::from_secs(60)
        );
        assert_eq!(next_check(LiveStatus::Upcoming, None, now), Duration::from_secs(60));
        assert_eq!(next_check(LiveStatus::Offline, None, now), Duration::from_secs(60));
    }
}
