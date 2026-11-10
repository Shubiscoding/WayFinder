# WayFinder

A fast, lightweight Spotlight-style launcher for Windows. Press Ctrl+Space,
type a command prefix, get results. It is built to stay running all day on
low-end laptops, so memory, CPU and startup time matter more than features.

## Active task
Work through `docs/agent-guidelines/fix.md` (immediate fixes), in order, one at
a time. Do not start anything else until the owner says Phase 0 is done.

## Goals (in priority order)
1. Fast and compact: low idle RAM/CPU, instant open
2. Keyboard-first UX
3. Quality-of-life tools: file search, folder search, conversion, web search
4. Community-friendly: open source (MIT), themes and extensibility planned

## Tech stack
- Shell: Tauri v2 (Rust backend, system webview / WebView2)
- Frontend: vanilla HTML/CSS/JS served by Vite. No framework.
- Index: SQLite via `rusqlite` (WAL mode)
- Crates in use: walkdir, dirs, serde, rusqlite, tauri plugins (opener,
  global-shortcut). Planned: notify, fuzzy-matcher, image.
- Platform: Windows only for now
- Rust crate name: `way_finder`

## Project layout
- `frontend/`  index.html, script.js, style.css, fonts/ (all UI, fonts bundled)
- `src-tauri/` Rust backend: main.rs, config.rs, search_engine/
- Full detail: `docs/agent-guidelines/architecture.md`

## Commands (prefix-based)
| Prefix | Purpose                 | Status                                  |
| ------ | ----------------------- | --------------------------------------- |
| #fil   | Find files              | Real: Rust index, ranked, max 5 results |
| #fol   | Find folders            | Real: Rust index, ranked, max 5 results |
| #con   | Convert units / files   | Units work in JS; files stubbed         |
| #bow   | Web search (DuckDuckGo) | Being fixed (fix.md #5)                 |

The user types `fil:`, `con:` etc. (or picks from the chip menu). The chip shows
it as `#fil`. On hold, do NOT start: `app:` (app launcher) and `spo:` (Spotify).
Planned later: clipboard history, themes.

## How to run
- Dev: `npm run tauri dev`
- Release build: `npm run tauri build`   (confirm before relying on it)
- Rust checks (inside `src-tauri/`): `cargo check` and `cargo clippy`
- Restart `npm run tauri dev` after changing `tauri.conf.json` or Rust code.

## Read these before working (docs/agent-guidelines/)
- `fix.md`             the current task list
- `architecture.md`    how the app is built, what is real vs mocked
- `conventions.md`     code style, threading, error handling, dependencies
- `workflows.md`       git rules, recipes, manual test checklist
- `decisions.md`       past decisions and why. Do not undo them silently.
- `roadmap.md`         what to work on and in what order. Do not jump ahead.
- `domain-glossary.md` project vocabulary

## Golden rules
1. **Human-friendly code first.** Solo-maintained project. The owner must be
   able to read, debug and change every file. Choose the boring option.
2. **Never commit to `main`.** Work on `updates` or a `feature/<name>` branch.
   Check the current branch before editing anything.
3. **Do not commit, push, merge or delete branches** unless asked. Suggest the
   commit message instead.
4. **Small, focused changes.** Do not refactor unrelated code. After a change,
   explain in 2-3 lines what changed and where.
5. **No new dependencies without asking.** Report size, last release date and
   how many sub-dependencies it adds. Prefer crates over external programs.
   Never enable a crate's default features without checking what they pull in.
6. **No new frameworks or build tools.** Frontend stays vanilla JS.
7. **Performance matters.** No polling loops, no heavy crates, nothing that
   raises idle memory or CPU.
8. **Never block the UI thread.** Tauri commands that touch the disk or the
   database use `#[tauri::command(async)]`. Never hold the shared database
   lock for long. The crawler uses its OWN connection.
9. **Never hardcode user paths or tool paths.** Use the OS home folder and
   look tools up properly. Never assume ffmpeg/ImageMagick is installed.
10. **No `unwrap()`, `expect()` or `panic!`** in app code. Return `Result` with
    plain-English messages. Failures shown to users use an error box.
11. **Never build a shell command from file names.** Open files and URLs with
    the opener plugin API, never `cmd /c start ...` (file names can contain
    `&`, which would run extra commands).
12. **Do not touch** `src-tauri/target/` or `node_modules/`. The index database
    lives in the app data folder and is generated.

## Frontend rules (summary)
- Debounce typing (80 ms) and drop answers for outdated queries.
- Show at most 5 suggestions. No scrollbars anywhere.
- Failures show the error box; small messages use the footer status line.

## Indexing rules (summary)
- Index root is the current user's home folder, resolved at runtime.
- Skip hidden items (use the Windows hidden attribute, not just a leading dot).
  Never follow symlinks or junctions. Do NOT skip every reparse point:
  OneDrive placeholder files carry that attribute and must stay indexed.
- Skip permission errors and continue. Never crash the crawl.
- Schema changes bump `SCHEMA_VERSION`. The index is a cache, so it is dropped
  and rebuilt, not migrated.
- All scope settings live in one Config struct so users can change them later.

## File conversion (summary)
- Every converter is one entry in a registry (input types, output types, how
  to convert). Adding one must never mean editing an if/else chain.
- Prototype phase: use Rust crates only. External tools come later.
- Later: bundle LGPL tools (e.g. ffmpeg) inside the installer. One-and-done
  install, works offline.
