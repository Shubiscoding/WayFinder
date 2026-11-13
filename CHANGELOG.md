# Changelog

All notable changes to WayFinder are listed here, newest first.
The format follows [Keep a Changelog](https://keepachangelog.com/), and the
project uses [Semantic Versioning](https://semver.org/).

## [2.0.0] - 2026-11-12
<!-- Set the real date when you tag the release. -->

### Added
- **`app:` command.** Find and open installed apps from the Start Menu
  (`.lnk` shortcuts). Results are ranked by name match and by how often you
  launch each app, and an empty `app:` query shows your most-used apps.
- **Drag out to copy.** Drag a `fil:` or `fol:` result onto the Desktop or into
  an Explorer window to make a copy. It is always a copy, even on the same
  drive, and the original is never moved. `Esc` cancels the drag.
- **System tray icon** with Show, Settings and Quit, and `sys:` commands
  (Settings, Quit WayFinder).
- **Single instance.** Starting WayFinder a second time shows the running
  launcher instead of starting another copy.
- **Optional autostart** with Windows (off by default). On an autostart launch,
  background work waits about 30 seconds so logging in stays quick.
- **Settings view.** Shows the path and exact text of `config.json`, including
  parse errors, with an "Open file" button.
- **Startup loading screen** with live elapsed time, a progress bar and an
  estimate of the time left. The launcher never gets stuck: if the startup sync
  fails, you get an error message and the launcher still opens.
- **Log file** in the app log folder, for diagnosing problems.
- **New config options:** `exclude_extensions`, `skip_dot_dirs` and
  `fallback_interval_secs`. Existing config files keep working, and missing
  options get defaults.

### Changed
- **Much faster startup.** On the author's PC, re-checking an index of about
  53,000 items went from roughly 88 seconds to about 2.6 seconds, and a brand-new
  index takes about 7 seconds. Your numbers will vary with drive and index size.
  The startup check now walks the disk once instead of twice.
- **A cleaner index.** Dot-folders (such as `.cargo`, `.rustup`) and a built-in
  list of binary, build and temporary file types (such as `.dll`) are no longer
  indexed by default. Folders are never filtered by extension, and dot-files
  such as `.gitignore` are still indexed. Change this with `skip_dot_dirs` and
  `exclude_extensions`.
- **Changed filters take effect on restart.** Adding a folder name or file type
  to your exclude lists removes the matching rows at the next startup, with no
  manual index reset.
- **Case-insensitive paths.** Windows casing and slash differences can no longer
  create duplicate index rows.
- **Safety scan interval** is now configurable and defaults to 15 minutes
  (it was fixed at 3 minutes).
- **Quieter background work.** The file watcher ignores WayFinder's own data,
  log and web-view cache folders, and it only takes the database write lock when
  there is real work to do.
- **Smarter app filtering.** Shortcuts such as "Uninstall ..." or "... Help"
  are hidden, but real apps with words like "Support" in the middle of their
  name are kept.
- **Index schema is now version 5.**

### Removed
- **Unit conversion** in `con:` (for example `con: 10 km to mi`). `con:` now
  converts images only: PNG, JPEG, WebP, BMP and TIFF.

### Fixed
- **Database lock contention.** Failed database commits are now retried once,
  then rolled back and logged, instead of silently losing writes. The watcher
  and the safety scan no longer compete for the write lock when nothing needs
  writing.
- **App launch counts are preserved** when a shortcut moves (for example from
  your user Start Menu folder to the all-users one), and a failed or empty
  Start Menu scan can no longer wipe your app list.
- **Progress bar and time estimate** during the startup check no longer run past
  100%.
- **Errors are logged, not swallowed.** Failed index writes during startup and
  watching are now counted and logged.

### Upgrade notes
- The file index rebuilds once on the first launch of v2, so the first start
  takes a few seconds longer.
- Your `app:` launch counts start fresh after the upgrade.
- Your existing `config.json` keeps working. If you want to change the new
  options, add them by hand (see the README), or leave them out to use the
  defaults.

## [1.0.0]
<!-- Add the release date if you want it listed. -->

First release.

### Added
- File and folder search (`fil:`, `fol:`) with ranked and fuzzy matching and a
  small boost for recently changed files.
- A live file watcher, a startup check and a periodic safety scan to keep the
  index fresh.
- Web search with `bow:` (opens DuckDuckGo in your default browser).
- `con:` conversions for units and image files.
- Global hotkey (`Ctrl+Space`) and a single `config.json` for settings.
- Pixel-font interface with bundled fonts that works fully offline.
