<div align="center">

# WayFinder

**Find anything on your PC in a keystroke.**

A fast, keyboard-first launcher for Windows. Press a hotkey, type a few letters, and jump straight to the file, folder, web search or conversion you need.

[Platform: Windows](https://img.shields.io/badge/platform-Windows-0078D6) [Built with Tauri v2](https://img.shields.io/badge/built%20with-Tauri%20v2-24C8D8) [Backend: Rust](https://img.shields.io/badge/backend-Rust-DEA584) [License: MIT](https://img.shields.io/badge/license-MIT-b8ff2c)

<!-- TODO: once the repo exists, add a release badge: ![Latest release](https://img.shields.io/github/v/release/YOUR-USERNAME/WayFinder) --> <!-- TODO: hero screenshot (compact launcher, nothing typed yet) --> <img src="docs/screenshots/hero.png" alt="WayFinder launcher" width="640"> </div>

---

## Why WayFinder?

Windows search is slow to open and slow to answer. WayFinder lives in the background, keeps its own small index of your files, and answers while you type. Everything stays on your machine: no account, no cloud, no telemetry.

## Features

- **Instant file and folder search.** Type `fil:` or `fol:` followed by a name. Results are ranked: exact name first, then names that start with your text, then word matches, then partial and fuzzy matches. Files you changed recently get a small boost.
- **Always up to date.** A background watcher notices new, renamed and deleted files within a second or so. A startup check and a periodic safety scan catch anything that was missed while the app was closed.
- **Web search in one step.** `bow:` opens your search in your default browser (DuckDuckGo).
- **Quick image conversions.** `con:` converts image files between common formats (PNG, JPG, WebP, BMP, TIFF) right in their folder, with instant suggestions.
- **Global hotkey.** `Ctrl+Space` shows and hides the launcher from anywhere. Change it in the config file.
- **Light on resources.** A small window, a local SQLite index and a Rust backend. Searching never freezes the window, even while the index is still being built.
- **A look of its own.** A pixel-font terminal style with bundled fonts, so it works fully offline.

<!-- TODO: add these lines if they ship in v0.1.0 (remove otherwise): - **Lives in the tray.** Quit from the tray icon or with `sys: quit`. - **Start with Windows.** Optional autostart, off by default. - **Settings page.** See your current configuration at a glance. --> <!-- TODO: results screenshot (for example, `fil:` with five results) --> <p align="center"> <img src="docs/screenshots/search.png" alt="Searching for files" width="640"> </p>

## Install

1. Go to the [Releases page](https://github.com/YOUR-USERNAME/WayFinder/releases) <!-- TODO: fix link -->
2. Download the latest `WayFinder_x.y.z_x64-setup.exe` <!-- TODO: match the real installer name -->
3. Run the installer, then press **Ctrl+Space**.

**Requirements:** Windows 10 or 11. WayFinder uses Microsoft's WebView2 runtime. It is already part of Windows 11 and up-to-date Windows 10, and the installer takes care of it if it is missing.

> **First launch:** WayFinder builds its file index on first start. A retro progress screen shows elapsed time and progress while indexing. Once indexing completes, the launcher is ready to search and a real-time background watcher keeps the index up to date.

<!-- TODO: keep this note only if the installer is not code-signed: > **Windows SmartScreen:** unsigned apps trigger a "Windows protected your PC" warning. Choose **More info**, then **Run anyway**. -->

## Using WayFinder

Press **Ctrl+Space**, then type a command prefix followed by your query. You can also pick a command from the dropdown on the left of the search bar.

|Prefix|What it does|Example|
|---|---|---|
|`fil:`|Search files|`fil: invoice`|
|`fol:`|Search folders|`fol: downloads`|
|`bow:`|Search the web in your default browser|`bow: rust lifetimes`|
|`con:`|Convert image files|`con: Lemon.jfif`|

### Keys

|Key|Action|
|---|---|
|`↑` `↓`|Move through results|
|`Enter`|Open the selected result|
|`Tab`|Switch command (in `con:`, jump between the two fields)|
|`Alt` + `↓`|Open the command dropdown|
|`Esc`|Clear the search, then close the launcher|

Results open with the program Windows normally uses for that file type, exactly like a double-click.

### Converting

- **Images:** PNG, JPG, JPEG, JFIF, WebP, BMP, and TIFF in any direction.
  - In `con:` mode, typing suggests indexed image files with matching names.
  - Pick a file with Enter, click, or Tab; WayFinder sets the first field and prefills the target in the second field (e.g. `Lemon.`).
  - Enter the target extension (or edit the target name) and press Enter. The new file is saved next to the original without changing the source or overwriting any existing file.

<!-- TODO: conversion screenshot --> <p align="center"> <img src="docs/screenshots/convert.png" alt="Converting an image" width="640"> </p>

## Configuration

WayFinder stores its settings in a single file:

```
%APPDATA%\com.wayfinder.app\config.json
```

It is created on first run. Edit it with any text editor and restart WayFinder to apply changes. If the file is missing or broken, WayFinder falls back to safe defaults and keeps working.

```json
{
  "version": 1,
  "hotkey": "ctrl+space",
  "index_roots": [],
  "exclude_dirs": ["node_modules", ".git"],
  "hide_on_blur": true
}
```

|Option|Meaning|
|---|---|
|`hotkey`|Global shortcut that shows or hides the launcher, for example `"alt+space"`|
|`index_roots`|Folders to index. Empty means your home folder. Example: `["C:\\Users\\you\\Documents", "D:\\Projects"]`|
|`exclude_dirs`|Folder names that are never indexed|
|`hide_on_blur`|Hide the launcher when it loses focus. Set to `false` only when debugging|

Hidden files and folders are never indexed.

## Privacy

- Your index and settings stay in `%APPDATA%\com.wayfinder.app\` on your own PC.
- WayFinder has no accounts and sends no usage data anywhere.
- The only time it touches the network is when _you_ run `bow:`, and then it is your browser that opens the page.
- Files are opened through the Windows shell, never by building command-line text from file names.

## Troubleshooting

**The hotkey does nothing.** Another program (often an input-method switcher) may already use `Ctrl+Space`. Pick another combination in `config.json`, such as `"alt+space"`, and restart.

**A file is missing from the results.** If you just installed WayFinder, the first index may still be building. Otherwise check that the file is not hidden, not inside an excluded folder, and inside one of your `index_roots`.

**Start the index over.** Quit WayFinder, delete `index.db`, `index.db-wal` and `index.db-shm` from `%APPDATA%\com.wayfinder.app\`, then start it again.

**Uninstall completely.** Remove WayFinder from Windows Settings, then delete the `%APPDATA%\com.wayfinder.app\` folder.

## Build from source

You will need:

- [Node.js](https://nodejs.org/) (LTS)
- [Rust](https://www.rust-lang.org/tools/install) (stable, recent enough for edition 2024)
- The [Tauri prerequisites for Windows](https://v2.tauri.app/start/prerequisites/): Microsoft C++ Build Tools and WebView2

```powershell
git clone https://github.com/YOUR-USERNAME/WayFinder.git   # TODO: fix link
cd WayFinder
npm install

npm run tauri dev       # run in development (frontend hot-reloads)
npm run tauri build     # build the installer
```

The installer is created under `src-tauri\target\release\bundle\`.

Restart `tauri dev` after changing Rust code, `Cargo.toml` or `tauri.conf.json`. To check the Rust side, run `cargo check` and `cargo clippy` inside `src-tauri/`.

## How it works

```
 Windows files ──► crawler (first run) ─┐
                                        ▼
 file watcher ──► SQLite index (WAL) ◄── startup check + periodic safety scan
                        │
                        ▼
 you type ──► ranked search (Rust) ──► results in the launcher
```

- **Frontend:** plain HTML, CSS and JavaScript, bundled with Vite.
- **Backend:** Rust on [Tauri v2](https://v2.tauri.app/).
- **Index:** SQLite in WAL mode, so searches keep working while the index is being updated.
- **Conversions:** a small registry where each conversion is one entry, so new formats are easy to add.

### Project layout

```
WayFinder/
├─ frontend/                 # index.html, style.css, script.js, fonts/
└─ src-tauri/
   ├─ src/
   │  ├─ main.rs             # entry point, commands, window and hotkey
   │  ├─ config.rs           # config.json: load, save, upgrade
   │  ├─ converter/          # conversion registry and image conversion
   │  └─ search_engine/      # crawler, database, search, watcher, reconcile
   ├─ capabilities/          # Tauri permissions
   └─ tauri.conf.json
```

## Roadmap

Ideas being explored, with no promises or dates:

- `app:` to launch installed programs from the Start Menu
- `spo:` to jump into Spotify
- Themes
- Editing settings inside the app
- Video conversion through a bundled ffmpeg
- Clipboard history

## Contributing

Contributions and ideas are welcome. Please open an issue first for anything larger than a small fix.

- Branches: `main` is release-only. Day-to-day work happens on `updates`, and bigger changes go on `feature/<short-name>`. Open pull requests against `updates`.
- Keep each commit to one logical change, with a plain-English one-line message that says what changed and why.
- Before opening a pull request, run `cargo check` and `cargo clippy` in `src-tauri/`, and try the affected command by hand.

<!-- TODO: link your docs folder or CONTRIBUTING.md here if you add one -->

## License

Released under the [MIT License](https://claude.ai/chat/LICENSE).

## Credits

- [Tauri](https://tauri.app/), the app framework
- [SQLite](https://www.sqlite.org/) through [rusqlite](https://github.com/rusqlite/rusqlite)
- [walkdir](https://github.com/BurntSushi/walkdir), [notify](https://github.com/notify-rs/notify), [fuzzy-matcher](https://github.com/lotabout/fuzzy-matcher) and [image](https://github.com/image-rs/image)
- Fonts: [Silkscreen](https://fonts.google.com/specimen/Silkscreen) and [VT323](https://fonts.google.com/specimen/VT323), both under the SIL Open Font License

---

<div align="center">

Made with care. If WayFinder saves you a few seconds a day, a star on the repo is much appreciated.

</div>