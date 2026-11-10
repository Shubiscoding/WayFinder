# Decisions

Each entry says what was decided and why. Do not undo a decision silently.
If you think one is wrong, say so and ask the owner. New decisions go at the
bottom.

## 1. Tauri + vanilla JS
Tauri v2 with a framework-free JS frontend. Why: small, fast, low idle memory,
and simple enough for one person to maintain.

## 2. Prefix commands
Commands use short 3-letter prefixes (`fil`, `fol`, `con`, `bow`). Why:
fast typing, keyboard-first, easy to extend.

## 3. Web search stays in JS
`#bow` builds a DuckDuckGo URL in JS and opens it with the Tauri opener
plugin's `openUrl`. `window.open` does nothing inside the webview, so never
use it there (this regressed once already). The old Google `web_search.rs` is
deleted.

## 4. Local SQLite index
Files and folders are indexed into SQLite so search is instant.
- Created once on first run in the app data directory, then kept fresh.
- Freshness: `notify` watcher (debounced), plus a startup reconcile, plus a
  cheap 3-minute fallback check if the watcher fails. The fallback only
  re-scans folders whose modified time changed. (Phase 5.)

## 5. Index scope
Index the current user's home folder, resolved at runtime. Skip hidden items.
Never follow symlinks or junctions, so crawling cannot loop or deadlock.
Excludes come from config (defaults `node_modules`, `target`, `.git`).
User-chosen scope comes later through the config file.
Do NOT blanket-skip reparse points: OneDrive placeholder files carry that
attribute and the owner's files live in OneDrive.

## 6. Ranking
Score matches: exact name 100, exact without extension 90, prefix 80, word
prefix 70, contains 60, fuzzy 0-50. SQL narrows candidates, Rust scores them.
Ties go to the shorter name. Later: frequency and recency bonuses.

## 7. Converter registry
Each conversion is one registry entry. Why: the owner wants it expandable,
from a simple doc-to-pdf user up to someone who needs video codecs.

## 8. Crates first for the prototype
The prototype converts with Rust crates only (e.g. `image`, default features
off). External tools come in a later release. Why: fewer moving parts, nothing
to bundle, easier debugging. No mature pure-Rust option is known for video or
doc-to-pdf, so those are deferred.

## 9. One-and-done installer (later releases)
When external tools return, they are bundled inside the installer as
sidecars. Nothing is downloaded after install and everything works offline.
- LGPL ffmpeg builds only, minimal codec set, license files included.
- Installer size is tracked. State the MB added before bundling a tool.
- The installer must handle the WebView2 runtime for older Windows 10
  machines (verify Tauri's `webviewInstallMode` option).
- Later: optional add-on packs are allowed, but the base install always works.

## 10. Config file + separate settings app (future)
All user options live in one human-readable config file. The launcher works
without the settings app. The settings app is a separate program that only
edits the file, so it costs nothing while closed. A shared Rust crate holds
the Config struct.

## 11. Error handling
Rust uses `Result` with plain-English messages (what we were doing, what went
wrong, what to try). No `unwrap`/`panic` in app code. The UI shows failures in
an error box. Technical detail goes to a log file.

## 12. Git workflow
Never commit to `main`. Work on `updates` or `feature/<name>`. Agents suggest
commits instead of making them. Why: `main` stays release-ready.

## 13. Open source, MIT
WayFinder is open source under the MIT license. Third-party tools stay as
separate executables under their own licenses (LGPL ffmpeg only).

## 14. Not a Docker image
WayFinder is a Windows desktop app with a window and a global hotkey, which
containers cannot provide. It ships as a normal Windows installer. Docker may
be used later for contributor dev environments or CI only.

## 15. Human-friendly code over cleverness
Solo maintainer. Readability and simplicity win over compactness.

## 16. The crawler has its own database connection (WAL)
The crawler opens a separate connection and the database runs in WAL mode.
Why: the old crawler locked the shared connection for the whole crawl, so
every search waited and the window froze. Never share one connection between
the crawler and searches.

## 17. Disk and database commands run off the UI thread
Tauri commands that touch disk or database use `#[tauri::command(async)]`.
Why: a plain command runs on the main thread and freezes the window.

## 18. Tiered search with a lowercase index column
Search uses a `name_lower` column with an index on `(is_dir, name_lower)`:
prefix matches via the index first, a slower "contains" scan only if needed.
Why: `LOWER(name) LIKE '%q%'` scanned the whole table on every keystroke and
`LIMIT 100` could miss the best match. User text is escaped in LIKE queries.

## 19. The index is a cache: rebuild, do not migrate
`PRAGMA user_version` holds `SCHEMA_VERSION`. An older layout is dropped and
the crawler rebuilds it. Why: the index can always be recreated from disk, so
migrations would be extra code for no benefit.

## 20. Frontend debounces and drops stale results
Typing waits 80 ms before searching; each search has a ticket number and
outdated answers are ignored. Why: avoids a search per keystroke and
out-of-order results.

## 21. Never open files through the shell
`open_path` uses the opener plugin's Rust API, never `cmd /c start`. Why:
file names come from the index and can contain `&`, which `cmd` treats as a
command separator (a file called `x&calc.exe` could run another program).

## 22. Five suggestions, no scrollbars
At most 5 results (`MAX_RESULTS` in Rust, `MAX_ITEMS` in JS) and scrollbars
are hidden everywhere. Why: compact launcher feel.

## 23. `app:` and `spo:` are on hold
Not started, parked in the roadmap. When revisited:
- `app:` should read Start Menu shortcuts (a small list), not index every
  `.exe`. Do not prioritize `.exe` inside `#fil`.
- `spo:` v1 should open Spotify through a `spotify:` link with no login.
  Spotify login (PKCE) only as an optional bring-your-own-Client-ID feature,
  because Spotify's 2026 development-mode rules require Premium for the app
  owner and cap users at 5.

## 24. Hide on blur, not a full-screen overlay
When the launcher loses focus, `main.rs` calls `window.hide()` via a
`WindowEvent::Focused(false)` handler. This covers clicks on the desktop or
on any other app window even though those clicks never reach our page.
A full-screen transparent overlay would also receive all clicks, but it
keeps the GPU compositing the whole desktop every frame — too expensive on
low-end laptops. The 720 × 560 window is composited only in that rectangle.
A 150 ms guard (`last_shown` timestamp) prevents an instant re-hide when some
Windows builds fire the blur event immediately after `window.show()`.
The `hide_on_blur` config option (bool, default true) lets developers turn it
off when devtools or another debug window needs to stay alongside the launcher.

## 25. Near-invisible html background prevents click-through
Fully transparent WebView2 pixels pass clicks to whatever is behind them.
Setting `html { background: rgba(0,0,0,0.01) }` makes those pixels
technically opaque (the wallpaper is still visible) so clicks in the empty
area of the window are captured and can dismiss the launcher. `body` stays
transparent; the visible colour comes only from `.launcher`.

## 26. Chip menu uses scaleX animation, not clip-path
The open/close sweep on `.launcher` was originally a `clip-path: inset()`
animation. `clip-path` on a parent clips all absolutely-positioned children,
including the `.menu` dropdown. The animation is now `transform: scaleX()`
+ `opacity` so clip-path is never set on `.launcher`, and the menu can
overflow freely. `z-index: 20` on `.menu` keeps it above the results list.

## 27. Converter registry and pure-Rust image prototype (Phase 4)
File conversions are defined by single `ConverterEntry` records in
`converter/registry.rs`, eliminating if/else chains. The prototype supports
image-to-image conversions (PNG, JPEG/JFIF, WebP, BMP, TIFF) using the
`image` crate with `default-features = false`. Source files can be specified
by absolute path, relative path, or bare filename (resolved against the SQLite
index). Output is saved in the source file's directory.

## 28. Audio and Doc-to-PDF deferred to Phase 6 sidecars
Crate research for pure-Rust audio and document conversion concluded:
- Audio: `symphonia` is an excellent pure-Rust decoder, but pure-Rust encoders
  for lossy formats (MP3, AAC) are fragmented, unmaintained, or require C
  bindings.
- Doc-to-PDF: Converting `.docx` to `.pdf` requires a complex layout and
  rendering engine. In pure Rust there is no standalone library capable of
  measuring and laying out arbitrary DOCX to PDF without an external engine
  (e.g. headless LibreOffice or Chromium).
Both will be supported in Phase 6 via bundled LGPL sidecar tools (such as
minimal ffmpeg builds and document engines) rather than fragile Rust wrappers.

## 29. Three-tier index freshness (Phase 5)
Index freshness is handled in three complementary layers:
1. `notify-debouncer-mini` background watcher with 500 ms window: provides
   instant updates for active file changes, handling editor burst-writes.
2. Startup reconcile: catches changes, deletions, and new files made while
   the application was closed, purging stale DB entries in chunks.
3. 3-minute lightweight fallback: periodically compares folder filesystem
   modification times to SQLite `modified_at` and only scans directories whose
   timestamps changed. Avoids whole-drive walking while protecting against
   dropped OS filesystem events.

## 30. Tiered ranking with SkimMatcherV2 and recency bonus (Phase 5)
Prefix and substring matches take precedence (scores 60–100). When fewer than
5 candidates match, a subsequence query selects candidates scored by
`SkimMatcherV2` into the 1–50 tier. A subtle recency bonus (+5 for <24h,
+2 for <7d) promotes recently touched work without overriding relevance.
Ties are broken by shorter name.
