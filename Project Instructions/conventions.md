# Conventions

## Philosophy: human-friendly first
WayFinder is maintained by one person. Every file must be readable and
changeable without help. When in doubt, choose the boring option.

- Prefer simple, explicit code over clever or compact code.
- Small functions with one job and a descriptive name.
- Comment the "why", not the "what".
- Each file starts with a short header comment explaining its role.
- Keep changes small and focused. Do not refactor unrelated code.
- No new frameworks, build tools or heavy abstractions unless asked.
- After finishing, explain in 2-3 lines what changed and where.

## Performance and threading
- No polling loops, no busy loops. (The short `index_ready` poll during the
  first crawl is the one allowed exception.)
- Avoid heavy dependencies. Check what a crate pulls in before suggesting it.
  Do not enable default features without looking at them (the `image` crate's
  defaults add AVIF encoding and make builds huge).
- A plain Tauri command runs on the UI thread and freezes the window.
  Commands that touch disk or the database use `#[tauri::command(async)]`.
- Never hold the shared database lock longer than one quick query.
  Long jobs (the crawler) open their own connection with `database::open()`.
- Return small results (top 5), not whole tables.
- Typing is debounced (80 ms) and out-of-date answers are dropped.

## Rust
- Functions that can fail return `Result<T, String>` (a small error enum is
  fine later). No `unwrap()`, `expect()` or `panic!` in app code. The only
  exception is startup code that truly cannot continue.
- Never silently discard an error (no `_ =` on a Result). Log it at least.
- Filesystem errors (permission denied, file vanished) are normal. Skip the
  entry and continue; do not crash.
- Never hardcode user paths or tool paths. Use the `dirs` crate for the home
  folder and the shared tool-lookup function for external programs.
- Never build a shell command from file names or user text. Open files with
  the opener plugin API, not `cmd /c start`.
- SQL: always use bound parameters (`?1`). When a user's text goes into a
  `LIKE`, escape `%`, `_` and `\`.
- Run `cargo clippy` and fix warnings in code you touched.
- Remove `#![allow(dead_code)]` and `#![allow(unused_variables)]` from a file
  once it is wired in.

## Error messages (plain English)
Every message says three things: what we were doing, what went wrong, what
the user can try.
- Good: "Windows could not open \"report.docx\". Check that it still exists and
  that you can open it."
- Bad: "Os { code: 2, kind: NotFound }"
Keep the technical detail too. Log it, and show only the plain version to the
user.

## Error handling in the UI
- Failures show an error box (modal) with the plain-English message and a
  Close button. Not just a console message.
- Rust sends the error string through the Tauri command's `Err` value. One
  shared `showError(message)` function in script.js displays it.
- Small expected things (e.g. "Copied 6.2 mi") stay in the footer status line.

## Frontend (vanilla JS and CSS)
- No framework, no bundled UI library.
- Build DOM with the existing `el()` helper and `textContent`. Do not insert
  untrusted text with `innerHTML` (file names are untrusted).
- A new command is one entry in the `COMMANDS` table.
- Theme colors are CSS variables in `:root` in style.css. Never hardcode a
  color elsewhere, so themes can be added later.
- Keep keyboard behavior consistent: Up/Down move, Enter runs, Tab switches
  mode, Esc clears, then hides.
- Show at most 5 suggestions. No visible scrollbars (`scrollbar-width: none`
  plus `::-webkit-scrollbar { display: none }`).
- Never use `window.open` for links in the app. Use the opener plugin.
- Respect `prefers-reduced-motion`.

## Dependencies
- Do not add dependencies without asking. For each proposal report: purpose,
  last release date, popularity, number of sub-dependencies, size impact.
- Prefer a maintained crate over an external program (prototype phase).
- Bundled third-party tools must be LGPL builds, with their license files
  included in the installer.
- Remove dependencies nothing uses.

## External tools (ffmpeg, ImageMagick, etc.)
- Never assume a tool is installed.
- Lookup order: bundled sidecar, app data folder, PATH, then an error box.
- Tool lookup lives in one function, not scattered across converters.
- Pass command arguments as separate `.arg()` calls with no embedded spaces.

## Naming and layout
- Rust: snake_case files and functions, one module per concern.
- JS: camelCase functions, UPPER_CASE for constant tables.
- Commands keep their 3-letter prefix style (fil, fol, con, bow).
- Generated files (`index.db`, `target/`, `node_modules/`, `dist/`) never go
  in git.
