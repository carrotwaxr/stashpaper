# Multi-Monitor Wallpaper Support Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Set different wallpapers on each monitor when `per_monitor` is enabled.

**Architecture:** On each rotation tick, fetch N images (one per monitor) and composite them into a single large image matching the bounding box of all monitors. Each image is scaled/placed at its monitor's position. The composite is set as one wallpaper via the existing `wallpaper` crate. This works universally (GNOME, KDE, Windows, macOS). Native per-monitor APIs (Windows `IDesktopWallpaper`, macOS `NSWorkspace`) are a future optimization but not in scope for v0.2.

**Tech Stack:** Rust `image` crate for compositing, Tauri `available_monitors()` for geometry, existing `wallpaper` crate for setting the result.

---

## Task 1: Add `image` dependency and `compositor` module skeleton

**Files:**
- Modify: `src-tauri/Cargo.toml`
- Create: `src-tauri/src/compositor.rs`
- Modify: `src-tauri/src/lib.rs` (add `mod compositor;`)

**Step 1: Add the `image` crate dependency**

In `src-tauri/Cargo.toml`, add to `[dependencies]`:

```toml
image = { version = "0.25", default-features = false, features = ["jpeg", "png", "webp"] }
```

**Step 2: Create the compositor module with MonitorGeometry struct**

Create `src-tauri/src/compositor.rs`:

```rust
use crate::error::AppError;
use image::{DynamicImage, GenericImageView, ImageFormat, RgbaImage};
use std::path::{Path, PathBuf};

/// Monitor position and size in physical pixels.
#[derive(Debug, Clone, PartialEq)]
pub struct MonitorGeometry {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}
```

**Step 3: Add `mod compositor;` to `src-tauri/src/lib.rs`**

After the existing `mod stash;` line, add:

```rust
mod compositor;
```

**Step 4: Verify it compiles**

Run: `cd src-tauri && cargo check`
Expected: compiles with no errors (warnings about unused imports are OK for now)

**Step 5: Commit**

```
feat(compositor): add image crate and compositor module skeleton
```

---

## Task 2: Implement `composite_wallpaper` function with tests

**Files:**
- Modify: `src-tauri/src/compositor.rs`

**Step 1: Write the failing test — single monitor passthrough**

The simplest case: one monitor = just resize the image to fit, no composition needed.

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn make_test_image(width: u32, height: u32, r: u8, g: u8, b: u8) -> PathBuf {
        let img = RgbaImage::from_fn(width, height, |_, _| image::Rgba([r, g, b, 255]));
        let path = std::env::temp_dir().join(format!("test_{}x{}_{}_{}.png", width, height, r, g));
        img.save(&path).unwrap();
        path
    }

    #[test]
    fn test_single_monitor_resizes_to_fit() {
        let img_path = make_test_image(200, 100, 255, 0, 0);
        let monitors = vec![MonitorGeometry { x: 0, y: 0, width: 1920, height: 1080 }];
        let output = std::env::temp_dir().join("test_composite_single.png");

        composite_wallpaper(&[img_path.clone()], &monitors, &output).unwrap();

        let result = image::open(&output).unwrap();
        assert_eq!(result.width(), 1920);
        assert_eq!(result.height(), 1080);

        // Cleanup
        let _ = std::fs::remove_file(&img_path);
        let _ = std::fs::remove_file(&output);
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cd src-tauri && cargo test compositor::tests::test_single_monitor_resizes_to_fit`
Expected: FAIL — `composite_wallpaper` function doesn't exist

**Step 3: Implement `composite_wallpaper`**

```rust
/// Composite multiple images onto a canvas matching the bounding box of all monitors.
/// Each image is resized to fill its monitor's area (crop to cover).
/// `image_paths[i]` maps to `monitors[i]`. If fewer images than monitors, the last
/// image is reused. Output is saved as PNG.
pub fn composite_wallpaper(
    image_paths: &[PathBuf],
    monitors: &[MonitorGeometry],
    output_path: &Path,
) -> Result<(), AppError> {
    if monitors.is_empty() || image_paths.is_empty() {
        return Err(AppError::Wallpaper("No monitors or images provided".into()));
    }

    // Calculate bounding box (monitors can have negative positions)
    let min_x = monitors.iter().map(|m| m.x).min().unwrap();
    let min_y = monitors.iter().map(|m| m.y).min().unwrap();
    let max_x = monitors.iter().map(|m| m.x + m.width as i32).max().unwrap();
    let max_y = monitors.iter().map(|m| m.y + m.height as i32).max().unwrap();

    let canvas_width = (max_x - min_x) as u32;
    let canvas_height = (max_y - min_y) as u32;

    let mut canvas = RgbaImage::new(canvas_width, canvas_height);

    for (i, monitor) in monitors.iter().enumerate() {
        let img_idx = i.min(image_paths.len() - 1);
        let img = image::open(&image_paths[img_idx])
            .map_err(|e| AppError::Wallpaper(format!("Failed to open image: {}", e)))?;

        let resized = crop_to_fill(&img, monitor.width, monitor.height);

        // Place on canvas, offsetting by the bounding box origin
        let dest_x = (monitor.x - min_x) as u32;
        let dest_y = (monitor.y - min_y) as u32;

        image::imageops::overlay(&mut canvas, &resized, dest_x as i64, dest_y as i64);
    }

    canvas
        .save(output_path)
        .map_err(|e| AppError::Wallpaper(format!("Failed to save composite: {}", e)))?;

    Ok(())
}

/// Resize an image to fill the target dimensions, cropping the excess.
/// This is the "Crop/Fill" behavior: the image covers the entire area with
/// no letterboxing, cropping from center if aspect ratios differ.
fn crop_to_fill(img: &DynamicImage, target_w: u32, target_h: u32) -> RgbaImage {
    let (src_w, src_h) = img.dimensions();

    let scale = f64::max(
        target_w as f64 / src_w as f64,
        target_h as f64 / src_h as f64,
    );

    let scaled_w = (src_w as f64 * scale).ceil() as u32;
    let scaled_h = (src_h as f64 * scale).ceil() as u32;

    let scaled = img.resize_exact(scaled_w, scaled_h, image::imageops::FilterType::Lanczos3);

    // Center crop
    let crop_x = (scaled_w.saturating_sub(target_w)) / 2;
    let crop_y = (scaled_h.saturating_sub(target_h)) / 2;

    scaled.crop_imm(crop_x, crop_y, target_w, target_h).to_rgba8()
}
```

**Step 4: Run test to verify it passes**

Run: `cd src-tauri && cargo test compositor::tests::test_single_monitor_resizes_to_fit`
Expected: PASS

**Step 5: Write test — two monitors side by side**

```rust
    #[test]
    fn test_two_monitors_side_by_side() {
        let red = make_test_image(100, 100, 255, 0, 0);
        let blue = make_test_image(100, 100, 0, 0, 255);
        let monitors = vec![
            MonitorGeometry { x: 0, y: 0, width: 1920, height: 1080 },
            MonitorGeometry { x: 1920, y: 0, width: 1920, height: 1080 },
        ];
        let output = std::env::temp_dir().join("test_composite_dual.png");

        composite_wallpaper(&[red.clone(), blue.clone()], &monitors, &output).unwrap();

        let result = image::open(&output).unwrap();
        // Canvas should span both monitors
        assert_eq!(result.width(), 3840);
        assert_eq!(result.height(), 1080);

        // Left side should be reddish, right side bluish
        let left_pixel = result.get_pixel(100, 540);
        assert!(left_pixel[0] > 200); // red channel high
        let right_pixel = result.get_pixel(2920, 540);
        assert!(right_pixel[2] > 200); // blue channel high

        let _ = std::fs::remove_file(&red);
        let _ = std::fs::remove_file(&blue);
        let _ = std::fs::remove_file(&output);
    }
```

**Step 6: Run test**

Run: `cd src-tauri && cargo test compositor::tests::test_two_monitors_side_by_side`
Expected: PASS (the implementation already handles this)

**Step 7: Write test — monitors with vertical offset (stacked/offset layout)**

```rust
    #[test]
    fn test_monitors_with_offset() {
        let img = make_test_image(100, 100, 128, 128, 128);
        let monitors = vec![
            MonitorGeometry { x: 0, y: 0, width: 1920, height: 1080 },
            MonitorGeometry { x: 1920, y: -200, width: 2560, height: 1440 },
        ];
        let output = std::env::temp_dir().join("test_composite_offset.png");

        composite_wallpaper(&[img.clone(), img.clone()], &monitors, &output).unwrap();

        let result = image::open(&output).unwrap();
        // Canvas: x from 0 to 1920+2560=4480, y from -200 to max(1080, -200+1440=1240) = 1240
        // Total: 4480 x (1240 - (-200)) = 4480 x 1440
        assert_eq!(result.width(), 4480);
        assert_eq!(result.height(), 1440);

        let _ = std::fs::remove_file(&img);
        let _ = std::fs::remove_file(&output);
    }
```

**Step 8: Run test**

Run: `cd src-tauri && cargo test compositor::tests::test_monitors_with_offset`
Expected: PASS

**Step 9: Write test — empty inputs error**

```rust
    #[test]
    fn test_empty_monitors_returns_error() {
        let img = make_test_image(100, 100, 0, 0, 0);
        let output = std::env::temp_dir().join("test_composite_empty.png");
        let result = composite_wallpaper(&[img.clone()], &[], &output);
        assert!(result.is_err());
        let _ = std::fs::remove_file(&img);
    }

    #[test]
    fn test_empty_images_returns_error() {
        let monitors = vec![MonitorGeometry { x: 0, y: 0, width: 1920, height: 1080 }];
        let output = std::env::temp_dir().join("test_composite_no_img.png");
        let result = composite_wallpaper(&[], &monitors, &output);
        assert!(result.is_err());
    }
```

**Step 10: Run all compositor tests**

Run: `cd src-tauri && cargo test compositor::tests`
Expected: all pass

**Step 11: Commit**

```
feat(compositor): implement image compositing for multi-monitor wallpapers
```

---

## Task 3: Add `fetch_images_at_pages` to stash client

**Files:**
- Modify: `src-tauri/src/stash.rs`

Currently `fetch_image_at_page` fetches one image. We need a batch version that fetches N consecutive pages in one rotation tick.

**Step 1: Write the test for `select_next_batch`**

Actually, we need `rotation.rs` to return N results. Add a new method `select_next_batch` to `RotationState`:

**Files:**
- Modify: `src-tauri/src/rotation.rs`

```rust
    #[test]
    fn test_select_next_batch_returns_n_results() {
        let mut state = RotationState::new();
        let results = state.select_next_batch(RotationMode::Sequential, 10, 3);
        assert_eq!(results.len(), 3);
        assert_eq!(results[0].page, 1);
        assert_eq!(results[1].page, 2);
        assert_eq!(results[2].page, 3);
    }

    #[test]
    fn test_select_next_batch_single_is_same_as_select_next() {
        let mut state1 = RotationState::new();
        let mut state2 = RotationState::new();
        let batch = state1.select_next_batch(RotationMode::Sequential, 10, 1);
        let single = state2.select_next(RotationMode::Sequential, 10).unwrap();
        assert_eq!(batch[0], single);
    }

    #[test]
    fn test_select_next_batch_clamps_to_count() {
        let mut state = RotationState::new();
        // Ask for 5 but only 3 available
        let results = state.select_next_batch(RotationMode::Sequential, 3, 5);
        assert_eq!(results.len(), 3);
    }
```

**Step 2: Run tests to verify they fail**

Run: `cd src-tauri && cargo test rotation::tests::test_select_next_batch`
Expected: FAIL — method doesn't exist

**Step 3: Implement `select_next_batch`**

Add to `RotationState` impl in `src-tauri/src/rotation.rs`:

```rust
    /// Select N next pages. Returns at most `min(n, count)` results.
    /// Each call to this is equivalent to calling `select_next` n times.
    pub fn select_next_batch(
        &mut self,
        mode: RotationMode,
        count: usize,
        n: usize,
    ) -> Vec<RotationResult> {
        let take = n.min(count);
        (0..take)
            .filter_map(|_| self.select_next(mode, count))
            .collect()
    }
```

**Step 4: Run tests**

Run: `cd src-tauri && cargo test rotation::tests::test_select_next_batch`
Expected: PASS

**Step 5: Commit**

```
feat(rotation): add select_next_batch for multi-monitor rotation
```

---

## Task 4: Wire multi-monitor into the engine

**Files:**
- Modify: `src-tauri/src/engine.rs`
- Modify: `src-tauri/src/lib.rs` (update `detect_monitor_resolution` to return all monitors)

This is the core integration. The `rotate` function changes from "fetch 1 image, set wallpaper" to "detect monitors, fetch N images, composite if per_monitor, set wallpaper."

**Step 1: Update `detect_monitor_resolution` to `detect_monitors` — returns all monitors**

In `src-tauri/src/lib.rs`, replace the existing command:

```rust
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MonitorInfo {
    pub width: u32,
    pub height: u32,
    pub x: i32,
    pub y: i32,
    pub scale_factor: f64,
}

#[tauri::command]
async fn detect_monitors(app: tauri::AppHandle) -> Vec<MonitorInfo> {
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
```

Also keep the old `detect_monitor_resolution` for backward compat in the frontend (or update the frontend call — see Task 5).

Update the `invoke_handler` to register `detect_monitors`.

**Step 2: Modify `engine::rotate` to support per-monitor**

Replace the `rotate` function in `src-tauri/src/engine.rs`:

```rust
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
        // Composite and set
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

        // Use Span mode for composited wallpaper (it fills entire desktop)
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
        // Set to "spanned" picture-options for composited wallpaper
        let _ = std::process::Command::new("gsettings")
            .args(["set", "org.gnome.desktop.background", "picture-options", "spanned"])
            .output();
    }

    wallpaper::set_mode(wallpaper::Mode::Span)
        .map_err(|e| AppError::Wallpaper(e.to_string()))?;

    Ok(())
}
```

**Step 3: Update `download_image` to support multiple concurrent downloads**

The current `download_image` cleans up ALL old `wallpaper_*` files before downloading. With multi-monitor we download N images sequentially, so the cleanup would delete the first image before the second downloads. Fix: clean up old files once before downloading all images, and use indexed filenames.

In `src-tauri/src/stash.rs`, modify `download_image` to take an optional index:

Actually, simpler: just move the cleanup to the engine before the download loop. Change `download_image` to not clean up, and add a `clean_wallpaper_cache` helper:

In `src-tauri/src/stash.rs`:

```rust
/// Remove old wallpaper files from cache directory.
pub fn clean_wallpaper_cache(cache_dir: &Path) {
    if let Ok(entries) = std::fs::read_dir(cache_dir) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            if name.to_string_lossy().starts_with("wallpaper_") {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
}
```

And remove the cleanup block from `download_image`.

Then in `engine::rotate`, call `stash::clean_wallpaper_cache(&cache_dir);` before the download loop.

**Step 4: Verify it compiles**

Run: `cd src-tauri && cargo check`

**Step 5: Run all tests**

Run: `cd src-tauri && cargo test`
Expected: All 34 existing + new compositor + batch tests pass

**Step 6: Commit**

```
feat(engine): wire multi-monitor compositing into rotation loop
```

---

## Task 5: Update frontend — remove "(coming soon)", show monitor count

**Files:**
- Modify: `src/components/Settings.tsx`
- Modify: `src/lib/types.ts`

**Step 1: Update the frontend to call `detect_monitors` and show results**

In `Settings.tsx`:
- Replace `detect_monitor_resolution` call with `detect_monitors`
- Show monitor count when `per_monitor` is checked: "2 monitors detected (1920x1080 + 2560x1440)"
- Remove the "(coming soon)" text from the per_monitor checkbox

In `types.ts`, add:

```typescript
export interface MonitorInfo {
  width: number;
  height: number;
  x: number;
  y: number;
  scale_factor: number;
}
```

**Step 2: Update the Display section in Settings.tsx**

Replace the per_monitor checkbox area:

```tsx
<label className="flex items-center gap-2">
  <input
    type="checkbox"
    checked={settings.per_monitor}
    onChange={(e) => update("per_monitor", e.target.checked)}
    className="h-4 w-4 rounded border-zinc-600 bg-zinc-800 text-blue-500 focus:ring-blue-500"
  />
  <span className="text-sm text-zinc-300">
    Different wallpaper per monitor
  </span>
</label>
{monitors.length > 1 && settings.per_monitor && (
  <p className="text-xs text-zinc-500 ml-6">
    {monitors.length} monitors detected:{" "}
    {monitors.map((m) => `${m.width}x${m.height}`).join(" + ")}
  </p>
)}
{monitors.length <= 1 && settings.per_monitor && (
  <p className="text-xs text-zinc-500 ml-6">
    Only 1 monitor detected — per-monitor has no effect.
  </p>
)}
```

**Step 3: TypeScript check and frontend build**

Run: `npx tsc --noEmit && npm run build`

**Step 4: Commit**

```
feat(ui): show monitor info and remove per-monitor "coming soon" label
```

---

## Task 6: Manual testing and verification

**Step 1: Run all Rust tests**

Run: `cd src-tauri && cargo test`
Expected: All tests pass (34 existing + ~8 new)

**Step 2: Run clippy**

Run: `cd src-tauri && cargo clippy -- -D warnings`
Expected: Clean

**Step 3: TypeScript check**

Run: `npx tsc --noEmit`
Expected: Clean

**Step 4: Manual test with `cargo tauri dev`**

Run: `cargo tauri dev`
Test:
1. Open Settings, check "Different wallpaper per monitor"
2. Save settings
3. Observe: if you have multiple monitors, each should get a different image
4. If single monitor, behavior should be unchanged
5. Click "Next Wallpaper" — should rotate all monitors at once

**Step 5: Commit any fixes, then final commit**

```
feat: multi-monitor wallpaper support via image compositing

Closes #2
```
