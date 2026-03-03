use crate::error::AppError;
use crate::rotation::RotationState;
use crate::settings::Settings;
use crate::stash;
use std::sync::Arc;
use tauri::Manager;
use tokio::sync::{mpsc, RwLock};

#[derive(Debug)]
pub enum Command {
    Next,
    Pause,
    Resume,
    SettingsUpdated,
    Quit,
}

pub type CommandTx = mpsc::Sender<Command>;
pub type CommandRx = mpsc::Receiver<Command>;

pub fn create_channel() -> (CommandTx, CommandRx) {
    mpsc::channel(32)
}

pub async fn run(
    mut rx: CommandRx,
    settings: Arc<RwLock<Settings>>,
    app_handle: tauri::AppHandle,
) {
    let mut paused = false;
    let mut rotation_state = RotationState::new();

    loop {
        let interval = {
            let s = settings.read().await;
            if !crate::settings::is_configured(&s) {
                drop(s);
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                continue;
            }
            s.interval.to_duration()
        };

        tokio::select! {
            cmd = rx.recv() => {
                match cmd {
                    Some(Command::Next) => {
                        do_rotate(&settings, &mut rotation_state, &app_handle).await;
                    }
                    Some(Command::Pause) => {
                        paused = true;
                    }
                    Some(Command::Resume) => {
                        paused = false;
                    }
                    Some(Command::SettingsUpdated) => {
                        rotation_state.reset();
                        // Immediately rotate with new settings
                        do_rotate(&settings, &mut rotation_state, &app_handle).await;
                    }
                    Some(Command::Quit) | None => break,
                }
            }
            _ = tokio::time::sleep(interval), if !paused => {
                do_rotate(&settings, &mut rotation_state, &app_handle).await;
            }
        }
    }
}

async fn do_rotate(
    settings: &Arc<RwLock<Settings>>,
    rotation_state: &mut RotationState,
    app_handle: &tauri::AppHandle,
) {
    match rotate(settings, rotation_state, app_handle).await {
        Ok(()) => {
            crate::update_tray_icon(app_handle, false, None);
        }
        Err(e) => {
            eprintln!("[StashPaper] Rotation error: {}", e);
            crate::update_tray_icon(app_handle, true, Some(&e.to_string()));
        }
    }
}

fn get_monitor_geometries(app: &tauri::AppHandle) -> Vec<crate::MonitorInfo> {
    app.available_monitors()
        .map(|monitors| {
            monitors
                .into_iter()
                .map(|m| {
                    let size = m.size();
                    let pos = m.position();
                    crate::MonitorInfo {
                        width: size.width,
                        height: size.height,
                        x: pos.x,
                        y: pos.y,
                        scale_factor: m.scale_factor(),
                    }
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Set wallpaper with Span mode (for composited multi-monitor images).
fn set_wallpaper_span(path: &str) -> Result<(), AppError> {
    wallpaper::set_from_path(path)
        .map_err(|e| AppError::Wallpaper(e.to_string()))?;

    #[cfg(target_os = "linux")]
    {
        let uri = format!("file://{}", path);
        let _ = std::process::Command::new("gsettings")
            .args(["set", "org.gnome.desktop.background", "picture-uri-dark", &uri])
            .output();
        let _ = std::process::Command::new("gsettings")
            .args(["set", "org.gnome.desktop.background", "picture-options", "spanned"])
            .output();
    }

    wallpaper::set_mode(wallpaper::Mode::Span)
        .map_err(|e| AppError::Wallpaper(e.to_string()))?;

    Ok(())
}

async fn rotate(
    settings: &Arc<RwLock<Settings>>,
    rotation_state: &mut RotationState,
    app_handle: &tauri::AppHandle,
) -> Result<(), AppError> {
    let s = settings.read().await.clone();

    let count = stash::query_image_count(&s).await?;
    if count == 0 {
        return Err(AppError::Stash("No images found".into()));
    }

    let cache_dir = app_handle
        .path()
        .app_cache_dir()
        .map_err(|e: tauri::Error| AppError::Settings(e.to_string()))?;

    // Clean up old cached wallpapers before downloading new ones
    stash::clean_wallpaper_cache(&cache_dir);

    // Determine how many images we need
    let monitors = get_monitor_geometries(app_handle);
    let num_images = if s.per_monitor && monitors.len() > 1 {
        monitors.len()
    } else {
        1
    };

    // Fetch N images
    let results = rotation_state.select_next_batch(s.rotation_mode, count, num_images);
    if results.is_empty() {
        return Err(AppError::Stash("No images found".into()));
    }

    let mut downloaded_paths = Vec::new();
    for result in &results {
        let image = stash::fetch_image_at_page(&s, result.page, result.random_seed)
            .await?
            .ok_or_else(|| AppError::Stash("Image not found at page".into()))?;

        let image_url = image
            .paths
            .image
            .ok_or_else(|| AppError::Stash("Image has no download URL".into()))?;

        let file_path = stash::download_image(&s, &image_url, &cache_dir).await?;
        downloaded_paths.push(file_path);
    }

    if s.per_monitor && monitors.len() > 1 {
        // Composite and set as spanned wallpaper
        let composite_path = cache_dir.join("composite_wallpaper.png");
        let geoms: Vec<crate::compositor::MonitorGeometry> = monitors
            .iter()
            .map(|m| crate::compositor::MonitorGeometry {
                x: m.x,
                y: m.y,
                width: m.width,
                height: m.height,
            })
            .collect();
        crate::compositor::composite_wallpaper(&downloaded_paths, &geoms, &composite_path)?;

        let path_str = composite_path
            .to_str()
            .ok_or_else(|| AppError::Wallpaper("Invalid file path".into()))?;

        set_wallpaper_span(path_str)?;
    } else {
        // Single monitor path (unchanged behavior)
        let path_str = downloaded_paths[0]
            .to_str()
            .ok_or_else(|| AppError::Wallpaper("Invalid file path".into()))?;
        set_wallpaper(path_str, &s)?;
    }

    Ok(())
}

fn set_wallpaper(path: &str, settings: &Settings) -> Result<(), AppError> {
    // Set wallpaper via the wallpaper crate (handles most DEs)
    wallpaper::set_from_path(path)
        .map_err(|e| AppError::Wallpaper(e.to_string()))?;

    // GNOME dark mode fix: the wallpaper crate only sets picture-uri,
    // but GNOME uses picture-uri-dark when color-scheme is prefer-dark
    #[cfg(target_os = "linux")]
    {
        let uri = format!("file://{}", path);
        let _ = std::process::Command::new("gsettings")
            .args(["set", "org.gnome.desktop.background", "picture-uri-dark", &uri])
            .output();
    }

    // Set the wallpaper mode based on settings
    let mode = match settings.fit_mode {
        crate::settings::FitMode::Center => wallpaper::Mode::Center,
        crate::settings::FitMode::Crop => wallpaper::Mode::Crop,
        crate::settings::FitMode::Fit => wallpaper::Mode::Fit,
        crate::settings::FitMode::Span => wallpaper::Mode::Span,
        crate::settings::FitMode::Stretch => wallpaper::Mode::Stretch,
        crate::settings::FitMode::Tile => wallpaper::Mode::Tile,
    };
    wallpaper::set_mode(mode)
        .map_err(|e| AppError::Wallpaper(e.to_string()))?;

    Ok(())
}
