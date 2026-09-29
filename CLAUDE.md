# StashPaper

System tray app that rotates desktop wallpapers from a Stash server's images: a Tauri v2 Rust backend in `src-tauri/` plus one React settings window, released for Linux, Windows and macOS and used day to day on Linux/GNOME.

## Commands
- Dev: `cargo tauri dev` (starts Vite and the app)
- Test: `cd src-tauri && cargo test`
- Lint: `cd src-tauri && cargo fmt --check && cargo clippy --all-targets -- -D warnings`, then `npx tsc --noEmit`
- Build: `npm run build` (frontend), `cargo tauri build` (bundles)
- Audit: `cd src-tauri && cargo audit`
- Smoke: `scripts/smoke.sh <binary>` launches the app with an empty profile (Linux: under `xvfb-run -a dbus-run-session --`)

Run them all before committing. CI runs tests, clippy and the frontend build on Linux, Windows and macOS, fmt, tsc, a version check and the smoke launch on Linux, and audits in a separate scheduled workflow. `cargo tauri build` runs only for release tags.

## Conventions that differ from defaults
- The version lives in `src-tauri/Cargo.toml` and `package.json` only; `tauri.conf.json` has none on purpose. CI fails when the two differ.
- Every Stash query goes through `build_variables()` in `stash.rs`, which merges pagination, the minimum-resolution filter and the random seed into the user's filter JSON. Don't assemble GraphQL variables anywhere else.
- `src/lib/types.ts` mirrors the Rust `Settings` struct by hand. Change both together.
- `Settings` is `#[serde(default)]` so old settings files keep loading. Add fields with defaults; don't rename or retype one without a migration.
- Settings are a plain JSON file in the app config dir (`~/.config/com.stashpaper.app/settings.json` on Linux), mode 600 because it holds the API key. Engine state (last rotation time, rotation position) is `state.json` in the app data dir, owned by `schedule.rs`.
- The tray menu is rebuilt from `TrayStatus` by `tray::refresh` whenever the engine's state changes. Change the menu there, not by holding menu item handles.
- `tauri-plugin-single-instance` must stay the first plugin registered in `lib.rs`.
- Log with the `log` macros (`tauri-plugin-log` writes a file); stderr is lost for Windows release builds and Finder-launched macOS apps.

## Pitfalls
- CI pins Rust (`toolchain:` in `ci.yml` and `release.yml`, kept equal). A newer local stable can flag lints CI doesn't know yet, and an older one misses lints CI enforces: `rustup update` when they disagree.
- GNOME dark mode reads `picture-uri-dark`, and the `wallpaper` crate only sets `picture-uri`: set both. Set `picture-options` explicitly too, or leaving per-monitor (spanned) mode keeps the spanned layout.
- Desktops cache wallpapers by path, so each download gets a unique timestamped filename. Reusing a path means the wallpaper doesn't visibly change.
- Delete old cache files only after the new wallpaper is set. Deleting first leaves the desktop pointing at a missing file whenever a rotation fails.
- Stash serves `paths.image` as the raw file: an image clip is a video, and a missing file is a plain-text error body. `download_image` checks the status, the content type (when sent) and that the bytes decode as an image before anything touches the desktop.
- The query filter fails closed (`parse_query_filter`): an unknown key or bad JSON is an error, never an empty filter, because an empty filter rotates through the whole library.
- GNOME has no per-monitor wallpapers. Per-monitor mode composites the images onto one canvas and sets it as `spanned`.
- WebKitGTK renders `<select>` options unreadable unless both the select and each option get explicit `color` and `backgroundColor` styles.
- Linux tray: the app panics at startup without `libayatana-appindicator3` (or `libappindicator3`). Tray tooltips and left-click events don't exist on Linux, so status goes in the menu's first line, not only the tooltip.
- Timers use `tokio::time::sleep`, whose clock stops while the machine is suspended. The engine sleeps at most a minute at a time and compares wall-clock time, so suspended time counts toward the interval.
- `default_window_icon()` returns a borrowed image. Use `Image::new_owned` to keep one in managed state.
