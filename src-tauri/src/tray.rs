//! The tray icon and its menu. The engine owns a `TrayStatus` and calls
//! `refresh` whenever it changes; the menu is rebuilt from it each time.

use std::sync::Mutex;
use std::time::SystemTime;
use tauri::{
    image::Image,
    menu::{Menu, MenuItem, PredefinedMenuItem, Submenu},
    AppHandle, Manager,
};

pub const TRAY_ID: &str = "main-tray";

/// Menu item ids. "Open in Stash" items are `open:<index>`.
pub const NEXT: &str = "next";
pub const PAUSE: &str = "pause";
pub const RESUME: &str = "resume";
pub const SETTINGS: &str = "settings";
pub const LOGS: &str = "logs";
pub const QUIT: &str = "quit";
pub const OPEN_PREFIX: &str = "open:";

pub struct TrayIcons {
    pub normal: Image<'static>,
    pub error: Image<'static>,
}

/// One "Open in Stash" entry: the image on one monitor (label empty when
/// there's only one image).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct OpenItem {
    pub label: String,
    pub url: String,
}

/// What the tray shows.
#[derive(Debug, Clone, Default)]
pub struct TrayStatus {
    /// A Stash URL is set
    pub configured: bool,
    pub paused: bool,
    pub last_changed: Option<SystemTime>,
    pub error: Option<String>,
    /// When the engine retries after `error`
    pub retry_at: Option<SystemTime>,
    pub open: Vec<OpenItem>,
}

/// The URLs behind the "Open in Stash" items, for the menu event handler.
#[derive(Default)]
pub struct OpenUrls(pub Mutex<Vec<String>>);

/// Apply a grayscale + red tint to RGBA icon data to produce an "error" variant.
pub fn make_error_icon(rgba: &[u8], width: u32, height: u32) -> Image<'static> {
    let mut tinted = rgba.to_vec();
    for pixel in tinted.as_chunks_mut::<4>().0 {
        let r = pixel[0] as f32;
        let g = pixel[1] as f32;
        let b = pixel[2] as f32;
        let gray = 0.299 * r + 0.587 * g + 0.114 * b;
        pixel[0] = (gray * 1.4).min(255.0) as u8;
        pixel[1] = (gray * 0.4).min(255.0) as u8;
        pixel[2] = (gray * 0.4).min(255.0) as u8;
        // alpha unchanged
    }
    Image::new_owned(tinted, width, height)
}

fn truncate(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let cut: String = text.chars().take(max_chars.saturating_sub(3)).collect();
    format!("{}...", cut.trim_end())
}

fn local(time: SystemTime) -> chrono::DateTime<chrono::Local> {
    chrono::DateTime::<chrono::Local>::from(time)
}

/// "at 14:05", "yesterday at 14:05" or "on Sep 26 at 14:05", relative to `now`.
fn when(time: SystemTime, now: SystemTime) -> String {
    let (time, now) = (local(time), local(now));
    let clock = time.format("%H:%M");
    let days = now
        .date_naive()
        .signed_duration_since(time.date_naive())
        .num_days();
    match days {
        0 => format!("at {}", clock),
        1 => format!("yesterday at {}", clock),
        _ => format!("on {} at {}", time.format("%b %-d"), clock),
    }
}

/// The first, disabled menu line. Linux trays show no tooltip, so this is the
/// only place a Linux user sees what the app is doing.
pub fn status_line(status: &TrayStatus, now: SystemTime) -> String {
    if !status.configured {
        return "Not set up yet: open Settings".into();
    }
    if status.paused {
        return match status.error {
            Some(_) => "Paused (the last try failed)".into(),
            None => "Paused".into(),
        };
    }
    if let Some(error) = &status.error {
        let line = match status.retry_at {
            Some(retry) => format!("Retrying {}: {}", when(retry, now), error),
            None => error.clone(),
        };
        return truncate(&line, 80);
    }
    match status.last_changed {
        Some(time) => format!("Changed {}", when(time, now)),
        None => "Waiting for the first wallpaper".into(),
    }
}

fn tooltip(status: &TrayStatus, now: SystemTime) -> String {
    let text = match &status.error {
        Some(error) => error.clone(),
        None => status_line(status, now),
    };
    truncate(&format!("StashPaper - {}", text), 250)
}

pub fn build_menu(app: &AppHandle, status: &TrayStatus) -> tauri::Result<Menu<tauri::Wry>> {
    let now = SystemTime::now();
    let status_item =
        MenuItem::with_id(app, "status", status_line(status, now), false, None::<&str>)?;
    let next = MenuItem::with_id(app, NEXT, "Next Wallpaper", status.configured, None::<&str>)?;
    let pause = if status.paused {
        MenuItem::with_id(app, RESUME, "Resume", true, None::<&str>)?
    } else {
        MenuItem::with_id(app, PAUSE, "Pause", status.configured, None::<&str>)?
    };
    let menu = Menu::with_items(
        app,
        &[
            &status_item,
            &PredefinedMenuItem::separator(app)?,
            &next,
            &pause,
        ],
    )?;

    match status.open.as_slice() {
        [] => {}
        [_] => menu.append(&MenuItem::with_id(
            app,
            format!("{}0", OPEN_PREFIX),
            "Open in Stash",
            true,
            None::<&str>,
        )?)?,
        items => {
            let submenu = Submenu::with_id(app, "open", "Open in Stash", true)?;
            for (index, item) in items.iter().enumerate() {
                submenu.append(&MenuItem::with_id(
                    app,
                    format!("{}{}", OPEN_PREFIX, index),
                    &item.label,
                    true,
                    None::<&str>,
                )?)?;
            }
            menu.append(&submenu)?;
        }
    }

    menu.append(&PredefinedMenuItem::separator(app)?)?;
    if status.error.is_some() {
        menu.append(&MenuItem::with_id(
            app,
            LOGS,
            "Open Log Folder",
            true,
            None::<&str>,
        )?)?;
    }
    menu.append_items(&[
        &MenuItem::with_id(app, SETTINGS, "Settings", true, None::<&str>)?,
        &MenuItem::with_id(app, QUIT, "Quit", true, None::<&str>)?,
    ])?;
    Ok(menu)
}

/// Bring the tray's menu, icon and tooltip in line with `status`.
pub fn refresh(app: &AppHandle, status: &TrayStatus) {
    *app.state::<OpenUrls>().0.lock().unwrap() =
        status.open.iter().map(|item| item.url.clone()).collect();

    let Some(tray) = app.tray_by_id(TRAY_ID) else {
        return;
    };
    match build_menu(app, status) {
        Ok(menu) => {
            if let Err(e) = tray.set_menu(Some(menu)) {
                log::warn!("Couldn't update the tray menu: {}", e);
            }
        }
        Err(e) => log::warn!("Couldn't build the tray menu: {}", e),
    }
    let icons = app.state::<TrayIcons>();
    let icon = if status.error.is_some() {
        &icons.error
    } else {
        &icons.normal
    };
    let _ = tray.set_icon(Some(icon.clone()));
    let _ = tray.set_tooltip(Some(tooltip(status, SystemTime::now())));
}

/// The Stash URL behind an `open:<index>` menu id, if there is one.
pub fn open_url_for(app: &AppHandle, menu_id: &str) -> Option<String> {
    let index: usize = menu_id.strip_prefix(OPEN_PREFIX)?.parse().ok()?;
    app.state::<OpenUrls>()
        .0
        .lock()
        .unwrap()
        .get(index)
        .cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_make_error_icon_grayscale_red_tint() {
        // White pixel: R=255, G=255, B=255, A=255
        // gray = 0.299*255 + 0.587*255 + 0.114*255 = 255
        // R = 255*1.4 = clamped 255, G = 255*0.4 = 102, B = 255*0.4 = 102
        let white_pixel = [255u8, 255, 255, 255];
        let result = make_error_icon(&white_pixel, 1, 1);
        let rgba = result.rgba();
        assert_eq!(rgba[0], 255); // R clamped
        assert_eq!(rgba[1], 102); // G dimmed
        assert_eq!(rgba[2], 102); // B dimmed
        assert_eq!(rgba[3], 255); // A preserved
    }

    #[test]
    fn test_make_error_icon_preserves_transparency() {
        // Transparent pixel
        let transparent = [100u8, 200, 50, 0];
        let result = make_error_icon(&transparent, 1, 1);
        let rgba = result.rgba();
        assert_eq!(rgba[3], 0); // alpha unchanged
    }

    #[test]
    fn test_make_error_icon_pure_green_gets_red_shift() {
        // Pure green: R=0, G=255, B=0, A=255
        // gray = 0.587*255 ≈ 149.685
        let green = [0u8, 255, 0, 255];
        let result = make_error_icon(&green, 1, 1);
        let rgba = result.rgba();
        // R should be significantly higher than G and B
        assert!(rgba[0] > rgba[1]);
        assert!(rgba[0] > rgba[2]);
    }

    fn at(day: u32, hour: u32, minute: u32) -> SystemTime {
        use chrono::TimeZone;
        chrono::Local
            .with_ymd_and_hms(2026, 9, day, hour, minute, 0)
            .unwrap()
            .into()
    }

    fn configured() -> TrayStatus {
        TrayStatus {
            configured: true,
            ..TrayStatus::default()
        }
    }

    #[test]
    fn status_line_says_what_the_app_is_doing() {
        let now = at(28, 16, 0);
        assert_eq!(
            status_line(&TrayStatus::default(), now),
            "Not set up yet: open Settings"
        );

        let mut status = configured();
        assert_eq!(status_line(&status, now), "Waiting for the first wallpaper");

        status.last_changed = Some(at(28, 14, 5));
        assert_eq!(status_line(&status, now), "Changed at 14:05");
        status.last_changed = Some(at(27, 14, 5));
        assert_eq!(status_line(&status, now), "Changed yesterday at 14:05");
        status.last_changed = Some(at(26, 14, 5));
        assert_eq!(status_line(&status, now), "Changed on Sep 26 at 14:05");

        status.error = Some("Stash error: the API key was rejected (HTTP 401)".into());
        status.retry_at = Some(at(28, 16, 1));
        assert_eq!(
            status_line(&status, now),
            "Retrying at 16:01: Stash error: the API key was rejected (HTTP 401)"
        );

        // pausing is the user's choice, but a failure behind it still shows
        status.paused = true;
        assert_eq!(status_line(&status, now), "Paused (the last try failed)");
        status.error = None;
        assert_eq!(status_line(&status, now), "Paused");
    }

    #[test]
    fn long_errors_are_cut_to_fit_a_menu() {
        let status = TrayStatus {
            error: Some("x".repeat(200)),
            ..configured()
        };
        let line = status_line(&status, SystemTime::now());
        assert!(line.chars().count() <= 80, "{line}");
        assert!(line.ends_with("..."));
    }

    #[test]
    fn tooltip_shows_more_of_the_error_but_not_all_of_a_huge_one() {
        let status = TrayStatus {
            error: Some("y".repeat(150)),
            ..configured()
        };
        assert_eq!(
            tooltip(&status, SystemTime::now()),
            format!("StashPaper - {}", "y".repeat(150))
        );

        let huge = TrayStatus {
            error: Some("z".repeat(1000)),
            ..configured()
        };
        assert!(tooltip(&huge, SystemTime::now()).chars().count() <= 250);
    }
}
