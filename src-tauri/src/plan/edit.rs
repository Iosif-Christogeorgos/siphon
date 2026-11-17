//! Edit: files that import and scrub smoothly in an editor.
//!
//! Every editor wants a constant frame rate and 48 kHz audio, and web video
//! promises neither, so the video is always re-encoded:
//!
//! - **Standard:** H.264 with a key frame every second in MP4, with AAC.
//!   Premiere Pro, DaVinci Resolve (including the free version on Windows,
//!   which reads 8-bit H.264), CapCut and VEGAS Pro all import it.
//! - **Smoothest editing:** ProRes 422 with uncompressed audio in MOV. Every
//!   frame stands on its own, so scrubbing is instant, at about ten times the
//!   size. CapCut doesn't offer it: its support couldn't be confirmed.

use super::codecs::{AudioCodec, Container, VideoCodec};
use super::registry::{Extra, Target};
use super::{info, max_by_key, AudioAction, Planner, Recipe, VideoAction, VideoEncode};

const SAMPLE_RATE: u32 = 48_000;

impl Planner<'_> {
    pub(super) fn edit(&self, target: &Target, quality: Option<u32>, smooth: bool) -> Recipe {
        let smooth = smooth && target.extras.contains(&Extra::SmoothEditing);
        // Intermediate files are enormous, so they default to 1080p.
        let best = if smooth { 1080 } else { u32::MAX };
        let pool = self.pool(u32::MAX, 120.0);
        let video = max_by_key(Self::at_resolution(&pool, quality.unwrap_or(best)), |video| {
            (video.fps as u32, video.codec.efficiency(), video.bitrate_rank())
        })
        .expect("plan() checks that videos exist");

        let (container, mut encode, audio) = if smooth {
            (
                Container::Mov,
                VideoEncode::quality(VideoCodec::Prores, self.caps.hardware(VideoCodec::Prores)),
                AudioAction::Encode {
                    codec: AudioCodec::Pcm,
                    bitrate_kbps: None,
                    sample_rate: Some(SAMPLE_RATE),
                    stereo: false,
                },
            )
        } else {
            let mut encode = self.encode(VideoCodec::H264);
            encode.keyframe_seconds = Some(1.0);
            (
                Container::Mp4,
                encode,
                AudioAction::Encode {
                    codec: AudioCodec::Aac,
                    bitrate_kbps: Some(320),
                    sample_rate: Some(SAMPLE_RATE),
                    stereo: false,
                },
            )
        };
        encode.constant_frame_rate = true;

        let mut recipe = self.recipe(container, format!("Ready for {}", target.name));
        recipe.audio = self.audio_for(&video, &[], AudioCodec::Aac).map(|mut step| {
            step.action = audio.clone();
            step
        });
        recipe.reason =
            Some("editors need a constant frame rate and 48 kHz sound, which web video doesn’t promise".into());
        if smooth {
            recipe.hints.push(info(
                "Intermediate files are large by design: they trade size for instant scrubbing. Picture quality matches the source; it can’t exceed it.",
            ));
        }
        recipe.video = Some(Self::video_step(&video, VideoAction::Encode(encode)));
        recipe
    }
}
