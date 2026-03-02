use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::Duration;
use tauri::Manager;

use crate::error::AppError;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    pub stash_url: String,
    pub api_key: String,
    pub query_filter: String,
    pub rotation_mode: RotationMode,
    pub interval: Interval,
    pub fit_mode: FitMode,
    pub per_monitor: bool,
    pub wifi_only: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RotationMode {
    Random,
    Sequential,
    Shuffle,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FitMode {
    Center,
    Crop,
    Fit,
    Span,
    Stretch,
    Tile,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Interval {
    FiveMinutes,
    FifteenMinutes,
    ThirtyMinutes,
    OneHour,
    FourHours,
    Daily,
}

impl Interval {
    pub fn to_duration(self) -> Duration {
        match self {
            Interval::FiveMinutes => Duration::from_secs(5 * 60),
            Interval::FifteenMinutes => Duration::from_secs(15 * 60),
            Interval::ThirtyMinutes => Duration::from_secs(30 * 60),
            Interval::OneHour => Duration::from_secs(60 * 60),
            Interval::FourHours => Duration::from_secs(4 * 60 * 60),
            Interval::Daily => Duration::from_secs(24 * 60 * 60),
        }
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            stash_url: String::new(),
            api_key: String::new(),
            query_filter: r#"{"image_filter": {}, "filter": {}}"#.to_string(),
            rotation_mode: RotationMode::Random,
            interval: Interval::ThirtyMinutes,
            fit_mode: FitMode::Crop,
            per_monitor: false,
            wifi_only: false,
        }
    }
}

fn settings_path(app: &tauri::AppHandle) -> Result<PathBuf, AppError> {
    let config_dir = app
        .path()
        .app_config_dir()
        .map_err(|e| AppError::Settings(e.to_string()))?;
    std::fs::create_dir_all(&config_dir)?;
    Ok(config_dir.join("settings.json"))
}

pub fn load(app: &tauri::AppHandle) -> Result<Settings, AppError> {
    let path = settings_path(app)?;
    if !path.exists() {
        return Ok(Settings::default());
    }
    let contents = std::fs::read_to_string(&path)?;
    serde_json::from_str(&contents).map_err(|e| AppError::Settings(e.to_string()))
}

pub fn save(app: &tauri::AppHandle, settings: &Settings) -> Result<(), AppError> {
    let path = settings_path(app)?;
    let contents =
        serde_json::to_string_pretty(settings).map_err(|e| AppError::Settings(e.to_string()))?;
    std::fs::write(&path, &contents)?;

    // Restrict file permissions to owner-only (contains API key)
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    }

    Ok(())
}

pub fn is_configured(settings: &Settings) -> bool {
    !settings.stash_url.is_empty() && !settings.api_key.is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_settings_serialization_roundtrip() {
        let settings = Settings {
            stash_url: "http://localhost:9999".into(),
            api_key: "test-key".into(),
            query_filter: r#"{"image_filter":{"tags":{"value":["wallpaper"],"modifier":"INCLUDES_ALL"}}}"#.into(),
            rotation_mode: RotationMode::Shuffle,
            interval: Interval::OneHour,
            fit_mode: FitMode::Crop,
            per_monitor: true,
            wifi_only: false,
        };

        let json = serde_json::to_string(&settings).unwrap();
        let deserialized: Settings = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.stash_url, "http://localhost:9999");
        assert_eq!(deserialized.rotation_mode, RotationMode::Shuffle);
        assert_eq!(deserialized.interval, Interval::OneHour);
        assert!(deserialized.per_monitor);
    }

    #[test]
    fn test_default_settings() {
        let settings = Settings::default();
        assert!(settings.stash_url.is_empty());
        assert_eq!(settings.rotation_mode, RotationMode::Random);
        assert_eq!(settings.interval, Interval::ThirtyMinutes);
        assert!(!settings.per_monitor);
        assert!(!settings.wifi_only);
    }

    #[test]
    fn test_interval_durations() {
        assert_eq!(Interval::FiveMinutes.to_duration(), Duration::from_secs(300));
        assert_eq!(Interval::Daily.to_duration(), Duration::from_secs(86400));
    }

    #[test]
    fn test_is_configured() {
        let mut settings = Settings::default();
        assert!(!is_configured(&settings));

        settings.stash_url = "http://localhost:9999".into();
        assert!(!is_configured(&settings));

        settings.api_key = "key".into();
        assert!(is_configured(&settings));
    }
}
