# Glossary

Project vocabulary, so terms are not misread.

- **Launcher**: the small always-on window opened with Ctrl+Space.
- **Prefix / command**: a 3-letter keyword that picks what the launcher does.
  Typed as `fil:` and shown as `#fil`. Current: `fil`, `fol`, `con`, `bow`.
- **Mode / chip**: the active command, shown as a small badge next to the
  input. Clicking it opens the mode menu.
- **Result item**: one row in the results list, shaped
  `{ tag, title, sub, run }`. `run` is what happens on Enter.
- **Index**: the SQLite database (`index.db`) listing files and folders, so
  search does not scan the disk each time. It is a cache: it can always be
  rebuilt from disk.
- **Crawler**: code that walks the disk and fills the index. Runs on its own
  thread with its own database connection.
- **Watcher**: the future `notify`-based code that updates the index when
  files change.
- **Reconcile**: comparing disk to the index at startup to catch changes made
  while the app was closed.
- **Fallback poll**: the cheap 3-minute check used only if the watcher fails.
- **Ranking / score**: points given to each match so the best one is first.
- **Tiered search**: look for prefix matches first (fast, uses the index),
  and only scan for "contains" matches if too few were found.
- **`name_lower`**: the lowercase copy of a file name stored in the index so
  case-insensitive prefix search can use an index.
- **Schema version (`user_version`)**: a number stored inside the database.
  If it is older than `SCHEMA_VERSION` in the code, the table is dropped and
  rebuilt.
- **WAL mode**: an SQLite setting that lets one connection write while others
  read. It is why searches do not wait for the crawler.
- **UI thread (main thread)**: the thread that draws the window. Slow work on
  it freezes the app. Commands marked `#[tauri::command(async)]` run elsewhere.
- **Debounce**: waiting a short time (80 ms) after the last keypress before
  searching, so we do not search on every key.
- **Stale result**: an answer to an older query that arrives after a newer one
  was started. The ticket counter in script.js throws these away.
- **Reparse point**: a Windows link-like entry (symlink, junction). Never
  followed by the crawler. OneDrive placeholder files also use this attribute
  and must NOT be skipped.
- **Opener plugin**: the Tauri plugin that opens files and URLs with the
  default app, without going through a shell.
- **Command injection**: when text from outside (like a file name containing
  `&`) gets run as part of a shell command. Why `open_path` never uses `cmd`.
- **Registry (converters)**: the table of supported conversions. One entry
  per conversion.
- **Sidecar**: an external program (e.g. ffmpeg) bundled inside the installer
  and launched by the app. Declared in `externalBin`.
- **Target triple**: the platform suffix on sidecar file names, such as
  `x86_64-pc-windows-msvc`.
- **Tauri command**: a Rust function the frontend can call with `invoke`.
- **Config file**: the single file holding user options (hotkey, roots,
  excludes; theme later).
- **Settings app**: the future separate program that edits the config file.
- **Error box**: the modal the UI shows when something fails.
- **Footer status**: the small line at the bottom for quick, non-error
  messages (e.g. "Copied 6.2 mi").
- **App data directory**: the per-user folder where the index and config live:
  `%APPDATA%\com.wayfinder.app\`.
- **One and done**: the goal that installing WayFinder needs nothing else.
