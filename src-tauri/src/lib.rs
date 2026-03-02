mod error;
mod settings;
mod stash;

use error::AppError;

#[tauri::command]
fn get_settings(app: tauri::AppHandle) -> Result<settings::Settings, AppError> {
    settings::load(&app)
}

#[tauri::command]
fn save_settings(app: tauri::AppHandle, settings: settings::Settings) -> Result<(), AppError> {
    settings::save(&app, &settings)
}

#[tauri::command]
async fn test_connection(url: String, api_key: String) -> Result<bool, AppError> {
    stash::test_connection(&url, &api_key).await
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![get_settings, save_settings, test_connection])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
