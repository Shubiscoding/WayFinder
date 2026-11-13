<div align="center">

# WayFinder

**Find anything on your PC in a keystroke.**

A fast, keyboard-first launcher for Windows. Press a hotkey, type a few letters, and jump straight to the file, folder, app, web search or conversion you need.

![Platform: Windows](https://img.shields.io/badge/platform-Windows-0078D6)
![Built with Tauri v2](https://img.shields.io/badge/built%20with-Tauri%20v2-24C8D8)
![Backend: Rust](https://img.shields.io/badge/backend-Rust-DEA584)
![License: MIT](https://img.shields.io/badge/license-MIT-b8ff2c)
![Latest release](https://img.shields.io/github/v/release/Shubiscoding/WayFinder)

<!-- TODO: hero screenshot (compact launcher, nothing typed yet) -->
<img src="docs/screenshots/hero.png" alt="WayFinder launcher" width="640">

</div>

---

## What's new in v2

Version 2 is the biggest update so far. The full list is in the [changelog](CHANGELOG.md).

- **`app:` opens your installed programs.** Type a few letters and launch anything from your Start Menu. Apps you use often rise to the top.
- **Drag a result out to copy it.** Drag a file or folder from the launcher onto the Desktop or into an Explorer window and WayFinder makes a copy. The original is never moved.
- **Much faster startup.** On the author's PC, re-checking an index of about 53,000 items dropped from roughly 88 seconds to about 2.6 seconds, and a brand-new index takes about 7 seconds.
- **A cleaner index.** Hidden dot-folders such as `.cargo` and `.rustup`, and junk file types such as `.dll`, are skipped by default, so results are less noisy. Both are configurable.
- **Lives in the tray.** Tray icon, `sys:` commands, a Settings view, single-instance protection, and optional start with Windows.
- **A loading screen with live progress** while the index is checked at startup.
- **Image conversion only.** Unit conversion (`con: 10 km to mi`) has been removed. `con:` now focuses on converting images.

**Upgrading from v1:** the file index rebuilds once on first launch, and your `app:` launch counts start fresh. Your `config.json` keeps working, and the new options get sensible defaults.

---

## Why WayFinder?

Windows search is slow to open and slow to answer. WayFinder lives in the background, keeps its own small index of your files, and answers while you type. Everything stays on your machine: no account, no cloud, no telemetry.

## Features

- **Instant file and folder search.** Type `fil:` or `fol:` followed by a name. Results are ranked: exact name first, then names that start with your text, then word matches, then partial and fuzzy matches. Files you changed recently get a small boost.
- **Open any installed app.** `app:` finds Start Menu shortcuts and ranks them by how well the name matches and how often you launch them.
- **Drag out to copy.** Drag a `fil:` or `fol:` result to the Desktop or an Explorer folder to copy it there.
- **Always up to date.** A background watcher notices new, renamed and deleted files within a second or so. A quick check at startup and a periodic safety scan catch anything that changed while the app was closed.
- **Web search in one step.** `bow:` opens your search in your default browser (DuckDuckGo).
- **Image conversion.** `con:` converts image files, for example `Lemon.png to webp`.
- **Global hotkey.** `Ctrl+Space` shows and hides the launcher from anywhere. Change it in the config file.
- **Lives in the tray.** Quit from the tray icon or with `sys: quit`. Optional autostart with Windows (off by default) and a Settings view are included.
- **Light on resources.** A small window, a local SQLite index and a Rust backend. A short loading screen shows while the index is checked at startup, and after that searching never freezes the window.
- **A look of its own.** A pixel-font terminal style with bundled fonts, so it works fully offline.

<!-- TODO: results screenshot (for example, `fil:` with five results) -->
<p align="center">
  <img src="docs/screenshots/search.png" alt="Searching for files" width="640">
</p>

## Install

1. Go to the [Releases page](https://github.com/Shubiscoding/WayFinder/releases/latest).
2. Download the latest `WayFinder_x.y.z_x64-setup.exe` <!-- TODO: match the real installer name -->
3. Run the installer, then press **Ctrl+Space**.

**Requirements:** Windows 10 or 11. WayFinder uses Microsoft's WebView2 runtime. It is already part of Windows 11 and up-to-date Windows 10, and the installer takes care of it if it is missing.

> **First launch:** WayFinder builds its index while a loading screen shows live progress. On the author's PC about 53,000 items took around 7 seconds. A large drive can take a minute or more. Later starts only re-check what changed and take a few seconds.

> **Windows SmartScreen:** WayFinder is not code-signed, so Windows may show a "Windows protected your PC" warning. Choose **More info**, then **Run anyway**.

## Using WayFinder

Press **Ctrl+Space**, then type a command prefix followed by your query. You can also pick a command from the dropdown on the left of the search bar, where each command appears as `#fil`, `#app` and so on.

| Prefix | What it does | Example |
|---|---|---|
| `fil:` | Search files | `fil: invoice` |
| `fol:` | Search folders | `fol: downloads` |
| `app:` | Open an installed app | `app: calculator` |
| `con:` | Convert image files | `con: Lemon.png to webp` |
| `bow:` | Search the web in your default browser | `bow: rust lifetimes` |
| `sys:` | System actions: Settings, Quit WayFinder | `sys: settings` |

### Keys

| Key | Action |
|---|---|
| `↑` `↓` | Move through results |
| `Enter` | Open the selected result |
| `Tab` | Switch command (in `con:`, pick a suggestion or jump between the two fields) |
| `Alt` + `↓` | Open the command dropdown |
| `Esc` | Clear the search, then close the launcher |

Results open with the program Windows normally uses for that file type, exactly like a double-click.

### Copy by dragging

Press and hold the mouse on a `fil:` or `fol:` result, then drag it onto the Desktop or into an Explorer window. WayFinder makes a **copy**, even when the destination is on the same drive. The original stays where it is. If the destination already has a file with that name, Windows asks what to do.

Press `Esc` while dragging to cancel. After a successful drop, the launcher hides itself.

### Converting images

- **Formats:** PNG, JPEG (`.jpg`, `.jpeg`, `.jfif`), WebP, BMP and TIFF (`.tif`, `.tiff`), in any direction.
- **How:** type `con:` and start typing the name of the image. Pick it from the suggestions, then choose the new name or extension, for example `Lemon.png to webp`.
- **Where the result goes:** the converted file is saved next to the original. The original is never changed, and WayFinder refuses to overwrite a file that already exists.

<!-- TODO: conversion screenshot -->
<p align="center">
  <img src="docs/screenshots/convert.png" alt="Converting an image" width="640">
</p>

## Configuration

WayFinder stores its settings in a single file:

```
%APPDATA%\com.wayfinder.app\config.json
```

It is created on first run. Edit it with any text editor and restart WayFinder to apply changes. To see your current settings, type `sys: settings` or choose **Settings** from the tray icon. If the file is missing or broken, WayFinder falls back to safe defaults and keeps working.

```json
{
  "version": 1,
  "hotkey": "ctrl+space",
  "index_roots": [],
  "exclude_dirs": ["node_modules", ".git"],
  "exclude_extensions": ["dll", "sys", "tmp", "pdb"],
  "skip_dot_dirs": true,
  "fallback_interval_secs": 900,
  "hide_on_blur": true,
  "autostart": false
}
```

The `exclude_extensions` list above is shortened. The built-in default list is longer.

| Option | Meaning |
|---|---|
| `hotkey` | Global shortcut that shows or hides the launcher, for example `"alt+space"` |
| `index_roots` | Folders to index. Empty means your home folder. Example: `["C:\\Users\\you\\Documents", "D:\\Projects"]` |
| `exclude_dirs` | Folder names that are never indexed |
| `exclude_extensions` | File types that are never indexed, written without the dot. Applies to files only; a folder named `x.dll` is still indexed |
| `skip_dot_dirs` | Skip folders whose names start with a dot, such as `.cargo` or `.rustup`. Files such as `.gitignore` are still indexed |
| `fallback_interval_secs` | How often the safety scan checks for missed changes, in seconds |
| `hide_on_blur` | Hide the launcher when it loses focus. Set to `false` only when debugging |
| `autostart` | Start WayFinder with Windows. Off by default |

### What gets indexed

- Your home folder, or the folders in `index_roots`.
- Not indexed: hidden files and folders, dot-folders (unless you turn the option off), the folder names in `exclude_dirs`, and the file types in `exclude_extensions`.
- Shortcuts and junctions are listed but never followed, so they cannot cause loops. OneDrive files are indexed.

## Privacy

- Your index and settings stay in `%APPDATA%\com.wayfinder.app\` on your own PC.
- WayFinder has no accounts and sends no usage data anywhere.
- The only time it touches the network is when *you* run `bow:`, and then it is your browser that opens the page.
- Files are opened through the Windows shell, never by building command-line text from file names.
- Dragging a file out only makes a copy. WayFinder never moves or deletes your files.

## Troubleshooting

**The hotkey does nothing.** Another program (often an input-method switcher) may already use `Ctrl+Space`. Pick another combination in `config.json`, such as `"alt+space"`, and restart.

**A file is missing from the results.** Check that the file is not hidden, not inside an excluded folder or a dot-folder, not one of the file types in `exclude_extensions`, and inside one of your `index_roots`.

**An app is missing from `app:`.** WayFinder lists apps that have a Start Menu shortcut. Microsoft Store apps are not supported yet.

**The launcher closes when I start dragging.** Make sure `hide_on_blur` is not set to `false` while testing something else, then try again. If it keeps happening, please open an issue.

**Start the index over.** Quit WayFinder, delete `index.db`, `index.db-wal` and `index.db-shm` from `%APPDATA%\com.wayfinder.app\`, then start it again.

**Where are the logs?** WayFinder writes a log file in its app log folder, under `%LOCALAPPDATA%\com.wayfinder.app\`. Include it when you report a problem.

**Uninstall completely.** Remove WayFinder from Windows Settings, then delete the `%APPDATA%\com.wayfinder.app\` folder.

## Build from source

You will need:

- [Node.js](https://nodejs.org/) (LTS)
- [Rust](https://www.rust-lang.org/tools/install) (stable, recent enough for edition 2024)
- The [Tauri prerequisites for Windows](https://v2.tauri.app/start/prerequisites/): Microsoft C++ Build Tools and WebView2

```powershell
cd WayFinder
npm install

npm run tauri dev       # run in development (frontend hot-reloads)
npm run tauri build     # build the installer
```

The installer is created under `src-tauri\target\release\bundle\`.

Restart `tauri dev` after changing Rust code, `Cargo.toml` or `tauri.conf.json`. To check the Rust side, run `cargo check`, `cargo clippy` and `cargo test` inside `src-tauri/`.

Tip: keep the project folder (and `src-tauri\target\`) outside OneDrive. Syncing build output can corrupt the build cache.

## How it works

```
 Windows files ──► startup sync (first crawl, or one-pass reconcile) ─┐
                                                                      ▼
 file watcher ──► SQLite index (WAL, case-insensitive paths) ◄── safety scan
                          │
                          ▼
 you type ──► ranked search (Rust) ──► results ──► open, or drag out to copy
```

- **Frontend:** plain HTML, CSS and JavaScript, bundled with Vite.
- **Backend:** Rust on [Tauri v2](https://v2.tauri.app/).
- **Index:** SQLite in WAL mode, so searches keep working while the index is being updated. One shared set of filters decides what is indexed, so the crawler, the startup check and the watcher always agree.
- **Startup:** one background pass either builds the index (first run) or walks the disk once to find what changed, then the watcher takes over.
- **Conversions:** a small registry where each conversion is one entry, so new formats are easy to add.

### Project layout

```
WayFinder/
├─ frontend/                 # index.html, style.css, script.js, fonts/
├─ docs/                     # screenshots and design notes
└─ src-tauri/
   ├─ src/
   │  ├─ main.rs             # entry point, commands, window, tray and hotkey
   │  ├─ config.rs           # config.json: load, save, defaults
   │  ├─ converter/          # conversion registry and image conversion
   │  └─ search_engine/      # apps, crawler, database, paths, reconcile, search, watcher
   ├─ capabilities/          # Tauri permissions
   └─ tauri.conf.json
```

## Roadmap

Ideas being explored, with no promises or dates:

- Microsoft Store apps and app icons in `app:`
- Dragging several files at once
- `spo:` to jump into Spotify
- Themes
- Editing settings inside the app
- Video conversion through a bundled ffmpeg
- Clipboard history

## Contributing

Contributions and ideas are welcome. Please open an issue first for anything larger than a small fix.

- Branches: `main` is release-only. Day-to-day work happens on `updates`, and bigger changes go on `feature/<short-name>`. Open pull requests against `updates`.
- Keep each commit to one logical change, with a plain-English one-line message that says what changed and why.
- Before opening a pull request, run `cargo check`, `cargo clippy` and `cargo test` in `src-tauri/`, and try the affected command by hand.
- See [CHANGELOG.md](CHANGELOG.md) for what changed in each version.

<!-- TODO: link your docs folder or CONTRIBUTING.md here if you add one -->

## License

Released under the [MIT License](LICENSE).

## Credits

- [Tauri](https://tauri.app/), the app framework, and its plugins
- [tauri-plugin-drag](https://github.com/crabnebula-dev/drag-rs) by CrabNebula, for dragging files out of the launcher
- [SQLite](https://www.sqlite.org/) through [rusqlite](https://github.com/rusqlite/rusqlite)
- [walkdir](https://github.com/BurntSushi/walkdir), [notify](https://github.com/notify-rs/notify), [fuzzy-matcher](https://github.com/lotabout/fuzzy-matcher) and [image](https://github.com/image-rs/image)
- Fonts: [Silkscreen](https://fonts.google.com/specimen/Silkscreen) and [VT323](https://fonts.google.com/specimen/VT323), both under the SIL Open Font License

---

<div align="center">

Made with care. If WayFinder saves you a few seconds a day, a star on the repo is much appreciated.

</div>
