//! When the next rotation is due, and the engine state that survives a restart.

use crate::rotation::RotationSnapshot;
use crate::settings::Settings;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// How long to wait before retrying after `failures` failed rotations in a row,
/// never longer than the interval itself.
pub fn retry_delay(failures: u32, interval: Duration) -> Duration {
    let delay = match failures {
        0 | 1 => Duration::from_secs(30),
        2 => Duration::from_secs(60),
        3 => Duration::from_secs(5 * 60),
        4 => Duration::from_secs(15 * 60),
        _ => Duration::from_secs(60 * 60),
    };
    delay.min(interval)
}

/// How long until the next rotation is due, measured on the wall clock so time
/// spent suspended counts. Never rotated means due now. A retry after a failure
/// follows `retry_delay`. The result never exceeds `interval`, so a clock set
/// backwards can't postpone rotation indefinitely.
pub fn time_until_due(
    now: SystemTime,
    last_success: Option<SystemTime>,
    last_failure: Option<SystemTime>,
    failures: u32,
    interval: Duration,
) -> Duration {
    let due = if failures > 0 {
        last_failure.map(|t| t + retry_delay(failures, interval))
    } else {
        last_success.map(|t| t + interval)
    };
    match due {
        None => Duration::ZERO,
        Some(due) => due
            .duration_since(now)
            .unwrap_or(Duration::ZERO)
            .min(interval),
    }
}

/// Engine state saved between runs, so a restart neither resets the timer nor
/// sends sequential mode back to the first image.
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SavedState {
    /// Seconds since the epoch of the last successful rotation
    pub last_rotated: Option<u64>,
    /// `selection_key` of the settings the position below belongs to
    pub selection_key: String,
    pub rotation: RotationSnapshot,
    /// The files on the desktop (one per monitor on Windows), so a failed
    /// rotation after a restart knows which cache files it must not delete
    pub current_files: Vec<PathBuf>,
    /// The single-file form of `current_files` that 0.3 saved; read only
    #[serde(skip_serializing)]
    pub current_wallpaper: Option<PathBuf>,
    /// The images on the desktop, for "Open in Stash" after a restart
    pub shown: Vec<ShownImage>,
    /// Pausing sticks across restarts
    pub paused: bool,
}

/// One image on the desktop: its Stash id and, with per-monitor wallpapers,
/// which monitor it's on.
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ShownImage {
    pub id: String,
    pub label: String,
    /// Its Stash page, fixed when it was shown (the server may change since)
    pub url: String,
}

impl SavedState {
    pub fn on_desktop(&self) -> Vec<PathBuf> {
        if self.current_files.is_empty() {
            self.current_wallpaper.iter().cloned().collect()
        } else {
            self.current_files.clone()
        }
    }

    pub fn last_rotated_time(&self) -> Option<SystemTime> {
        self.last_rotated
            .map(|secs| UNIX_EPOCH + Duration::from_secs(secs))
    }
}

/// The settings that decide which image comes next. A saved position only
/// applies while these are unchanged.
pub fn selection_key(settings: &Settings) -> String {
    // Normalized, so cleaning up an old saved URL doesn't count as a new server
    let url = crate::settings::normalize_stash_url(&settings.stash_url)
        .unwrap_or_else(|_| settings.stash_url.clone());
    format!(
        "{}|{:?}|{:?}|{}|{}",
        url,
        settings.rotation_mode,
        settings.min_resolution,
        settings.per_monitor,
        settings.query_filter
    )
}

pub fn unix_secs(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub fn state_path(data_dir: &Path) -> PathBuf {
    data_dir.join("state.json")
}

/// Load saved state. A missing or unreadable file means a fresh start.
pub fn load(path: &Path) -> SavedState {
    let Ok(contents) = std::fs::read_to_string(path) else {
        return SavedState::default();
    };
    serde_json::from_str(&contents).unwrap_or_else(|e| {
        log::warn!("Ignoring unreadable {}: {}", path.display(), e);
        SavedState::default()
    })
}

/// Save state atomically: write a temp file, then rename it over the old one.
pub fn save(path: &Path, state: &SavedState) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(state)?)?;
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOUR: Duration = Duration::from_secs(3600);
    const MIN: Duration = Duration::from_secs(60);

    fn at(secs: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(1_800_000_000 + secs)
    }

    #[test]
    fn never_rotated_is_due_now() {
        assert_eq!(time_until_due(at(0), None, None, 0, HOUR), Duration::ZERO);
    }

    #[test]
    fn waits_out_the_interval_from_the_last_success() {
        let wait = time_until_due(at(600), Some(at(0)), None, 0, HOUR);
        assert_eq!(wait, HOUR - Duration::from_secs(600));
    }

    #[test]
    fn overdue_is_due_now() {
        // e.g. the machine was suspended past the interval
        let wait = time_until_due(at(5 * 3600), Some(at(0)), None, 0, HOUR);
        assert_eq!(wait, Duration::ZERO);
    }

    #[test]
    fn a_failure_retries_soon_and_backs_off() {
        let fail = at(100);
        assert_eq!(
            time_until_due(fail, Some(at(0)), Some(fail), 1, HOUR),
            Duration::from_secs(30)
        );
        assert_eq!(time_until_due(fail, Some(at(0)), Some(fail), 2, HOUR), MIN);
        assert_eq!(
            time_until_due(fail, Some(at(0)), Some(fail), 3, HOUR),
            5 * MIN
        );
        assert_eq!(
            time_until_due(fail, Some(at(0)), Some(fail), 4, HOUR),
            15 * MIN
        );
        assert_eq!(time_until_due(fail, Some(at(0)), Some(fail), 9, HOUR), HOUR);
    }

    #[test]
    fn retries_never_wait_longer_than_the_interval() {
        let five_min = 5 * MIN;
        assert_eq!(retry_delay(9, five_min), five_min);
        assert_eq!(
            time_until_due(at(0), None, Some(at(0)), 9, five_min),
            five_min
        );
    }

    #[test]
    fn a_clock_set_backwards_waits_at_most_one_interval() {
        // last success recorded "in the future" relative to now
        let wait = time_until_due(at(0), Some(at(10 * 3600)), None, 0, HOUR);
        assert_eq!(wait, HOUR);
    }

    #[test]
    fn selection_key_tracks_what_picks_images() {
        let base = Settings::default();
        let other_filter = Settings {
            query_filter:
                r#"{"image_filter": {"rating100": {"value": 80, "modifier": "GREATER_THAN"}}}"#
                    .into(),
            ..Settings::default()
        };
        let other_interval = Settings {
            interval: crate::settings::Interval::Daily,
            ..Settings::default()
        };
        let other_server = Settings {
            stash_url: "http://other:9999".into(),
            ..Settings::default()
        };
        assert_ne!(selection_key(&base), selection_key(&other_filter));
        assert_ne!(selection_key(&base), selection_key(&other_server));
        let tidied = Settings {
            stash_url: "http://localhost:9999".into(),
            ..Settings::default()
        };
        let untidy = Settings {
            stash_url: "http://localhost:9999/graphql".into(),
            ..Settings::default()
        };
        assert_eq!(selection_key(&tidied), selection_key(&untidy));
        assert_eq!(selection_key(&base), selection_key(&other_interval));
    }

    #[test]
    fn state_round_trips_and_a_bad_file_means_fresh_start() {
        let dir = tempfile::tempdir().unwrap();
        let path = state_path(dir.path());
        assert_eq!(load(&path), SavedState::default());

        let state = SavedState {
            last_rotated: Some(1_800_000_000),
            selection_key: "k".into(),
            rotation: RotationSnapshot {
                current_index: 7,
                random_seed: Some(42),
                random_page: 3,
                sort_seed: 99,
            },
            current_files: vec![PathBuf::from("/cache/wallpaper_1_0.jpg")],
            current_wallpaper: None,
            shown: vec![ShownImage {
                id: "12".into(),
                label: "Monitor 1 (1920x1080)".into(),
                url: "http://stash:9999/images/12".into(),
            }],
            paused: true,
        };
        save(&path, &state).unwrap();
        assert_eq!(load(&path), state);
        assert!(!path.with_extension("json.tmp").exists());

        std::fs::write(&path, b"{not json").unwrap();
        assert_eq!(load(&path), SavedState::default());
    }

    #[test]
    fn state_saved_by_0_3_still_names_the_file_on_the_desktop() {
        let old = r#"{"current_wallpaper": "/cache/wallpaper_1_0.jpg"}"#;
        let state: SavedState = serde_json::from_str(old).unwrap();
        assert_eq!(
            state.on_desktop(),
            vec![PathBuf::from("/cache/wallpaper_1_0.jpg")]
        );
    }
}
