# StashPaper

Desktop system tray app that rotates wallpapers from a Stash media server. Tauri v2 (Rust backend) + React frontend.

## Tech Stack

- **Frontend**: React 19, Vite 7, Tailwind CSS 4, TypeScript 5.8
- **Backend**: Tauri v2 (Rust), Tokio, Reqwest, `wallpaper` crate
- **Stash API**: GraphQL via `findImages` query with `filter` + `image_filter` variables
- **Storage**: JSON settings file in OS app config directory (`~/.config/stashpaper/`)

## Quick Reference

```bash
# Dev (starts both Vite + Tauri)
cargo tauri dev

# Build
cargo tauri build

# Rust tests
cd src-tauri && cargo test

# Type check frontend
npx tsc --noEmit

# Frontend build only
npm run build
```

## Architecture

```
┌─────────────────────────────────┐
│ System Tray (Tauri)             │
│ Next | Pause | Settings | Quit  │
└────────┬────────────────────────┘
         │
┌────────▼────────────────────────┐
│ Rotation Engine (engine.rs)     │
│ Timer loop + command channel    │
│ Commands: Next/Pause/Resume/Quit│
└────────┬────────────────────────┘
         │
┌────────▼────────────────────────┐     ┌───────────────────────┐
│ Rotation State (rotation.rs)    │     │ Settings (settings.rs)│
│ Random / Sequential / Shuffle   │     │ JSON persistence      │
└────────┬────────────────────────┘     │ 0o600 file perms      │
         │                              └───────────────────────┘
┌────────▼────────────────────────┐
│ Stash Client (stash.rs)         │
│ GraphQL: findImages query       │
│ Auth: ApiKey header             │
│ Download → cache dir            │
└─────────────────────────────────┘
         │
┌────────▼────────────────────────┐
│ wallpaper crate                 │
│ + GNOME dark mode gsettings     │
└─────────────────────────────────┘
```

### Settings Window (React)

Single-window settings UI at 520x680, shown on first run or tray click. Sections: Stash Connection, Query Filter (JSON), Rotation, Display. Window hides to tray on close.

### Stash GraphQL Integration

Query pattern: `findImages(filter: $filter, image_filter: $image_filter)` where user provides the full JSON for both filter objects. The engine merges in `per_page: 1` and `page: N` for pagination. Auth via `ApiKey` header.

### Platform Notes

- **GNOME/Linux**: Dark mode requires setting both `picture-uri` and `picture-uri-dark` via gsettings
- **WebKitGTK**: Dropdown `<select>` elements need explicit styling to be readable
- Cache files use timestamp-based unique filenames for cache busting

## Key Files

| File | Purpose |
|------|---------|
| `src-tauri/src/lib.rs` | Tauri setup, command registration, system tray, window management |
| `src-tauri/src/engine.rs` | Rotation timer loop, command handling, wallpaper setting |
| `src-tauri/src/rotation.rs` | Random/Sequential/Shuffle state machine |
| `src-tauri/src/stash.rs` | GraphQL client, image download, variables builder |
| `src-tauri/src/settings.rs` | Settings struct, JSON persistence, validation |
| `src-tauri/src/error.rs` | Custom error type (Stash/Wallpaper/Settings) |
| `src/components/Settings.tsx` | Settings UI panel (connection, filter, rotation, display) |
| `src/lib/types.ts` | TypeScript types mirroring Rust settings |

## Skill Directory

| Area | Skill | What it covers |
|------|-------|----------------|
| **Stash API** | `stash` | GraphQL API, plugin system, scraper system |
| **Stash-Box** | `stash-box` | StashDB metadata API, edit/voting workflow, fingerprints |
| **GraphQL** | `graphql-patterns` | Query patterns, codegen, Stash ecosystem |
| **UI styling** | `tailwind-css-patterns` | Utility-first patterns, responsive design |
| **Frontend** | `frontend-design` | Component design, polish, avoiding generic AI aesthetics |
| **Git workflow** | `git-preferences` | Commit conventions, branching, PR workflow |
| **Testing (Rust)** | — | `cargo test` in src-tauri; unit tests for rotation, stash, settings |

## Development Lifecycle

### Working on changes

1. **Orient** → Read CLAUDE.md, check git status, check brain for prior context
2. **Plan** → For non-trivial work, use `superpowers:writing-plans` or `EnterPlanMode`
3. **Implement** → Write code, run `cargo test` and `npx tsc --noEmit` frequently
4. **Verify** → `superpowers:verification-before-completion` (evidence before claims)
5. **Review** → `superpowers:requesting-code-review` for self-review
6. **Complete** → `superpowers:finishing-a-development-branch`

### Lifecycle Gates

**Before committing:**
- Rust tests pass: `cd src-tauri && cargo test`
- TypeScript compiles: `npx tsc --noEmit`
- Frontend builds: `npm run build`

**Before creating a PR:**
- All above gates pass
- Self-review completed
- Commit messages follow conventional format (feat/fix/docs/refactor/chore)
