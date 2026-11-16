//! User preferences, kept in `settings.json` in the app's config folder.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

use crate::error::{AppError, Result};

/// How downloaded files are named. The extension is always added by Siphon.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FilenameStyle {
    /// `Title`
    Title,
    /// `Title [id]`: unique per video, so two uploads with one title don't collide.
    TitleId,
    /// `Channel - Title`
    ChannelTitle,
    /// `2026-11-16 Title` (upload date)
    DateTitle,
    /// The user's own yt-dlp output template.
    Custom,
}

impl FilenameStyle {
    /// The yt-dlp output template for this style, without extension.
    /// Lengths are capped in bytes so paths stay within OS limits.
    pub fn template(self, custom: &str) -> String {
        match self {
            FilenameStyle::Title => "%(title).180B".into(),
            FilenameStyle::TitleId => "%(title).180B [%(id)s]".into(),
            FilenameStyle::ChannelTitle => "%(channel,uploader|Unknown).60B - %(title).150B".into(),
            FilenameStyle::DateTitle => "%(upload_date>%Y-%m-%d|undated)s %(title).180B".into(),
            FilenameStyle::Custom => custom.trim().to_string(),
        }
    }
}

pub const MAX_CONCURRENT_LIMIT: usize = 6;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Where downloads are saved. `None` means the system Downloads folder.
    pub output_dir: Option<PathBuf>,
    pub filename: FilenameStyle,
    /// Used when `filename` is `custom`: a yt-dlp output template without extension.
    pub custom_template: String,
    /// How many downloads run at once; the rest wait in the queue.
    pub max_concurrent: usize,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            output_dir: None,
            filename: FilenameStyle::TitleId,
            custom_template: "%(title)s [%(id)s]".into(),
            max_concurrent: 3,
        }
    }
}

impl Settings {
    /// Brings values from disk or the UI into their allowed ranges.
    fn sanitized(mut self) -> Result<Self> {
        self.max_concurrent = self.max_concurrent.clamp(1, MAX_CONCURRENT_LIMIT);
        self.custom_template = self.custom_template.trim().to_string();
        if self.filename == FilenameStyle::Custom {
            let template = &self.custom_template;
            // A name has to vary per video, or every download overwrites the last.
            if !template.contains("%(") {
                return Err(AppError::internal(
                    "A custom file name needs at least one field, such as %(title)s, so each video gets its own name.",
                ));
            }
            // Folders are chosen with the folder setting, not smuggled into the name.
            if template.contains(['/', '\\']) || template.contains("..") {
                return Err(AppError::internal("A custom file name can’t contain folders."));
            }
        }
        Ok(self)
    }

    /// The yt-dlp output template for file names, without extension.
    pub fn filename_template(&self) -> String {
        self.filename.template(&self.custom_template)
    }
}

pub struct SettingsStore {
    file: PathBuf,
    current: Mutex<Settings>,
}

impl SettingsStore {
    pub fn load(app: &AppHandle) -> Result<Self> {
        let dir = app
            .path()
            .app_config_dir()
            .map_err(|err| AppError::setup("Couldn't locate the settings folder.").with_details(err.to_string()))?;
        std::fs::create_dir_all(&dir)?;
        let file = dir.join("settings.json");
        // A missing or damaged file just means defaults.
        let current = read(&file)
            .and_then(|settings| settings.sanitized().ok())
            .unwrap_or_default();
        Ok(Self {
            file,
            current: Mutex::new(current),
        })
    }

    pub fn get(&self) -> Settings {
        self.current.lock().unwrap().clone()
    }

    /// Validates, saves and returns the settings as stored.
    pub fn set(&self, settings: Settings) -> Result<Settings> {
        let settings = settings.sanitized()?;
        // Write-then-rename so a crash can't leave a half-written file.
        let tmp = self.file.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(&settings)?)?;
        std::fs::rename(&tmp, &self.file)?;
        *self.current.lock().unwrap() = settings.clone();
        Ok(settings)
    }
}

fn read(file: &Path) -> Option<Settings> {
    serde_json::from_slice(&std::fs::read(file).ok()?).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn concurrency_is_kept_in_range() {
        let settings = Settings {
            max_concurrent: 99,
            ..Settings::default()
        }
        .sanitized()
        .unwrap();
        assert_eq!(settings.max_concurrent, MAX_CONCURRENT_LIMIT);
        let settings = Settings {
            max_concurrent: 0,
            ..Settings::default()
        }
        .sanitized()
        .unwrap();
        assert_eq!(settings.max_concurrent, 1);
    }

    #[test]
    fn custom_names_must_vary_and_stay_in_the_folder() {
        let custom = |template: &str| {
            Settings {
                filename: FilenameStyle::Custom,
                custom_template: template.into(),
                ..Settings::default()
            }
            .sanitized()
        };
        assert!(custom("%(uploader)s - %(title)s").is_ok());
        assert!(custom("video").is_err());
        assert!(custom("../%(title)s").is_err());
        assert!(custom("sub\\%(title)s").is_err());
    }

    #[test]
    fn old_settings_files_fill_in_new_fields() {
        let settings: Settings = serde_json::from_str(r#"{"max_concurrent": 2}"#).unwrap();
        assert_eq!(settings.max_concurrent, 2);
        assert_eq!(settings.filename, FilenameStyle::TitleId);
    }
}
