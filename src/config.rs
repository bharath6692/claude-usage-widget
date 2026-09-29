//! Persisted settings: `%APPDATA%\ClaudeUsageWidget\settings.json`.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// The endpoint 429s aggressively below roughly three minutes even with the
/// correct User-Agent, so this is the real floor rather than a preference.
pub const POLL_FLOOR_SECS: u64 = 180;

/// The intervals offered in the tray menu.
pub const ALLOWED_INTERVALS_SECS: [u64; 4] = [180, 300, 900, 3600];

pub const APP_DIR: &str = "ClaudeUsageWidget";
pub const SETTINGS_FILE: &str = "settings.json";
pub const LOG_FILE: &str = "widget.log";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DisplayMode {
    /// Painted over the taskbar, as in the reference design.
    Docked,
    /// Tray icon only, with details in the click popup. Always-works fallback.
    TrayOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThemePref {
    Auto,
    Dark,
    Light,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub poll_interval_secs: u64,
    pub display_mode: DisplayMode,
    pub theme: ThemePref,
    pub alerts_enabled: bool,
    /// Percentages that raise a toast when crossed, e.g. `[80, 95]`.
    pub thresholds: Vec<u8>,
    pub start_with_windows: bool,
    pub lock_position: bool,
    /// Pixels left of the taskbar's right edge — the draggable divider.
    pub overlay_x_offset: i32,
    /// Override for testing auth states against a copied credential file.
    pub credentials_path: Option<PathBuf>,
    /// `version.json` location for the update check.
    pub update_manifest_url: Option<String>,
    /// `#RRGGBB` override when the auto-detected taskbar color is wrong.
    pub background_override: Option<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            poll_interval_secs: POLL_FLOOR_SECS,
            display_mode: DisplayMode::Docked,
            theme: ThemePref::Auto,
            alerts_enabled: true,
            thresholds: vec![80, 95],
            start_with_windows: false,
            lock_position: false,
            overlay_x_offset: 0,
            credentials_path: None,
            update_manifest_url: None,
            background_override: None,
        }
    }
}

impl Settings {
    pub fn poll_interval(&self) -> Duration {
        Duration::from_secs(self.poll_interval_secs.max(POLL_FLOOR_SECS))
    }

    /// Clamp anything a hand-edited file could get wrong.
    pub fn sanitize(&mut self) {
        if !ALLOWED_INTERVALS_SECS.contains(&self.poll_interval_secs) {
            self.poll_interval_secs = self
                .poll_interval_secs
                .max(POLL_FLOOR_SECS)
                .min(ALLOWED_INTERVALS_SECS[ALLOWED_INTERVALS_SECS.len() - 1]);
        }
        self.thresholds.retain(|t| (1..=100).contains(t));
        self.thresholds.sort_unstable();
        self.thresholds.dedup();
    }

    pub fn credentials_path_or_default(&self) -> PathBuf {
        self.credentials_path
            .clone()
            .unwrap_or_else(crate::creds::default_path)
    }
}

/// `%APPDATA%\ClaudeUsageWidget`
pub fn app_data_dir() -> PathBuf {
    std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(APP_DIR)
}

pub fn settings_path() -> PathBuf {
    app_data_dir().join(SETTINGS_FILE)
}

pub fn log_path() -> PathBuf {
    app_data_dir().join(LOG_FILE)
}

/// Load settings, falling back to defaults for a missing or corrupt file.
/// A bad settings file must never stop the widget from starting.
pub fn load(path: &Path) -> Settings {
    let mut settings = std::fs::read_to_string(path)
        .ok()
        .and_then(|raw| serde_json::from_str::<Settings>(&raw).ok())
        .unwrap_or_default();
    settings.sanitize();
    settings
}

pub fn save(path: &Path, settings: &Settings) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(settings)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    // Write-then-rename so a crash mid-write cannot leave a truncated file.
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, json)?;
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_use_the_poll_floor() {
        let s = Settings::default();
        assert_eq!(s.poll_interval(), Duration::from_secs(180));
        assert_eq!(s.display_mode, DisplayMode::Docked);
        assert!(s.alerts_enabled);
    }

    #[test]
    fn interval_below_the_floor_is_raised() {
        // A hand-edited 30s would just earn sustained 429s.
        let mut s = Settings {
            poll_interval_secs: 30,
            ..Default::default()
        };
        s.sanitize();
        assert!(s.poll_interval_secs >= POLL_FLOOR_SECS);
        assert_eq!(s.poll_interval(), Duration::from_secs(180));
    }

    #[test]
    fn absurd_interval_is_capped() {
        let mut s = Settings {
            poll_interval_secs: 9_999_999,
            ..Default::default()
        };
        s.sanitize();
        assert_eq!(s.poll_interval_secs, 3600);
    }

    #[test]
    fn thresholds_are_deduped_sorted_and_range_checked() {
        let mut s = Settings {
            thresholds: vec![95, 0, 80, 95, 200, 100],
            ..Default::default()
        };
        s.sanitize();
        assert_eq!(s.thresholds, vec![80, 95, 100]);
    }

    #[test]
    fn round_trips_through_disk() {
        let dir = std::env::temp_dir().join("cuw-settings-roundtrip");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");

        let original = Settings {
            poll_interval_secs: 900,
            display_mode: DisplayMode::TrayOnly,
            theme: ThemePref::Dark,
            thresholds: vec![70, 90],
            start_with_windows: true,
            overlay_x_offset: -120,
            ..Default::default()
        };
        save(&path, &original).unwrap();
        assert_eq!(load(&path), original);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn corrupt_file_falls_back_to_defaults() {
        let dir = std::env::temp_dir().join("cuw-settings-corrupt");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");
        std::fs::write(&path, "{ not json").unwrap();

        assert_eq!(load(&path), Settings::default());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn partial_file_keeps_defaults_for_absent_keys() {
        let dir = std::env::temp_dir().join("cuw-settings-partial");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");
        std::fs::write(&path, r#"{"theme":"light"}"#).unwrap();

        let loaded = load(&path);
        assert_eq!(loaded.theme, ThemePref::Light);
        assert_eq!(loaded.poll_interval_secs, POLL_FLOOR_SECS);
        assert!(loaded.alerts_enabled);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn missing_file_yields_defaults() {
        assert_eq!(load(Path::new("C:/nope/does/not/exist.json")), Settings::default());
    }
}
