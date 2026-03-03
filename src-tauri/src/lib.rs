mod engine;
mod error;
mod rotation;
mod settings;
mod stash;

use error::AppError;
use settings::Settings;
use std::sync::Arc;
use tauri::{
    image::Image,
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Manager,
};
use tokio::sync::RwLock;

const TRAY_ID: &str = "main-tray";

struct AppState {
    settings: Arc<RwLock<Settings>>,
    engine_tx: engine::CommandTx,
}

struct TrayIcons {
    normal: Image<'static>,
    error: Image<'static>,
}

/// Apply a grayscale + red tint to RGBA icon data to produce an "error" variant.
fn make_error_icon(rgba: &[u8], width: u32, height: u32) -> Image<'static> {
    let mut tinted = rgba.to_vec();
    for pixel in tinted.chunks_exact_mut(4) {
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

/// Update the tray icon and tooltip based on error state.
pub fn update_tray_icon(app: &tauri::AppHandle, is_error: bool, message: Option<&str>) {
    if let Some(tray) = app.tray_by_id(TRAY_ID) {
        let icons = app.state::<TrayIcons>();
        let icon = if is_error {
            &icons.error
        } else {
            &icons.normal
        };
        let _ = tray.set_icon(Some(icon.clone()));
        let tooltip = match (is_error, message) {
            (false, _) => "StashPaper".to_string(),
            (true, Some(msg)) => format!("StashPaper - {}", msg),
            (true, None) => "StashPaper - Error".to_string(),
        };
        let _ = tray.set_tooltip(Some(&tooltip));
    }
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

#[derive(serde::Serialize)]
struct MonitorResolution {
    width: u32,
    height: u32,
}

#[tauri::command]
async fn detect_monitor_resolution(app: tauri::AppHandle) -> Option<MonitorResolution> {
    app.primary_monitor()
        .ok()
        .flatten()
        .map(|monitor| {
            let size = monitor.size();
            MonitorResolution {
                width: size.width,
                height: size.height,
            }
        })
}

#[tauri::command]
async fn test_query(new_settings: Settings) -> Result<usize, AppError> {
    stash::test_query(&new_settings).await
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
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            get_settings,
            save_settings,
            test_connection,
            test_query,
            detect_monitor_resolution,
            next_wallpaper,
            pause_rotation,
            resume_rotation,
        ])
        .setup(|app| {
            // Load settings
            let loaded = settings::load(app.handle())?;
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

            // Generate normal + error tray icons (must own the data for 'static)
            let icon_ref = app.default_window_icon().unwrap();
            let normal_icon = Image::new_owned(
                icon_ref.rgba().to_vec(),
                icon_ref.width(),
                icon_ref.height(),
            );
            let error_icon =
                make_error_icon(icon_ref.rgba(), icon_ref.width(), icon_ref.height());
            app.manage(TrayIcons {
                normal: normal_icon.clone(),
                error: error_icon,
            });

            // Clone tx for tray menu closure
            let tray_tx = tx.clone();
            let _tray = TrayIconBuilder::with_id(TRAY_ID)
                .icon(normal_icon)
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
