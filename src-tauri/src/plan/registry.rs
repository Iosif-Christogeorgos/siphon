//! Every preset Siphon offers: the groups, the targets inside them and the
//! account tiers of the Share targets.
//!
//! This is the single source of truth. The frontend reads it through the
//! `get_presets` command and defines no presets of its own. What each target
//! can play or needs, and the sources for the limits, are in `docs/presets.md`.

use serde::{Deserialize, Serialize};

/// What the download is for. The user picks a group, then a target in it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Group {
    Watch,
    Edit,
    Audio,
    Max,
    Share,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TargetId {
    // Watch
    Windows,
    Apple,
    Android,
    Tv,
    // Edit
    Premiere,
    Resolve,
    Capcut,
    Vegas,
    // Audio
    Music,
    Original,
    // Max quality
    Max,
    // Share
    Discord,
    Whatsapp,
    Social,
}

/// An account tier or platform of a Share target, with its upload limit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TierId {
    DiscordFree,
    DiscordNitroBasic,
    DiscordNitro,
    WhatsappStandard,
    WhatsappHd,
    WhatsappDocument,
    Tiktok,
    Reels,
    Shorts,
}

/// The file type the Music target produces.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MusicFormat {
    /// Plays everywhere, which makes it the default.
    #[default]
    Mp3,
    M4a,
}

/// What the quality row offers for a target.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum QualityKind {
    /// Resolutions the video offers.
    Resolution,
    /// Audio bitrates (Music).
    Bitrate,
    /// No quality row (Max quality manages itself, and so does Share).
    None,
}

/// A secondary choice shown next to a target's chips, only when it applies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Extra {
    /// Pick an account tier (Share).
    Tier,
    /// "Smoothest editing (large files)": an intermediate codec instead of H.264.
    SmoothEditing,
    /// MP3 or M4A.
    MusicFormat,
}

#[derive(Debug, Serialize)]
pub struct Tier {
    pub id: TierId,
    pub label: &'static str,
    /// The upload limit in megabytes (10⁶ bytes).
    pub limit_mb: u32,
    /// The service shows videos no sharper than this (short side), so more is wasted.
    pub max_lines: u32,
    /// False when the file is sent as a document rather than played in the chat.
    pub inline: bool,
}

impl Tier {
    pub fn limit_bytes(&self) -> u64 {
        u64::from(self.limit_mb) * 1_000_000
    }
}

#[derive(Debug, Serialize)]
pub struct Target {
    pub id: TargetId,
    #[serde(skip)]
    pub group: Group,
    pub label: &'static str,
    /// As it reads in a sentence: "Plays natively on {name}".
    #[serde(skip)]
    pub name: &'static str,
    /// An icon name; the frontend maps names to icons.
    pub icon: &'static str,
    pub quality: QualityKind,
    pub extras: &'static [Extra],
    pub tiers: &'static [Tier],
}

#[derive(Debug, Serialize)]
pub struct GroupInfo {
    pub id: Group,
    pub label: &'static str,
    pub icon: &'static str,
    /// One line about the group; shown in place of the chips when it has one target.
    pub description: &'static str,
    pub targets: Vec<&'static Target>,
    /// The target picked when the group is first chosen.
    pub default_target: TargetId,
}

/// Bitrates the Music target offers, in kbit/s.
pub const MUSIC_BITRATES: [u32; 4] = [128, 192, 256, 320];

const fn tier(id: TierId, label: &'static str, limit_mb: u32, max_lines: u32) -> Tier {
    Tier {
        id,
        label,
        limit_mb,
        max_lines,
        inline: true,
    }
}

// Where sources disagree, the limit is the cautious end of them (docs/presets.md):
// a file made for a smaller limit still sends, one made for a larger does not.
const DISCORD_TIERS: &[Tier] = &[
    tier(TierId::DiscordFree, "Free · 20 MB", 20, 1080),
    tier(TierId::DiscordNitroBasic, "Nitro Basic · 50 MB", 50, 1080),
    tier(TierId::DiscordNitro, "Nitro · 500 MB", 500, 1080),
];
// WhatsApp's help centre: 64 MB at 480p on slower connections, 100 MB at 720p
// in HD; anything up to 2 GB as a document, which isn't played in the chat.
const WHATSAPP_TIERS: &[Tier] = &[
    tier(TierId::WhatsappStandard, "Standard · 64 MB", 64, 480),
    tier(TierId::WhatsappHd, "HD · 100 MB", 100, 720),
    Tier {
        inline: false,
        ..tier(TierId::WhatsappDocument, "Document · 2 GB", 2000, 2160)
    },
];
// Short-form platforms show 1080p at most. TikTok's limit is the lowest app limit (Android).
const SOCIAL_TIERS: &[Tier] = &[
    tier(TierId::Tiktok, "TikTok · 72 MB", 72, 1080),
    tier(TierId::Reels, "Reels · 300 MB", 300, 1080),
    tier(TierId::Shorts, "Shorts · 1 GB", 1000, 1080),
];

const fn target(
    id: TargetId,
    group: Group,
    label: &'static str,
    name: &'static str,
    icon: &'static str,
    quality: QualityKind,
    extras: &'static [Extra],
) -> Target {
    Target {
        id,
        group,
        label,
        name,
        icon,
        quality,
        extras,
        tiers: &[],
    }
}

use Extra::{MusicFormat as MusicExtra, SmoothEditing};
use QualityKind::{Bitrate, Resolution};

static TARGETS: [Target; 14] = [
    target(
        TargetId::Windows,
        Group::Watch,
        "Windows",
        "Windows 11",
        "monitor",
        Resolution,
        &[],
    ),
    target(
        TargetId::Apple,
        Group::Watch,
        "iPhone / iPad / Mac",
        "Apple devices",
        "apple",
        Resolution,
        &[],
    ),
    target(
        TargetId::Android,
        Group::Watch,
        "Android",
        "Android",
        "smartphone",
        Resolution,
        &[],
    ),
    target(TargetId::Tv, Group::Watch, "TV", "smart TVs", "tv", Resolution, &[]),
    target(
        TargetId::Premiere,
        Group::Edit,
        "Premiere Pro",
        "Premiere Pro",
        "clapperboard",
        Resolution,
        &[SmoothEditing],
    ),
    target(
        TargetId::Resolve,
        Group::Edit,
        "DaVinci Resolve",
        "DaVinci Resolve",
        "clapperboard",
        Resolution,
        &[SmoothEditing],
    ),
    // CapCut's support for intermediate codecs couldn't be confirmed.
    target(
        TargetId::Capcut,
        Group::Edit,
        "CapCut",
        "CapCut",
        "clapperboard",
        Resolution,
        &[],
    ),
    target(
        TargetId::Vegas,
        Group::Edit,
        "Vegas",
        "VEGAS Pro",
        "clapperboard",
        Resolution,
        &[SmoothEditing],
    ),
    target(
        TargetId::Music,
        Group::Audio,
        "Music",
        "any music player",
        "music",
        Bitrate,
        &[MusicExtra],
    ),
    target(
        TargetId::Original,
        Group::Audio,
        "Best audio (original)",
        "",
        "audio-lines",
        QualityKind::None,
        &[],
    ),
    target(
        TargetId::Max,
        Group::Max,
        "Max quality",
        "",
        "gem",
        QualityKind::None,
        &[],
    ),
    Target {
        tiers: DISCORD_TIERS,
        ..target(
            TargetId::Discord,
            Group::Share,
            "Discord",
            "Discord",
            "message-circle",
            QualityKind::None,
            &[Extra::Tier],
        )
    },
    Target {
        tiers: WHATSAPP_TIERS,
        ..target(
            TargetId::Whatsapp,
            Group::Share,
            "WhatsApp",
            "WhatsApp",
            "phone",
            QualityKind::None,
            &[Extra::Tier],
        )
    },
    Target {
        tiers: SOCIAL_TIERS,
        ..target(
            TargetId::Social,
            Group::Share,
            "TikTok / Reels / Shorts",
            "short-form video apps",
            "sparkles",
            QualityKind::None,
            &[Extra::Tier],
        )
    },
];

impl TargetId {
    pub fn info(self) -> &'static Target {
        TARGETS
            .iter()
            .find(|target| target.id == self)
            .expect("every target is in the registry")
    }

    pub fn group(self) -> Group {
        self.info().group
    }
}

impl Target {
    /// The tier to use: `chosen` if it belongs to this target, otherwise the first.
    pub fn tier(&self, chosen: Option<TierId>) -> Option<&'static Tier> {
        let tiers: &'static [Tier] = self.tiers;
        tiers
            .iter()
            .find(|tier| Some(tier.id) == chosen)
            .or_else(|| tiers.first())
    }
}

impl Group {
    pub const ALL: [Group; 5] = [Group::Watch, Group::Edit, Group::Audio, Group::Max, Group::Share];

    pub fn default_target(self) -> TargetId {
        match self {
            Group::Watch => TargetId::Windows,
            Group::Edit => TargetId::Premiere,
            Group::Audio => TargetId::Music,
            Group::Max => TargetId::Max,
            Group::Share => TargetId::Discord,
        }
    }

    fn info(self) -> GroupInfo {
        let (label, icon, description) = match self {
            Group::Watch => ("Watch", "play", "The best version that plays on the device, as it is."),
            Group::Edit => ("Edit", "scissors", "Files that import and scrub smoothly in an editor."),
            Group::Audio => ("Audio", "headphones", "Just the sound."),
            Group::Max => (
                "Max quality",
                "gem",
                "The best streams exactly as the site has them. Not every device plays them.",
            ),
            Group::Share => ("Share", "send", "The best quality that fits under an upload limit."),
        };
        GroupInfo {
            id: self,
            label,
            icon,
            description,
            targets: TARGETS.iter().filter(|target| target.group == self).collect(),
            default_target: self.default_target(),
        }
    }
}

#[derive(Debug, Serialize)]
pub struct Registry {
    pub groups: Vec<GroupInfo>,
    pub music_bitrates: [u32; 4],
}

pub fn registry() -> Registry {
    Registry {
        groups: Group::ALL.into_iter().map(Group::info).collect(),
        music_bitrates: MUSIC_BITRATES,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_group_has_its_default_target_and_every_target_one_group() {
        let registry = registry();
        let mut seen = 0;
        for group in &registry.groups {
            assert!(group.targets.iter().any(|target| target.id == group.default_target));
            seen += group.targets.len();
        }
        assert_eq!(seen, TARGETS.len());
    }

    #[test]
    fn share_targets_have_tiers_and_fall_back_to_the_first() {
        let discord = TargetId::Discord.info();
        assert_eq!(discord.tier(None).map(|tier| tier.id), Some(TierId::DiscordFree));
        assert_eq!(
            discord.tier(Some(TierId::DiscordNitro)).map(|tier| tier.limit_mb),
            Some(500)
        );
        // A tier of another target is ignored.
        assert_eq!(
            discord.tier(Some(TierId::Tiktok)).map(|tier| tier.id),
            Some(TierId::DiscordFree)
        );
        assert!(TargetId::Windows.info().tier(None).is_none());
    }
}
