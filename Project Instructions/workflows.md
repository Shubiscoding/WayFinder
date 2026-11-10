# Workflows

## Running and checking
- Dev: `npm run tauri dev`. Restart it after changing `tauri.conf.json`,
  `Cargo.toml` or Rust code (hot reload only covers the frontend).
- Release build: `npm run tauri build`   (confirm before relying on it)
- Rust checks, inside `src-tauri/`: `cargo check`, `cargo clippy`
- There are no automated tests yet. After a change, run the app, try the
  affected command by hand, and say what you tested. The manual checklist
  below is the minimum.

## Manual test checklist (run after search, open or UI changes)
1. App starts; Ctrl+Space shows and hides the launcher.
2. Fresh index: delete `index.db`, `index.db-wal`, `index.db-shm` in
   `%APPDATA%\com.wayfinder.app\`, start the app, type while indexing. The
   window must stay responsive.
3. `fil:` exact file name comes first; `fol:` finds folders.
4. Typing fast never shows results for an old query.
5. A file named `test&calc.txt` opens in Notepad and Calculator does NOT open.
6. `bow:` opens DuckDuckGo in the default browser.
7. Never more than 5 rows, no scrollbars, nothing clipped.
8. `con:` still converts `10 km to mi`.
9. A failure (e.g. open a deleted file) shows the error box.
10. Compact mode: open the chip menu with click AND with Alt+Down; all 4 items
    are fully visible without clipping.
11. Results mode: chip menu opens on top of result rows (`z-index: 20`).
12. Keyboard Up/Down/Enter navigates and selects within the chip menu; clicking
    outside the menu closes it.
13. Click outside launcher:
    - Click in transparent area far from launcher (inside window): closes launcher.
    - Click outside window (desktop or other app): window hides on focus loss.
    - Click inside launcher, on chip, or on a result: stays open (clicking a result opens it and dismisses the launcher).
    - Click on error box (modal): stays open (error box is not treated as outside).
14. `hide_on_blur`: set to `false` in `config.json`; launcher no longer hides on focus loss (for devtools debugging).
15. `con:` file conversion:
    - Try `Lemon.png to jpg`: creates `Lemon.jpg` and displays "Saved Lemon.jpg" in the status bar.
    - Try unsupported conversion e.g. `report.docx to pdf`: shows error modal with clear plain-English explanation.
    - Try non-existent file e.g. `missing.png to jpg`: shows error modal explaining the file was not found.
16. Phase 5 Index freshness & ranking:
    - Create a new file in an indexed directory; search for it immediately: appears without app restart (watcher).
    - Delete the file: disappears from search results.
    - Fuzzy search: typing query abbreviations (e.g. `dwnld`) finds files with matching character sequence via SkimMatcherV2.
    - Recency bonus: files modified recently receive ranking precedence over older files with identical name relevance.

## Git workflow
Branches:
- `main`: stable, release-ready only. NEVER commit to `main` directly.
  Nothing lands here except a merge from `updates` when cutting a release.
- `updates`: the working branch for day-to-day development.
- `feature/<short-name>`: for larger or risky work (e.g. `feature/watcher`).
  Branch from `updates`, merge back into `updates` when done.

Agent rules:
- Before changing any file, check the current branch. If it is `main`,
  switch to `updates` (or create a feature branch) first.
- Do not commit, push, merge or delete branches unless the owner asks.
  Suggest the commit message instead.
- Never force-push or rewrite history.
- Commit messages: one plain-English line saying what changed and why
  (e.g. "Search: give the crawler its own database connection").
- One logical change per commit.
- Before deleting files, make sure a commit exists that contains them.

### One-time setup (owner runs this in the project root)
```powershell
git init -b main
git add .
git commit -m "Initial commit"
git branch updates
git switch updates
```
Run `git status` before the first commit and confirm `node_modules/`, `dist/`
and `src-tauri/target/` are NOT listed. The index database lives in the app
data folder, not in the project.

## Keeping the build folder small
`src-tauri/target/` can reach several GB. It is never shipped to users.
- Reclaim space: run `cargo clean` inside `src-tauri/` (next build is slow).
- Smaller clean: delete `src-tauri/target/debug/incremental`.
- Fewer and lighter dependencies = a smaller `target/`.

## Recipe: add a frontend command
1. Write a `searchXxx(query)` function in script.js that returns items
   `{ tag, title, sub, run }` (or a Promise of them).
2. Add an entry to `COMMANDS` with `hint`, `placeholder`, `empty`, `search`.
3. The 3-letter key becomes the prefix automatically (`xxx:`) and appears
   in the chip menu.
4. Update the command table in AGENTS.md.

## Recipe: add a Tauri command (Rust to frontend)
1. Write the Rust function returning `Result<T, String>` with a plain-English
   error.
2. Mark it `#[tauri::command(async)]` if it touches disk or the database
   (a plain `#[tauri::command]` freezes the window). Register it in `main.rs`.
3. Check `capabilities/default.json` for any permission it needs.
4. Call it from JS with `invoke`. On error call `showError(message)`.

## Recipe: change the database layout
1. Edit the `CREATE TABLE` in `database.rs`.
2. Bump `SCHEMA_VERSION`. On the next launch the old table is dropped and the
   crawler rebuilds the index. No migration code.
3. Update the schema section in architecture.md.

## Recipe: add a file converter
1. Check for a maintained crate first. Report its size and dependencies.
2. Add ONE entry to the converter registry: input extensions, output
   extensions, the conversion function, and any tool needed.
3. Do not edit existing if/else chains. If the registry does not exist yet,
   build it first (Phase 4).
4. Failures return a plain-English error that the UI shows in an error box.

## Recipe: add a config option
1. Add the field to the Config struct with a built-in default.
2. Read it from the config file; never from a scattered constant.
3. Bump the config `version` if the format changed, and handle old files.
4. Mention the new option so the settings app can expose it later.

## Definition of done
- Works when run with `npm run tauri dev` and the manual checklist passes for
  the area you touched.
- `cargo check` passes; no new clippy warnings in touched code.
- No new `unwrap()`, no hardcoded paths, no shell strings built from names.
- Slow work is off the UI thread.
- Errors are plain English and shown in an error box.
- Docs updated if behavior or structure changed (architecture.md,
  decisions.md, roadmap.md).
