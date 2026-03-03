# StashPaper

Desktop system tray app that automatically rotates wallpapers from a [Stash](https://stashapp.cc/) media server.

Built with [Tauri v2](https://v2.tauri.app/) (Rust backend) and React.

## Features

- **Wallpaper rotation** from your Stash image library with configurable intervals
- **Rotation modes**: Random (seeded, no repeats), Sequential, Shuffle
- **Query filtering**: Full control over which images are selected using Stash's GraphQL filter syntax
- **Minimum resolution filtering**: Only use images above 720p, 1080p, 1440p, or 4K
- **Test query**: Preview how many images match your filter before saving
- **System tray**: Next wallpaper, Pause/Resume, Settings, Quit
- **Error indication**: Tray icon changes on errors with tooltip showing the issue
- **Cross-platform**: Linux, Windows, macOS

## Install

### Download

Grab the latest release for your platform from the [Releases](https://github.com/carrotwaxr/stashpaper/releases) page.

| Platform | Format | Notes |
|----------|--------|-------|
| **Linux** (Debian/Ubuntu) | `.deb` | Dependencies installed automatically via apt |
| **Linux** (other) | `AppImage` | Self-contained, no install needed |
| **Windows** | `.exe` (NSIS) | WebView2 auto-installed if missing (pre-installed on Win 10/11) |
| **macOS** | `.dmg` | See [macOS note](#macos) below |

### Linux

**Debian/Ubuntu** (recommended):

```bash
sudo dpkg -i stashpaper_*.deb
sudo apt-get install -f  # install any missing dependencies
```

**AppImage**:

```bash
chmod +x StashPaper_*.AppImage
./StashPaper_*.AppImage
```

**Required system libraries** (installed automatically by the `.deb`, manual install for AppImage if needed):

```bash
sudo apt install libwebkit2gtk-4.1-0 libgtk-3-0 libappindicator3-1
```

### Windows

Run the `.exe` installer. WebView2 is required but comes pre-installed on Windows 10 and 11. The installer will download it automatically if missing.

### macOS

Open the `.dmg` and drag StashPaper to Applications.

> **Note:** StashPaper is not signed with an Apple Developer certificate. macOS will block the first launch with "app is damaged." To open it: right-click the app > Open > Open. You only need to do this once.

## Setup

1. Launch StashPaper — it starts in the system tray
2. Click the tray icon > **Settings**
3. Enter your **Stash server URL** (e.g., `http://localhost:9999`) and **API key**
4. Click **Test Connection** to verify
5. Optionally add a **Query Filter** to select specific images (see below)
6. Click **Test Query** to see how many images match
7. **Save Settings** and wallpapers will start rotating

### Query Filter

The query filter lets you control which images StashPaper pulls from Stash. It accepts JSON with `filter` and/or `image_filter` keys matching the Stash `findImages` GraphQL query.

**Examples:**

Only images tagged "wallpaper":

```json
{
  "image_filter": {
    "tags": {
      "value": ["wallpaper"],
      "modifier": "INCLUDES_ALL"
    }
  }
}
```

Only images rated 4+, sorted by rating:

```json
{
  "filter": {
    "sort": "rating",
    "direction": "DESC"
  },
  "image_filter": {
    "rating100": {
      "value": 80,
      "modifier": "GREATER_THAN"
    }
  }
}
```

StashPaper automatically handles pagination. If using Random rotation mode, it injects a seeded random sort to avoid repeats.

## Build from Source

### Prerequisites

- [Node.js](https://nodejs.org/) (LTS)
- [Rust](https://rustup.rs/) (stable)
- [Tauri CLI](https://v2.tauri.app/start/prerequisites/): `cargo install tauri-cli --version "^2"`

**Linux additional dependencies:**

```bash
sudo apt install libwebkit2gtk-4.1-dev build-essential curl wget file \
  libxdo-dev libssl-dev libayatana-appindicator3-dev librsvg2-dev
```

### Build

```bash
npm install
cargo tauri build
```

Output binaries are in `src-tauri/target/release/bundle/`.

### Development

```bash
npm install
cargo tauri dev
```

## Configuration

Settings are stored in your OS config directory:

| OS | Path |
|----|------|
| Linux | `~/.config/stashpaper/settings.json` |
| Windows | `%APPDATA%\com.stashpaper.app\settings.json` |
| macOS | `~/Library/Application Support/com.stashpaper.app/settings.json` |

The settings file has restrictive permissions (owner-only read/write) since it contains your Stash API key.

## License

[MIT](LICENSE)
