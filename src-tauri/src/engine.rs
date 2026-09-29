use crate::error::AppError;
use crate::rotation::RotationState;
use crate::schedule;
use crate::settings::Settings;
use crate::stash;
use crate::tray::TrayStatus;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tauri::Manager;
use tokio::sync::{mpsc, RwLock};

#[derive(Debug, PartialEq)]
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

/// How often the loop rechecks the wall clock while waiting. Sleeps are capped
/// at this so time spent suspended counts toward the interval.
const POLL: Duration = Duration::from_secs(60);

/// What the engine loop does next.
#[derive(Debug, PartialEq)]
enum Step {
    Rotate,
    Wait(Duration),
}

/// Everything the engine tracks between rotations.
struct Engine {
    rotation: RotationState,
    count_hint: Option<usize>,
    last_success: Option<SystemTime>,
    last_failure: Option<SystemTime>,
    failures: u32,
    status: TrayStatus,
    state_path: Option<PathBuf>,
    /// `schedule::selection_key` of the settings the rotation position belongs to
    selection_key: String,
    /// The files the last successful rotation put on the desktop
    current: Vec<PathBuf>,
    /// Shared with the blocking thread that sets wallpapers
    desktop: Arc<std::sync::Mutex<crate::desktop::Desktop>>,
    /// The images on the desktop, for "Open in Stash"
    shown: Vec<schedule::ShownImage>,
    /// The local date the tray was last drawn on, so "Changed at 14:05" can
    /// become "yesterday at 14:05" after midnight
    refreshed_on: Option<chrono::NaiveDate>,
    /// The status changed outside a rotation (e.g. a clock correction)
    needs_refresh: bool,
}

/// "Open in Stash" entries for the images on the desktop.
fn image_url(stash_url: &str, id: &str) -> String {
    format!("{}/images/{}", stash_url.trim_end_matches('/'), id)
}

/// "Open in Stash" entries for the images on the desktop. State saved before
/// images kept their URL falls back to the current server.
fn open_items(stash_url: &str, shown: &[schedule::ShownImage]) -> Vec<crate::tray::OpenItem> {
    shown
        .iter()
        .map(|image| crate::tray::OpenItem {
            label: image.label.clone(),
            url: if image.url.is_empty() {
                image_url(stash_url, &image.id)
            } else {
                image.url.clone()
            },
        })
        .collect()
}

impl Engine {
    fn restore(app: &tauri::AppHandle, settings: &Settings) -> Self {
        let state_path = match app.path().app_data_dir() {
            Ok(dir) => Some(schedule::state_path(&dir)),
            Err(e) => {
                log::warn!("No app data dir, so rotation state won't be saved: {}", e);
                None
            }
        };
        let saved = state_path
            .as_deref()
            .map(schedule::load)
            .unwrap_or_default();
        let state_dir = state_path
            .as_deref()
            .and_then(Path::parent)
            .map(Path::to_path_buf);
        let mut engine = Self::from_saved(saved, settings, state_path);
        engine.desktop = Arc::new(std::sync::Mutex::new(crate::desktop::Desktop::detect(
            state_dir.as_deref(),
        )));
        engine
    }

    fn from_saved(
        saved: schedule::SavedState,
        settings: &Settings,
        state_path: Option<PathBuf>,
    ) -> Self {
        // A saved position only means something for the settings it came from
        let selection_key = schedule::selection_key(settings);
        let rotation = if saved.selection_key == selection_key {
            RotationState::from_snapshot(saved.rotation.clone())
        } else {
            RotationState::new()
        };
        let last_success = saved.last_rotated_time();

        Self {
            rotation,
            count_hint: None,
            last_success,
            last_failure: None,
            failures: 0,
            status: TrayStatus {
                configured: crate::settings::is_configured(settings),
                paused: saved.paused,
                last_changed: last_success,
                open: open_items(&settings.stash_url, &saved.shown),
                ..TrayStatus::default()
            },
            state_path,
            selection_key,
            current: saved.on_desktop(),
            desktop: Arc::new(std::sync::Mutex::new(crate::desktop::Desktop::detect(None))),
            shown: saved.shown,
            refreshed_on: None,
            needs_refresh: false,
        }
    }

    /// Decide what the loop does now. Waits never exceed `POLL`, so the next
    /// decision rechecks the wall clock (suspended time counts).
    fn next_step(&mut self, now: SystemTime, interval: Duration) -> Step {
        // A clock set backwards leaves these in the future, which would push
        // the next rotation out by however far the clock moved
        if self.last_success.is_some_and(|t| t > now) {
            self.last_success = Some(now);
            self.status.last_changed = Some(now);
            self.needs_refresh = true;
        }
        if self.last_failure.is_some_and(|t| t > now) {
            self.last_failure = Some(now);
            self.status.retry_at = Some(now + schedule::retry_delay(self.failures, interval));
            self.needs_refresh = true;
        }
        if !self.status.configured || self.status.paused {
            return Step::Wait(POLL);
        }
        let wait = schedule::time_until_due(
            now,
            self.last_success,
            self.last_failure,
            self.failures,
            interval,
        );
        if wait.is_zero() {
            Step::Rotate
        } else {
            Step::Wait(wait.min(POLL))
        }
    }

    /// Update the engine after a rotation attempt that started at `now`.
    fn record(
        &mut self,
        now: SystemTime,
        result: Result<(Vec<PathBuf>, Vec<schedule::ShownImage>), AppError>,
        settings: &Settings,
    ) {
        match result {
            Ok((wallpaper_path, shown)) => {
                self.current = wallpaper_path;
                self.failures = 0;
                self.last_failure = None;
                self.last_success = Some(now);
                self.status.error = None;
                self.status.retry_at = None;
                self.status.last_changed = Some(now);
                self.status.open = open_items(&settings.stash_url, &shown);
                self.shown = shown;
            }
            Err(e) => {
                self.failures += 1;
                self.last_failure = Some(now);
                self.status.error = Some(e.to_string());
                self.status.retry_at = Some(
                    now + schedule::retry_delay(self.failures, settings.interval.to_duration()),
                );
            }
        }
    }

    /// After a settings save: forget the rotation position if the settings that
    /// pick images changed, and give a failing rotation a fresh start either way.
    fn settings_changed(&mut self, settings: &Settings) {
        let key = schedule::selection_key(settings);
        if key != self.selection_key {
            self.rotation.reset();
            self.count_hint = None;
            self.selection_key = key;
        }
        self.failures = 0;
        self.last_failure = None;
        self.status.configured = crate::settings::is_configured(settings);
        if !self.status.configured {
            // Nothing will rotate until it is set up again, so an old error
            // would only linger
            self.status.error = None;
            self.status.retry_at = None;
        }
    }

    fn refresh_tray(&mut self, app: &tauri::AppHandle) {
        crate::tray::refresh(app, &self.status);
        self.refreshed_on = Some(chrono::Local::now().date_naive());
        self.needs_refresh = false;
    }

    fn set_paused(&mut self, paused: bool, app: &tauri::AppHandle) {
        self.status.paused = paused;
        self.save();
        self.refresh_tray(app);
    }

    async fn rotate_and_record(
        &mut self,
        settings: &Arc<RwLock<Settings>>,
        app: &tauri::AppHandle,
    ) {
        let s = settings.read().await.clone();
        let result = rotate(
            &s,
            &mut self.rotation,
            &mut self.count_hint,
            &self.current,
            &self.desktop,
            app,
        )
        .await;
        match &result {
            Ok(_) => log::info!("Wallpaper changed"),
            Err(e) => log::error!("Rotation failed: {}", e),
        }
        let succeeded = result.is_ok();
        // Timed from when the attempt ended: a request that hung for 30s
        // shouldn't use up the 30s retry delay
        self.record(SystemTime::now(), result, &s);
        if succeeded {
            self.save();
        }
        self.refresh_tray(app);
    }

    fn save(&self) {
        let Some(path) = &self.state_path else {
            return;
        };
        let state = schedule::SavedState {
            last_rotated: self.last_success.map(schedule::unix_secs),
            selection_key: self.selection_key.clone(),
            rotation: self.rotation.snapshot(),
            current_files: self.current.clone(),
            current_wallpaper: None,
            shown: self.shown.clone(),
            paused: self.status.paused,
        };
        if let Err(e) = schedule::save(path, &state) {
            log::warn!("Couldn't save the rotation state: {}", e);
        }
    }
}

pub async fn run(mut rx: CommandRx, settings: Arc<RwLock<Settings>>, app: tauri::AppHandle) {
    let mut engine = Engine::restore(&app, &*settings.read().await);
    engine.refresh_tray(&app);

    loop {
        let interval = settings.read().await.interval.to_duration();
        let step = engine.next_step(SystemTime::now(), interval);
        if engine.needs_refresh || engine.refreshed_on != Some(chrono::Local::now().date_naive()) {
            engine.refresh_tray(&app);
        }
        let wait = match step {
            Step::Rotate => {
                engine.rotate_and_record(&settings, &app).await;
                continue;
            }
            Step::Wait(wait) => wait,
        };

        tokio::select! {
            cmd = rx.recv() => {
                match cmd {
                    Some(Command::Next) => {
                        if engine.status.configured {
                            engine.rotate_and_record(&settings, &app).await;
                        }
                    }
                    Some(Command::Pause) => engine.set_paused(true, &app),
                    Some(Command::Resume) => engine.set_paused(false, &app),
                    Some(Command::SettingsUpdated) => {
                        engine.settings_changed(&*settings.read().await);
                        // Show the new settings at work right away
                        if engine.status.configured {
                            engine.rotate_and_record(&settings, &app).await;
                        } else {
                            engine.refresh_tray(&app);
                        }
                    }
                    Some(Command::Quit) | None => break,
                }
            }
            // Wake up to recheck the wall clock; next_step decides
            _ = tokio::time::sleep(wait) => {}
        }
    }
    log::info!("Rotation engine stopped");
}

/// How many unusable images a rotation skips before giving up.
const MAX_SKIPS: usize = 5;

/// Delete files a failed rotation downloaded. They never reached the desktop, and
/// nothing else would clean them up while rotations keep failing.
fn discard(paths: &[impl AsRef<Path>]) {
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

/// One downloaded image and its Stash id.
#[derive(Debug)]
struct Picked {
    path: PathBuf,
    id: String,
}

impl AsRef<Path> for Picked {
    fn as_ref(&self) -> &Path {
        &self.path
    }
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
) -> Result<Vec<Picked>, AppError> {
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
            if tried.len() < count {
                // e.g. a new shuffle that starts with a page this batch already
                // used; other pages are still untried
                continue;
            }
            // Every page has had its turn in this batch
            return if paths.is_empty() {
                Err(no_usable_image(&skipped))
            } else {
                Ok(paths)
            };
        }
        let (fresh_count, image) = match stash::fetch_image_at_page(
            client,
            settings,
            pick.page,
            pick.random_seed,
            Some(pick.sort_seed),
        )
        .await
        {
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
        let (id, url) = match image {
            Some(image) => (image.id, image.paths.image),
            None => (String::new(), None),
        };
        let outcome = match url {
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
            stash::Download::Saved(path) => paths.push(Picked { path, id }),
            stash::Download::Unusable(reason) => {
                log::warn!("Skipping image {}: {}", id, reason);
                skipped.push(reason);
                if skipped.len() >= MAX_SKIPS.min(count) {
                    discard(&paths);
                    return Err(no_usable_image(&skipped));
                }
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum PlanKind {
    Single,
    Composite,
    PerMonitor,
}

/// How many images a rotation fetches and how they go on the desktop, from the
/// per-monitor setting, the monitor count and what the desktop can do.
/// Per-monitor only means something with several monitors and a desktop that
/// can show it; otherwise every monitor gets the same image.
fn plan_for(
    per_monitor_setting: bool,
    monitors: usize,
    support: crate::desktop::PerMonitor,
) -> (usize, PlanKind) {
    use crate::desktop::PerMonitor;
    if !per_monitor_setting || monitors < 2 {
        return (1, PlanKind::Single);
    }
    match support {
        PerMonitor::Native => (monitors, PlanKind::PerMonitor),
        PerMonitor::Spanned => (monitors, PlanKind::Composite),
        PerMonitor::Unsupported => (1, PlanKind::Single),
    }
}

/// How a rotation's images go on the desktop.
#[derive(Debug, Clone, Copy)]
enum Plan<'a> {
    /// The first image on every monitor
    Single,
    /// Composited onto one canvas spanning the monitors
    Composite(&'a [crate::compositor::MonitorGeometry]),
    /// Each image on its own monitor; the last repeats if there are fewer
    PerMonitor(&'a [crate::MonitorInfo]),
}

/// Put `images` on the desktop through `set`, and only then delete older cache
/// files. Deleting first would leave the desktop pointing at a missing file
/// whenever a rotation fails. `current` is what the last successful rotation
/// put on the desktop. Returns the files now on the desktop.
fn apply_wallpaper(
    images: &[PathBuf],
    plan: Plan,
    cache_dir: &Path,
    current: &[PathBuf],
    set: impl FnOnce(crate::desktop::Placement) -> Result<(), AppError>,
) -> Result<Vec<PathBuf>, AppError> {
    use crate::desktop::Placement;
    let (on_desktop, result) = match plan {
        Plan::Single => (vec![images[0].clone()], set(Placement::Single(&images[0]))),
        Plan::Composite(geoms) => {
            let composite = cache_dir.join(format!(
                "wallpaper_composite_{}.jpg",
                stash::timestamp_millis()
            ));
            if let Err(e) = crate::compositor::composite_wallpaper(images, geoms, &composite) {
                // Nothing reached the desktop
                discard(images);
                discard(&[composite]);
                return Err(e);
            }
            let result = set(Placement::Spanned(&composite));
            (vec![composite], result)
        }
        Plan::PerMonitor(monitors) => {
            let pairs: Vec<(crate::MonitorInfo, PathBuf)> = monitors
                .iter()
                .enumerate()
                .map(|(i, m)| (m.clone(), images[i.min(images.len() - 1)].clone()))
                .collect();
            (images.to_vec(), set(Placement::PerMonitor(&pairs)))
        }
    };
    let on_desktop_refs: Vec<&Path> = on_desktop.iter().map(PathBuf::as_path).collect();
    match result {
        Ok(()) => {
            stash::clean_wallpaper_cache(cache_dir, &on_desktop_refs);
            Ok(on_desktop)
        }
        Err(e) => {
            // A setter can fail after applying the file to some monitors or
            // workspaces, so keep it along with the last good ones. Everything
            // else goes, so repeated failures can't fill the cache. With no
            // known good file (first rotation since start), delete nothing
            // older: the desktop may still point at one of them.
            if current.is_empty() {
                discard(
                    &images
                        .iter()
                        .filter(|p| !on_desktop.contains(p))
                        .cloned()
                        .collect::<Vec<_>>(),
                );
            } else {
                let mut keep = on_desktop_refs;
                keep.extend(current.iter().map(PathBuf::as_path));
                stash::clean_wallpaper_cache(cache_dir, &keep);
            }
            Err(e)
        }
    }
}

/// Run one rotation. Returns the files now on the desktop and the images on it.
async fn rotate(
    s: &Settings,
    rotation_state: &mut RotationState,
    count_hint: &mut Option<usize>,
    current: &[PathBuf],
    desktop: &Arc<std::sync::Mutex<crate::desktop::Desktop>>,
    app_handle: &tauri::AppHandle,
) -> Result<(Vec<PathBuf>, Vec<schedule::ShownImage>), AppError> {
    let client = stash::client_for(s)?;

    let cache_dir = app_handle
        .path()
        .app_cache_dir()
        .map_err(|e: tauri::Error| AppError::Settings(e.to_string()))?;

    let monitors = crate::monitor_infos(app_handle);
    let support = desktop
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .per_monitor();
    let (wanted, kind) = plan_for(s.per_monitor, monitors.len(), support);
    let per_monitor = kind != PlanKind::Single;

    let picked = download_batch(&client, s, rotation_state, count_hint, wanted, &cache_dir).await?;
    let shown: Vec<schedule::ShownImage> = picked
        .iter()
        .enumerate()
        .map(|(index, p)| schedule::ShownImage {
            id: p.id.clone(),
            url: image_url(&s.stash_url, &p.id),
            label: match monitors.get(index) {
                Some(m) if per_monitor => {
                    format!("Monitor {} ({}x{})", index + 1, m.width, m.height)
                }
                _ => String::new(),
            },
        })
        .collect();
    let images: Vec<PathBuf> = picked.into_iter().map(|p| p.path).collect();

    let geoms: Vec<crate::compositor::MonitorGeometry> = monitors
        .iter()
        .map(|m| crate::compositor::MonitorGeometry {
            x: m.x,
            y: m.y,
            width: m.width,
            height: m.height,
        })
        .collect();

    // Compositing, the wallpaper tools and file deletes all block, so keep them
    // off the async runtime
    let fit = s.fit_mode;
    let current = current.to_vec();
    let desktop = desktop.clone();
    let on_desktop = tokio::task::spawn_blocking(move || {
        let plan = match kind {
            PlanKind::PerMonitor => Plan::PerMonitor(&monitors),
            PlanKind::Composite => Plan::Composite(&geoms),
            PlanKind::Single => Plan::Single,
        };
        apply_wallpaper(&images, plan, &cache_dir, &current, |placement| {
            desktop
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .apply(placement, fit)
        })
    })
    .await
    .map_err(|e| AppError::Wallpaper(e.to_string()))??;

    Ok((on_desktop, shown))
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
        page_with_id(server, count, at, "1")
    }

    fn page_with_id(server: &MockServer, count: usize, at: &str, id: &str) -> ResponseTemplate {
        ResponseTemplate::new(200).set_body_json(json!({
            "data": {"findImages": {"count": count, "images": [
                {"id": id, "paths": {"image": format!("{}{}", server.uri(), at)}}
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
                // the image id is the page number, to check ids line up
                .respond_with(page_with_id(server, count, at, &(i + 1).to_string()))
                .expect(1)
                .mount(server)
                .await;
        }
    }

    async fn run_batch(
        server: &MockServer,
        count_hint: &mut Option<usize>,
        wanted: usize,
    ) -> (Result<Vec<Picked>, AppError>, tempfile::TempDir) {
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
        assert_eq!(std::fs::read(&paths[0].path).unwrap(), png_bytes());
        assert_eq!(paths[0].id, "2", "the image from page 2");
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
    async fn an_explicit_random_sort_gets_one_fixed_seed() {
        let server = stash_with_files().await;
        Mock::given(method("POST"))
            .and(path("/graphql"))
            .respond_with(page(&server, 10, "/good"))
            .mount(&server)
            .await;
        let settings = Settings {
            query_filter: r#"{"filter": {"sort": "random"}}"#.into(),
            ..settings_for(&server)
        };
        let client = stash::client_for(&settings).unwrap();
        let dir = tempfile::tempdir().unwrap();
        download_batch(
            &client,
            &settings,
            &mut RotationState::new(),
            &mut Some(10),
            2,
            dir.path(),
        )
        .await
        .unwrap();

        let sorts: Vec<String> = server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .filter_map(|r| serde_json::from_slice::<serde_json::Value>(&r.body).ok())
            .filter(|body| body["variables"]["filter"]["per_page"] == 1)
            .map(|body| {
                body["variables"]["filter"]["sort"]
                    .as_str()
                    .unwrap()
                    .to_string()
            })
            .collect();
        assert_eq!(sorts.len(), 2);
        assert!(sorts[0].starts_with("random_"), "{sorts:?}");
        assert_eq!(sorts[0], sorts[1], "the same order for every page");
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
            .map(|p| p.path.file_name().unwrap().to_string_lossy().into_owned())
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
        let ids: Vec<String> = result.unwrap().into_iter().map(|p| p.id).collect();
        assert_eq!(ids, ["1", "2"]);
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

    use crate::desktop::Placement;

    fn refuse(_: Placement) -> Result<(), AppError> {
        Err(AppError::Wallpaper("desktop said no".into()))
    }

    #[test]
    fn a_failed_set_keeps_the_current_wallpaper_file() {
        let (dir, old, new) = cache_with_old_wallpaper();
        let result = apply_wallpaper(
            std::slice::from_ref(&new),
            Plan::Single,
            dir.path(),
            &[],
            refuse,
        );
        assert!(result.is_err());
        // with no known good file, nothing older may go: the desktop could be on it
        assert!(old.exists(), "the file on the desktop must survive");
        // a setter can fail after applying the file to some monitors
        assert!(new.exists());
    }

    #[test]
    fn repeated_failed_sets_keep_the_cache_bounded() {
        let (dir, current, new) = cache_with_old_wallpaper();
        let stale = dir.path().join("wallpaper_0_0.png");
        std::fs::write(&stale, png_bytes()).unwrap();
        let result = apply_wallpaper(
            std::slice::from_ref(&new),
            Plan::Single,
            dir.path(),
            std::slice::from_ref(&current),
            refuse,
        );
        assert!(result.is_err());
        assert!(current.exists(), "the last good file stays");
        assert!(
            new.exists(),
            "the file the setter may have partly applied stays"
        );
        assert!(!stale.exists(), "anything else goes");
    }

    #[test]
    fn a_successful_set_cleans_up_everything_else() {
        let (dir, old, new) = cache_with_old_wallpaper();
        let mut set_to = None;
        let on_desktop = apply_wallpaper(
            std::slice::from_ref(&new),
            Plan::Single,
            dir.path(),
            &[],
            |placement| {
                set_to = Some(format!("{:?}", placement));
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(on_desktop, vec![new.clone()]);
        assert_eq!(set_to, Some(format!("{:?}", Placement::Single(&new))));
        assert!(!old.exists());
        assert!(new.exists());
    }

    #[test]
    fn a_composite_is_spanned_and_is_all_that_stays() {
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
        let on_desktop = apply_wallpaper(
            &[old.clone(), new.clone()],
            Plan::Composite(&monitors),
            dir.path(),
            &[],
            |placement| {
                assert!(matches!(placement, Placement::Spanned(_)));
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(on_desktop.len(), 1);
        assert!(on_desktop[0]
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("wallpaper_composite_"));
        let left: Vec<PathBuf> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();
        assert_eq!(left, on_desktop);
    }

    #[test]
    fn per_monitor_sets_each_image_and_keeps_them_all() {
        let (dir, old, new) = cache_with_old_wallpaper();
        let stale = dir.path().join("wallpaper_0_0.png");
        std::fs::write(&stale, png_bytes()).unwrap();
        let monitor = |x: i32| crate::MonitorInfo {
            width: 1920,
            height: 1080,
            x,
            y: 0,
            scale_factor: 1.0,
        };
        // three monitors, two images: the last one repeats
        let monitors = [monitor(0), monitor(1920), monitor(3840)];
        let mut placed = Vec::new();
        let on_desktop = apply_wallpaper(
            &[old.clone(), new.clone()],
            Plan::PerMonitor(&monitors),
            dir.path(),
            &[],
            |placement| {
                if let Placement::PerMonitor(pairs) = placement {
                    placed = pairs.iter().map(|(m, p)| (m.x, p.clone())).collect();
                }
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(
            placed,
            vec![(0, old.clone()), (1920, new.clone()), (3840, new.clone())]
        );
        assert_eq!(on_desktop, vec![old.clone(), new.clone()]);
        assert!(old.exists() && new.exists());
        assert!(!stale.exists());
        // no composite was made
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 2);
    }

    // The loop's decisions, on a fake clock

    use crate::schedule::{SavedState, ShownImage};
    use crate::settings::{Interval, RotationMode};
    use std::time::UNIX_EPOCH;

    const HOUR: Duration = Duration::from_secs(3600);

    fn t(secs: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(1_800_000_000 + secs)
    }

    fn set_up() -> Settings {
        Settings {
            stash_url: "http://stash:9999/".into(),
            api_key: "key".into(),
            interval: Interval::OneHour,
            rotation_mode: RotationMode::Sequential,
            ..Settings::default()
        }
    }

    fn fresh(settings: &Settings) -> Engine {
        Engine::from_saved(SavedState::default(), settings, None)
    }

    fn ok() -> Result<(Vec<PathBuf>, Vec<ShownImage>), AppError> {
        Ok((
            vec![PathBuf::from("/cache/wallpaper_1_0.jpg")],
            vec![ShownImage {
                id: "12".into(),
                label: String::new(),
                url: "http://stash:9999/images/12".into(),
            }],
        ))
    }

    fn failed() -> Result<(Vec<PathBuf>, Vec<ShownImage>), AppError> {
        Err(AppError::Stash("the server returned HTTP 502".into()))
    }

    /// Follow the loop's decisions from `start` until it rotates; return when.
    fn next_rotation(engine: &mut Engine, start: SystemTime) -> SystemTime {
        let mut now = start;
        for _ in 0..100_000 {
            match engine.next_step(now, HOUR) {
                Step::Rotate => return now,
                Step::Wait(wait) => {
                    assert!(!wait.is_zero() && wait <= POLL, "waited {wait:?}");
                    now += wait;
                }
            }
        }
        panic!("never rotated");
    }

    #[test]
    fn a_fresh_start_rotates_right_away() {
        assert_eq!(fresh(&set_up()).next_step(t(0), HOUR), Step::Rotate);
    }

    #[test]
    fn after_a_success_it_waits_the_interval_a_minute_at_a_time() {
        let settings = set_up();
        let mut engine = fresh(&settings);
        engine.record(t(0), ok(), &settings);
        assert_eq!(next_rotation(&mut engine, t(0)), t(3600));
    }

    #[test]
    fn failures_back_off_and_a_success_restores_the_interval() {
        let settings = set_up();
        let mut engine = fresh(&settings);
        engine.record(t(0), failed(), &settings);
        assert_eq!(next_rotation(&mut engine, t(0)), t(30));
        engine.record(t(30), failed(), &settings);
        assert_eq!(next_rotation(&mut engine, t(30)), t(90));
        engine.record(t(90), failed(), &settings);
        assert_eq!(next_rotation(&mut engine, t(90)), t(390));
        assert!(engine.status.retry_at.is_some());
        engine.record(t(390), ok(), &settings);
        assert_eq!(next_rotation(&mut engine, t(390)), t(390 + 3600));
        assert!(engine.status.error.is_none());
    }

    #[test]
    fn paused_or_not_set_up_never_rotates() {
        let settings = set_up();
        let mut paused = fresh(&settings);
        paused.status.paused = true;
        assert_eq!(paused.next_step(t(0), HOUR), Step::Wait(POLL));

        let mut unconfigured = fresh(&Settings::default());
        assert_eq!(unconfigured.next_step(t(0), HOUR), Step::Wait(POLL));
    }

    #[test]
    fn a_clock_set_backwards_costs_at_most_one_interval() {
        let settings = set_up();
        let mut engine = fresh(&settings);
        // a rotation while the clock ran 10 hours ahead, then it's corrected
        engine.record(t(10 * 3600), ok(), &settings);
        assert_eq!(next_rotation(&mut engine, t(0)), t(3600));
    }

    #[test]
    fn a_restart_picks_up_where_it_left_off() {
        let settings = set_up();
        let saved = SavedState {
            last_rotated: Some(schedule::unix_secs(t(0))),
            selection_key: schedule::selection_key(&settings),
            rotation: crate::rotation::RotationSnapshot {
                current_index: 7,
                ..Default::default()
            },
            // saved before images kept their URL
            shown: vec![ShownImage {
                id: "12".into(),
                label: String::new(),
                url: String::new(),
            }],
            paused: false,
            ..SavedState::default()
        };

        // two days later, it's due and continues at page 8
        let mut engine = Engine::from_saved(saved.clone(), &settings, None);
        assert_eq!(engine.next_step(t(2 * 86400), HOUR), Step::Rotate);
        let next = engine
            .rotation
            .select_next(RotationMode::Sequential, 10)
            .unwrap();
        assert_eq!(next.page, 8);
        assert_eq!(engine.status.open[0].url, "http://stash:9999/images/12");

        // ten minutes later, it isn't due yet
        let mut engine = Engine::from_saved(saved.clone(), &settings, None);
        assert!(matches!(engine.next_step(t(600), HOUR), Step::Wait(_)));

        // with a different filter the old position doesn't apply
        let other = Settings {
            query_filter: r#"{"filter": {"sort": "rating"}}"#.into(),
            ..settings
        };
        let mut engine = Engine::from_saved(saved, &other, None);
        let next = engine
            .rotation
            .select_next(RotationMode::Sequential, 10)
            .unwrap();
        assert_eq!(next.page, 1);
    }

    #[test]
    fn pausing_survives_a_restart() {
        let saved = SavedState {
            paused: true,
            ..SavedState::default()
        };
        let engine = Engine::from_saved(saved, &set_up(), None);
        assert!(engine.status.paused);
    }

    #[test]
    fn saving_settings_keeps_the_position_unless_image_selection_changed() {
        let settings = set_up();
        let mut engine = fresh(&settings);
        for _ in 0..3 {
            engine.rotation.select_next(RotationMode::Sequential, 10);
        }
        engine.settings_changed(&Settings {
            interval: Interval::Daily,
            ..settings.clone()
        });
        let next = engine
            .rotation
            .select_next(RotationMode::Sequential, 10)
            .unwrap();
        assert_eq!(next.page, 4);

        engine.settings_changed(&Settings {
            query_filter: r#"{"filter": {"sort": "rating"}}"#.into(),
            ..settings
        });
        let next = engine
            .rotation
            .select_next(RotationMode::Sequential, 10)
            .unwrap();
        assert_eq!(next.page, 1);
    }

    #[test]
    fn saving_an_unconfigured_state_clears_a_stale_error() {
        let settings = set_up();
        let mut engine = fresh(&settings);
        engine.record(t(0), failed(), &settings);
        engine.settings_changed(&Settings::default());
        assert!(!engine.status.configured);
        assert!(engine.status.error.is_none());
        assert!(engine.status.retry_at.is_none());
    }

    #[test]
    fn a_clock_correction_also_fixes_what_the_tray_says() {
        let settings = set_up();
        let mut engine = fresh(&settings);
        engine.record(t(10 * 3600), failed(), &settings);
        assert_eq!(engine.status.retry_at, Some(t(10 * 3600 + 30)));
        engine.next_step(t(0), HOUR);
        assert_eq!(engine.status.retry_at, Some(t(30)));
        assert!(engine.needs_refresh);

        let mut engine = fresh(&settings);
        engine.record(t(10 * 3600), ok(), &settings);
        engine.next_step(t(0), HOUR);
        assert_eq!(engine.status.last_changed, Some(t(0)));
    }

    #[test]
    fn open_in_stash_keeps_the_server_an_image_came_from() {
        let saved = SavedState {
            shown: vec![ShownImage {
                id: "12".into(),
                label: String::new(),
                url: "http://old-server:9999/images/12".into(),
            }],
            ..SavedState::default()
        };
        let moved = Settings {
            stash_url: "http://new-server:9999".into(),
            ..set_up()
        };
        let engine = Engine::from_saved(saved, &moved, None);
        assert_eq!(
            engine.status.open[0].url,
            "http://old-server:9999/images/12"
        );
    }

    #[test]
    fn the_plan_follows_the_setting_the_monitors_and_the_desktop() {
        use crate::desktop::PerMonitor::{Native, Spanned, Unsupported};
        let cases = [
            ((false, 3, Native), (1, PlanKind::Single)),
            ((true, 1, Native), (1, PlanKind::Single)),
            ((true, 3, Native), (3, PlanKind::PerMonitor)),
            ((true, 2, Spanned), (2, PlanKind::Composite)),
            // KDE, XFCE, macOS, swaybg, feh: one image for all
            ((true, 3, Unsupported), (1, PlanKind::Single)),
        ];
        for ((setting, monitors, support), expected) in cases {
            assert_eq!(
                plan_for(setting, monitors, support),
                expected,
                "{setting} {monitors} {support:?}"
            );
        }
    }
}
