use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::Duration;
use tauri::Manager;

use crate::error::AppError;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub stash_url: String,
    pub api_key: String,
    pub query_filter: String,
    pub rotation_mode: RotationMode,
    pub interval: Interval,
    pub fit_mode: FitMode,
    pub min_resolution: MinResolution,
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

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum MinResolution {
    #[default]
    None,
    Hd720,
    FullHd1080,
    Qhd1440,
    Uhd4k,
}

impl MinResolution {
    /// Returns the Stash `image_filter.resolution` criterion JSON for this minimum,
    /// or `None` if no filtering is requested.
    pub fn to_stash_filter(&self) -> Option<serde_json::Value> {
        let bucket = match self {
            MinResolution::None => return Option::None,
            MinResolution::Hd720 => "WEB_HD",
            MinResolution::FullHd1080 => "STANDARD_HD",
            MinResolution::Qhd1440 => "FULL_HD",
            MinResolution::Uhd4k => "QUAD_HD",
        };
        Some(serde_json::json!({
            "value": bucket,
            "modifier": "GREATER_THAN"
        }))
    }
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
            min_resolution: MinResolution::None,
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
    match serde_json::from_str(&contents) {
        Ok(settings) => Ok(settings),
        Err(e) => {
            eprintln!("[StashPaper] Failed to parse settings, using defaults: {}", e);
            Ok(Settings::default())
        }
    }
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
            min_resolution: MinResolution::None,
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
    fn test_missing_fields_use_defaults() {
        // Simulates loading old settings.json that's missing new fields
        let json = r#"{"stash_url": "http://localhost:9999", "api_key": "key"}"#;
        let settings: Settings = serde_json::from_str(json).unwrap();
        assert_eq!(settings.stash_url, "http://localhost:9999");
        assert_eq!(settings.api_key, "key");
        // Missing fields should get defaults
        assert_eq!(settings.rotation_mode, RotationMode::Random);
        assert_eq!(settings.interval, Interval::ThirtyMinutes);
        assert_eq!(settings.fit_mode, FitMode::Crop);
        assert!(!settings.per_monitor);
        assert!(!settings.wifi_only);
    }

    #[test]
    fn test_unknown_fields_ignored() {
        // Simulates loading settings.json with removed/renamed fields
        let json = r#"{
            "stash_url": "http://localhost:9999",
            "api_key": "key",
            "image_filter": "old_field_that_no_longer_exists"
        }"#;
        let settings: Settings = serde_json::from_str(json).unwrap();
        assert_eq!(settings.stash_url, "http://localhost:9999");
    }

    #[test]
    fn test_min_resolution_serde_roundtrip() {
        let settings = Settings {
            min_resolution: MinResolution::FullHd1080,
            ..Settings::default()
        };
        let json = serde_json::to_string(&settings).unwrap();
        assert!(json.contains("\"full_hd1080\""));
        let deserialized: Settings = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.min_resolution, MinResolution::FullHd1080);
    }

    #[test]
    fn test_min_resolution_to_stash_filter() {
        assert!(MinResolution::None.to_stash_filter().is_none());

        let filter = MinResolution::Hd720.to_stash_filter().unwrap();
        assert_eq!(filter["value"], "WEB_HD");
        assert_eq!(filter["modifier"], "GREATER_THAN");

        let filter = MinResolution::FullHd1080.to_stash_filter().unwrap();
        assert_eq!(filter["value"], "STANDARD_HD");

        let filter = MinResolution::Qhd1440.to_stash_filter().unwrap();
        assert_eq!(filter["value"], "FULL_HD");

        let filter = MinResolution::Uhd4k.to_stash_filter().unwrap();
        assert_eq!(filter["value"], "QUAD_HD");
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
