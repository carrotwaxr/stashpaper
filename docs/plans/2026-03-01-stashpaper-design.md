# StashPaper — Design Document

**Date:** 2026-03-01
**Status:** Approved

## Overview

Desktop system tray app that queries a Stash instance via GraphQL to fetch images and rotate them as desktop wallpapers. Built with Tauri v2 (Rust backend + React/Vite/Tailwind frontend). Targets macOS, Linux, and Windows.

## Architecture

**Core loop:**

1. On interval tick, execute the user's `findImages` GraphQL query against their Stash instance
2. Select an image from results based on rotation mode (random / sequential / shuffle-no-repeat)
3. Download the image via Stash's API
4. Set it as wallpaper via OS-native APIs (per-monitor if configured)
5. On failure (network down, Stash unreachable), keep current wallpaper and retry next tick

**Tech stack:**

- Tauri v2 — app shell, system tray, settings storage, OS integration
- React 19 + Vite + Tailwind CSS — settings/config UI (minimal, only shown on demand)
- Rust backend — wallpaper setting, image downloading, rotation scheduling, network checks
- `tauri-plugin-store` — persisted JSON config
- `wallpaper` crate — cross-platform wallpaper setting (macOS/Linux/Windows)

## Configuration & Settings

### Stash Connection

- Stash server URL (e.g. `http://localhost:9999`)
- API key (masked input)
- "Test Connection" button — validates via a simple GraphQL query

### Image Query

- Text area for a `findImages` GraphQL query snippet
- Ships with a sensible default query
- User customizes by crafting queries in Stash's GraphQL Playground

### Rotation Settings

- **Mode:** Random / Sequential / Shuffle (no repeat) — dropdown
- **Interval presets:** 5min, 15min, 30min, 1hr, 4hr, daily — dropdown
- **Wi-Fi only:** Toggle to pause rotation when not on Wi-Fi

### Display Settings

- **Mode:** Same wallpaper on all monitors / Different wallpaper per monitor
- Detected monitors listed (informational)

### Persistence

All settings persisted via `tauri-plugin-store` to JSON in the app's config directory. Changes take effect immediately (rotation timer restarts on save).

## System Tray

**Tray menu (right-click):**

- **Next Wallpaper** — skip to next image immediately
- **Pause / Resume** — toggle rotation
- **Settings** — opens the config panel window
- **Quit**

## Runtime Behavior

- Launches minimized to system tray (no window on startup after initial config)
- Rotation timer runs in the Rust backend
- Re-queries Stash each rotation tick to pick up library changes
- If Stash is unreachable, keeps current wallpaper silently, retries next tick
- Wi-Fi check runs before each fetch if the toggle is enabled

## Multi-Monitor

- User-configurable: same wallpaper on all monitors or different per monitor
- Uses `wallpaper` crate's per-monitor support
- Detects available monitors and displays them in settings

## Future Considerations (not in scope for v1)

- Mobile companion app (Android wallpaper setting, iOS browse-only)
- Tauri auto-updater integration
- Launch at login
