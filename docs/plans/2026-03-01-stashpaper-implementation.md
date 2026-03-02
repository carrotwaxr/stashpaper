# StashPaper Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Build a cross-platform system tray app that rotates desktop wallpapers from a Stash instance via GraphQL.

**Architecture:** Tauri v2 app with React+Vite+Tailwind frontend (settings UI only), Rust backend for Stash API communication, wallpaper setting, and rotation scheduling. System tray for controls, JSON file for settings persistence.

**Tech Stack:** Tauri v2, React 19, TypeScript, Vite, Tailwind CSS v4, Rust, reqwest, wallpaper (crate), tokio, serde, rand

---

## Task 1: Scaffold Tauri v2 Project

**Goal:** Create the project skeleton with all dependencies configured.

**Step 1: Create Tauri project**

Run:
```bash
cd ~/code && npm create tauri-app@latest stashpaper -- --template react-ts
cd stashpaper
npm install
```

**Step 2: Add Tailwind CSS v4**

Run:
```bash
npm install tailwindcss @tailwindcss/vite
```

Modify `vite.config.ts`:
```ts
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";

export default defineConfig({
  plugins: [react(), tailwindcss()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    watch: {
      ignored: ["**/src-tauri/**"],
    },
  },
});
```

Replace `src/styles.css` with:
```css
@import "tailwindcss";
```

**Step 3: Update Rust dependencies**

Replace `src-tauri/Cargo.toml` `[dependencies]` section:
```toml
[dependencies]
tauri = { version = "2", features = ["tray-icon"] }
tauri-plugin-opener = "2"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
reqwest = { version = "0.12", features = ["json"] }
tokio = { version = "1", features = ["full"] }
wallpaper = "3.2"
rand = "0.9"
thiserror = "2"
```

**Step 4: Configure Tauri**

Replace `src-tauri/tauri.conf.json`:
```json
{
  "$schema": "https://schema.tauri.app/config/2",
  "productName": "StashPaper",
  "version": "0.1.0",
  "identifier": "com.stashpaper.app",
  "build": {
    "frontendDist": "../dist",
    "devUrl": "http://localhost:1420",
    "beforeDevCommand": "npm run dev",
    "beforeBuildCommand": "npm run build"
  },
  "app": {
    "windows": [
      {
        "label": "main",
        "title": "StashPaper Settings",
        "width": 520,
        "height": 680,
        "visible": false,
        "resizable": true,
        "center": true
      }
    ],
    "security": {
      "csp": null
    }
  },
  "bundle": {
    "active": true,
    "targets": "all",
    "icon": [
      "icons/32x32.png",
      "icons/128x128.png",
      "icons/128x128@2x.png",
      "icons/icon.icns",
      "icons/icon.ico"
    ]
  }
}
```

**Step 5: Update capabilities**

Replace `src-tauri/capabilities/default.json`:
```json
{
  "$schema": "../gen/schemas/desktop-schema.json",
  "identifier": "default",
  "description": "Default capabilities",
  "windows": ["main"],
  "permissions": [
    "core:default"
  ]
}
```

**Step 6: Verify**

Run: `cd ~/code/stashpaper && npm run tauri dev`

Expected: App compiles and an empty window appears (it will be hidden by default, so you may need to temporarily set `"visible": true` to verify, then set it back).

**Step 7: Commit**

```bash
git init && git add -A && git commit -m "feat: scaffold Tauri v2 project with React + Tailwind"
```

---

## Task 2: Error Types & Settings Module

**Goal:** Define app error types and settings types with JSON file persistence.

**Files:**
- Create: `src-tauri/src/error.rs`
- Create: `src-tauri/src/settings.rs`
- Modify: `src-tauri/src/lib.rs`
- Create: `src/lib/types.ts`

**Step 1: Create error types**

Create `src-tauri/src/error.rs`:
```rust
use serde::Serialize;

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("Stash error: {0}")]
    Stash(String),

    #[error("Wallpaper error: {0}")]
    Wallpaper(String),

    #[error("Settings error: {0}")]
    Settings(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

impl Serialize for AppError {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.to_string().as_str())
    }
}
```

**Step 2: Create settings types and persistence**

Create `src-tauri/src/settings.rs`:
```rust
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::Duration;

use crate::error::AppError;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    pub stash_url: String,
    pub api_key: String,
    pub image_filter: String,
    pub rotation_mode: RotationMode,
    pub interval: Interval,
    pub per_monitor: bool,
    pub wifi_only: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RotationMode {
    Random,
    Sequential,
    Shuffle,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Interval {
    FiveMinutes,
    FifteenMinutes,
    ThirtyMinutes,
    OneHour,
    FourHours,
    Daily,
}

impl Interval {
    pub fn to_duration(self) -> Duration {
        match self {
            Interval::FiveMinutes => Duration::from_secs(5 * 60),
            Interval::FifteenMinutes => Duration::from_secs(15 * 60),
            Interval::ThirtyMinutes => Duration::from_secs(30 * 60),
            Interval::OneHour => Duration::from_secs(60 * 60),
            Interval::FourHours => Duration::from_secs(4 * 60 * 60),
            Interval::Daily => Duration::from_secs(24 * 60 * 60),
        }
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            stash_url: String::new(),
            api_key: String::new(),
            image_filter: "{}".to_string(),
            rotation_mode: RotationMode::Random,
            interval: Interval::ThirtyMinutes,
            per_monitor: false,
            wifi_only: false,
        }
    }
}

fn settings_path(app: &tauri::AppHandle) -> Result<PathBuf, AppError> {
    let config_dir = app
        .path()
        .app_config_dir()
        .map_err(|e| AppError::Settings(e.to_string()))?;
    std::fs::create_dir_all(&config_dir)?;
    Ok(config_dir.join("settings.json"))
}

pub fn load(app: &tauri::AppHandle) -> Result<Settings, AppError> {
    let path = settings_path(app)?;
    if !path.exists() {
        return Ok(Settings::default());
    }
    let contents = std::fs::read_to_string(&path)?;
    serde_json::from_str(&contents).map_err(|e| AppError::Settings(e.to_string()))
}

pub fn save(app: &tauri::AppHandle, settings: &Settings) -> Result<(), AppError> {
    let path = settings_path(app)?;
    let contents =
        serde_json::to_string_pretty(settings).map_err(|e| AppError::Settings(e.to_string()))?;
    std::fs::write(&path, contents)?;
    Ok(())
}

pub fn is_configured(settings: &Settings) -> bool {
    !settings.stash_url.is_empty() && !settings.api_key.is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_settings_serialization_roundtrip() {
        let settings = Settings {
            stash_url: "http://localhost:9999".into(),
            api_key: "test-key".into(),
            image_filter: r#"{"tags":{"value":["wallpaper"],"modifier":"INCLUDES_ALL"}}"#.into(),
            rotation_mode: RotationMode::Shuffle,
            interval: Interval::OneHour,
            per_monitor: true,
            wifi_only: false,
        };

        let json = serde_json::to_string(&settings).unwrap();
        let deserialized: Settings = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.stash_url, "http://localhost:9999");
        assert_eq!(deserialized.rotation_mode, RotationMode::Shuffle);
        assert_eq!(deserialized.interval, Interval::OneHour);
        assert!(deserialized.per_monitor);
    }

    #[test]
    fn test_default_settings() {
        let settings = Settings::default();
        assert!(settings.stash_url.is_empty());
        assert_eq!(settings.rotation_mode, RotationMode::Random);
        assert_eq!(settings.interval, Interval::ThirtyMinutes);
        assert!(!settings.per_monitor);
        assert!(!settings.wifi_only);
    }

    #[test]
    fn test_interval_durations() {
        assert_eq!(Interval::FiveMinutes.to_duration(), Duration::from_secs(300));
        assert_eq!(Interval::Daily.to_duration(), Duration::from_secs(86400));
    }

    #[test]
    fn test_is_configured() {
        let mut settings = Settings::default();
        assert!(!is_configured(&settings));

        settings.stash_url = "http://localhost:9999".into();
        assert!(!is_configured(&settings));

        settings.api_key = "key".into();
        assert!(is_configured(&settings));
    }
}
```

**Step 3: Create TypeScript types**

Create `src/lib/types.ts`:
```ts
export type RotationMode = "random" | "sequential" | "shuffle";

export type Interval =
  | "five_minutes"
  | "fifteen_minutes"
  | "thirty_minutes"
  | "one_hour"
  | "four_hours"
  | "daily";

export interface Settings {
  stash_url: string;
  api_key: string;
  image_filter: string;
  rotation_mode: RotationMode;
  interval: Interval;
  per_monitor: boolean;
  wifi_only: boolean;
}

export const INTERVAL_LABELS: Record<Interval, string> = {
  five_minutes: "5 minutes",
  fifteen_minutes: "15 minutes",
  thirty_minutes: "30 minutes",
  one_hour: "1 hour",
  four_hours: "4 hours",
  daily: "Daily",
};

export const ROTATION_MODE_LABELS: Record<RotationMode, string> = {
  random: "Random",
  sequential: "Sequential",
  shuffle: "Shuffle (no repeat)",
};
```

**Step 4: Wire up lib.rs with settings commands**

Replace `src-tauri/src/lib.rs`:
```rust
mod error;
mod settings;

use error::AppError;

#[tauri::command]
fn get_settings(app: tauri::AppHandle) -> Result<settings::Settings, AppError> {
    settings::load(&app)
}

#[tauri::command]
fn save_settings(app: tauri::AppHandle, settings: settings::Settings) -> Result<(), AppError> {
    settings::save(&app, &settings)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![get_settings, save_settings])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
```

**Step 5: Run tests**

Run: `cd ~/code/stashpaper/src-tauri && cargo test`

Expected: All tests pass.

**Step 6: Verify compilation**

Run: `cd ~/code/stashpaper && npm run tauri dev`

Expected: Compiles without errors.

**Step 7: Commit**

```bash
git add -A && git commit -m "feat: add settings types, persistence, and error handling"
```

---

## Task 3: Stash Client

**Goal:** Rust module to query Stash via GraphQL and download images.

**Files:**
- Create: `src-tauri/src/stash.rs`
- Modify: `src-tauri/src/lib.rs`

**Step 1: Create the Stash client module**

Create `src-tauri/src/stash.rs`:
```rust
use crate::error::AppError;
use crate::settings::Settings;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Deserialize)]
struct GraphQLResponse<T> {
    data: Option<T>,
    errors: Option<Vec<GraphQLError>>,
}

#[derive(Debug, Deserialize)]
struct GraphQLError {
    message: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FindImagesData {
    find_images: FindImagesResult,
}

#[derive(Debug, Deserialize)]
pub struct FindImagesResult {
    pub count: usize,
    pub images: Vec<StashImage>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct StashImage {
    pub id: String,
    pub paths: ImagePaths,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ImagePaths {
    pub image: Option<String>,
}

#[derive(Debug, Serialize)]
struct GraphQLRequest {
    query: String,
    variables: serde_json::Value,
}

const FIND_IMAGES_QUERY: &str = r#"
query FindImages($filter: FindFilterType, $image_filter: ImageFilterType) {
  findImages(filter: $filter, image_filter: $image_filter) {
    count
    images {
      id
      paths {
        image
      }
    }
  }
}
"#;

fn client_for(settings: &Settings) -> Result<Client, AppError> {
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(
        "ApiKey",
        settings
            .api_key
            .trim()
            .parse()
            .map_err(|e: reqwest::header::InvalidHeaderValue| AppError::Stash(e.to_string()))?,
    );

    Client::builder()
        .default_headers(headers)
        .build()
        .map_err(|e| AppError::Stash(e.to_string()))
}

pub async fn test_connection(url: &str, api_key: &str) -> Result<bool, AppError> {
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(
        "ApiKey",
        api_key
            .trim()
            .parse()
            .map_err(|e: reqwest::header::InvalidHeaderValue| AppError::Stash(e.to_string()))?,
    );

    let client = Client::builder()
        .default_headers(headers)
        .build()
        .map_err(|e| AppError::Stash(e.to_string()))?;

    let body = GraphQLRequest {
        query: "query { systemStatus { databaseSchema } }".into(),
        variables: serde_json::json!({}),
    };

    let resp = client
        .post(format!("{}/graphql", url.trim_end_matches('/')))
        .json(&body)
        .send()
        .await
        .map_err(|e| AppError::Stash(e.to_string()))?;

    Ok(resp.status().is_success())
}

pub async fn query_image_count(settings: &Settings) -> Result<usize, AppError> {
    let client = client_for(settings)?;
    let image_filter: serde_json::Value =
        serde_json::from_str(&settings.image_filter).unwrap_or(serde_json::json!({}));

    let body = GraphQLRequest {
        query: FIND_IMAGES_QUERY.into(),
        variables: serde_json::json!({
            "filter": { "per_page": 1, "page": 1 },
            "image_filter": image_filter,
        }),
    };

    let resp = client
        .post(format!(
            "{}/graphql",
            settings.stash_url.trim_end_matches('/')
        ))
        .json(&body)
        .send()
        .await
        .map_err(|e| AppError::Stash(e.to_string()))?;

    let gql: GraphQLResponse<FindImagesData> =
        resp.json().await.map_err(|e| AppError::Stash(e.to_string()))?;

    if let Some(errors) = gql.errors {
        if let Some(err) = errors.first() {
            return Err(AppError::Stash(err.message.clone()));
        }
    }

    Ok(gql
        .data
        .map(|d| d.find_images.count)
        .unwrap_or(0))
}

pub async fn fetch_image_at_page(
    settings: &Settings,
    page: usize,
) -> Result<Option<StashImage>, AppError> {
    let client = client_for(settings)?;
    let image_filter: serde_json::Value =
        serde_json::from_str(&settings.image_filter).unwrap_or(serde_json::json!({}));

    let body = GraphQLRequest {
        query: FIND_IMAGES_QUERY.into(),
        variables: serde_json::json!({
            "filter": { "per_page": 1, "page": page },
            "image_filter": image_filter,
        }),
    };

    let resp = client
        .post(format!(
            "{}/graphql",
            settings.stash_url.trim_end_matches('/')
        ))
        .json(&body)
        .send()
        .await
        .map_err(|e| AppError::Stash(e.to_string()))?;

    let gql: GraphQLResponse<FindImagesData> =
        resp.json().await.map_err(|e| AppError::Stash(e.to_string()))?;

    if let Some(errors) = gql.errors {
        if let Some(err) = errors.first() {
            return Err(AppError::Stash(err.message.clone()));
        }
    }

    Ok(gql
        .data
        .and_then(|d| d.find_images.images.into_iter().next()))
}

pub async fn download_image(
    settings: &Settings,
    image_url: &str,
    cache_dir: &Path,
) -> Result<PathBuf, AppError> {
    let client = client_for(settings)?;

    let resp = client
        .get(image_url)
        .send()
        .await
        .map_err(|e| AppError::Stash(e.to_string()))?;

    let content_type = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("image/jpeg");

    let ext = match content_type {
        t if t.contains("png") => "png",
        t if t.contains("webp") => "webp",
        t if t.contains("gif") => "gif",
        _ => "jpg",
    };

    std::fs::create_dir_all(cache_dir)?;
    let file_path = cache_dir.join(format!("current_wallpaper.{}", ext));

    let bytes = resp
        .bytes()
        .await
        .map_err(|e| AppError::Stash(e.to_string()))?;
    std::fs::write(&file_path, &bytes)?;

    Ok(file_path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_graphql_request_serialization() {
        let body = GraphQLRequest {
            query: FIND_IMAGES_QUERY.into(),
            variables: serde_json::json!({
                "filter": { "per_page": 1, "page": 1 },
                "image_filter": {},
            }),
        };
        let json = serde_json::to_string(&body).unwrap();
        assert!(json.contains("findImages"));
        assert!(json.contains("per_page"));
    }

    #[test]
    fn test_find_images_response_parsing() {
        let json = r#"{
            "data": {
                "findImages": {
                    "count": 42,
                    "images": [{
                        "id": "123",
                        "paths": {
                            "image": "http://localhost:9999/image/123/image"
                        }
                    }]
                }
            }
        }"#;

        let resp: GraphQLResponse<FindImagesData> = serde_json::from_str(json).unwrap();
        let data = resp.data.unwrap();
        assert_eq!(data.find_images.count, 42);
        assert_eq!(data.find_images.images.len(), 1);
        assert_eq!(data.find_images.images[0].id, "123");
    }

    #[test]
    fn test_error_response_parsing() {
        let json = r#"{
            "data": null,
            "errors": [{"message": "Something went wrong"}]
        }"#;

        let resp: GraphQLResponse<FindImagesData> = serde_json::from_str(json).unwrap();
        assert!(resp.data.is_none());
        assert_eq!(resp.errors.unwrap()[0].message, "Something went wrong");
    }

    #[test]
    fn test_image_filter_parsing_empty() {
        let filter: serde_json::Value =
            serde_json::from_str("{}").unwrap_or(serde_json::json!({}));
        assert!(filter.is_object());
    }

    #[test]
    fn test_image_filter_parsing_with_tags() {
        let filter_str = r#"{"tags":{"value":["wallpaper"],"modifier":"INCLUDES_ALL"}}"#;
        let filter: serde_json::Value = serde_json::from_str(filter_str).unwrap();
        assert!(filter["tags"]["value"].is_array());
    }
}
```

**Step 2: Add test_connection command and register module**

Add to `src-tauri/src/lib.rs`:
```rust
mod stash;

#[tauri::command]
async fn test_connection(url: String, api_key: String) -> Result<bool, AppError> {
    stash::test_connection(&url, &api_key).await
}
```

Register in the `invoke_handler`:
```rust
.invoke_handler(tauri::generate_handler![get_settings, save_settings, test_connection])
```

**Step 3: Run tests**

Run: `cd ~/code/stashpaper/src-tauri && cargo test`

Expected: All tests pass.

**Step 4: Commit**

```bash
git add -A && git commit -m "feat: add Stash GraphQL client with image querying and download"
```

---

## Task 4: Rotation Logic

**Goal:** Pure rotation selection logic with thorough tests.

**Files:**
- Create: `src-tauri/src/rotation.rs`

**Step 1: Create rotation module with mode selection logic**

Create `src-tauri/src/rotation.rs`:
```rust
use crate::settings::RotationMode;
use rand::seq::SliceRandom;
use rand::Rng;

#[derive(Debug)]
pub struct RotationState {
    current_index: usize,
    shuffle_order: Vec<usize>,
    shuffle_position: usize,
    last_count: usize,
}

impl RotationState {
    pub fn new() -> Self {
        Self {
            current_index: 0,
            shuffle_order: Vec::new(),
            shuffle_position: 0,
            last_count: 0,
        }
    }

    pub fn reset(&mut self) {
        self.current_index = 0;
        self.shuffle_order.clear();
        self.shuffle_position = 0;
        self.last_count = 0;
    }

    /// Select the next page number (1-based) based on the rotation mode.
    /// Returns None if count is 0.
    pub fn select_next(&mut self, mode: RotationMode, count: usize) -> Option<usize> {
        if count == 0 {
            return None;
        }

        // If count changed, reset shuffle
        if count != self.last_count {
            self.last_count = count;
            if mode == RotationMode::Shuffle {
                self.regenerate_shuffle(count);
            }
            // For sequential, clamp index
            if self.current_index > count {
                self.current_index = 0;
            }
        }

        Some(match mode {
            RotationMode::Random => {
                let mut rng = rand::rng();
                rng.random_range(1..=count)
            }
            RotationMode::Sequential => {
                self.current_index += 1;
                if self.current_index > count {
                    self.current_index = 1;
                }
                self.current_index
            }
            RotationMode::Shuffle => {
                if self.shuffle_position >= self.shuffle_order.len() {
                    self.regenerate_shuffle(count);
                }
                let page = self.shuffle_order[self.shuffle_position];
                self.shuffle_position += 1;
                page
            }
        })
    }

    fn regenerate_shuffle(&mut self, count: usize) {
        let mut rng = rand::rng();
        self.shuffle_order = (1..=count).collect();
        self.shuffle_order.shuffle(&mut rng);
        self.shuffle_position = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_select_next_returns_none_for_zero_count() {
        let mut state = RotationState::new();
        assert_eq!(state.select_next(RotationMode::Random, 0), None);
        assert_eq!(state.select_next(RotationMode::Sequential, 0), None);
        assert_eq!(state.select_next(RotationMode::Shuffle, 0), None);
    }

    #[test]
    fn test_random_returns_valid_range() {
        let mut state = RotationState::new();
        for _ in 0..100 {
            let page = state.select_next(RotationMode::Random, 10).unwrap();
            assert!(page >= 1 && page <= 10);
        }
    }

    #[test]
    fn test_sequential_cycles_through_all() {
        let mut state = RotationState::new();
        let count = 5;

        for expected in 1..=5 {
            assert_eq!(
                state.select_next(RotationMode::Sequential, count),
                Some(expected)
            );
        }
        // Should wrap around
        assert_eq!(
            state.select_next(RotationMode::Sequential, count),
            Some(1)
        );
    }

    #[test]
    fn test_sequential_handles_count_change() {
        let mut state = RotationState::new();

        // Advance to position 3
        for _ in 0..3 {
            state.select_next(RotationMode::Sequential, 5);
        }
        assert_eq!(state.current_index, 3);

        // Count shrinks to 2 — index should clamp
        let page = state.select_next(RotationMode::Sequential, 2).unwrap();
        assert!(page >= 1 && page <= 2);
    }

    #[test]
    fn test_shuffle_visits_all_before_repeating() {
        let mut state = RotationState::new();
        let count = 5;
        let mut seen = std::collections::HashSet::new();

        for _ in 0..count {
            let page = state.select_next(RotationMode::Shuffle, count).unwrap();
            assert!(page >= 1 && page <= count);
            seen.insert(page);
        }

        // All pages should have been visited
        assert_eq!(seen.len(), count);
    }

    #[test]
    fn test_shuffle_regenerates_after_exhaustion() {
        let mut state = RotationState::new();
        let count = 3;

        // Exhaust first shuffle
        for _ in 0..count {
            state.select_next(RotationMode::Shuffle, count);
        }

        // Next call should start a new shuffle
        let page = state.select_next(RotationMode::Shuffle, count).unwrap();
        assert!(page >= 1 && page <= count);
    }

    #[test]
    fn test_reset_clears_state() {
        let mut state = RotationState::new();
        state.select_next(RotationMode::Sequential, 5);
        state.select_next(RotationMode::Sequential, 5);

        state.reset();
        assert_eq!(state.current_index, 0);
        assert!(state.shuffle_order.is_empty());

        // After reset, sequential should start from 1 again
        assert_eq!(
            state.select_next(RotationMode::Sequential, 5),
            Some(1)
        );
    }
}
```

**Step 2: Register module in lib.rs**

Add to `src-tauri/src/lib.rs`:
```rust
mod rotation;
```

**Step 3: Run tests**

Run: `cd ~/code/stashpaper/src-tauri && cargo test`

Expected: All tests pass.

**Step 4: Commit**

```bash
git add -A && git commit -m "feat: add rotation mode selection logic with tests"
```

---

## Task 5: Rotation Engine & Wallpaper Commands

**Goal:** Background rotation task with command channel, wallpaper setting, and tray menu controls.

**Files:**
- Create: `src-tauri/src/engine.rs`
- Modify: `src-tauri/src/lib.rs` (major rewrite — add tray, engine, commands)

**Step 1: Create the rotation engine**

Create `src-tauri/src/engine.rs`:
```rust
use crate::error::AppError;
use crate::rotation::RotationState;
use crate::settings::Settings;
use crate::stash;
use std::sync::Arc;
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
                // Not configured yet, wait a bit and check again
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
    let s = settings.read().await;

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
        .map_err(|e| AppError::Settings(e.to_string()))?;

    let file_path = stash::download_image(&s, &image_url, &cache_dir).await?;

    let path_str = file_path
        .to_str()
        .ok_or_else(|| AppError::Wallpaper("Invalid file path".into()))?;

    wallpaper::set_from_path(path_str)
        .map_err(|e| AppError::Wallpaper(e.to_string()))?;

    Ok(())
}
```

**Step 2: Rewrite lib.rs with tray, engine, and all commands**

Replace `src-tauri/src/lib.rs`:
```rust
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
    tray::{TrayIconBuilder, TrayIconEvent, MouseButton, MouseButtonState},
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

            let _tray = TrayIconBuilder::new()
                .icon(app.default_window_icon().unwrap().clone())
                .tooltip("StashPaper")
                .menu(&menu)
                .menu_on_left_click(false)
                .on_menu_event(move |app, event| {
                    let tx = tx.clone();
                    match event.id.as_ref() {
                        "next" => {
                            let tx = tx.clone();
                            tauri::async_runtime::spawn(async move {
                                let _ = tx.send(engine::Command::Next).await;
                            });
                        }
                        "pause" => {
                            // Toggle pause/resume
                            let tx = tx.clone();
                            tauri::async_runtime::spawn(async move {
                                // Simple toggle — sends Pause; user clicks again for Resume
                                // TODO: track paused state to toggle label
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

            // Hide window on close (minimize to tray)
            let main_window = app.get_webview_window("main").unwrap();
            main_window.on_window_event(move |event| {
                if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                    api.prevent_close();
                    // window.hide() is called from the event, but we need the window ref
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
```

**Note on window hide-on-close:** The `on_window_event` closure above prevents close but needs to actually hide the window. Since the closure doesn't have the window reference directly, update to:

In the `on_window_event`, the event is on the window itself. The implementing agent should check the Tauri v2 API — if `window.on_window_event` provides access to the window, call `window.hide()`. Otherwise, store the window handle and hide from the closure. The key behavior is: clicking the window close button hides it to tray instead of quitting.

**Step 3: Run tests**

Run: `cd ~/code/stashpaper/src-tauri && cargo test`

Expected: All tests pass (engine/tray code isn't unit tested — tested manually).

**Step 4: Verify compilation**

Run: `cd ~/code/stashpaper && npm run tauri dev`

Expected: App compiles. Tray icon appears. On first run (no settings file), settings window opens.

**Step 5: Commit**

```bash
git add -A && git commit -m "feat: add rotation engine, wallpaper setting, and system tray"
```

---

## Task 6: Settings UI

**Goal:** React settings form with Tailwind styling, wired to Rust commands.

**Files:**
- Create: `src/components/Settings.tsx`
- Modify: `src/App.tsx`
- Modify: `src/styles.css`

**Step 1: Create the Settings component**

Create `src/components/Settings.tsx`:
```tsx
import { useState, useEffect } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { Settings, RotationMode, Interval } from "../lib/types";
import { INTERVAL_LABELS, ROTATION_MODE_LABELS } from "../lib/types";

const DEFAULT_FILTER = `{
  "resolution": {
    "value": "STANDARD_HD",
    "modifier": "GREATER_THAN"
  }
}`;

export default function SettingsPanel() {
  const [settings, setSettings] = useState<Settings>({
    stash_url: "",
    api_key: "",
    image_filter: DEFAULT_FILTER,
    rotation_mode: "random",
    interval: "thirty_minutes",
    per_monitor: false,
    wifi_only: false,
  });
  const [connectionStatus, setConnectionStatus] = useState<
    "idle" | "testing" | "success" | "error"
  >("idle");
  const [saveStatus, setSaveStatus] = useState<"idle" | "saved" | "error">(
    "idle"
  );

  useEffect(() => {
    invoke<Settings>("get_settings").then((loaded) => {
      if (loaded.stash_url || loaded.api_key) {
        setSettings(loaded);
      }
    });
  }, []);

  const testConnection = async () => {
    setConnectionStatus("testing");
    try {
      const ok = await invoke<boolean>("test_connection", {
        url: settings.stash_url,
        apiKey: settings.api_key,
      });
      setConnectionStatus(ok ? "success" : "error");
    } catch {
      setConnectionStatus("error");
    }
  };

  const saveSettings = async () => {
    try {
      await invoke("save_settings", { newSettings: settings });
      setSaveStatus("saved");
      setTimeout(() => setSaveStatus("idle"), 2000);
    } catch {
      setSaveStatus("error");
    }
  };

  const update = <K extends keyof Settings>(key: K, value: Settings[K]) => {
    setSettings((prev) => ({ ...prev, [key]: value }));
  };

  return (
    <div className="min-h-screen bg-zinc-900 text-zinc-100 p-6">
      <h1 className="text-xl font-bold mb-6">StashPaper Settings</h1>

      {/* Stash Connection */}
      <section className="mb-6">
        <h2 className="text-sm font-semibold text-zinc-400 uppercase tracking-wide mb-3">
          Stash Connection
        </h2>
        <div className="space-y-3">
          <div>
            <label className="block text-sm text-zinc-400 mb-1">
              Server URL
            </label>
            <input
              type="text"
              value={settings.stash_url}
              onChange={(e) => update("stash_url", e.target.value)}
              placeholder="http://localhost:9999"
              className="w-full bg-zinc-800 border border-zinc-700 rounded px-3 py-2 text-sm focus:outline-none focus:border-zinc-500"
            />
          </div>
          <div>
            <label className="block text-sm text-zinc-400 mb-1">API Key</label>
            <input
              type="password"
              value={settings.api_key}
              onChange={(e) => update("api_key", e.target.value)}
              placeholder="Your Stash API key"
              className="w-full bg-zinc-800 border border-zinc-700 rounded px-3 py-2 text-sm focus:outline-none focus:border-zinc-500"
            />
          </div>
          <button
            onClick={testConnection}
            disabled={
              connectionStatus === "testing" ||
              !settings.stash_url ||
              !settings.api_key
            }
            className="bg-zinc-700 hover:bg-zinc-600 disabled:opacity-50 px-4 py-2 rounded text-sm transition-colors"
          >
            {connectionStatus === "testing"
              ? "Testing..."
              : "Test Connection"}
          </button>
          {connectionStatus === "success" && (
            <span className="ml-3 text-green-400 text-sm">Connected</span>
          )}
          {connectionStatus === "error" && (
            <span className="ml-3 text-red-400 text-sm">
              Connection failed
            </span>
          )}
        </div>
      </section>

      {/* Image Filter */}
      <section className="mb-6">
        <h2 className="text-sm font-semibold text-zinc-400 uppercase tracking-wide mb-3">
          Image Filter
        </h2>
        <p className="text-xs text-zinc-500 mb-2">
          Paste an ImageFilterType JSON from Stash's GraphQL Playground.
          Use {"{ }"} for all images.
        </p>
        <textarea
          value={settings.image_filter}
          onChange={(e) => update("image_filter", e.target.value)}
          rows={6}
          spellCheck={false}
          className="w-full bg-zinc-800 border border-zinc-700 rounded px-3 py-2 text-sm font-mono focus:outline-none focus:border-zinc-500 resize-y"
        />
      </section>

      {/* Rotation Settings */}
      <section className="mb-6">
        <h2 className="text-sm font-semibold text-zinc-400 uppercase tracking-wide mb-3">
          Rotation
        </h2>
        <div className="grid grid-cols-2 gap-4">
          <div>
            <label className="block text-sm text-zinc-400 mb-1">Mode</label>
            <select
              value={settings.rotation_mode}
              onChange={(e) =>
                update("rotation_mode", e.target.value as RotationMode)
              }
              className="w-full bg-zinc-800 border border-zinc-700 rounded px-3 py-2 text-sm focus:outline-none focus:border-zinc-500"
            >
              {Object.entries(ROTATION_MODE_LABELS).map(([value, label]) => (
                <option key={value} value={value}>
                  {label}
                </option>
              ))}
            </select>
          </div>
          <div>
            <label className="block text-sm text-zinc-400 mb-1">
              Interval
            </label>
            <select
              value={settings.interval}
              onChange={(e) =>
                update("interval", e.target.value as Interval)
              }
              className="w-full bg-zinc-800 border border-zinc-700 rounded px-3 py-2 text-sm focus:outline-none focus:border-zinc-500"
            >
              {Object.entries(INTERVAL_LABELS).map(([value, label]) => (
                <option key={value} value={value}>
                  {label}
                </option>
              ))}
            </select>
          </div>
        </div>
      </section>

      {/* Display Settings */}
      <section className="mb-6">
        <h2 className="text-sm font-semibold text-zinc-400 uppercase tracking-wide mb-3">
          Display
        </h2>
        <label className="flex items-center gap-2 text-sm cursor-pointer">
          <input
            type="checkbox"
            checked={settings.per_monitor}
            onChange={(e) => update("per_monitor", e.target.checked)}
            className="rounded bg-zinc-800 border-zinc-700"
          />
          Different wallpaper per monitor
        </label>
        <p className="text-xs text-zinc-500 mt-1 ml-6">
          Per-monitor support varies by platform and desktop environment.
        </p>
      </section>

      {/* Network Settings */}
      <section className="mb-8">
        <h2 className="text-sm font-semibold text-zinc-400 uppercase tracking-wide mb-3">
          Network
        </h2>
        <label className="flex items-center gap-2 text-sm cursor-pointer">
          <input
            type="checkbox"
            checked={settings.wifi_only}
            onChange={(e) => update("wifi_only", e.target.checked)}
            className="rounded bg-zinc-800 border-zinc-700"
          />
          Only rotate when connected to Wi-Fi
        </label>
      </section>

      {/* Save */}
      <button
        onClick={saveSettings}
        className="w-full bg-blue-600 hover:bg-blue-500 py-2.5 rounded font-medium text-sm transition-colors"
      >
        {saveStatus === "saved" ? "Saved!" : "Save Settings"}
      </button>
      {saveStatus === "error" && (
        <p className="text-red-400 text-sm mt-2">Failed to save settings.</p>
      )}
    </div>
  );
}
```

**Step 2: Update App.tsx**

Replace `src/App.tsx`:
```tsx
import SettingsPanel from "./components/Settings";

export default function App() {
  return <SettingsPanel />;
}
```

**Step 3: Clean up default files**

Delete the default Tauri template files that are no longer needed:
- `src/App.css` (styles are in Tailwind now)
- `src/assets/` directory (if exists, not needed)

Update `src/main.tsx` to import styles:
```tsx
import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import "./styles.css";

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>
);
```

**Step 4: Verify**

Run: `cd ~/code/stashpaper && npm run tauri dev`

Expected: Settings window appears with dark theme, all form fields render, test connection button works against a running Stash instance.

**Step 5: Commit**

```bash
git add -A && git commit -m "feat: add settings UI with Tailwind styling"
```

---

## Task 7: Integration & Polish

**Goal:** Fix remaining integration issues, verify end-to-end functionality.

**Step 1: Fix window hide-on-close**

In `src-tauri/src/lib.rs`, the `on_window_event` closure needs to actually hide the window. Update the close handler to properly hide:

```rust
// In setup, after getting main_window:
let hide_window = main_window.clone();
main_window.on_window_event(move |event| {
    if let tauri::WindowEvent::CloseRequested { api, .. } = event {
        api.prevent_close();
        let _ = hide_window.hide();
    }
});
```

**Step 2: Add first-run trigger for initial rotation**

After settings are saved for the first time, the rotation engine should immediately set a wallpaper rather than waiting for the interval. The `SettingsUpdated` command already resets state. Modify `engine.rs` to trigger a rotation after settings update:

In `engine::run`, update the `SettingsUpdated` handler:
```rust
Some(Command::SettingsUpdated) => {
    rotation_state.reset();
    // Immediately set a wallpaper with new settings
    do_rotate(&settings, &mut rotation_state, &app_handle).await;
}
```

**Step 3: End-to-end verification**

1. Run: `npm run tauri dev`
2. Settings window should appear (first run)
3. Enter Stash URL and API key, click Test Connection — should show "Connected"
4. Configure filter, mode, interval
5. Click Save Settings
6. Wallpaper should change immediately
7. Close settings window — app stays in system tray
8. Right-click tray → "Next Wallpaper" — wallpaper changes
9. Right-click tray → "Settings" — settings window reopens
10. Right-click tray → "Quit" — app exits

**Step 4: Commit**

```bash
git add -A && git commit -m "feat: polish integration — window hide, immediate rotation on save"
```

---

## Notes

**Per-monitor wallpaper:** v1 uses the `wallpaper` crate which sets the same wallpaper on all monitors. The `per_monitor` setting exists in the UI but is not yet wired to per-monitor behavior. A future enhancement can switch to the `more-wallpapers` crate (per-monitor on Linux) or native OS APIs.

**Wi-Fi only toggle:** The setting is stored but not yet checked before rotation. A future task should add network interface detection before fetching.

**Pause/Resume toggle:** The tray menu currently only sends Pause. A future enhancement should track paused state and toggle the menu item label between "Pause" and "Resume".

These are deliberate scope cuts to ship a working v1 quickly.
