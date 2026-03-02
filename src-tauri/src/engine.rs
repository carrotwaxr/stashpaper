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
    if let Err(e) = rotate(settings, rotation_state, app_handle).await {
        eprintln!("[StashPaper] Rotation error: {}", e);
    }
}

async fn rotate(
    settings: &Arc<RwLock<Settings>>,
    rotation_state: &mut RotationState,
    app_handle: &tauri::AppHandle,
) -> Result<(), AppError> {
    // Clone settings and release the lock before network I/O
    let s = settings.read().await.clone();

    let count = stash::query_image_count(&s).await?;
    if count == 0 {
        return Ok(());
    }

    let page = rotation_state
        .select_next(s.rotation_mode, count)
        .ok_or_else(|| AppError::Stash("No images found".into()))?;

    let image = stash::fetch_image_at_page(&s, page)
        .await?
        .ok_or_else(|| AppError::Stash("Image not found at page".into()))?;

    let image_url = image
        .paths
        .image
        .ok_or_else(|| AppError::Stash("Image has no download URL".into()))?;

    let cache_dir = app_handle
        .path()
        .app_cache_dir()
        .map_err(|e: tauri::Error| AppError::Settings(e.to_string()))?;

    let file_path = stash::download_image(&s, &image_url, &cache_dir).await?;

    let path_str = file_path
        .to_str()
        .ok_or_else(|| AppError::Wallpaper("Invalid file path".into()))?;

    // Set the wallpaper using the fit mode from settings
    wallpaper::set_from_path(path_str)
        .map_err(|e| AppError::Wallpaper(e.to_string()))?;

    // Set the wallpaper mode based on settings
    let mode = match s.fit_mode {
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
