//! What Siphon remembers between downloads: the last group and target, the
//! quality, the Share tiers and the user's own saved presets. Kept in
//! `choices.json` in the app's config folder, next to `settings.json`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::{AppHandle, Manager};

use crate::error::{AppError, Result};
use crate::plan::registry::{Group, MusicFormat, TargetId, TierId};
use crate::plan::{Content, ManualOptions};
use crate::ytdlp::subtitles::language_tag;

/// Which kind of choice is active.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Active {
    /// A preset from the registry.
    #[default]
    Preset,
    /// The custom settings being edited.
    Custom,
    /// One of the user's saved presets.
    Saved { id: String },
}

/// Custom settings the user saved under a name.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SavedPreset {
    pub id: String,
    pub name: String,
    pub options: ManualOptions,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Choices {
    pub active: Active,
    pub group: Group,
    /// The target last used in each group.
    pub targets: HashMap<Group, TargetId>,
    /// The quality row for video: a resolution (short side). `None` is "Best".
    pub quality: Option<u32>,
    /// The quality row for Music: a bitrate in kbit/s. `None` is "Best".
    pub music_kbps: Option<u32>,
    /// The tier picked for each Share target.
    pub tiers: HashMap<TargetId, TierId>,
    pub music_format: MusicFormat,
    pub custom: ManualOptions,
    pub saved: Vec<SavedPreset>,
    /// The language subtitles are wanted in, for every video that has it
    /// ("en", "pt-br"); `None` is none, which is where a new user starts.
    pub subtitles: Option<String>,
}

impl Default for Choices {
    fn default() -> Self {
        Self {
            active: Active::Preset,
            // A new user starts on the best streams as the site has them.
            group: Group::Max,
            targets: HashMap::new(),
            quality: None,
            music_kbps: None,
            tiers: HashMap::new(),
            music_format: MusicFormat::default(),
            custom: ManualOptions::default(),
            saved: Vec::new(),
            subtitles: None,
        }
    }
}

/// The longest name a saved preset may have.
const MAX_NAME: usize = 40;

/// The longest language tag kept ("zh-hant-tw" is ten).
const MAX_LANGUAGE: usize = 20;

impl Choices {
    /// Drops anything that no longer makes sense: a target remembered for the
    /// wrong group, a tier of another target, a saved preset that is gone.
    fn sanitized(mut self) -> Result<Self> {
        self.targets.retain(|group, target| target.group() == *group);
        self.tiers
            .retain(|target, tier| target.info().tiers.iter().any(|known| known.id == *tier));
        // Stream ids belong to one video, not to what is remembered.
        self.custom.video_format = None;
        self.custom.audio_format = None;
        // The subtitle language is remembered once, for every kind of download.
        self.custom.subtitles = None;
        self.subtitles = self.subtitles.as_deref().map(language_tag).filter(|tag| {
            !tag.is_empty() && tag.len() <= MAX_LANGUAGE && tag.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        });
        for preset in &mut self.saved {
            preset.name = preset.name.trim().chars().take(MAX_NAME).collect();
            // Stream ids belong to one video, not to a preset.
            preset.options.video_format = None;
            preset.options.audio_format = None;
            preset.options.subtitles = None;
        }
        if self.saved.iter().any(|preset| preset.name.is_empty()) {
            return Err(AppError::internal("A saved preset needs a name."));
        }
        if let Active::Saved { id } = &self.active {
            if !self.saved.iter().any(|preset| &preset.id == id) {
                self.active = Active::Custom;
            }
        }
        Ok(self)
    }
}

pub struct ChoiceStore {
    file: PathBuf,
    current: Mutex<Choices>,
}

impl ChoiceStore {
    pub fn load(app: &AppHandle) -> Result<Self> {
        let dir = app
            .path()
            .app_config_dir()
            .map_err(|err| AppError::setup("Couldn't locate the settings folder.").with_details(err.to_string()))?;
        std::fs::create_dir_all(&dir)?;
        let file = dir.join("choices.json");
        // A missing or damaged file just means defaults.
        let current = read(&file)
            .and_then(|choices| choices.sanitized().ok())
            .unwrap_or_default();
        Ok(Self {
            file,
            current: Mutex::new(current),
        })
    }

    pub fn get(&self) -> Choices {
        self.current.lock().unwrap().clone()
    }

    /// True when nothing has been saved yet: an install from before presets
    /// were remembered here, or a first launch.
    pub fn is_new(&self) -> bool {
        !self.file.exists()
    }

    /// Validates, saves and returns the choices as stored.
    pub fn set(&self, choices: Choices) -> Result<Choices> {
        let choices = choices.sanitized()?;
        // Write-then-rename so a crash can't leave a half-written file.
        let tmp = self.file.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(&choices)?)?;
        std::fs::rename(&tmp, &self.file)?;
        *self.current.lock().unwrap() = choices.clone();
        Ok(choices)
    }
}

fn read(file: &Path) -> Option<Choices> {
    let mut value: Value = serde_json::from_slice(&std::fs::read(file).ok()?).ok()?;
    forget_unknown(&mut value);
    serde_json::from_value(value).ok()
}

/// Drops a group, target or tier this version doesn't have, rather than the
/// whole file with the user's saved presets: the Edit group and its targets
/// were removed after 0.15, and `choices.json` remembers them for anyone who
/// once picked one.
fn forget_unknown(value: &mut Value) {
    fn known<T: serde::de::DeserializeOwned>(value: &Value) -> bool {
        serde_json::from_value::<T>(value.clone()).is_ok()
    }
    let Some(choices) = value.as_object_mut() else {
        return;
    };
    if choices.get("group").is_some_and(|group| !known::<Group>(group)) {
        choices.remove("group");
    }
    if let Some(Value::Object(targets)) = choices.get_mut("targets") {
        targets.retain(|group, target| known::<Group>(&Value::String(group.clone())) && known::<TargetId>(target));
    }
    if let Some(Value::Object(tiers)) = choices.get_mut("tiers") {
        tiers.retain(|target, tier| known::<TargetId>(&Value::String(target.clone())) && known::<TierId>(tier));
    }
}

// ── Migration ───────────────────────────────────────────────────────────
//
// Up to 0.6, the interface kept the last choice in the webview's storage
// (`siphon.options`), with one flat list of presets. The frontend hands that
// value over once; this turns it into the new choices. This is the only place
// the old preset ids may appear.

/// Builds choices from the old `siphon.options` state.
pub fn from_legacy(old: &Value) -> Choices {
    let mut choices = Choices::default();
    let text = |key: &str| old.get(key).and_then(Value::as_str);

    if let Some(manual) = old.get("manual") {
        choices.custom = ManualOptions {
            content: field(manual, "content").unwrap_or(Content::VideoAudio),
            video_format: None,
            audio_format: None,
            container: field(manual, "container"),
            // The old custom mode offered no codec the new one lacks.
            video_codec: field(manual, "video_codec"),
            audio_codec: field(manual, "audio_codec"),
            subtitles: None,
        };
    }

    let size_limit = old.get("sizeLimitMb").and_then(Value::as_u64);
    let target = match text("preset") {
        Some("iphone" | "macos") => Some(TargetId::Apple),
        Some("android") => Some(TargetId::Android),
        Some("windows") => Some(TargetId::Windows),
        Some("tv") => Some(TargetId::Tv),
        Some("audio") => Some(TargetId::Original),
        Some("max") => Some(TargetId::Max),
        Some("share") => {
            let (target, tier) = match size_limit {
                Some(16) => (TargetId::Whatsapp, TierId::WhatsappStandard),
                Some(50) => (TargetId::Discord, TierId::DiscordNitroBasic),
                Some(100) => (TargetId::Whatsapp, TierId::WhatsappHd),
                Some(500) => (TargetId::Discord, TierId::DiscordNitro),
                _ => (TargetId::Discord, TierId::DiscordFree),
            };
            choices.tiers.insert(target, tier);
            Some(target)
        }
        _ => None,
    };
    if let Some(target) = target {
        choices.group = target.group();
        choices.targets.insert(target.group(), target);
    }
    if text("mode") == Some("manual") {
        choices.active = Active::Custom;
    }
    choices.sanitized().unwrap_or_default()
}

fn field<T: serde::de::DeserializeOwned>(value: &Value, key: &str) -> Option<T> {
    value
        .get(key)
        .cloned()
        .and_then(|value| serde_json::from_value(value).ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::codecs::Container;
    use serde_json::json;

    #[test]
    fn a_new_user_starts_on_max_quality() {
        assert_eq!(Choices::default().group, Group::Max);
        assert_eq!(Choices::default().active, Active::Preset);
    }

    #[test]
    fn old_device_presets_become_watch_targets() {
        let choices = from_legacy(&json!({"mode": "preset", "preset": "iphone", "exactCut": false}));
        assert_eq!(choices.group, Group::Watch);
        assert_eq!(choices.targets.get(&Group::Watch), Some(&TargetId::Apple));
        assert_eq!(choices.active, Active::Preset);
        assert_eq!(
            from_legacy(&json!({"preset": "macos"})).targets.get(&Group::Watch),
            Some(&TargetId::Apple)
        );
    }

    #[test]
    fn the_old_share_limit_picks_a_target_and_tier() {
        let choices = from_legacy(&json!({"preset": "share", "sizeLimitMb": 50}));
        assert_eq!(choices.group, Group::Share);
        assert_eq!(choices.targets.get(&Group::Share), Some(&TargetId::Discord));
        assert_eq!(choices.tiers.get(&TargetId::Discord), Some(&TierId::DiscordNitroBasic));

        let whatsapp = from_legacy(&json!({"preset": "share", "sizeLimitMb": 16}));
        assert_eq!(whatsapp.targets.get(&Group::Share), Some(&TargetId::Whatsapp));
    }

    #[test]
    fn audio_and_max_carry_over_and_editing_is_gone() {
        // Edit was removed after 0.15: the old editing preset starts where a new user does.
        assert_eq!(from_legacy(&json!({"preset": "editing"})), Choices::default());
        assert_eq!(from_legacy(&json!({"preset": "audio"})).group, Group::Audio);
        assert_eq!(from_legacy(&json!({"preset": "max"})).group, Group::Max);
    }

    #[test]
    fn custom_mode_keeps_its_settings_but_not_stream_ids() {
        let choices = from_legacy(&json!({
            "mode": "manual",
            "preset": "windows",
            "manual": {"content": "video_only", "container": "mkv", "video_format": "137", "video_codec": null}
        }));
        assert_eq!(choices.active, Active::Custom);
        assert_eq!(choices.custom.content, Content::VideoOnly);
        assert_eq!(choices.custom.container, Some(Container::Mkv));
        assert_eq!(choices.custom.video_format, None);
    }

    #[test]
    fn unknown_or_damaged_values_fall_back_to_defaults() {
        assert_eq!(from_legacy(&json!({"preset": "toaster"})), Choices::default());
        assert_eq!(from_legacy(&json!("nonsense")), Choices::default());
    }

    #[test]
    fn saved_choices_are_checked() {
        let mut choices = Choices::default();
        choices.targets.insert(Group::Watch, TargetId::Discord);
        choices.tiers.insert(TargetId::Discord, TierId::Tiktok);
        choices.active = Active::Saved { id: "gone".into() };
        let choices = choices.sanitized().unwrap();
        assert!(choices.targets.is_empty());
        assert!(choices.tiers.is_empty());
        assert_eq!(choices.active, Active::Custom);

        let unnamed = Choices {
            saved: vec![SavedPreset {
                id: "a".into(),
                name: "  ".into(),
                options: ManualOptions::default(),
            }],
            ..Choices::default()
        };
        assert!(unnamed.sanitized().is_err());
    }

    #[test]
    fn choices_files_from_older_versions_fill_in_new_fields() {
        let choices: Choices = serde_json::from_str(r#"{"group": "share"}"#).unwrap();
        assert_eq!(choices.group, Group::Share);
    }

    #[test]
    fn the_subtitle_language_is_kept_once_and_cleaned() {
        let mut choices = Choices {
            subtitles: Some("pt_BR".into()),
            ..Choices::default()
        };
        choices.custom.subtitles = Some("en".into());
        let choices = choices.sanitized().unwrap();
        assert_eq!(choices.subtitles.as_deref(), Some("pt-br"));
        assert_eq!(choices.custom.subtitles, None);
        // Nothing that isn't a language tag.
        for odd in ["", "en; rm -rf", "a-very-long-language-tag-indeed"] {
            let choices = Choices {
                subtitles: Some(odd.into()),
                ..Choices::default()
            };
            assert_eq!(choices.sanitized().unwrap().subtitles, None, "{odd}");
        }
        // A new user has none.
        assert_eq!(Choices::default().subtitles, None);
    }

    #[test]
    fn a_removed_group_or_target_costs_nothing_else() {
        // A choices.json from 0.15, by someone who once picked Edit.
        let dir = std::env::temp_dir().join(format!("siphon-choices-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("choices.json");
        std::fs::write(
            &file,
            r#"{"active": {"kind": "preset"}, "group": "edit",
                "targets": {"edit": "premiere", "watch": "apple"},
                "tiers": {"discord": "discord_nitro"}, "smooth_editing": true,
                "saved": [{"id": "a", "name": "Mine", "options": {"content": "video_audio", "container": "mkv",
                           "video_codec": null, "audio_codec": null}}],
                "subtitles": "el"}"#,
        )
        .unwrap();
        let choices = read(&file).unwrap().sanitized().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(choices.group, Group::Max);
        assert_eq!(choices.targets.len(), 1);
        assert_eq!(choices.targets.get(&Group::Watch), Some(&TargetId::Apple));
        assert_eq!(choices.tiers.get(&TargetId::Discord), Some(&TierId::DiscordNitro));
        assert_eq!(choices.saved.len(), 1);
        assert_eq!(choices.subtitles.as_deref(), Some("el"));
    }

    #[test]
    fn a_saved_cut_mode_is_dropped() {
        // Up to 0.8, trims could be "fast" (from the nearest key frame).
        // Every cut is exact now: the old choice is read past and not kept.
        for old in [
            r#"{"group": "max", "exact_cut": false}"#,
            r#"{"group": "max", "exact_cut": true}"#,
        ] {
            let choices: Choices = serde_json::from_str(old).unwrap();
            assert_eq!(choices.group, Group::Max);
            let saved = serde_json::to_string(&choices).unwrap();
            assert!(!saved.contains("exact_cut"), "{saved}");
        }
    }
}
