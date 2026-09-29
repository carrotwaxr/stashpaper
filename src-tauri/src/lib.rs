mod compositor;
mod engine;
mod error;
mod rotation;
mod schedule;
mod settings;
mod stash;
mod tray;

use error::AppError;
use settings::Settings;
use std::sync::Arc;
use tauri::{
    image::Image,
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Manager,
};
use tauri_plugin_autostart::ManagerExt as _;
use tauri_plugin_opener::OpenerExt as _;
use tokio::sync::RwLock;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MonitorInfo {
    pub width: u32,
    pub height: u32,
    pub x: i32,
    pub y: i32,
    pub scale_factor: f64,
}

struct AppState {
    settings: Arc<RwLock<Settings>>,
    engine_tx: engine::CommandTx,
    /// Set when the settings file couldn't be read at startup
    load_warning: std::sync::Mutex<Option<String>>,
}

/// The monitors as Tauri reports them, in physical pixels.
pub fn monitor_infos(app: &tauri::AppHandle) -> Vec<MonitorInfo> {
    app.available_monitors()
        .map(|monitors| {
            monitors
                .into_iter()
                .map(|m| {
                    let size = m.size();
                    let pos = m.position();
                    MonitorInfo {
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

fn show_settings_window(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

fn send_to_engine(tx: &engine::CommandTx, command: engine::Command) {
    let tx = tx.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(e) = tx.send(command).await {
            log::error!("The rotation engine isn't running: {}", e);
        }
    });
}

#[tauri::command]
fn get_autostart(app: tauri::AppHandle) -> Result<bool, AppError> {
    app.autolaunch()
        .is_enabled()
        .map_err(|e| AppError::Settings(e.to_string()))
}

#[tauri::command]
fn set_autostart(app: tauri::AppHandle, enabled: bool) -> Result<(), AppError> {
    let autolaunch = app.autolaunch();
    let result = if enabled {
        autolaunch.enable()
    } else {
        autolaunch.disable()
    };
    result.map_err(|e| AppError::Settings(e.to_string()))
}

#[tauri::command]
async fn get_settings(state: tauri::State<'_, AppState>) -> Result<Settings, AppError> {
    Ok(state.settings.read().await.clone())
}

#[tauri::command]
async fn save_settings(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    new_settings: Settings,
) -> Result<Settings, AppError> {
    // Clean up the URL, and refuse a filter the engine couldn't apply rather
    // than save it and fail every rotation after
    let new_settings = settings::prepare(new_settings)?;
    settings::save(&app, &new_settings)?;
    // The unreadable file has now been replaced
    *state
        .load_warning
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
    *state.settings.write().await = new_settings.clone();
    state
        .engine_tx
        .send(engine::Command::SettingsUpdated)
        .await
        .map_err(|e| AppError::Settings(e.to_string()))?;
    Ok(new_settings)
}

#[tauri::command]
async fn test_connection(url: String, api_key: String) -> Result<(), AppError> {
    stash::test_connection(&settings::normalize_stash_url(&url)?, &api_key).await
}

#[tauri::command]
async fn next_wallpaper(state: tauri::State<'_, AppState>) -> Result<(), AppError> {
    state
        .engine_tx
        .send(engine::Command::Next)
        .await
        .map_err(|e| AppError::Settings(e.to_string()))
}

#[tauri::command]
async fn pause_rotation(state: tauri::State<'_, AppState>) -> Result<(), AppError> {
    state
        .engine_tx
        .send(engine::Command::Pause)
        .await
        .map_err(|e| AppError::Settings(e.to_string()))
}

#[tauri::command]
async fn resume_rotation(state: tauri::State<'_, AppState>) -> Result<(), AppError> {
    state
        .engine_tx
        .send(engine::Command::Resume)
        .await
        .map_err(|e| AppError::Settings(e.to_string()))
}

#[tauri::command]
async fn detect_monitors(app: tauri::AppHandle) -> Vec<MonitorInfo> {
    monitor_infos(&app)
}

#[tauri::command]
async fn test_query(new_settings: Settings) -> Result<usize, AppError> {
    stash::test_query(&settings::prepare(new_settings)?).await
}

/// The settings window calls this once it has rendered, so the log (and the
/// CI smoke test) can tell the window actually works.
#[tauri::command]
fn window_ready() {
    log::info!("Settings window ready");
}

#[tauri::command]
fn settings_load_warning(state: tauri::State<'_, AppState>) -> Option<String> {
    state
        .load_warning
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        // Must be registered first: a second launch hands over to the running
        // instance and exits before anything else starts
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            log::info!("Another launch handed over to this one; showing settings");
            show_settings_window(app);
        }))
        .plugin(
            tauri_plugin_log::Builder::new()
                .level(log::LevelFilter::Info)
                .max_file_size(1_000_000)
                .rotation_strategy(tauri_plugin_log::RotationStrategy::KeepSome(3))
                .build(),
        )
        .plugin(
            tauri_plugin_autostart::Builder::new()
                .app_name("StashPaper")
                .build(),
        )
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            get_settings,
            save_settings,
            test_connection,
            test_query,
            detect_monitors,
            next_wallpaper,
            pause_rotation,
            resume_rotation,
            get_autostart,
            set_autostart,
            settings_load_warning,
            window_ready,
        ])
        .setup(|app| {
            log::info!("StashPaper {} starting", app.package_info().version);

            // Load settings
            let (loaded, load_warning) = settings::load(app.handle())?;
            let show_settings = !settings::is_configured(&loaded) || load_warning.is_some();
            let shared_settings = Arc::new(RwLock::new(loaded));

            // Create engine channel
            let (tx, rx) = engine::create_channel();

            // Register app state
            app.manage(AppState {
                settings: shared_settings.clone(),
                engine_tx: tx.clone(),
                load_warning: std::sync::Mutex::new(load_warning),
            });
            app.manage(tray::OpenUrls::default());

            // Generate normal + error tray icons (must own the data for 'static)
            let icon_ref = app.default_window_icon().unwrap();
            let normal_icon = Image::new_owned(
                icon_ref.rgba().to_vec(),
                icon_ref.width(),
                icon_ref.height(),
            );
            let error_icon =
                tray::make_error_icon(icon_ref.rgba(), icon_ref.width(), icon_ref.height());
            app.manage(tray::TrayIcons {
                normal: normal_icon.clone(),
                error: error_icon,
            });

            // The engine replaces this menu as soon as it starts
            let menu = tray::build_menu(app.handle(), &tray::TrayStatus::default())?;
            let tray_tx = tx.clone();
            let _tray = TrayIconBuilder::with_id(tray::TRAY_ID)
                .icon(normal_icon)
                .tooltip("StashPaper")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(move |app, event| match event.id.as_ref() {
                    id if tray::command_for(id).is_some() => {
                        if let Some(command) = tray::command_for(id) {
                            send_to_engine(&tray_tx, command);
                        }
                    }
                    tray::SETTINGS => show_settings_window(app),
                    tray::LOGS => {
                        let opened = app
                            .path()
                            .app_log_dir()
                            .map_err(|e| e.to_string())
                            .and_then(|dir| {
                                app.opener()
                                    .open_path(dir.to_string_lossy(), None::<&str>)
                                    .map_err(|e| e.to_string())
                            });
                        if let Err(e) = opened {
                            log::warn!("Couldn't open the log folder: {}", e);
                        }
                    }
                    tray::QUIT => {
                        send_to_engine(&tray_tx, engine::Command::Quit);
                        app.exit(0);
                    }
                    id => {
                        if let Some(url) = tray::open_url_for(app, id) {
                            if let Err(e) = app.opener().open_url(url, None::<&str>) {
                                log::warn!("Couldn't open the image in Stash: {}", e);
                            }
                        }
                    }
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        show_settings_window(tray.app_handle());
                    }
                })
                .build(app)?;

            // Hide window on close (minimize to tray instead of quitting)
            let main_window = app.get_webview_window("main").unwrap();
            let hide_window = main_window.clone();
            main_window.on_window_event(move |event| {
                if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                    api.prevent_close();
                    let _ = hide_window.hide();
                }
            });

            // Show settings on first run, or when the settings file was unreadable
            if show_settings {
                show_settings_window(app.handle());
            }

            // Start rotation engine
            let engine_settings = shared_settings.clone();
            let engine_handle = app.handle().clone();
            let engine_task = tauri::async_runtime::spawn(async move {
                engine::run(rx, engine_settings, engine_handle).await;
            });
            // A panic in the engine would otherwise only reach stderr, and the
            // tray would silently stop responding
            tauri::async_runtime::spawn(async move {
                if let Err(e) = engine_task.await {
                    log::error!("The rotation engine stopped unexpectedly: {}", e);
                }
            });

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
