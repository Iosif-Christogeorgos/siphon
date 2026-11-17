//! Watch: the best video and audio that play natively on a device.
//!
//! Each device says which codecs it plays as they are, and what to convert
//! to when the video offers none of them at the chosen quality. The sources
//! behind every entry are in `docs/presets.md`.

use super::codecs::{AudioCodec, Container, VideoCodec};
use super::registry::{Target, TargetId};
use super::{info, max_by_key, warning, Capabilities, Planner, Recipe, Video, VideoAction, VideoEncode};

/// No device preset goes above 4K or 60 frames a second.
pub const MAX_LINES: u32 = 2160;
pub const MAX_FPS: f64 = 60.0;

/// A codec the device plays as it is, in the container it plays it from.
struct Native {
    video: VideoCodec,
    container: Container,
    /// Audio that can be copied alongside it; anything else becomes AAC.
    audio: &'static [AudioCodec],
    /// Something worth saying when this one is picked.
    note: Option<&'static str>,
}

const fn native(video: VideoCodec, container: Container, audio: &'static [AudioCodec]) -> Native {
    Native {
        video,
        container,
        audio,
        note: None,
    }
}

const AAC: &[AudioCodec] = &[AudioCodec::Aac, AudioCodec::Ac3];

/// What a device plays, in order of preference, given what is installed.
fn natives(target: TargetId, caps: &Capabilities) -> Vec<Native> {
    use VideoCodec::{Av1, Hevc, Vp9, H264};
    match target {
        // Media Player plays H.264 out of the box. VP9, AV1 and HEVC need
        // Microsoft's extensions; Siphon copies them only when this PC has them.
        TargetId::Windows => {
            let mut natives = vec![native(H264, Container::Mp4, &[AudioCodec::Aac, AudioCodec::Mp3])];
            if caps.windows.hevc {
                natives.push(Native {
                    note: Some("HEVC plays in Media Player through the HEVC Video Extensions, which this PC has."),
                    ..native(Hevc, Container::Mp4, &[AudioCodec::Aac])
                });
            }
            if caps.windows.vp9 {
                natives.push(Native {
                    note: Some("VP9 plays in Media Player through the VP9 Video Extensions, which this PC has."),
                    ..native(Vp9, Container::Webm, &[AudioCodec::Opus, AudioCodec::Vorbis])
                });
            }
            if caps.windows.av1 {
                natives.push(Native {
                    note: Some("AV1 plays in Media Player through the AV1 Video Extension, which this PC has."),
                    ..native(Av1, Container::Mp4, &[AudioCodec::Aac])
                });
            }
            natives
        }
        // Photos and QuickTime take H.264 and HEVC with AAC in MP4. Whether
        // Photos imports AV1 couldn't be confirmed, so AV1 is converted.
        TargetId::Apple => vec![native(Hevc, Container::Mp4, AAC), native(H264, Container::Mp4, AAC)],
        // Hardware H.264, HEVC and VP9 decoders are on nearly every Android
        // phone in use; AV1 only on recent ones. AAC is the safe sound.
        TargetId::Android => vec![
            native(H264, Container::Mp4, &[AudioCodec::Aac]),
            native(Hevc, Container::Mp4, &[AudioCodec::Aac]),
            native(Vp9, Container::Mp4, &[AudioCodec::Aac]),
        ],
        // H.264 plays on every smart TV; every 4K set decodes HEVC. VP9 and AV1
        // from a USB stick are hit-and-miss.
        TargetId::Tv => vec![native(H264, Container::Mp4, AAC), native(Hevc, Container::Mp4, AAC)],
        _ => unreachable!("only Watch targets"),
    }
}

/// What "Best" means on the device (short side).
fn best_lines(target: TargetId) -> u32 {
    match target {
        // Phone screens and their decoders: higher is in the quality row.
        TargetId::Android => 1080,
        _ => MAX_LINES,
    }
}

/// What to convert to when nothing native is offered at the chosen quality.
fn encode_target(target: TargetId, lines: u32, caps: &Capabilities) -> VideoCodec {
    let hevc_above_hd = if lines > 1080 {
        VideoCodec::Hevc
    } else {
        VideoCodec::H264
    };
    match target {
        // HEVC keeps Apple's files small and Photos takes it, but only a
        // hardware encoder makes it quick; H.264 on the GPU beats HEVC on the CPU.
        TargetId::Apple if caps.has_hardware(VideoCodec::Hevc) => VideoCodec::Hevc,
        TargetId::Apple if caps.has_hardware(VideoCodec::H264) => VideoCodec::H264,
        TargetId::Apple | TargetId::Tv | TargetId::Android => hevc_above_hd,
        TargetId::Windows if caps.windows.hevc => hevc_above_hd,
        _ => VideoCodec::H264,
    }
}

impl Planner<'_> {
    pub(super) fn watch(&self, target: &Target, quality: Option<u32>) -> Recipe {
        let natives = natives(target.id, self.caps);
        let pool = self.pool(MAX_LINES, MAX_FPS);
        let cut = self.cuts();
        // A cut re-encodes anyway: a stream counts as native only if it
        // can be re-encoded in its own codec (H.264, HEVC).
        let native_of = |video: &Video| {
            natives
                .iter()
                .position(|native| native.video == video.codec)
                .filter(|_| !cut || matches!(video.codec, VideoCodec::H264 | VideoCodec::Hevc))
        };

        let candidates = Self::at_resolution(&pool, quality.unwrap_or(best_lines(target.id)));
        // Copy the best native stream at that resolution …
        let native = max_by_key(candidates.iter().copied().filter(|v| native_of(v).is_some()), |video| {
            (
                video.fps as u32,
                std::cmp::Reverse(native_of(video)),
                video.bitrate_rank(),
            )
        });
        // … otherwise convert the best stream of any kind.
        let any = max_by_key(candidates.iter().copied(), |video| {
            (video.fps as u32, video.codec.efficiency(), video.bitrate_rank())
        })
        .expect("plan() checks that videos exist");

        let mut recipe = self.recipe(Container::Mp4, format!("Plays natively on {}", target.name));
        match native {
            Some(video) => {
                let native = &natives[native_of(&video).expect("filtered on it")];
                recipe.container = native.container;
                recipe.audio = self.audio_for(&video, native.audio, AudioCodec::Aac);
                if let Some(note) = native.note {
                    recipe.hints.push(info(note));
                }
                if recipe.encodes_audio() {
                    let source = recipe.audio.as_ref().map_or("this", |audio| audio.source.label());
                    recipe.reason = Some(format!("{} can’t play {source} audio in this file type", target.name));
                }
                let action = if cut {
                    VideoAction::Encode(self.encode(video.codec))
                } else {
                    VideoAction::Copy
                };
                recipe.video = Some(Self::video_step(&video, action));
            }
            None => {
                let codec = encode_target(target.id, any.lines, self.caps);
                let audio: &[AudioCodec] = natives
                    .iter()
                    .find(|native| native.video == codec)
                    .map_or(&[AudioCodec::Aac], |native| native.audio);
                recipe.audio = self.audio_for(&any, audio, AudioCodec::Aac);
                recipe.reason = Some(format!(
                    "{} can’t play {} out of the box",
                    target.name,
                    any.codec.label()
                ));
                let mut encode: VideoEncode = self.encode(codec);
                encode.max_fps = Some(MAX_FPS);
                recipe.video = Some(Self::video_step(&any, VideoAction::Encode(encode)));
            }
        }
        if recipe.video.as_ref().is_some_and(|video| video.hdr) {
            recipe.hints.push(warning(
                "This video is HDR. On screens without HDR, colours may look washed out.",
            ));
        }
        recipe
    }
}
