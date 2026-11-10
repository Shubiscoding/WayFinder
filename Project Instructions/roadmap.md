# Roadmap

Work in this order. Do not start a later phase unless the owner asks.
Tick items off as they are finished. The owner edits this list freely.

## Phase 0: Immediate fixes (tonight) — see fix.md
- [ ] Fix 0: git snapshot
- [x] Fix 1: crawler uses its own connection (WAL)
- [x] Fix 2: tiered search, `name_lower` column, schema version 2, async
      commands, hotkey read from config (new `main.rs`)
- [x] Fix 3: debounce + stale-result guard in script.js
- [x] Fix 4: safe `open_path` (no `cmd`)
- [x] Fix 5: `#bow` works again (`openUrl`)
- [x] Fix 6: max 5 suggestions, hidden scrollbars, window height 560
- [x] Fix 7: Cargo.toml cleanup (remove tokio, async-recursion; `image` off)
- [x] Fix 8: delete stale files from `src-tauri/src/`
- [x] Bug 1: chip menu dropdown clipping (scaleX animation, z-index: 20)
- [x] Bug 2: click outside & blur hide (hide_on_blur, 150ms guard, near-invisible html background)
- [ ] Test checklist in fix.md all pass

## Phase 1: Make search real (done, apart from Phase 0)
- [x] Move `build.rs`, `tauri.conf.json`, `icons/` to `src-tauri/` (real
      copies exist; stale copies deleted by Fix 8)
- [x] Crawler: runtime home folder, skips errors, skips hidden (Windows
      attribute), one transaction, no `unwrap()`
- [x] Database init in the app data directory, `is_dir` column, name index
- [x] Tauri commands so #fil and #fol call Rust
- [x] "Indexing..." state in the UI during the first crawl
- [x] Remove old `web_search.rs` and `search.rs` leftovers (Fix 8 + Fix 2)
- [ ] Test how junctions are reported on Windows (test folder; no blanket
      reparse-point skip because of OneDrive)

## Phase 2: Global hotkey (done)
- [x] Ctrl+Space registered system-wide with the global-shortcut plugin
- [x] Show/hide the window from the hotkey
- [x] Read the hotkey from config (Fix 2)

## Phase 3: Errors and config
- [x] Shared `showError()` and an error box in the UI
- [x] Config struct + config file with defaults and a `version`
- [x] Root path and excludes read from config
- [x] Hotkey read from config (Fix 2)
- [x] Pixel fonts bundled locally (no network)
- [ ] Show config load errors in the error box (currently console only)

## Phase 4: Conversion prototype (crates only)
- [x] Converter registry (one entry per conversion)
- [x] Image to image with the `image` crate (default features OFF)
- [x] Wire `#con` file conversion to Rust, with error boxes
- [x] Decide on audio and doc-to-pdf (crate research first)

## Phase 5: Fresh index, then better ranking
- [x] `notify` watcher with debouncing (`notify-debouncer-mini`)
- [x] Startup reconcile (disk vs DB)
- [x] 3-minute cheap fallback (only changed folders)
- [x] Basic ranking tiers: exact, no-extension, prefix, word, contains
      (becomes real in Fix 2)
- [x] Fuzzy matching with `fuzzy-matcher` (`SkimMatcherV2`)
- [x] Frequency / recency bonus
- [x] Optional: partial results during the first crawl (commit in batches)

## Phase 5.5: Release features (quit, autostart, settings)

### 5.5.0 Prep (gate: do not start the features until this is done)
- [ ] Git snapshot (Fix 0), then create `feature/phase-5-5`
### 5.5.1 Logging to a file (needed to debug everything below)
- [ ] Add the log plugin; replace `eprintln!` with `log::` macros
- [ ] Done when: a release build writes a log file in the app log folder
### 5.5.2 One show path, then quit + tray
- [ ] `show_launcher(app)` helper: records `last_shown`, shows, focuses,
      emits `launcher-shown`. Hotkey, tray and single-instance all call it
- [ ] `quit_app` command (`app.exit(0)`), registered in `main.rs`
- [ ] Tray icon (`tray-icon` feature): Show, Quit (Settings added in 5.5.5)
- [ ] `sys:` command with "Quit WayFinder" (decide: type `sys: quit`, or
      show the actions on an empty query)
- [ ] Done when: tray Quit and `sys: quit` both end the process, including
      the WebView2 child processes, and the hotkey is free again
### 5.5.3 Single instance
- [ ] Single-instance plugin, registered FIRST; its callback calls
      `show_launcher`
- [ ] Done when: starting the exe twice leaves one process tree, one watcher
      and one crawler, and the second start shows the first launcher
### 5.5.4 Autostart (opt-in)
- [ ] `autostart: bool` in `Config`, default `false` (serde default, so no
      version bump; note it in decisions.md)
- [ ] Autostart plugin with an `--autostart` argument; at startup make the
      OS entry match the config, and log failures
- [ ] On an autostart launch only: delay background reconcile about 30 s
- [ ] Done when: after sign-out/in, the app is running and hidden, the hotkey
      works, there is no console window, and turning it off removes the entry
### 5.5.5 Settings page (read-only v1)
- [ ] `get_config` command: returns the exact text of `config.json`, its
      path, and any parse error
- [ ] `open_config` command (the index-only `open_path` rejects it)
- [ ] Modal panel in the UI (same pattern as the error box, not result rows,
      because of `MAX_ITEMS = 5`): path, the JSON text, Open file, Close,
      "restart to apply changes"
- [ ] Opens from `sys: settings` and from a tray "Settings" item
- [ ] Done when: broken JSON still shows its text plus the error, Esc closes,
      clicking inside stays open, no scrollbars
### 5.5.6 Close-out
- [ ] Add checklist items 17–21 (quit, tray, single instance, autostart,
      settings); update item 10 (the chip menu now has 5 entries)
- [ ] Update architecture.md, decisions.md, the AGENTS.md command table
- [ ] Review `capabilities/default.json`
- [ ] Real `npm run tauri build`, tested on the installed build
- [ ] Merge `feature/phase-5-5` into `updates`
## Phase 6: Release and growth (on hold)
- [ ] Git set up, first release merged `updates` into `main`
- [ ] Release profile in Cargo.toml (size optimization)
- [ ] Bundled ffmpeg sidecar (LGPL, minimal build) for video
- [ ] Installer with WebView2 handling
- [ ] Themes (CSS variables in the config file)
- [ ] Settings app (separate program, edits the config file)
- [ ] Per-file-type shortcuts
- [ ] Clipboard history
- [ ] Dev container / CI for contributors (optional)

## Ideas parking lot (on hold, do not start)
- `app:` prefix: read Start Menu shortcuts (user + all-users folders), start
  with `.lnk` files; Windows Store apps later. Not an `.exe` boost in `#fil`.
- `spo:` v1: open Spotify with a `spotify:search:` style link (test it), no
  login. Later, optional login with PKCE and the user's own Client ID
  (needs Premium and the 5-user dev-mode limit applies). Tokens in Windows
  Credential Manager, never in `config.json`.
