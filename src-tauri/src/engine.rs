use crate::error::AppError;
use crate::rotation::RotationState;
use crate::settings::Settings;
use crate::stash;
use std::collections::HashSet;
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

/// Delete files a failed rotation downloaded. They never reached the desktop, and
/// nothing else would clean them up while rotations keep failing.
fn discard(paths: &[PathBuf]) {
    for path in paths {
        let _ = std::fs::remove_file(path);
    }
}

fn no_images() -> AppError {
    AppError::Stash("no images match the query filter and minimum resolution".into())
}

fn no_usable_image(skipped: &[String]) -> AppError {
    AppError::Stash(format!(
        "no usable image in {} tries, last problem: {}. Check the query filter",
        skipped.len(),
        skipped.last().map(String::as_str).unwrap_or("none")
    ))
}

/// Pick and download `wanted` images, skipping any a desktop can't show.
/// `count_hint` carries the library size between rotations, so a rotation
/// normally costs one GraphQL request per image instead of two. A page is tried
/// at most once per batch, so a skip can't put one image on two monitors.
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
    let mut tried = HashSet::new();

    loop {
        if count == 0 {
            *count_hint = None;
            return Err(no_images());
        }
        *count_hint = Some(count);
        if paths.len() >= wanted.min(count) {
            return Ok(paths);
        }

        let pick = rotation_state
            .select_next(settings.rotation_mode, count)
            .ok_or_else(no_images)?;
        if !tried.insert(pick.page) {
            // Every page has had its turn in this batch
            return if paths.is_empty() {
                Err(no_usable_image(&skipped))
            } else {
                Ok(paths)
            };
        }
        let (fresh_count, image) =
            match stash::fetch_image_at_page(client, settings, pick.page, pick.random_seed).await {
                Ok(result) => result,
                Err(e) => {
                    discard(&paths);
                    return Err(e);
                }
            };
        count = fresh_count;
        *count_hint = Some(count);
        if count == 0 {
            *count_hint = None;
            discard(&paths);
            return Err(no_images());
        }
        let outcome = match image.and_then(|image| image.paths.image) {
            Some(url) => match stash::download_image(client, &url, cache_dir, paths.len()).await {
                Ok(outcome) => outcome,
                Err(e) => {
                    discard(&paths);
                    return Err(e);
                }
            },
            // The page is past the end of a library that shrank since the last
            // count. Not the image's fault, so it doesn't use up a skip; the
            // next pick uses the fresh count.
            None if pick.page > count => continue,
            None => stash::Download::Unusable("the image has no file".into()),
        };

        match outcome {
            stash::Download::Saved(path) => paths.push(path),
            stash::Download::Unusable(reason) => {
                eprintln!("[StashPaper] Skipping image: {}", reason);
                skipped.push(reason);
                if skipped.len() >= MAX_SKIPS.min(count) {
                    discard(&paths);
                    return Err(no_usable_image(&skipped));
                }
            }
        }
    }
}

fn path_str(path: &Path) -> Result<&str, AppError> {
    path.to_str()
        .ok_or_else(|| AppError::Wallpaper("Invalid file path".into()))
}

/// Put `images` on the desktop through `set` (compositing them first for a
/// multi-monitor layout), and only then delete older cache files. Deleting first
/// would leave the desktop pointing at a missing file whenever a rotation fails.
/// `set` gets the file and whether it's a spanned composite.
fn apply_wallpaper(
    images: &[PathBuf],
    monitors: Option<&[crate::compositor::MonitorGeometry]>,
    cache_dir: &Path,
    set: impl FnOnce(&str, bool) -> Result<(), AppError>,
) -> Result<PathBuf, AppError> {
    let wallpaper_path = match monitors {
        Some(_) => cache_dir.join(format!(
            "wallpaper_composite_{}.jpg",
            stash::timestamp_millis()
        )),
        None => images[0].clone(),
    };
    let result = (|| {
        if let Some(geoms) = monitors {
            crate::compositor::composite_wallpaper(images, geoms, &wallpaper_path)?;
        }
        set(path_str(&wallpaper_path)?, monitors.is_some())
    })();
    match result {
        Ok(()) => {
            stash::clean_wallpaper_cache(cache_dir, &wallpaper_path);
            Ok(wallpaper_path)
        }
        Err(e) => {
            // The desktop still shows the old file; drop only this batch's
            discard(images);
            discard(&[wallpaper_path]);
            Err(e)
        }
    }
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

    let geoms: Vec<crate::compositor::MonitorGeometry> = monitors
        .iter()
        .map(|m| crate::compositor::MonitorGeometry {
            x: m.x,
            y: m.y,
            width: m.width,
            height: m.height,
        })
        .collect();
    apply_wallpaper(
        &images,
        per_monitor.then_some(geoms.as_slice()),
        &cache_dir,
        |path, spanned| {
            if spanned {
                set_wallpaper_span(path)
            } else {
                set_wallpaper(path, &s)
            }
        },
    )?;

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
    use wiremock::matchers::{body_partial_json, method, path};
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

    /// Matches the count query (per_page 0)
    fn count_query() -> impl wiremock::Match {
        body_partial_json(json!({"variables": {"filter": {"per_page": 0}}}))
    }

    /// Matches the request for one page
    fn page_query(n: usize) -> impl wiremock::Match {
        body_partial_json(json!({"variables": {"filter": {"per_page": 1, "page": n}}}))
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

    async fn serve_pages(server: &MockServer, count: usize, files: &[&str]) {
        Mock::given(method("POST"))
            .and(path("/graphql"))
            .and(count_query())
            .respond_with(page(server, count, files[0]))
            .mount(server)
            .await;
        for (i, at) in files.iter().enumerate() {
            Mock::given(method("POST"))
                .and(path("/graphql"))
                .and(page_query(i + 1))
                .respond_with(page(server, count, at))
                .expect(1)
                .mount(server)
                .await;
        }
    }

    async fn run_batch(
        server: &MockServer,
        count_hint: &mut Option<usize>,
        wanted: usize,
    ) -> (Result<Vec<PathBuf>, AppError>, tempfile::TempDir) {
        let settings = settings_for(server);
        let client = stash::client_for(&settings).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let result = download_batch(
            &client,
            &settings,
            &mut RotationState::new(),
            count_hint,
            wanted,
            dir.path(),
        )
        .await;
        (result, dir)
    }

    #[tokio::test]
    async fn skips_an_unusable_image_and_moves_to_the_next_page() {
        let server = stash_with_files().await;
        // page 1 is a video clip, page 2 a real image
        serve_pages(&server, 3, &["/clip", "/good"]).await;

        let mut count_hint = None;
        let (result, _dir) = run_batch(&server, &mut count_hint, 1).await;
        let paths = result.unwrap();
        assert_eq!(paths.len(), 1);
        assert_eq!(std::fs::read(&paths[0]).unwrap(), png_bytes());
        assert_eq!(count_hint, Some(3));
    }

    #[tokio::test]
    async fn gives_up_after_trying_every_image() {
        let server = stash_with_files().await;
        serve_pages(&server, 2, &["/clip", "/clip"]).await;

        let (result, _dir) = run_batch(&server, &mut None, 1).await;
        let err = result.unwrap_err().to_string();
        assert!(err.contains("no usable image in 2 tries"), "{err}");
        assert!(err.contains("video/mp4, not an image"), "{err}");
    }

    #[tokio::test]
    async fn gives_up_after_five_skips_in_a_big_library() {
        let server = stash_with_files().await;
        Mock::given(method("POST"))
            .and(path("/graphql"))
            .respond_with(page(&server, 50, "/clip"))
            // the count query plus five pages, and no more
            .expect(6)
            .mount(&server)
            .await;

        let (result, _dir) = run_batch(&server, &mut None, 1).await;
        let err = result.unwrap_err().to_string();
        assert!(err.contains("no usable image in 5 tries"), "{err}");
    }

    #[tokio::test]
    async fn a_known_count_costs_one_request_per_image() {
        let server = stash_with_files().await;
        Mock::given(method("POST"))
            .and(path("/graphql"))
            .and(count_query())
            .respond_with(page(&server, 10, "/good"))
            .expect(0)
            .mount(&server)
            .await;
        for n in [1, 2] {
            Mock::given(method("POST"))
                .and(path("/graphql"))
                .and(page_query(n))
                .respond_with(page(&server, 10, "/good"))
                .expect(1)
                .mount(&server)
                .await;
        }

        let (result, _dir) = run_batch(&server, &mut Some(10), 2).await;
        let names: Vec<String> = result
            .unwrap()
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert!(names[0].contains("_0."), "{names:?}");
        assert!(names[1].contains("_1."), "{names:?}");
    }

    #[tokio::test]
    async fn a_skip_does_not_put_one_image_on_two_monitors() {
        let server = stash_with_files().await;
        // three monitors, three images, one of them unusable: sequential
        // would wrap back to page 1, which this batch already used
        serve_pages(&server, 3, &["/good", "/good", "/clip"]).await;

        let (result, _dir) = run_batch(&server, &mut None, 3).await;
        // two distinct images; the compositor reuses the last for monitor 3
        assert_eq!(result.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn a_library_that_shrank_is_noticed_in_the_same_rotation() {
        let server = stash_with_files().await;
        let empty = ResponseTemplate::new(200).set_body_json(json!({
            "data": {"findImages": {"count": 1, "images": []}}
        }));
        // the hint says 10, but the filter now matches 1 image
        Mock::given(method("POST"))
            .and(path("/graphql"))
            .and(page_query(4))
            .respond_with(empty)
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/graphql"))
            .and(page_query(1))
            .respond_with(page(&server, 1, "/good"))
            .expect(1)
            .mount(&server)
            .await;

        let settings = settings_for(&server);
        let client = stash::client_for(&settings).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let mut rotation = RotationState::new();
        for _ in 0..3 {
            rotation.select_next(settings.rotation_mode, 10);
        }
        let mut count_hint = Some(10);
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
        assert_eq!(count_hint, Some(1));
    }

    #[tokio::test]
    async fn a_failed_batch_leaves_no_files_behind() {
        let server = stash_with_files().await;
        Mock::given(method("POST"))
            .and(path("/graphql"))
            .and(page_query(1))
            .respond_with(page(&server, 50, "/good"))
            .with_priority(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/graphql"))
            .respond_with(page(&server, 50, "/clip"))
            .mount(&server)
            .await;

        // page 1 downloads, then five clips end the batch
        let (result, dir) = run_batch(&server, &mut Some(50), 2).await;
        assert!(result.is_err());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
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

        let mut count_hint = Some(5);
        let (result, _dir) = run_batch(&server, &mut count_hint, 1).await;
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("no images match the query filter"));
        assert_eq!(count_hint, None);
    }

    fn cache_with_old_wallpaper() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let old = dir.path().join("wallpaper_1_0.png");
        let new = dir.path().join("wallpaper_2_0.png");
        std::fs::write(&old, png_bytes()).unwrap();
        std::fs::write(&new, png_bytes()).unwrap();
        (dir, old, new)
    }

    #[test]
    fn a_failed_set_keeps_the_current_wallpaper_file() {
        let (dir, old, new) = cache_with_old_wallpaper();
        let result = apply_wallpaper(std::slice::from_ref(&new), None, dir.path(), |_, _| {
            Err(AppError::Wallpaper("desktop said no".into()))
        });
        assert!(result.is_err());
        assert!(old.exists(), "the file on the desktop must survive");
        assert!(
            !new.exists(),
            "a batch that never reached the desktop is dropped"
        );
    }

    #[test]
    fn a_successful_set_cleans_up_everything_else() {
        let (dir, old, new) = cache_with_old_wallpaper();
        let mut set_to = None;
        let path = apply_wallpaper(
            std::slice::from_ref(&new),
            None,
            dir.path(),
            |p, spanned| {
                set_to = Some((p.to_string(), spanned));
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(path, new);
        assert_eq!(set_to, Some((new.to_str().unwrap().to_string(), false)));
        assert!(!old.exists());
        assert!(new.exists());
    }

    #[test]
    fn per_monitor_sets_the_composite_and_keeps_only_it() {
        let (dir, old, new) = cache_with_old_wallpaper();
        let monitors = [
            crate::compositor::MonitorGeometry {
                x: 0,
                y: 0,
                width: 4,
                height: 4,
            },
            crate::compositor::MonitorGeometry {
                x: 4,
                y: 0,
                width: 4,
                height: 4,
            },
        ];
        let path = apply_wallpaper(
            &[old.clone(), new.clone()],
            Some(&monitors),
            dir.path(),
            |_, spanned| {
                assert!(spanned);
                Ok(())
            },
        )
        .unwrap();
        assert!(path
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("wallpaper_composite_"));
        let left: Vec<PathBuf> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();
        assert_eq!(left, vec![path]);
    }
}
