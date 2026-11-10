# Architecture

## Overview
WayFinder is a Tauri v2 desktop app. A vanilla JS frontend runs in the system
webview (WebView2 on Windows) and a Rust backend does the heavy lifting.
Vite serves the frontend in dev.

## Project layout
```
package.json, vite.config.mjs   npm scripts + Vite config
frontend/                       UI (no framework)
  index.html                    markup, search bar, results list, error box
  script.js                     all UI logic: commands, rendering, keyboard
  style.css                     pixel/lime theme (CSS variables in :root)
  fonts/                        Silkscreen + VT323, bundled (works offline)
src-tauri/                      Rust backend
  Cargo.toml                    crate name: way_finder
  tauri.conf.json               window + bundle settings (the real one)
  build.rs                      Tauri build script (the real one)
  icons/                        app icons
  capabilities/default.json     Tauri permissions
  images/                       sample images for converter testing
  src/
    main.rs                     entry point, plugins, hotkey, commands
    config.rs                   config.json load/save, defaults, versioning
    search_engine/
      crawler.rs                walks the disk and fills the index in batches
      database.rs               SQLite open, schema, inserts, upserts
      reconcile.rs              startup reconcile and cheap 3-minute fallback
      search.rs                 ranked search (prefix, contains, fuzzy, recency)
      watcher.rs                debounced real-time filesystem watcher
    converter/
      mod.rs                    convert_file dispatch and path resolution
      registry.rs               converter table (ConverterEntry slice)
      image_conv.rs             pure-Rust image conversion (image crate)
```

App data folder (config + index): `%APPDATA%\com.wayfinder.app\` (Tauri's
app data dir, named after the identifier in tauri.conf.json).
Files there: `config.json`, `index.db` (plus `index.db-wal` / `-shm`).

## Window
Borderless, transparent, no shadow, always on top, hidden from the taskbar,
hidden at start, not resizable. Size 720 × 560.
`shadow: false` is required, otherwise Windows draws a white outline. Do not
change these settings without asking.

**The window is not full-screen.** A full-screen transparent overlay keeps the
GPU compositing the entire desktop every frame, which costs significant GPU and
RAM on low-end laptops. A fixed 720 × 560 window is composited only in that
rectangle.

**Click-through prevention.** Fully transparent pixels in a WebView2 window
pass mouse clicks to whatever is behind them. `html` has `background:
rgba(0,0,0,0.01)` — invisible but technically opaque, so those pixels are no
longer click-through. `body` stays transparent so only the dark launcher panel
is visible.

**Hide on focus loss.** When `config.hide_on_blur` is `true` (the default),
`main.rs` registers a `WindowEvent::Focused(false)` handler that calls
`window.hide()`. This covers every click outside the window (desktop or another
app) even though those clicks never reach the page. A 150 ms guard ignores the
spurious blur that some Windows builds fire immediately after `window.show()`.
To disable during debugging (e.g. when devtools steal focus), set
`"hide_on_blur": false` in `config.json`.

## Command flow (frontend)
1. User types a prefix (`fil:`, `fol:`, `con:`, `bow:`) or picks a mode from
   the chip menu. `setMode()` switches the active command.
2. Each command in the `COMMANDS` table in script.js has a `search(query)`
   function that returns result items `{ tag, title, sub, run }` (or a
   Promise of them).
3. Typing calls `scheduleUpdate()`: wait 80 ms, then `update()` searches.
   Each search gets a ticket number; if a newer search started, the old
   answer is dropped. At most 5 items are shown.
4. `render()` draws the items; Enter calls the selected item's `run()`.

Adding a command = adding one entry to `COMMANDS` (see workflows.md).

## What is real vs. stubbed
| Feature       | State                                                     |
|---------------|-----------------------------------------------------------|
| #fil / #fol   | Real. Rust index + ranked search through Tauri commands.  |
| #con (units)  | Working, pure JS (UNITS / TEMPS tables).                  |
| #con (files)  | Real for images (pure Rust `image` crate: PNG, JPG, WebP, |
|               | BMP, TIFF). Audio & doc-to-pdf deferred to Phase 6 sidecars.|
| #bow          | Real. opener plugin `openUrl` opens DuckDuckGo.           |
| Ctrl+Space    | Real global hotkey from Rust. Reads `config.hotkey`.      |
| Config        | Loads with defaults. Errors only print to the console.    |
| Index refresh | Real. notify-debouncer-mini watcher + startup reconcile   |
|               | + 3-minute lightweight fallback (Phase 5).                |
| `app:`, `spo:`| On hold. Not started.                                     |

## Data flow
Frontend --(Tauri invoke)--> Rust command --> search_engine / converter --> SQLite / Disk
- The frontend never touches the filesystem directly.
- Rust returns plain-English error strings; the frontend shows them in an
  error box.
- Commands: `search_files`, `search_folders`, `index_ready`, `hide_window`,
  `open_path`, `convert_file`.

## Threading and database connections
- A plain Tauri command runs on the main (UI) thread and freezes the window
  while it works. Commands that touch disk or the database use
  `#[tauri::command(async)]`.
- Dedicated connections to `index.db`, all opened with `database::open()`:
  1. The app's connection (in `Db` state), used by searches.
  2. The crawler's connection, created inside `run_crawl`.
  3. The watcher's connection, created inside `run_watcher`.
  4. The reconcile connection, created inside `run_startup_reconcile` and fallback.
  WAL mode lets background tasks write while searches read. Never share one
  connection across threads.
- The first crawl commits in batches of 500 rows, so search results become
  available immediately and `index_ready` turns true early.

## Indexing design

### 1. First run
- DB lives in the app data directory. `create_table()` checks
  `PRAGMA user_version`. If it is lower than `SCHEMA_VERSION` (now 2), the
  table is dropped and recreated, which triggers a fresh crawl. The index is
  a cache of the disk, so rebuilding is fine.
- If the table is empty, the crawler runs on a background thread.
- Crawler inserts happen inside transactions committed in batches of 500
  entries with cached prepared statements.

### 2. Keeping it fresh (Phase 5)
1. **Primary**: `notify-debouncer-mini` watches the indexed folders on a
   background thread. A 500 ms debounce window aggregates burst writes from
   editors or downloaders. On change, the exact filesystem state is inspected:
   existing paths are upserted into SQLite; deleted paths (and deleted folder
   prefixes) are dropped.
2. **Startup reconcile**: on launch, if the index exists, a background task
   compares disk against the DB, deleting missing paths and indexing any new or
   modified files added while the app was closed.
3. **Cheap fallback**: checks every 3 minutes. Compares each folder's
   filesystem modified time to its stored timestamp. Only re-scans folders whose
   timestamp changed. Never does a full re-walk on the timer.

### 3. Index scope (current defaults)
- Root: the current user's home folder, resolved at runtime with the `dirs`
  crate. Never hardcoded. Configurable through `config.json` (`index_roots`).
- Skip hidden files and folders (Windows hidden attribute).
- Never follow symlinks or junctions (`follow_links(false)`).
- Do NOT blanket-skip reparse points. OneDrive placeholder files use that
  attribute, and the owner's files live in OneDrive. Test junction handling
  with a test folder before adding any extra check.
- Excludes from config (defaults: `node_modules`, `target`, `.git`).

### 4. Schema (version 3)
`files(id, name, name_lower, path UNIQUE, parent, extension, is_dir, created_at,
modified_at)` with indexes on `(is_dir, name_lower)` and `(parent)`. Timestamps
are Unix seconds. `parent` enables instant direct-child queries for deletion
detection in the fallback loop. `is_dir` separates #fil from #fol. `name_lower`
is the lowercase name used for fast, case-insensitive prefix search.

## Search and ranking (search.rs)
1. Prefix query: `name_lower >= q AND name_lower < q + U+10FFFF`, ordered by
   `name_lower`, `LIMIT 50`. Uses the index. An exact name sorts first, so it
   is always fetched.
2. If fewer than 5 candidates: "contains" query (`LIKE %q%` with `%`, `_`, `\`
   escaped), `LIMIT 200`. Scans the table.
3. If still fewer than 5 candidates: fuzzy sequence query (`LIKE %c1%c2%c3%`),
   `LIMIT 50`, evaluated with `fuzzy-matcher` (`SkimMatcherV2`).
4. Rust scores candidates (1-100) + recency bonus (+5 for <24h, +2 for <7d).
   Ties are broken by shorter name. Returns top 5 results.

| Match type                        | Base Score |
|-----------------------------------|------------|
| Exact name (case-insensitive)     | 100        |
| Exact name without extension      | 90         |
| Name starts with query            | 80         |
| A word in the name starts with it | 70         |
| Name contains query               | 60         |
| Fuzzy match (SkimMatcherV2)       | 1-50       |
Recency bonus: +5 if modified within last 24h, +2 if modified within last 7 days.

## Opening files and links
- Files and folders open through the opener plugin's Rust API in the
  `open_path` command. Never through `cmd`, a shell string or `start`.
- Web links open through the opener plugin's JS `openUrl`.

## File conversion design
- Registry: each conversion is one small entry in `CONVERTERS` (input extensions,
  output extensions, tool label, converter function). Adding a conversion
  never modifies an if/else chain.
- Pure-Rust image prototype: uses `image` crate with default features OFF
  (only PNG, JPEG, WebP, BMP, TIFF enabled to prevent crate bloat).
- Path resolution: accepts absolute paths, relative paths, or resolves bare
  file names against the SQLite index. Destination is saved in the same folder.
- External tools: audio, video and doc-to-pdf require external engines (e.g.
  bundled LGPL ffmpeg sidecar in Phase 6). Pure Rust crates for lossy audio
  encoding and doc-to-pdf layout do not exist without bundling heavy runtimes.

## Configuration
- All user options live in ONE human-readable `config.json` in the app data
  directory: `version`, `hotkey`, `index_roots`, `exclude_dirs`,
  `hide_on_blur` today; theme and per-file-type shortcuts later.
- The launcher works fully without the settings app. Missing or broken config
  falls back to defaults. Showing an error box for it is still to do.
- The launcher will reload config when the file changes (reuses `notify`).
- The settings app (future) is a separate program that only edits the config
  file. A small shared crate (cargo workspace) will hold the Config struct.

## Known issues (remaining)
- Config load errors are only printed to the console.
- Junction behavior of `walkdir` on Windows is untested.
