mod engine;
mod error;
mod rotation;
mod settings;
mod stash;

use error::AppError;
use settings::Settings;
use std::sync::Arc;
use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Manager,
};
use tokio::sync::RwLock;

struct AppState {
    settings: Arc<RwLock<Settings>>,
    engine_tx: engine::CommandTx,
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
) -> Result<(), AppError> {
    settings::save(&app, &new_settings)?;
    *state.settings.write().await = new_settings;
    state
        .engine_tx
        .send(engine::Command::SettingsUpdated)
        .await
        .map_err(|e| AppError::Settings(e.to_string()))?;
    Ok(())
}

#[tauri::command]
async fn test_connection(url: String, api_key: String) -> Result<bool, AppError> {
    stash::test_connection(&url, &api_key).await
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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            get_settings,
            save_settings,
            test_connection,
            next_wallpaper,
            pause_rotation,
            resume_rotation,
        ])
        .setup(|app| {
            // Load settings
            let loaded = settings::load(&app.handle())?;
            let first_run = !settings::is_configured(&loaded);
            let shared_settings = Arc::new(RwLock::new(loaded));

            // Create engine channel
            let (tx, rx) = engine::create_channel();

            // Register app state
            app.manage(AppState {
                settings: shared_settings.clone(),
                engine_tx: tx.clone(),
            });

            // Build tray menu
            let next_item =
                MenuItem::with_id(app, "next", "Next Wallpaper", true, None::<&str>)?;
            let pause_item =
                MenuItem::with_id(app, "pause", "Pause", true, None::<&str>)?;
            let settings_item =
                MenuItem::with_id(app, "settings", "Settings", true, None::<&str>)?;
            let quit_item =
                MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
            let menu = Menu::with_items(
                app,
                &[&next_item, &pause_item, &settings_item, &quit_item],
            )?;

            // Clone tx for tray menu closure
            let tray_tx = tx.clone();
            let _tray = TrayIconBuilder::new()
                .icon(app.default_window_icon().unwrap().clone())
                .tooltip("StashPaper")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(move |app, event| {
                    let tx = tray_tx.clone();
                    match event.id.as_ref() {
                        "next" => {
                            tauri::async_runtime::spawn(async move {
                                let _ = tx.send(engine::Command::Next).await;
                            });
                        }
                        "pause" => {
                            tauri::async_runtime::spawn(async move {
                                let _ = tx.send(engine::Command::Pause).await;
                            });
                        }
                        "settings" => {
                            if let Some(window) = app.get_webview_window("main") {
                                let _ = window.show();
                                let _ = window.unminimize();
                                let _ = window.set_focus();
                            }
                        }
                        "quit" => {
                            let tx = tx.clone();
                            tauri::async_runtime::spawn(async move {
                                let _ = tx.send(engine::Command::Quit).await;
                            });
                            app.exit(0);
                        }
                        _ => {}
                    }
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        let app = tray.app_handle();
                        if let Some(window) = app.get_webview_window("main") {
                            let _ = window.show();
                            let _ = window.unminimize();
                            let _ = window.set_focus();
                        }
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

            // Show settings on first run
            if first_run {
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.show();
                    let _ = window.set_focus();
                }
            }

            // Start rotation engine
            let engine_settings = shared_settings.clone();
            let engine_handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                engine::run(rx, engine_settings, engine_handle).await;
            });

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
