use crate::error::AppError;
use crate::rotation::RotationState;
use crate::settings::Settings;
use crate::stash;
use std::path::{Path, PathBuf};
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

pub async fn run(mut rx: CommandRx, settings: Arc<RwLock<Settings>>, app_handle: tauri::AppHandle) {
    let mut paused = false;
    let mut rotation_state = RotationState::new();
    let mut count_hint: Option<usize> = None;

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
                        do_rotate(&settings, &mut rotation_state, &mut count_hint, &app_handle).await;
                    }
                    Some(Command::Pause) => {
                        paused = true;
                    }
                    Some(Command::Resume) => {
                        paused = false;
                    }
                    Some(Command::SettingsUpdated) => {
                        rotation_state.reset();
                        count_hint = None;
                        // Immediately rotate with new settings
                        do_rotate(&settings, &mut rotation_state, &mut count_hint, &app_handle).await;
                    }
                    Some(Command::Quit) | None => break,
                }
            }
            _ = tokio::time::sleep(interval), if !paused => {
                do_rotate(&settings, &mut rotation_state, &mut count_hint, &app_handle).await;
            }
        }
    }
}

async fn do_rotate(
    settings: &Arc<RwLock<Settings>>,
    rotation_state: &mut RotationState,
    count_hint: &mut Option<usize>,
    app_handle: &tauri::AppHandle,
) {
    match rotate(settings, rotation_state, count_hint, app_handle).await {
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
    wallpaper::set_from_path(path).map_err(|e| AppError::Wallpaper(e.to_string()))?;

    #[cfg(target_os = "linux")]
    {
        let uri = format!("file://{}", path);
        let _ = std::process::Command::new("gsettings")
            .args([
                "set",
                "org.gnome.desktop.background",
                "picture-uri-dark",
                &uri,
            ])
            .output();
        let _ = std::process::Command::new("gsettings")
            .args([
                "set",
                "org.gnome.desktop.background",
                "picture-options",
                "spanned",
            ])
            .output();
    }

    set_mode_or_log(wallpaper::Mode::Span);

    Ok(())
}

/// Set the fit mode. The crate can't do this on macOS or on desktops it doesn't
/// recognize, but by then the image itself is set, so that's not a failed rotation.
fn set_mode_or_log(mode: wallpaper::Mode) {
    if let Err(e) = wallpaper::set_mode(mode) {
        eprintln!("[StashPaper] Couldn't set the fit mode: {}", e);
    }
}

/// How many unusable images a rotation skips before giving up.
const MAX_SKIPS: usize = 5;

/// Pick and download `wanted` images, skipping any a desktop can't show.
/// `count_hint` carries the library size between rotations, so a rotation
/// normally costs one GraphQL request per image instead of two.
async fn download_batch(
    client: &reqwest::Client,
    settings: &Settings,
    rotation_state: &mut RotationState,
    count_hint: &mut Option<usize>,
    wanted: usize,
    cache_dir: &Path,
) -> Result<Vec<PathBuf>, AppError> {
    let mut count = match *count_hint {
        Some(count) => count,
        None => stash::query_image_count(client, settings).await?,
    };
    let mut paths = Vec::new();
    let mut skipped = Vec::new();

    loop {
        if count == 0 {
            *count_hint = None;
            return Err(AppError::Stash("No images found".into()));
        }
        *count_hint = Some(count);
        if paths.len() >= wanted.min(count) {
            return Ok(paths);
        }

        let pick = rotation_state
            .select_next(settings.rotation_mode, count)
            .ok_or_else(|| AppError::Stash("No images found".into()))?;
        let (fresh_count, image) =
            stash::fetch_image_at_page(client, settings, pick.page, pick.random_seed).await?;
        let outcome = match image.and_then(|image| image.paths.image) {
            Some(url) => stash::download_image(client, &url, cache_dir, paths.len()).await?,
            // The library shrank since the count, or the image has no file
            None => stash::Download::Unusable(format!("nothing at page {}", pick.page)),
        };
        count = fresh_count;
        if count == 0 {
            *count_hint = None;
            return Err(AppError::Stash("No images found".into()));
        }

        match outcome {
            stash::Download::Saved(path) => paths.push(path),
            stash::Download::Unusable(reason) => {
                eprintln!("[StashPaper] Skipping image: {}", reason);
                skipped.push(reason);
                if skipped.len() >= MAX_SKIPS.min(count.max(1)) {
                    return Err(AppError::Stash(format!(
                        "No usable image after {} tries (last: {})",
                        skipped.len(),
                        skipped[skipped.len() - 1]
                    )));
                }
            }
        }
    }
}

fn path_str(path: &Path) -> Result<&str, AppError> {
    path.to_str()
        .ok_or_else(|| AppError::Wallpaper("Invalid file path".into()))
}

async fn rotate(
    settings: &Arc<RwLock<Settings>>,
    rotation_state: &mut RotationState,
    count_hint: &mut Option<usize>,
    app_handle: &tauri::AppHandle,
) -> Result<(), AppError> {
    let s = settings.read().await.clone();
    let client = stash::client_for(&s)?;

    let cache_dir = app_handle
        .path()
        .app_cache_dir()
        .map_err(|e: tauri::Error| AppError::Settings(e.to_string()))?;

    let monitors = get_monitor_geometries(app_handle);
    let per_monitor = s.per_monitor && monitors.len() > 1;
    let wanted = if per_monitor { monitors.len() } else { 1 };

    let images =
        download_batch(&client, &s, rotation_state, count_hint, wanted, &cache_dir).await?;

    let wallpaper_path = if per_monitor {
        // Composite and set as spanned wallpaper
        let composite_path = cache_dir.join(format!(
            "wallpaper_composite_{}.jpg",
            stash::timestamp_millis()
        ));
        let geoms: Vec<crate::compositor::MonitorGeometry> = monitors
            .iter()
            .map(|m| crate::compositor::MonitorGeometry {
                x: m.x,
                y: m.y,
                width: m.width,
                height: m.height,
            })
            .collect();
        crate::compositor::composite_wallpaper(&images, &geoms, &composite_path)?;
        set_wallpaper_span(path_str(&composite_path)?)?;
        composite_path
    } else {
        set_wallpaper(path_str(&images[0])?, &s)?;
        images[0].clone()
    };

    // Only now that the desktop points at the new file is it safe to delete the
    // old ones: deleting first leaves the desktop pointing at nothing if this
    // rotation fails.
    stash::clean_wallpaper_cache(&cache_dir, &wallpaper_path);

    Ok(())
}

fn set_wallpaper(path: &str, settings: &Settings) -> Result<(), AppError> {
    // Set wallpaper via the wallpaper crate (handles most DEs)
    wallpaper::set_from_path(path).map_err(|e| AppError::Wallpaper(e.to_string()))?;

    // GNOME fixes: set picture-uri-dark for dark mode, and picture-options
    // to match fit_mode (important if switching back from per-monitor/spanned)
    #[cfg(target_os = "linux")]
    {
        let uri = format!("file://{}", path);
        let _ = std::process::Command::new("gsettings")
            .args([
                "set",
                "org.gnome.desktop.background",
                "picture-uri-dark",
                &uri,
            ])
            .output();

        let gnome_option = match settings.fit_mode {
            crate::settings::FitMode::Center => "centered",
            crate::settings::FitMode::Crop => "zoom",
            crate::settings::FitMode::Fit => "scaled",
            crate::settings::FitMode::Span => "spanned",
            crate::settings::FitMode::Stretch => "stretched",
            crate::settings::FitMode::Tile => "wallpaper",
        };
        let _ = std::process::Command::new("gsettings")
            .args([
                "set",
                "org.gnome.desktop.background",
                "picture-options",
                gnome_option,
            ])
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
    set_mode_or_log(mode);

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn png_bytes() -> Vec<u8> {
        let img = image::RgbImage::from_pixel(2, 2, image::Rgb([10, 20, 30]));
        let mut out = std::io::Cursor::new(Vec::new());
        img.write_to(&mut out, image::ImageFormat::Png).unwrap();
        out.into_inner()
    }

    /// A findImages response whose one image is served from `at` on `server`.
    fn page(server: &MockServer, count: usize, at: &str) -> ResponseTemplate {
        ResponseTemplate::new(200).set_body_json(json!({
            "data": {"findImages": {"count": count, "images": [
                {"id": "1", "paths": {"image": format!("{}{}", server.uri(), at)}}
            ]}}
        }))
    }

    async fn stash_with_files() -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/good"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(png_bytes(), "image/png"))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/clip"))
            .respond_with(
                ResponseTemplate::new(200).set_body_raw(b"....ftypmp42".to_vec(), "video/mp4"),
            )
            .mount(&server)
            .await;
        server
    }

    fn settings_for(server: &MockServer) -> Settings {
        Settings {
            stash_url: server.uri(),
            api_key: "key".into(),
            rotation_mode: crate::settings::RotationMode::Sequential,
            ..Settings::default()
        }
    }

    #[tokio::test]
    async fn skips_an_unusable_image_and_keeps_going() {
        let server = stash_with_files().await;
        // The count query and the first page point at a video clip; later pages
        // point at a real image.
        Mock::given(method("POST"))
            .and(path("/graphql"))
            .respond_with(page(&server, 3, "/clip"))
            .up_to_n_times(2)
            .with_priority(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/graphql"))
            .respond_with(page(&server, 3, "/good"))
            .mount(&server)
            .await;

        let settings = settings_for(&server);
        let client = stash::client_for(&settings).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let mut rotation = RotationState::new();
        let mut count_hint = None;

        let paths = download_batch(
            &client,
            &settings,
            &mut rotation,
            &mut count_hint,
            1,
            dir.path(),
        )
        .await
        .unwrap();
        assert_eq!(paths.len(), 1);
        assert_eq!(std::fs::read(&paths[0]).unwrap(), png_bytes());
        assert_eq!(count_hint, Some(3));
    }

    #[tokio::test]
    async fn gives_up_when_nothing_is_usable() {
        let server = stash_with_files().await;
        Mock::given(method("POST"))
            .and(path("/graphql"))
            .respond_with(page(&server, 2, "/clip"))
            .mount(&server)
            .await;

        let settings = settings_for(&server);
        let client = stash::client_for(&settings).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let err = download_batch(
            &client,
            &settings,
            &mut RotationState::new(),
            &mut None,
            1,
            dir.path(),
        )
        .await
        .unwrap_err()
        .to_string();
        // two images in the library, so two tries
        assert!(err.contains("No usable image after 2 tries"), "{err}");
        assert!(err.contains("video/mp4"), "{err}");
    }

    #[tokio::test]
    async fn a_known_count_costs_one_request_per_image() {
        let server = stash_with_files().await;
        Mock::given(method("POST"))
            .and(path("/graphql"))
            .respond_with(page(&server, 10, "/good"))
            .expect(2)
            .mount(&server)
            .await;

        let settings = settings_for(&server);
        let client = stash::client_for(&settings).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let mut count_hint = Some(10);
        let paths = download_batch(
            &client,
            &settings,
            &mut RotationState::new(),
            &mut count_hint,
            2,
            dir.path(),
        )
        .await
        .unwrap();
        assert_eq!(paths.len(), 2);
        assert_ne!(
            paths[0], paths[1],
            "each image in a batch gets its own file"
        );
    }

    #[tokio::test]
    async fn an_empty_library_is_an_error_and_forgets_the_count() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/graphql"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": {"findImages": {"count": 0, "images": []}}
            })))
            .mount(&server)
            .await;

        let settings = settings_for(&server);
        let client = stash::client_for(&settings).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let mut count_hint = Some(5);
        let err = download_batch(
            &client,
            &settings,
            &mut RotationState::new(),
            &mut count_hint,
            1,
            dir.path(),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("No images found"));
        assert_eq!(count_hint, None);
    }
}
