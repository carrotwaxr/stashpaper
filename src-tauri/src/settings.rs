use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
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
    pub fn to_stash_filter(self) -> Option<serde_json::Value> {
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

/// Load settings. A file that doesn't parse is kept as `settings.json.bak`, and
/// the second value says so, so the next save can't silently overwrite the only
/// copy of the user's API key and filter.
pub fn load(app: &tauri::AppHandle) -> Result<(Settings, Option<String>), AppError> {
    load_from(&settings_path(app)?)
}

fn load_from(path: &Path) -> Result<(Settings, Option<String>), AppError> {
    if !path.exists() {
        return Ok((Settings::default(), None));
    }
    let bytes = std::fs::read(path)?;
    // Editors like Notepad add a byte order mark; serde doesn't want it
    let text = std::str::from_utf8(bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(&bytes))
        .map_err(|e| e.to_string());
    let parsed = text.and_then(|text| serde_json::from_str(text).map_err(|e| e.to_string()));
    match parsed {
        Ok(settings) => Ok((settings, None)),
        Err(e) => {
            log::warn!("Failed to parse settings, using defaults: {}", e);
            let warning = match back_up(path, &bytes) {
                Ok(backup) => format!(
                    "Your settings file couldn't be read ({}), so these are the defaults. \
                     The old file is saved as {}.",
                    e,
                    backup.display()
                ),
                Err(backup_error) => {
                    log::warn!("Couldn't back up the settings file: {}", backup_error);
                    format!(
                        "Your settings file couldn't be read ({}), so these are the defaults, \
                         and it couldn't be backed up ({}). Saving will replace it.",
                        e, backup_error
                    )
                }
            };
            Ok((Settings::default(), Some(warning)))
        }
    }
}

/// Keep an unreadable settings file next to the real one. An existing backup
/// with different contents is left alone and this one gets a timestamped name.
fn back_up(path: &Path, contents: &[u8]) -> std::io::Result<PathBuf> {
    let backup = path.with_extension("json.bak");
    match std::fs::read(&backup) {
        Ok(existing) if existing == contents => return Ok(backup),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            write_private(&backup, contents)?;
            return Ok(backup);
        }
        _ => {}
    }
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let dated = path.with_extension(format!("{}.json.bak", secs));
    write_private(&dated, contents)?;
    Ok(dated)
}

pub fn save(app: &tauri::AppHandle, settings: &Settings) -> Result<(), AppError> {
    save_to(&settings_path(app)?, settings)
}

fn save_to(path: &Path, settings: &Settings) -> Result<(), AppError> {
    let contents =
        serde_json::to_string_pretty(settings).map_err(|e| AppError::Settings(e.to_string()))?;
    write_private(path, contents.as_bytes())?;
    Ok(())
}

/// Write `contents` to `path` atomically, and on Unix readable only by the owner
/// from the moment the file exists (it holds the API key).
fn write_private(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    let _ = std::fs::remove_file(&tmp);
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&tmp)?;
    file.write_all(contents)?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(&tmp, path)
}

/// Settings as the window sent them, cleaned up and checked before they're
/// saved or tested: the URL normalized, the filter one the engine can apply.
pub fn prepare(mut settings: Settings) -> Result<Settings, AppError> {
    settings.stash_url = normalize_stash_url(&settings.stash_url)?;
    crate::stash::parse_query_filter(&settings.query_filter)?;
    Ok(settings)
}

/// A Stash server URL is all it takes: the API key is only needed when Stash has
/// a login configured, and it has none out of the box.
pub fn is_configured(settings: &Settings) -> bool {
    !settings.stash_url.trim().is_empty()
}

/// Clean up a Server URL as typed: trim it, drop trailing slashes and a trailing
/// `/graphql` (the app adds that itself), and require http or https. Blank stays
/// blank (not configured yet).
pub fn normalize_stash_url(raw: &str) -> Result<String, AppError> {
    let mut url = raw.trim().trim_end_matches('/').to_string();
    if url.to_ascii_lowercase().ends_with("/graphql") {
        url.truncate(url.len() - "/graphql".len());
        url = url.trim_end_matches('/').to_string();
    }
    if url.is_empty() {
        return Ok(url);
    }
    match reqwest::Url::parse(&url) {
        Ok(parsed) if matches!(parsed.scheme(), "http" | "https") && parsed.host().is_some() => {
            Ok(url)
        }
        _ => Err(AppError::Settings(
            "the Server URL must look like http://host:9999 or https://stash.example.com".into(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_settings_serialization_roundtrip() {
        let settings = Settings {
            stash_url: "http://localhost:9999".into(),
            api_key: "test-key".into(),
            query_filter:
                r#"{"image_filter":{"tags":{"value":["wallpaper"],"modifier":"INCLUDES_ALL"}}}"#
                    .into(),
            rotation_mode: RotationMode::Shuffle,
            interval: Interval::OneHour,
            fit_mode: FitMode::Crop,
            min_resolution: MinResolution::None,
            per_monitor: true,
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
    }

    #[test]
    fn test_interval_durations() {
        assert_eq!(
            Interval::FiveMinutes.to_duration(),
            Duration::from_secs(300)
        );
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
    fn test_is_configured_needs_only_a_url() {
        let mut settings = Settings::default();
        assert!(!is_configured(&settings));

        // Stash has no login out of the box, so no API key is fine
        settings.stash_url = "http://localhost:9999".into();
        assert!(is_configured(&settings));
    }

    #[test]
    fn test_normalize_stash_url() {
        for (raw, want) in [
            ("", ""),
            ("   ", ""),
            ("http://localhost:9999", "http://localhost:9999"),
            ("  http://localhost:9999/  ", "http://localhost:9999"),
            ("http://localhost:9999/graphql", "http://localhost:9999"),
            (
                "https://stash.example.com/GraphQL/",
                "https://stash.example.com",
            ),
            (
                "https://example.com/stash/graphql",
                "https://example.com/stash",
            ),
        ] {
            assert_eq!(normalize_stash_url(raw).unwrap(), want, "{raw}");
        }
        for raw in ["localhost:9999", "stash.lan", "ftp://stash.lan", "http://"] {
            assert!(normalize_stash_url(raw).is_err(), "{raw}");
        }
    }

    #[test]
    fn test_old_settings_with_wifi_only_still_load() {
        let json = r#"{"stash_url": "http://localhost:9999", "wifi_only": true}"#;
        let settings: Settings = serde_json::from_str(json).unwrap();
        assert_eq!(settings.stash_url, "http://localhost:9999");
    }

    #[test]
    fn test_a_bad_settings_file_is_kept_and_reported() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(
            &path,
            b"{\"stash_url\": \"http://x\", \"interval\": \"hourly\"}",
        )
        .unwrap();

        let (settings, warning) = load_from(&path).unwrap();
        assert!(settings.stash_url.is_empty(), "falls back to defaults");
        let warning = warning.expect("the user is told");
        assert!(warning.contains("settings.json.bak"), "{warning}");
        assert_eq!(
            std::fs::read(dir.path().join("settings.json.bak")).unwrap(),
            std::fs::read(&path).unwrap()
        );
    }

    #[test]
    fn test_a_second_broken_file_doesnt_overwrite_the_first_backup() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, b"{broken one").unwrap();
        load_from(&path).unwrap();
        std::fs::write(&path, b"{broken two").unwrap();
        let (_, warning) = load_from(&path).unwrap();

        assert_eq!(
            std::fs::read(dir.path().join("settings.json.bak")).unwrap(),
            b"{broken one"
        );
        let warning = warning.unwrap();
        assert!(!warning.contains("settings.json.bak."), "{warning}");
        let dated: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|name| {
                name.starts_with("settings.")
                    && name.ends_with(".json.bak")
                    && name != "settings.json.bak"
            })
            .collect();
        assert_eq!(dated.len(), 1, "{dated:?}");
    }

    #[cfg(unix)]
    #[test]
    fn test_a_failed_backup_still_starts_with_defaults() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, b"{broken").unwrap();
        // a read-only config dir: the backup can't be written
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o500)).unwrap();
        if std::fs::write(dir.path().join("probe"), b"x").is_ok() {
            // running as root, which ignores the mode
            std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
            return;
        }
        let result = load_from(&path);
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();

        let (settings, warning) = result.unwrap();
        assert!(settings.stash_url.is_empty());
        assert!(warning.unwrap().contains("couldn't be backed up"));
    }

    #[test]
    fn test_a_bom_or_non_utf8_file_never_stops_startup() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, b"\xEF\xBB\xBF{\"stash_url\": \"http://x:9999\"}").unwrap();
        let (settings, warning) = load_from(&path).unwrap();
        assert_eq!(settings.stash_url, "http://x:9999");
        assert!(warning.is_none());

        std::fs::write(&path, b"{\"stash_url\": \"caf\xE9\"}").unwrap();
        let (settings, warning) = load_from(&path).unwrap();
        assert!(settings.stash_url.is_empty());
        assert!(warning.is_some());
    }

    #[test]
    fn test_prepare_normalizes_the_url_and_checks_the_filter() {
        let prepared = prepare(Settings {
            stash_url: "http://host:9999/graphql".into(),
            ..Settings::default()
        })
        .unwrap();
        assert_eq!(prepared.stash_url, "http://host:9999");

        assert!(prepare(Settings {
            stash_url: "host:9999".into(),
            ..Settings::default()
        })
        .is_err());
        assert!(prepare(Settings {
            query_filter: r#"{"imagefilter": {}}"#.into(),
            ..Settings::default()
        })
        .is_err());
    }

    #[test]
    fn test_save_goes_through_a_temp_file_and_never_writes_in_place() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, b"{\"api_key\": \"old\"}").unwrap();
        // something in the way of the temp file: writing in place would
        // succeed, going through the temp file can't
        let blocker = dir.path().join("settings.json.tmp");
        std::fs::create_dir(&blocker).unwrap();
        std::fs::write(blocker.join("x"), b"x").unwrap();

        assert!(save_to(&path, &Settings::default()).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"{\"api_key\": \"old\"}");
    }

    #[test]
    fn test_save_round_trips_and_leaves_no_temp_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let settings = Settings {
            stash_url: "http://localhost:9999".into(),
            api_key: "secret".into(),
            ..Settings::default()
        };
        save_to(&path, &settings).unwrap();
        save_to(&path, &settings).unwrap(); // overwriting works too
        let (loaded, warning) = load_from(&path).unwrap();
        assert!(warning.is_none());
        assert_eq!(loaded.api_key, "secret");
        assert!(!dir.path().join("settings.json.tmp").exists());
    }

    #[cfg(unix)]
    #[test]
    fn test_saved_settings_are_private_even_over_an_open_file() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, b"{}").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        save_to(&path, &Settings::default()).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }
}
