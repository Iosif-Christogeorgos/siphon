//! Device presets: what each target can play without conversion.
//!
//! The sources behind every entry are listed in `docs/device-support.md`.
//! When a claim couldn't be confirmed, the preset takes the cautious side
//! (convert rather than hand over a file that might not play).

use serde::{Deserialize, Serialize};

use super::codecs::{AudioCodec, Container, VideoCodec};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PresetId {
    Iphone,
    Android,
    Windows,
    Macos,
    Tv,
    /// Premiere / DaVinci Resolve: constant frame rate, intra-frame codec.
    Editing,
    /// Discord / WhatsApp: fit under an upload size limit.
    Share,
    Audio,
    /// Best streams available, whatever the format.
    Max,
}

/// One natively playable combination, in order of preference.
pub struct Tier {
    pub video: VideoCodec,
    pub container: Container,
    /// Audio codecs that can be copied as-is alongside this video.
    pub audio: &'static [AudioCodec],
    /// A caveat worth telling the user about, if any.
    pub caveat: Option<&'static str>,
}

pub struct DeviceProfile {
    /// As it reads in a sentence: "plays natively on {name}".
    pub name: &'static str,
    pub tiers: &'static [Tier],
    /// Short side in pixels (2160 = 4K).
    pub max_lines: u32,
    pub max_fps: f64,
    /// Conversion target up to 1080p, where H.264 is universal …
    pub encode_hd: VideoCodec,
    /// … and above it, where HEVC keeps 4K files a sensible size.
    pub encode_uhd: VideoCodec,
}

impl DeviceProfile {
    pub fn tier_of(&self, codec: VideoCodec) -> Option<(usize, &Tier)> {
        self.tiers.iter().enumerate().find(|(_, tier)| tier.video == codec)
    }

    pub fn encode_target(&self, lines: u32) -> VideoCodec {
        if lines > 1080 {
            self.encode_uhd
        } else {
            self.encode_hd
        }
    }
}

const fn tier(video: VideoCodec, container: Container, audio: &'static [AudioCodec]) -> Tier {
    Tier {
        video,
        container,
        audio,
        caveat: None,
    }
}

use AudioCodec::{Aac, Ac3, Mp3, Opus, Vorbis};
use VideoCodec::{Av1, Hevc, Vp9, H264};

// Apple: the Photos app and QuickTime take H.264 and HEVC in MP4/MOV with AAC.
// VP9, WebM and MKV are not supported. Recent iPhones and Macs can decode AV1,
// but whether Photos imports it is unconfirmed, so AV1 is converted.
static IPHONE: DeviceProfile = DeviceProfile {
    name: "iPhone",
    tiers: &[
        tier(Hevc, Container::Mp4, &[Aac, Ac3]),
        tier(H264, Container::Mp4, &[Aac, Ac3]),
    ],
    max_lines: 2160,
    max_fps: 60.0,
    encode_hd: H264,
    encode_uhd: Hevc,
};

static MACOS: DeviceProfile = DeviceProfile {
    name: "Mac",
    tiers: &[
        tier(Hevc, Container::Mp4, &[Aac, Ac3]),
        tier(H264, Container::Mp4, &[Aac, Ac3]),
    ],
    max_lines: 2160,
    max_fps: 60.0,
    encode_hd: H264,
    encode_uhd: Hevc,
};

// Android's platform decoders: H.264, HEVC (5.0+), VP9 (4.4+), AV1 (10+), with
// AAC or Opus, all readable from MP4. AV1 goes last: older phones decode it in
// software or not at all.
static ANDROID: DeviceProfile = DeviceProfile {
    name: "Android",
    tiers: &[
        tier(H264, Container::Mp4, &[Aac, Opus]),
        tier(Hevc, Container::Mp4, &[Aac, Opus]),
        tier(Vp9, Container::Mp4, &[Aac, Opus]),
        Tier {
            video: Av1,
            container: Container::Mp4,
            audio: &[Aac, Opus],
            caveat: Some("AV1 needs Android 10 or newer, and plays smoothly only on recent phones."),
        },
    ],
    max_lines: 2160,
    max_fps: 60.0,
    encode_hd: H264,
    encode_uhd: Hevc,
};

// Windows 11 plays H.264 out of the box. VP9 and AV1 rely on free Microsoft
// Store extensions that most PCs already have. HEVC needs a paid extension,
// so it is never chosen and is converted when it's all a site offers.
static WINDOWS: DeviceProfile = DeviceProfile {
    name: "Windows 11",
    tiers: &[
        tier(H264, Container::Mp4, &[Aac, Mp3]),
        Tier {
            video: Vp9,
            container: Container::Webm,
            audio: &[Opus, Vorbis],
            caveat: Some(
                "VP9 plays in Media Player through Microsoft’s free VP9 Video Extensions, which most PCs already have.",
            ),
        },
        Tier {
            video: Av1,
            container: Container::Mp4,
            audio: &[Aac],
            caveat: Some("AV1 plays in Media Player through Microsoft’s free AV1 Video Extension."),
        },
    ],
    max_lines: 2160,
    max_fps: 60.0,
    encode_hd: H264,
    encode_uhd: H264,
};

// Smart TVs vary by brand and year. H.264 + AAC in MP4 plays on all of them,
// and every 4K set decodes HEVC. VP9/AV1 from USB is hit-and-miss.
static TV: DeviceProfile = DeviceProfile {
    name: "your TV",
    tiers: &[
        tier(H264, Container::Mp4, &[Aac, Ac3]),
        tier(Hevc, Container::Mp4, &[Aac, Ac3]),
    ],
    max_lines: 2160,
    max_fps: 60.0,
    encode_hd: H264,
    encode_uhd: Hevc,
};

/// The playback profile behind a device preset; `None` for the task presets
/// (editing, share, audio, max), which have their own logic.
pub fn device(preset: PresetId) -> Option<&'static DeviceProfile> {
    match preset {
        PresetId::Iphone => Some(&IPHONE),
        PresetId::Android => Some(&ANDROID),
        PresetId::Windows => Some(&WINDOWS),
        PresetId::Macos => Some(&MACOS),
        PresetId::Tv => Some(&TV),
        PresetId::Editing | PresetId::Share | PresetId::Audio | PresetId::Max => None,
    }
}

/// Upload limits offered by the Share preset, in megabytes.
///
/// These move around (Discord's free tier has been 8, 25, 10 and reportedly
/// 20 MB), so the user picks the one that matches their account and the
/// default stays on the smallest.
pub const SHARE_LIMITS_MB: [u32; 5] = [10, 16, 50, 100, 500];
