//! User preferences, kept in `settings.json` in the app's config folder.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Deserializer, Serialize};
use tauri::{AppHandle, Manager};

use crate::binaries::sources::Channel;
use crate::binaries::UpdatePolicy;
use crate::error::{AppError, Result};

/// How downloaded files are named. The extension is always added by Siphon.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FilenameStyle {
    /// `Title`
    Title,
    /// `Title [id]`: unique per video, so two uploads with one title don't collide.
    #[default]
    TitleId,
    /// `Channel - Title`
    ChannelTitle,
    /// `2026-11-16 Title` (upload date)
    DateTitle,
}

impl FilenameStyle {
    /// The yt-dlp output template for this style, without extension.
    /// Lengths are capped in bytes so names stay within Windows' 255 with
    /// what Siphon and yt-dlp add while working (`.siphon-…`, `.f137`, `.part`):
    /// at most 213, as Channel - Title. The id too: a direct link's is its
    /// file name, as its title is, which once made a name over 360.
    pub fn template(self) -> &'static str {
        match self {
            FilenameStyle::Title => "%(title).180B",
            FilenameStyle::TitleId => "%(title).180B [%(id).30B]",
            FilenameStyle::ChannelTitle => "%(channel,uploader|Unknown).60B - %(title).150B",
            FilenameStyle::DateTitle => "%(upload_date>%Y-%m-%d|undated)s %(title).180B",
        }
    }
}

/// A style this version doesn't know is read as the default rather than
/// failing the file, which would lose every other setting with it. Up to
/// 0.18.1 there was `custom`, the user's own yt-dlp template.
fn known_style<'de, D: Deserializer<'de>>(deserializer: D) -> std::result::Result<FilenameStyle, D::Error> {
    let value = serde_json::Value::deserialize(deserializer)?;
    Ok(FilenameStyle::deserialize(value).unwrap_or_default())
}

/// Whether newer versions of yt-dlp, FFmpeg and Deno are installed without asking.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UpdateMode {
    Auto,
    Ask,
}

pub const MAX_CONCURRENT_LIMIT: usize = 6;

/// The slowest and fastest speed limits, in bytes per second: 0.1 and 1000 MB/s.
pub const MIN_SPEED_LIMIT: u64 = 100 * 1024;
pub const MAX_SPEED_LIMIT: u64 = 1000 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Where downloads are saved. `None` means the system Downloads folder.
    pub output_dir: Option<PathBuf>,
    #[serde(deserialize_with = "known_style")]
    pub filename: FilenameStyle,
    /// How many downloads run at once; the rest wait in the queue.
    pub max_concurrent: usize,
    /// yt-dlp's release channel. Nightly gets site fixes sooner.
    pub update_channel: Channel,
    pub update_mode: UpdateMode,
    /// All downloads together get at most this many bytes per second;
    /// `None` is unlimited. (MB here is 1,048,576 bytes, as speeds are shown.)
    pub speed_limit: Option<u64>,
    /// The languages the user listens and reads in, the one they'd rather
    /// first ("el", "en"): an MKV's tracks in them are kept (see
    /// `local::mkv::pick`). Empty until chosen: see [`my_languages`].
    pub languages: Vec<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            output_dir: None,
            filename: FilenameStyle::TitleId,
            max_concurrent: 3,
            update_channel: Channel::Stable,
            update_mode: UpdateMode::Auto,
            speed_limit: None,
            languages: Vec::new(),
        }
    }
}

impl Settings {
    /// Brings values from disk or the UI into their allowed ranges.
    fn sanitized(mut self) -> Self {
        self.max_concurrent = self.max_concurrent.clamp(1, MAX_CONCURRENT_LIMIT);
        self.languages = tags(self.languages.iter().map(String::as_str));
        self.speed_limit = self
            .speed_limit
            .map(|limit| limit.clamp(MIN_SPEED_LIMIT, MAX_SPEED_LIMIT));
        self
    }

    /// The user's languages, best first: those chosen, or, until some are,
    /// [`default_languages`].
    pub fn my_languages(&self, subtitles: Option<&str>) -> Vec<String> {
        if self.languages.is_empty() {
            default_languages(subtitles)
        } else {
            self.languages.clone()
        }
    }

    pub fn update_policy(&self) -> UpdatePolicy {
        match self.update_mode {
            UpdateMode::Auto => UpdatePolicy::Install,
            UpdateMode::Ask => UpdatePolicy::Ask,
        }
    }

    /// The yt-dlp output template for file names, without extension.
    pub fn filename_template(&self) -> &'static str {
        self.filename.template()
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
        Ok(Self::open(dir.join("settings.json")))
    }

    /// The settings stored in `file`. A missing or damaged file just means defaults.
    fn open(file: PathBuf) -> Self {
        let current = read(&file).map(Settings::sanitized).unwrap_or_default();
        Self::at(file, current)
    }

    fn at(file: PathBuf, current: Settings) -> Self {
        Self {
            file,
            current: Mutex::new(current),
        }
    }

    pub fn get(&self) -> Settings {
        self.current.lock().unwrap().clone()
    }

    /// Validates, saves and returns the settings as stored.
    pub fn set(&self, settings: Settings) -> Result<Settings> {
        let settings = settings.sanitized();
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
        .sanitized();
        assert_eq!(settings.max_concurrent, MAX_CONCURRENT_LIMIT);
        let settings = Settings {
            max_concurrent: 0,
            ..Settings::default()
        }
        .sanitized();
        assert_eq!(settings.max_concurrent, 1);
    }

    #[test]
    fn a_custom_name_from_before_falls_back_without_losing_the_rest() {
        // Up to 0.18.1 a file could name its own template; that style is gone.
        let file = r#"{"output_dir": "D:\\Videos", "filename": "custom", "custom_template": "%(uploader)s - %(title)s",
                       "max_concurrent": 5, "speed_limit": 5242880}"#;
        let settings: Settings = serde_json::from_str(file).unwrap();
        assert_eq!(settings.filename, FilenameStyle::TitleId);
        assert_eq!(settings.output_dir, Some(PathBuf::from(r"D:\Videos")));
        assert_eq!(settings.max_concurrent, 5);
        assert_eq!(settings.speed_limit, Some(5 * 1024 * 1024));
        // The styles still there are read as they were.
        let settings: Settings = serde_json::from_str(r#"{"filename": "date_title"}"#).unwrap();
        assert_eq!(settings.filename, FilenameStyle::DateTitle);
    }

    #[test]
    fn every_file_name_style_stays_short_enough_for_windows() {
        for style in [
            FilenameStyle::Title,
            FilenameStyle::TitleId,
            FilenameStyle::ChannelTitle,
            FilenameStyle::DateTitle,
        ] {
            let template = style.template();
            // Each field's longest (a date is ten characters), and the text between them.
            let mut longest = 0;
            let mut rest = template;
            while let Some(at) = rest.find("%(") {
                longest += at;
                let end = rest[at..].find(')').unwrap() + at;
                let field = &rest[at..=end];
                let after = &rest[end + 1..];
                longest += if field.starts_with("%(upload_date>") {
                    10
                } else {
                    let cap = after.strip_prefix('.').and_then(|cap| cap.split_once('B'));
                    let (bytes, tail) = cap.unwrap_or_else(|| panic!("{template}: {field} has no length cap"));
                    rest = tail;
                    longest += bytes.parse::<usize>().unwrap();
                    continue;
                };
                rest = after.strip_prefix('s').unwrap_or(after);
            }
            longest += rest.len();
            assert!(longest <= 213, "{template} can make a name of {longest} bytes");
        }
    }

    #[test]
    fn old_settings_files_fill_in_new_fields() {
        let settings: Settings = serde_json::from_str(r#"{"max_concurrent": 2}"#).unwrap();
        assert_eq!(settings.max_concurrent, 2);
        assert_eq!(settings.filename, FilenameStyle::TitleId);
        // Files from before the speed limit mean no limit.
        assert_eq!(settings.speed_limit, None);
    }

    #[test]
    fn speed_limit_is_kept_in_range() {
        let limited = |limit| {
            Settings {
                speed_limit: limit,
                ..Settings::default()
            }
            .sanitized()
            .speed_limit
        };
        assert_eq!(limited(None), None);
        assert_eq!(limited(Some(5 * 1024 * 1024)), Some(5 * 1024 * 1024));
        assert_eq!(limited(Some(0)), Some(MIN_SPEED_LIMIT));
        assert_eq!(limited(Some(u64::MAX)), Some(MAX_SPEED_LIMIT));
    }

    #[test]
    fn limits_from_old_presets_are_kept_as_they_were() {
        // The one-click limits were 2, 5, 10 and 25 MB/s before 20, 30, 40 and 50.
        // A limit is stored as bytes per second, so one saved from an old preset
        // reads back unchanged (the interface shows it as a custom limit).
        for mb in [2, 5, 10, 25] {
            let bytes = mb * 1024 * 1024;
            let file = format!(r#"{{"max_concurrent": 3, "speed_limit": {bytes}}}"#);
            let settings: Settings = serde_json::from_str(&file).unwrap();
            assert_eq!(settings.sanitized().speed_limit, Some(bytes), "{mb} MB/s");
        }
    }

    #[test]
    fn speed_limit_is_saved_and_survives_a_restart() {
        let dir = std::env::temp_dir().join(format!("siphon-settings-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("settings.json");

        let store = SettingsStore::at(file.clone(), Settings::default());
        store
            .set(Settings {
                speed_limit: Some(5 * 1024 * 1024),
                ..Settings::default()
            })
            .unwrap();
        // What the next launch reads.
        assert_eq!(
            SettingsStore::open(file.clone()).get().speed_limit,
            Some(5 * 1024 * 1024)
        );

        // And taking it off again sticks too.
        store.set(Settings::default()).unwrap();
        assert_eq!(SettingsStore::open(file).get().speed_limit, None);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

/// The languages taken as the user's until they choose some: the one they
/// want subtitles in, if they chose one, then Windows' languages, in the
/// order Windows lists them.
pub fn default_languages(subtitles: Option<&str>) -> Vec<String> {
    let windows: Vec<String> = windows::Globalization::ApplicationLanguages::Languages()
        .map(|languages| languages.into_iter().map(|tag| tag.to_string()).collect())
        .unwrap_or_default();
    tags(subtitles.into_iter().chain(windows.iter().map(String::as_str)))
}

/// Language tags as Siphon keeps them: the language alone ("en-US" → "en",
/// "gre" → "el"), each once, in order.
fn tags<'a>(languages: impl Iterator<Item = &'a str>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for language in languages {
        let tag = crate::ytdlp::subtitles::language_tag(language);
        let tag = crate::ytdlp::subtitles::primary(&tag).to_string();
        if !tag.is_empty() && tag != "und" && !out.contains(&tag) {
            out.push(tag);
        }
    }
    out
}

#[cfg(test)]
mod language_tests {
    use super::*;

    #[test]
    fn languages_are_kept_as_the_language_alone_each_once() {
        let settings = Settings {
            languages: vec!["el-GR".into(), "gre".into(), "en-US".into(), "".into(), "und".into()],
            ..Settings::default()
        }
        .sanitized();
        assert_eq!(settings.languages, ["el", "en"]);
        assert_eq!(settings.my_languages(Some("fr")), ["el", "en"]);
    }

    #[test]
    fn until_chosen_they_start_with_the_subtitle_language() {
        let languages = Settings::default().my_languages(Some("pt-br"));
        assert_eq!(languages.first().map(String::as_str), Some("pt"));
    }
}
