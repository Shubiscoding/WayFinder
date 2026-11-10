# WayFinder — Urgent Fix Guide

Drop-in fixes for every bug found in the code review. Not compiled here, so run `cargo check` after each step.

## Apply order

| # | Fix | Files touched | Severity |
|---|-----|---------------|----------|
| 1 | Schema v3 + batch transactions + LIKE-prefix bug | `database.rs` | High |
| 2 | Hidden/excluded/symlink-safe helpers + `index_tree` | `crawler.rs` | High |
| 3 | Watcher rewrite | `watcher.rs` | High |
| 4 | Reconcile + fallback rewrite | `reconcile.rs` | Medium |
| 5 | Converter: lock, overwrite, ambiguity, formats | `converter/mod.rs`, `registry.rs`, `image_conv.rs` | High |
| 6 | Config robustness + visible warnings | `config.rs` | Medium |
| 7 | `main.rs` wiring | `main.rs` | High/Medium |
| 8 | Frontend fixes | `script.js` | Medium/Low |
| 9 | Search ranking polish | `search.rs` | Low |

Fix 1 changes the schema (adds a `parent` column). The index is only a cache, so it drops and re-crawls automatically on first launch.

---

## 1. `database.rs`

### 1a. Schema v3 with a `parent` column
This lets the fallback find a folder's direct children cheaply, so it can detect deletions.

```rust
const SCHEMA_VERSION: i64 = 3;   // was 2
```

Replace the `CREATE TABLE` batch inside `create_table`:

```rust
conn.execute_batch(
    "CREATE TABLE IF NOT EXISTS files (
        id          INTEGER PRIMARY KEY AUTOINCREMENT,
        name        TEXT NOT NULL,
        name_lower  TEXT NOT NULL,
        path        TEXT NOT NULL UNIQUE,
        parent      TEXT,
        extension   TEXT,
        is_dir      INTEGER NOT NULL DEFAULT 0,
        created_at  INTEGER,
        modified_at INTEGER
    );
    CREATE INDEX IF NOT EXISTS idx_files_kind_name ON files (is_dir, name_lower);
    CREATE INDEX IF NOT EXISTS idx_files_parent ON files (parent);",
)?;
```

### 1b. `insert_entry` / `upsert_entry` write `parent`; upsert keeps `created_at`
No caller changes are needed, because `parent` is derived from `entry.path`.

```rust
fn parent_of(path: &str) -> Option<String> {
    Path::new(path).parent().map(|p| p.to_string_lossy().to_string())
}

pub fn insert_entry(conn: &Connection, entry: &FileEntry) -> Result<()> {
    let mut stmt = conn.prepare_cached(
        "INSERT OR IGNORE INTO files
            (name, name_lower, path, parent, extension, is_dir, created_at, modified_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
    )?;
    stmt.execute(params![
        &entry.name,
        entry.name.to_lowercase(),
        &entry.path,
        parent_of(&entry.path),
        &entry.extension,
        entry.is_dir as i64,
        unix_secs(entry.created_at),
        unix_secs(entry.modified_at),
    ])?;
    Ok(())
}

pub fn upsert_entry(conn: &Connection, entry: &FileEntry) -> Result<()> {
    let mut stmt = conn.prepare_cached(
        "INSERT INTO files
            (name, name_lower, path, parent, extension, is_dir, created_at, modified_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT(path) DO UPDATE SET
            name        = excluded.name,
            name_lower  = excluded.name_lower,
            parent      = excluded.parent,
            extension   = excluded.extension,
            is_dir      = excluded.is_dir,
            created_at  = COALESCE(excluded.created_at, created_at),
            modified_at = excluded.modified_at",
    )?;
    stmt.execute(params![
        &entry.name,
        entry.name.to_lowercase(),
        &entry.path,
        parent_of(&entry.path),
        &entry.extension,
        entry.is_dir as i64,
        unix_secs(entry.created_at),
        unix_secs(entry.modified_at),
    ])?;
    Ok(())
}
```

### 1c. Fix `delete_by_path_prefix`
**The bug:** `"{}\\%"` produced `\%`, which is a *literal percent* under `ESCAPE '\'`. Windows children were never deleted.

```rust
pub fn delete_by_path_prefix(conn: &Connection, dir_path: &str) -> Result<()> {
    let normalized = dir_path.trim_end_matches(['\\', '/']);
    let escaped = escape_like(normalized);
    // Windows: escaped literal backslash (`\\`) followed by the wildcard.
    let pattern_win = format!("{escaped}\\\\%");
    let pattern_posix = format!("{escaped}/%");
    let mut stmt = conn.prepare_cached(
        "DELETE FROM files WHERE path LIKE ?1 ESCAPE '\\' OR path LIKE ?2 ESCAPE '\\'",
    )?;
    stmt.execute(params![pattern_win, pattern_posix])?;
    Ok(())
}
```

### 1d. New helpers: `is_indexed`, `children_of`, `BatchTx`
Add these imports at the top of the file, then the new code at the bottom.

```rust
use std::collections::HashSet;
```

```rust
/// True if `path` already has a row.
pub fn is_indexed(conn: &Connection, path: &str) -> bool {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM files WHERE path = ?1)",
        [path],
        |r| r.get::<_, i64>(0),
    )
    .map(|v| v != 0)
    .unwrap_or(false)
}

/// Paths of the direct children of `folder` (empty set on any error).
pub fn children_of(conn: &Connection, folder: &str) -> HashSet<String> {
    let mut out = HashSet::new();
    if let Ok(mut stmt) = conn.prepare_cached("SELECT path FROM files WHERE parent = ?1") {
        if let Ok(rows) = stmt.query_map([folder], |r| r.get::<_, String>(0)) {
            out.extend(rows.flatten());
        }
    }
    out
}

/// Commits every `BATCH` writes so a long job never holds the write lock for
/// minutes (which made watcher writes time out). Commits on drop.
pub struct BatchTx<'a> {
    conn: &'a Connection,
    pending: u32,
    open: bool,
}

const BATCH: u32 = 500;

impl<'a> BatchTx<'a> {
    pub fn new(conn: &'a Connection) -> Self {
        let open = conn.execute_batch("BEGIN").is_ok();
        BatchTx { conn, pending: 0, open }
    }

    pub fn conn(&self) -> &Connection {
        self.conn
    }

    /// Call after each write.
    pub fn tick(&mut self) {
        self.pending += 1;
        if self.pending >= BATCH {
            if self.open {
                let _ = self.conn.execute_batch("COMMIT");
            }
            self.pending = 0;
            self.open = self.conn.execute_batch("BEGIN").is_ok();
        }
    }

    pub fn finish(mut self) {
        if self.open {
            let _ = self.conn.execute_batch("COMMIT");
            self.open = false;
        }
    }
}

impl Drop for BatchTx<'_> {
    fn drop(&mut self) {
        if self.open {
            let _ = self.conn.execute_batch("COMMIT");
        }
    }
}
```

---

## 2. `crawler.rs`: shared helpers

Update the imports:

```rust
use crate::search_engine::database::{self, BatchTx, FileEntry};
```

Add:

```rust
/// True if `path` OR any ancestor below its root is hidden.
/// (The old `is_path_hidden` only checked the leaf, so files inside hidden
/// folders such as AppData were indexed by the watcher.)
pub fn is_hidden_in_roots(path: &Path, roots: &[PathBuf]) -> bool {
    let root = match roots.iter().find(|r| path.starts_with(r)) {
        Some(r) => r,
        None => return is_path_hidden(path),
    };
    let mut cur = Some(path);
    while let Some(p) = cur {
        if p == root.as_path() {
            break;
        }
        if is_path_hidden(p) {
            return true;
        }
        cur = p.parent();
    }
    false
}

/// True if any directory component below the root is excluded. The leaf is
/// only checked when it is a directory, matching how the crawler works.
/// Components ABOVE the root are ignored, so a home path containing e.g.
/// "target" no longer disables indexing.
pub fn is_excluded_in_roots(
    path: &Path,
    is_dir: bool,
    roots: &[PathBuf],
    exclude_dirs: &[String],
) -> bool {
    let rel = match roots.iter().find_map(|r| path.strip_prefix(r).ok()) {
        Some(r) => r,
        None => return true, // outside every root
    };
    let count = rel.components().count();
    rel.components().enumerate().any(|(i, c)| {
        if i + 1 == count && !is_dir {
            return false;
        }
        is_excluded_name(&c.as_os_str().to_string_lossy(), exclude_dirs)
    })
}

/// Index `start` and everything under it. Never follows links, so it cannot
/// loop on symlink/junction cycles. Returns the number of rows written.
pub fn index_tree(tx: &mut BatchTx, start: &Path, exclude_dirs: &[String]) -> u64 {
    let mut n = 0;
    let walker = WalkDir::new(start).follow_links(false).into_iter();
    for result in walker.filter_entry(|e| e.depth() == 0 || !should_skip(e, exclude_dirs)) {
        let entry = match result {
            Ok(e) => e,
            Err(_) => continue,
        };
        let meta = match entry.metadata() {
            Ok(m) => m,
            Err(_) => continue,
        };
        let is_dir = entry.file_type().is_dir();
        let fe = FileEntry {
            name: entry.file_name().to_string_lossy().to_string(),
            path: entry.path().to_string_lossy().to_string(),
            extension: if is_dir {
                None
            } else {
                entry.path().extension().map(|e| e.to_string_lossy().to_string())
            },
            is_dir,
            created_at: meta.created().ok(),
            modified_at: meta.modified().ok(),
        };
        if database::upsert_entry(tx.conn(), &fe).is_ok() {
            tx.tick();
            n += 1;
        }
    }
    n
}
```

---

## 3. `watcher.rs`: full replacement

Fixes: hidden ancestors, symlink recursion (`symlink_metadata` plus `index_tree` instead of recursion), per-batch transactions, exclude checks scoped to the root, no re-walk of existing folders on every mtime change, and no indexing of the root itself.

```rust
// watcher.rs — Real-time filesystem watcher (notify-debouncer-mini).

use crate::search_engine::crawler;
use crate::search_engine::database::{self, BatchTx, FileEntry};
use notify_debouncer_mini::{new_debouncer, notify::RecursiveMode};
use std::path::{Path, PathBuf};
use std::sync::mpsc::channel;
use std::time::Duration;

pub fn start_watcher(db_path: PathBuf, roots: Vec<PathBuf>, exclude_dirs: Vec<String>) {
    std::thread::spawn(move || {
        run_watcher(&db_path, &roots, &exclude_dirs);
    });
}

fn run_watcher(db_path: &Path, roots: &[PathBuf], exclude_dirs: &[String]) {
    if roots.is_empty() {
        return;
    }

    let conn = match database::open(db_path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[watcher] Could not open database connection: {e}");
            return;
        }
    };

    let (tx, rx) = channel();
    let mut debouncer = match new_debouncer(Duration::from_millis(500), tx) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("[watcher] Could not initialize filesystem debouncer: {e}");
            return;
        }
    };

    for root in roots {
        if root.exists() {
            if let Err(e) = debouncer.watcher().watch(root, RecursiveMode::Recursive) {
                eprintln!("[watcher] Could not watch {}: {e}", root.display());
            } else {
                eprintln!("[watcher] Watching {} for changes", root.display());
            }
        }
    }

    while let Ok(event_result) = rx.recv() {
        match event_result {
            Ok(events) => {
                // One transaction per debounced batch instead of one fsync per event.
                let mut batch = BatchTx::new(&conn);
                for event in events {
                    handle_path_change(&mut batch, &event.path, roots, exclude_dirs);
                }
                batch.finish();
            }
            Err(e) => eprintln!("[watcher] Watch error: {e:?}"),
        }
    }
}

fn remove_subtree(tx: &mut BatchTx, path_str: &str) {
    let _ = database::delete_by_path(tx.conn(), path_str);
    let _ = database::delete_by_path_prefix(tx.conn(), path_str);
    tx.tick();
}

fn handle_path_change(
    tx: &mut BatchTx,
    path: &Path,
    roots: &[PathBuf],
    exclude_dirs: &[String],
) {
    // The root itself is never an index row.
    if roots.iter().any(|r| r.as_path() == path) {
        return;
    }
    let path_str = path.to_string_lossy().to_string();

    // symlink_metadata: never follow links (matches the crawler's rule and
    // makes symlink cycles impossible).
    let meta = match std::fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            remove_subtree(tx, &path_str); // deleted or renamed away
            return;
        }
        Err(_) => return, // permission etc.: leave the index untouched
    };
    let is_dir = meta.is_dir();

    if crawler::is_excluded_in_roots(path, is_dir, roots, exclude_dirs)
        || crawler::is_hidden_in_roots(path, roots)
    {
        // Previously indexed but now hidden/excluded: drop it.
        remove_subtree(tx, &path_str);
        return;
    }

    // A directory we have never seen (created, copied in, renamed into place)
    // is indexed with one safe walk. Known directories only get a row update,
    // so every mtime change does not re-walk the whole subtree.
    if is_dir && !database::is_indexed(tx.conn(), &path_str) {
        crawler::index_tree(tx, path, exclude_dirs);
        return;
    }

    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| path_str.clone());
    let entry = FileEntry {
        name,
        path: path_str.clone(),
        extension: if is_dir {
            None
        } else {
            path.extension().map(|e| e.to_string_lossy().to_string())
        },
        is_dir,
        created_at: meta.created().ok(),
        modified_at: meta.modified().ok(),
    };
    match database::upsert_entry(tx.conn(), &entry) {
        Ok(()) => tx.tick(),
        Err(e) => eprintln!("[watcher] Upsert error for {path_str}: {e}"),
    }
}
```

---

## 4. `reconcile.rs`: full replacement

Fixes:
- The fallback now **removes deleted children**.
- The fallback now **indexes the contents of newly discovered folders**.
- The startup reconcile commits in batches, so it no longer holds one long write lock.
- A missing or unreadable folder only purges on `NotFound`.
- Rows under an **unplugged root** are kept, while rows under a root **removed from config** are purged.

```rust
// reconcile.rs — Startup reconcile and cheap 3-minute fallback.

use crate::search_engine::crawler;
use crate::search_engine::database::{self, BatchTx, FileEntry};
use rusqlite::Connection;
use std::collections::{HashMap, HashSet};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};
use walkdir::WalkDir;

enum Placement {
    UnderLiveRoot,
    UnderMissingRoot, // drive unplugged / share offline: keep the rows
    Orphan,           // no configured root contains it: root was removed
}

fn root_status(roots: &[PathBuf]) -> Vec<(PathBuf, bool)> {
    roots.iter().map(|r| (r.clone(), r.exists())).collect()
}

fn classify(path: &Path, roots: &[(PathBuf, bool)]) -> Placement {
    let mut under_missing = false;
    for (r, live) in roots {
        if path.starts_with(r) {
            if *live {
                return Placement::UnderLiveRoot;
            }
            under_missing = true;
        }
    }
    if under_missing { Placement::UnderMissingRoot } else { Placement::Orphan }
}

fn mtime_secs(meta: &std::fs::Metadata) -> Option<i64> {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
}

fn is_not_found(path: &Path) -> bool {
    matches!(std::fs::symlink_metadata(path), Err(e) if e.kind() == ErrorKind::NotFound)
}

// ---------------------------------------------------------------------------
// Startup reconcile
// ---------------------------------------------------------------------------

pub fn run_startup_reconcile(db_path: &Path, roots: &[PathBuf], exclude_dirs: &[String]) {
    if roots.is_empty() {
        return; // never purge the index just because roots failed to resolve
    }
    let conn = match database::open(db_path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[reconcile] Could not open database: {e}");
            return;
        }
    };
    eprintln!("[reconcile] Starting startup reconcile …");
    let live = root_status(roots);

    // 1. Load what the index knows.
    let mut known: HashMap<String, Option<i64>> = HashMap::new();
    {
        let mut stmt = match conn.prepare("SELECT path, modified_at FROM files") {
            Ok(s) => s,
            Err(e) => {
                eprintln!("[reconcile] Could not query files: {e}");
                return;
            }
        };
        if let Ok(rows) = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<i64>>(1)?))) {
            for (p, m) in rows.flatten() {
                known.insert(p, m);
            }
        }
    }

    // 2. Purge rows that are really gone.
    let mut stale: Vec<String> = Vec::new();
    for p in known.keys() {
        match classify(Path::new(p), &live) {
            Placement::UnderMissingRoot => {}
            Placement::Orphan => stale.push(p.clone()),
            Placement::UnderLiveRoot => {
                if is_not_found(Path::new(p)) {
                    stale.push(p.clone());
                }
            }
        }
    }
    for chunk in stale.chunks(500) {
        if let Err(e) = database::delete_missing(&conn, chunk) {
            eprintln!("[reconcile] Error purging missing paths: {e}");
        }
    }

    // 3. Walk live roots for new/changed items, committing in batches.
    let mut tx = BatchTx::new(&conn);
    let mut updated: u64 = 0;
    for (root, is_live) in &live {
        if !*is_live {
            continue;
        }
        let walker = WalkDir::new(root).follow_links(false).into_iter();
        for result in walker.filter_entry(|e| {
            if e.depth() == 0 {
                return true;
            }
            let name = e.file_name().to_string_lossy();
            if e.file_type().is_dir() && crawler::is_excluded_name(&name, exclude_dirs) {
                return false;
            }
            !crawler::is_hidden(e)
        }) {
            let entry = match result {
                Ok(e) => e,
                Err(_) => continue,
            };
            if entry.depth() == 0 {
                continue;
            }
            let path_str = entry.path().to_string_lossy().to_string();
            let meta = match entry.metadata() {
                Ok(m) => m,
                Err(_) => continue,
            };
            let disk_mod = mtime_secs(&meta);
            let needs_update = match known.get(&path_str) {
                None => true,
                Some(stored) => disk_mod != *stored,
            };
            if !needs_update {
                continue;
            }
            let is_dir = entry.file_type().is_dir();
            let fe = FileEntry {
                name: entry.file_name().to_string_lossy().to_string(),
                path: path_str,
                extension: if is_dir {
                    None
                } else {
                    entry.path().extension().map(|e| e.to_string_lossy().to_string())
                },
                is_dir,
                created_at: meta.created().ok(),
                modified_at: meta.modified().ok(),
            };
            if database::upsert_entry(tx.conn(), &fe).is_ok() {
                tx.tick();
                updated += 1;
            }
        }
    }
    tx.finish();
    eprintln!(
        "[reconcile] Startup reconcile complete: removed {}, updated {updated} items.",
        stale.len()
    );
}

// ---------------------------------------------------------------------------
// 3-minute fallback
// ---------------------------------------------------------------------------

pub fn start_fallback_loop(db_path: PathBuf, roots: Vec<PathBuf>, exclude_dirs: Vec<String>) {
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_secs(180));
        run_cheap_fallback(&db_path, &roots, &exclude_dirs);
    });
}

fn run_cheap_fallback(db_path: &Path, roots: &[PathBuf], exclude_dirs: &[String]) {
    if roots.is_empty() {
        return;
    }
    let conn = match database::open(db_path) {
        Ok(c) => c,
        Err(_) => return,
    };
    let live = root_status(roots);

    let folders: Vec<(String, Option<i64>)> = {
        let mut stmt = match conn.prepare("SELECT path, modified_at FROM files WHERE is_dir = 1") {
            Ok(s) => s,
            Err(_) => return,
        };
        stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<i64>>(1)?)))
            .map(|rows| rows.flatten().collect())
            .unwrap_or_default()
    };

    let mut tx = BatchTx::new(&conn);
    for (folder_str, stored_mod) in folders {
        let folder = Path::new(&folder_str);

        match classify(folder, &live) {
            Placement::UnderMissingRoot => continue,
            Placement::Orphan => {
                let _ = database::delete_by_path(tx.conn(), &folder_str);
                let _ = database::delete_by_path_prefix(tx.conn(), &folder_str);
                tx.tick();
                continue;
            }
            Placement::UnderLiveRoot => {}
        }

        let meta = match std::fs::symlink_metadata(folder) {
            Ok(m) => m,
            Err(e) if e.kind() == ErrorKind::NotFound => {
                let _ = database::delete_by_path(tx.conn(), &folder_str);
                let _ = database::delete_by_path_prefix(tx.conn(), &folder_str);
                tx.tick();
                continue;
            }
            Err(_) => continue, // permission / transient error: do NOT purge
        };

        let current = mtime_secs(&meta);
        if current == stored_mod {
            continue;
        }

        sync_folder(&mut tx, folder, &folder_str, roots, exclude_dirs);

        let _ = tx.conn().execute(
            "UPDATE files SET modified_at = ?1 WHERE path = ?2",
            rusqlite::params![current, folder_str],
        );
        tx.tick();
    }
    tx.finish();
}

/// Make the index's view of `folder`'s direct children match the disk:
/// upsert what exists, fully index NEW sub-folders, remove what vanished.
fn sync_folder(
    tx: &mut BatchTx,
    folder: &Path,
    folder_str: &str,
    roots: &[PathBuf],
    exclude_dirs: &[String],
) {
    let read = match std::fs::read_dir(folder) {
        Ok(r) => r,
        Err(_) => return, // unreadable: keep the index as it is
    };
    let db_children: HashSet<String> = database::children_of(tx.conn(), folder_str);
    let mut on_disk: HashSet<String> = HashSet::new();
    let mut complete = true; // only purge if we saw the whole directory

    for res in read {
        let child = match res {
            Ok(c) => c,
            Err(_) => {
                complete = false;
                continue;
            }
        };
        let p = child.path();
        let meta = match std::fs::symlink_metadata(&p) {
            Ok(m) => m,
            Err(_) => {
                complete = false;
                continue;
            }
        };
        let is_dir = meta.is_dir();
        if crawler::is_excluded_in_roots(&p, is_dir, roots, exclude_dirs)
            || crawler::is_hidden_in_roots(&p, roots)
        {
            continue; // not in on_disk, so an old row for it is purged below
        }
        let ps = p.to_string_lossy().to_string();
        on_disk.insert(ps.clone());

        if is_dir && !db_children.contains(&ps) {
            // New folder: index its whole subtree now. Otherwise its contents
            // would never be scanned (its mtime would already match next cycle).
            crawler::index_tree(tx, &p, exclude_dirs);
        } else {
            let fe = FileEntry {
                name: child.file_name().to_string_lossy().to_string(),
                path: ps,
                extension: if is_dir {
                    None
                } else {
                    p.extension().map(|e| e.to_string_lossy().to_string())
                },
                is_dir,
                created_at: meta.created().ok(),
                modified_at: meta.modified().ok(),
            };
            if database::upsert_entry(tx.conn(), &fe).is_ok() {
                tx.tick();
            }
        }
    }

    if complete {
        for gone in db_children.difference(&on_disk) {
            let _ = database::delete_by_path(tx.conn(), gone);
            let _ = database::delete_by_path_prefix(tx.conn(), gone);
            tx.tick();
        }
    }
}
```

Known limitation: the root folder's own direct children are covered by the watcher and startup reconcile, but not by the fallback, because roots have no index row to compare against.

---

## 5. Converter

### 5a. `converter/mod.rs`: full replacement
Fixes:
- It is split so the DB lock is **not** held during encoding.
- It never overwrites an existing file.
- The index lookup prefers the newest match and uses Unicode lowercase.
- `jpg`/`jpeg`/`jfif` and `tif`/`tiff` count as the same format.
- A failed conversion leaves no partial file behind.

```rust
// converter/mod.rs — File conversion (Phase 4).

pub mod image_conv;
pub mod registry;

use rusqlite::Connection;
use serde::Serialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize)]
pub struct ConversionResult {
    pub source_path: String,
    pub target_path: String,
    pub message: String,
}

/// Step 1 (needs the DB, fast): turn user text into a real file path.
pub fn resolve_source(source: &str, conn: Option<&Connection>) -> Result<PathBuf, String> {
    let trimmed = source.trim().trim_matches('"');
    if trimmed.is_empty() {
        return Err("Please provide a file to convert.".to_string());
    }

    let direct = PathBuf::from(trimmed);
    if direct.is_file() {
        return Ok(direct);
    }

    // Must match how the index stores names: Unicode lowercase.
    let file_name = direct
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(trimmed)
        .to_lowercase();

    if let Some(c) = conn {
        // Newest match first, so "image.png" resolves to the one you used last.
        if let Ok(mut stmt) = c.prepare_cached(
            "SELECT path FROM files
             WHERE is_dir = 0 AND name_lower = ?1
             ORDER BY modified_at DESC
             LIMIT 5",
        ) {
            if let Ok(rows) = stmt.query_map([&file_name], |r| r.get::<_, String>(0)) {
                for p in rows.flatten() {
                    let pb = PathBuf::from(p);
                    if pb.is_file() {
                        return Ok(pb);
                    }
                }
            }
        }
    }

    Err(format!(
        "Could not find \"{trimmed}\" on your computer. \
         Make sure the file still exists or enter its full path."
    ))
}

fn canonical_ext(e: &str) -> &str {
    match e {
        "jpeg" | "jfif" => "jpg",
        "tif" => "tiff",
        other => other,
    }
}

fn unique_destination(parent: &Path, stem: &str, ext: &str) -> Result<PathBuf, String> {
    let first = parent.join(format!("{stem}.{ext}"));
    if !first.exists() {
        return Ok(first);
    }
    for n in 1..1000 {
        let candidate = parent.join(format!("{stem} ({n}).{ext}"));
        if !candidate.exists() {
            return Ok(candidate);
        }
    }
    Err(format!(
        "Too many files named \"{stem}\" already exist next to the original. Clean some up and try again."
    ))
}

/// Step 2 (slow, no DB needed): do the actual conversion.
pub fn convert_resolved(source_path: &Path, target_ext: &str) -> Result<ConversionResult, String> {
    let from_ext = source_path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if from_ext.is_empty() {
        return Err(format!(
            "File \"{}\" has no file extension. WayFinder cannot determine its format.",
            source_path.display()
        ));
    }

    let clean_target = target_ext.trim().trim_start_matches('.').to_ascii_lowercase();
    if clean_target.is_empty() {
        return Err("Please specify a target format (e.g. \"png to jpg\").".to_string());
    }
    if canonical_ext(&from_ext) == canonical_ext(&clean_target) {
        return Err(format!(
            "\".{from_ext}\" and \".{clean_target}\" are the same format. Nothing to convert."
        ));
    }

    let converter = registry::find_converter(&from_ext, &clean_target).ok_or_else(|| {
        format!("Conversion from \".{from_ext}\" to \".{clean_target}\" is not supported yet.")
    })?;

    let stem = source_path.file_stem().and_then(|s| s.to_str()).unwrap_or("converted");
    let parent = source_path.parent().unwrap_or_else(|| Path::new("."));
    let dest_path = unique_destination(parent, stem, &clean_target)?;

    if let Err(e) = (converter.convert)(source_path, &dest_path) {
        let _ = std::fs::remove_file(&dest_path); // never leave a half-written file
        return Err(e);
    }

    let target_name = dest_path.file_name().and_then(|n| n.to_str()).unwrap_or("file");
    Ok(ConversionResult {
        source_path: source_path.display().to_string(),
        target_path: dest_path.display().to_string(),
        message: format!("Saved {target_name}"),
    })
}
```

### 5b. `registry.rs`: make Rust match what the UI advertises
```rust
to_extensions: &["png", "jpg", "jpeg", "jfif", "webp", "bmp", "tiff", "tif"],
```

### 5c. `image_conv.rs`: full replacement
Fixes:
- Alpha images no longer fail when saved as JPEG; transparency is flattened onto white instead of black.
- WebP and BMP get a color type their encoders accept.
- 16-bit sources are handled.

```rust
// image_conv.rs — Pure-Rust image conversion using the `image` crate.

use image::{DynamicImage, ImageFormat, Rgb, RgbImage};
use std::path::Path;

/// Composite transparency onto white (JPEG has no alpha channel).
fn flatten_on_white(img: &DynamicImage) -> RgbImage {
    let rgba = img.to_rgba8();
    let (w, h) = rgba.dimensions();
    let mut out = RgbImage::new(w, h);
    for (x, y, p) in rgba.enumerate_pixels() {
        let a = p[3] as u32;
        let blend = |c: u8| ((c as u32 * a + 255 * (255 - a)) / 255) as u8;
        out.put_pixel(x, y, Rgb([blend(p[0]), blend(p[1]), blend(p[2])]));
    }
    out
}

pub fn convert_image(source: &Path, destination: &Path) -> Result<(), String> {
    if !source.exists() {
        return Err(format!(
            "Source file \"{}\" does not exist. Check that the file was not moved or deleted.",
            source.display()
        ));
    }

    let img = image::open(source).map_err(|e| {
        format!(
            "Could not read image \"{}\": {e}. Check that the file is a valid image and not corrupted.",
            source.display()
        )
    })?;

    let ext = destination
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();

    let format = match ext.as_str() {
        "png" => ImageFormat::Png,
        "jpg" | "jpeg" | "jfif" => ImageFormat::Jpeg,
        "webp" => ImageFormat::WebP,
        "bmp" => ImageFormat::Bmp,
        "tiff" | "tif" => ImageFormat::Tiff,
        _ => {
            return Err(format!(
                "Unsupported destination image format \".{ext}\". Supported formats: png, jpg, webp, bmp, tiff."
            ));
        }
    };

    if let Some(parent) = destination.parent() {
        if !parent.as_os_str().is_empty() && !parent.exists() {
            std::fs::create_dir_all(parent).map_err(|e| {
                format!("Could not create folder \"{}\": {e}. Check folder permissions.", parent.display())
            })?;
        }
    }

    // Hand each encoder a colour type it actually supports.
    let prepared = match format {
        ImageFormat::Jpeg => DynamicImage::ImageRgb8(flatten_on_white(&img)),
        ImageFormat::WebP | ImageFormat::Bmp => {
            if img.color().has_alpha() {
                DynamicImage::ImageRgba8(img.to_rgba8())
            } else {
                DynamicImage::ImageRgb8(img.to_rgb8())
            }
        }
        _ => img,
    };

    prepared.save_with_format(destination, format).map_err(|e| {
        format!(
            "Could not write converted image to \"{}\": {e}. Check disk space and permissions.",
            destination.display()
        )
    })
}
```

### 5d. `Cargo.toml`: confirm the feature flags
```toml
image = { version = "0.25", default-features = false, features = ["png", "jpeg", "webp", "bmp", "tiff"] }
```

---

## 6. `config.rs`

Fixes: a partial config keeps its valid fields, and problems are returned so the UI can show them.

Replace the struct header and drop the per-field default:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)] // any missing field falls back to Default::default()
pub struct Config {
    pub version: u32,
    pub hotkey: String,
    pub index_roots: Vec<String>,
    pub exclude_dirs: Vec<String>,
    pub hide_on_blur: bool,
}
// delete: fn default_hide_on_blur() -> bool { true }
```

Safer default excludes: a normal folder called "Target" is no longer skipped.
```rust
exclude_dirs: vec!["node_modules".to_string(), ".git".to_string()],
```
This only affects new config files. Existing users can delete `"target"` from their own `config.json`.

New `load` signature, returning a warning to show:

```rust
pub fn load(dir: &PathBuf) -> (Config, Option<String>) {
    let path = dir.join("config.json");

    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let d = Config::default();
            let warn = save(dir, &d)
                .err()
                .map(|we| format!("Could not write the default config: {we}"));
            return (d, warn);
        }
        Err(e) => {
            return (
                Config::default(),
                Some(format!("Could not read {}: {e}. Using built-in defaults.", path.display())),
            );
        }
    };

    match serde_json::from_str::<Config>(&text) {
        Ok(mut cfg) => {
            let mut warn = None;
            if cfg.version < CURRENT_VERSION {
                cfg = upgrade(cfg);
                if let Err(e) = save(dir, &cfg) {
                    warn = Some(format!("Could not save the upgraded config: {e}"));
                }
            }
            (cfg, warn)
        }
        Err(e) => (
            Config::default(),
            Some(format!(
                "Your config.json ({}) is not valid JSON for WayFinder: {e}. \
                 Built-in defaults are in use until it is fixed.",
                path.display()
            )),
        ),
    }
}
```

---

## 7. `main.rs`

### 7a. Imports
Delete the duplicate `use std::sync::Arc; use std::time::Instant;` inside `main()` and update the top:

```rust
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tauri::{Emitter, Manager, State};
```

### 7b. New state types
```rust
/// Set to true when the first crawl (or startup reconcile) has finished.
struct CrawlDone(Arc<AtomicBool>);

/// A one-shot message the frontend shows in the error box at startup.
struct StartupWarning(Mutex<Option<String>>);

#[derive(serde::Serialize)]
struct IndexStatus {
    has_rows: bool,
    complete: bool,
}
```

### 7c. Commands

Replace `index_ready` with `index_status`:

```rust
#[tauri::command(async)]
fn index_status(db: State<'_, Db>, done: State<'_, CrawlDone>) -> Result<IndexStatus, String> {
    let conn = db.0.lock().map_err(|e| format!("The file index is busy, try again: {e}"))?;
    let has_rows: i64 = conn
        .query_row("SELECT EXISTS(SELECT 1 FROM files)", [], |r| r.get(0))
        .map_err(|e| format!("Could not check the file index: {e}"))?;
    Ok(IndexStatus { has_rows: has_rows != 0, complete: done.0.load(Ordering::Acquire) })
}

#[tauri::command]
fn take_startup_warning(w: State<'_, StartupWarning>) -> Option<String> {
    w.0.lock().ok().and_then(|mut g| g.take())
}
```

`open_path` now only opens paths that are in the index, so an injected string can't launch arbitrary files:

```rust
#[tauri::command(async)]
fn open_path(app: tauri::AppHandle, db: State<'_, Db>, path: String) -> Result<(), String> {
    {
        let conn = db.0.lock().map_err(|e| format!("The file index is busy, try again: {e}"))?;
        if !search_engine::database::is_indexed(&conn, &path) {
            return Err(format!("\"{path}\" is not in the file index, so WayFinder will not open it."));
        }
    } // lock released before launching

    app.opener()
        .open_path(path.clone(), None::<&str>)
        .map_err(|e| format!("Windows could not open \"{path}\": {e}. Check that it still exists and that you can open it."))
}
```

New `open_url`. It doesn't depend on `withGlobalTauri` or JS opener permissions, and only allows web links:

```rust
#[tauri::command]
fn open_url(app: tauri::AppHandle, url: String) -> Result<(), String> {
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return Err("Only web addresses (http/https) can be opened.".to_string());
    }
    app.opener()
        .open_url(url, None::<&str>)
        .map_err(|e| format!("Could not open your web browser: {e}"))
}
```

`convert_file` no longer holds the DB lock during encoding:

```rust
#[tauri::command(async)]
fn convert_file(
    source: String,
    target_ext: String,
    db: State<'_, Db>,
) -> Result<converter::ConversionResult, String> {
    // Hold the lock only for the quick lookup...
    let source_path = {
        let guard = db.0.lock().ok();
        converter::resolve_source(&source, guard.as_deref())?
    }; // ...released here, before the slow encode.
    converter::convert_resolved(&source_path, &target_ext)
}
```

### 7d. Hotkey handler
Fixes: the timestamp is recorded before `show()`, a visible-but-unfocused window is brought to the front instead of hidden, and the frontend is told to reset.

```rust
.with_handler(move |app, _shortcut, event| {
    if event.state != ShortcutState::Pressed {
        return;
    }
    if let Some(window) = app.get_webview_window("main") {
        let visible = window.is_visible().unwrap_or(false);
        let focused = window.is_focused().unwrap_or(true);
        if visible && focused {
            let _ = window.hide();
        } else {
            // Record BEFORE showing so the blur guard cannot race us.
            if let Ok(mut t) = last_shown_shortcut.lock() {
                *t = Instant::now();
            }
            let _ = window.show();
            let _ = window.set_focus();
            let _ = app.emit("launcher-shown", ());
        }
    }
})
```

### 7e. `setup`: config warning, hotkey warning, crawl flag, fallback roots

```rust
let (cfg, cfg_warning) = config::load(&data_dir);
let mut warning = cfg_warning;

use tauri_plugin_global_shortcut::GlobalShortcutExt;
if let Err(e) = app.global_shortcut().register(cfg.hotkey.as_str()) {
    let msg = format!(
        "Could not register the hotkey \"{}\": {e}. Another program may be using it.",
        cfg.hotkey
    );
    eprintln!("[main] {msg}");
    warning.get_or_insert(msg);
}
app.manage(StartupWarning(Mutex::new(warning)));
```

Replace the background-thread block:

```rust
let crawl_done = Arc::new(AtomicBool::new(!needs_crawl));
app.manage(CrawlDone(Arc::clone(&crawl_done)));

let db_path_bg = db_path.clone();
let index_roots_bg = index_roots.clone();
let exclude_dirs_bg = exclude_dirs.clone();
let done_bg = Arc::clone(&crawl_done);

std::thread::spawn(move || {
    if needs_crawl {
        crawler::run_crawl(&db_path_bg, &index_roots_bg, &exclude_dirs_bg);
    } else {
        reconcile::run_startup_reconcile(&db_path_bg, &index_roots_bg, &exclude_dirs_bg);
    }
    done_bg.store(true, Ordering::Release); // also true if the crawl failed, so the UI never polls forever
});

watcher::start_watcher(db_path.clone(), index_roots.clone(), exclude_dirs.clone());

// fallback now needs the roots (to tell "folder deleted" from "drive unplugged")
reconcile::start_fallback_loop(db_path, index_roots, exclude_dirs);
```

### 7f. Handler list
```rust
.invoke_handler(tauri::generate_handler![
    search_files,
    search_folders,
    index_status,        // replaces index_ready
    take_startup_warning,
    hide_window,
    open_path,
    open_url,
    convert_file,
])
```

---

## 8. `script.js`

### 8a. Unit conversion: prototype keys and the `oz` constant
```js
const UNITS = {
    length: { m: 1, km: 1000, cm: 0.01, mi: 1609.344, ft: 0.3048, in: 0.0254 },
    mass: { kg: 1, g: 0.001, lb: 0.45359237, oz: 0.028349523125 },
};

const has = (obj, key) => Object.hasOwn(obj, key);

function convertUnits(value, from, to) {
    if (has(TEMPS, from) && has(TEMPS, to)) return TEMPS[to][1](TEMPS[from][0](value));
    for (const group of Object.values(UNITS)) {
        if (has(group, from) && has(group, to)) return (value * group[from]) / group[to];
    }
    return null;
}
```
And in `searchConvert`:
```js
if (result === null || !Number.isFinite(result)) return [];
```

### 8b. `update()`: a thrown search no longer leaves a stale list
```js
let result = [];
if (query) {
    try {
        result = await COMMANDS[mode].search(query);
    } catch (err) {
        console.error('[search]', err);
        result = [];
    }
}
if (ticket !== searchTicket) return;
```

### 8c. IME-safe Enter
At the top of `onKeydown`:
```js
function onKeydown(e) {
    if (e.isComposing) return; // Japanese/Chinese/Korean IME confirmation
    ...
```

### 8d. `openUrl` goes through Rust
```js
async function openUrl(url) {
    try {
        await invoke('open_url', { url });
    } catch (err) {
        showError(`${err}`);
    }
}
```

### 8e. Reset on show: replace the `window` `focus` listener
Replace the old `window.addEventListener('focus', ...)` block with:

```js
function resetLauncher() {
    launcher.classList.remove('closing');
    launcher.hidden = false;
    // Replay the open sweep even if the element was never display:none.
    launcher.style.animation = 'none';
    void launcher.offsetWidth;
    launcher.style.animation = '';
    setMode(mode);   // clears the query and re-renders
    input.focus();
}

// Rust emits this when the hotkey shows the window.
const tauriListen = window.__TAURI__?.event?.listen;
if (tauriListen) tauriListen('launcher-shown', resetLauncher);
```

### 8f. Index status polling: replace `checkIndexReady`
```js
let indexComplete = true;

async function checkIndexReady() {
    try {
        const s = await invoke('index_status');
        const wasIndexing = indexing;
        // Only block search while there is nothing to search yet.
        indexing = !s.complete && !s.has_rows;

        if (!s.complete) {
            indexComplete = false;
            setTimeout(checkIndexReady, 2000);   // keep watching until the crawl ends
        } else if (!indexComplete) {
            indexComplete = true;
            say('Index complete');
        }
        if (wasIndexing && !indexing) update();
    } catch (_err) {
        indexing = false; // never block search because of a status failure
    }
}
```

### 8g. Startup warning (bootstrap section)
```js
buildMenu();
setMode('fil');
checkIndexReady();
invoke('take_startup_warning').then((msg) => msg && showError(msg)).catch(() => {});
```

---

## 9. `search.rs` (low priority)

Shortest names first in the prefix fetch, so `report.docx` can't be cut off by 50 longer names. The contains and fuzzy fetches prefer recent files.

```rust
// fetch_prefix
"SELECT name, path, extension, is_dir, modified_at
 FROM files
 WHERE is_dir = ?1 AND name_lower >= ?2 AND name_lower < ?3
 ORDER BY length(name_lower), name_lower
 LIMIT ?4"

// fetch_contains and fetch_fuzzy: add before LIMIT
"... ORDER BY modified_at DESC LIMIT ?3"
```
Count characters, not bytes, in `search_internal`:
```rust
if candidates.len() < MAX_RESULTS && q.chars().count() >= 2 && q.chars().count() <= 12 {
```

---

## Verification checklist

1. **Schema:** after the first launch, the log shows a full crawl, since version 3 drops the old table.
2. **LIKE-prefix fix:** index a folder with files, delete the folder in Explorer, wait about 1 s, and search for a file that was inside it. It should be gone.
3. **Hidden folders:** edit a file inside `%APPDATA%` and confirm no new row appears.
4. **Symlink cycle:** `mklink /J C:\Users\you\loop C:\Users\you` (or `ln -s`), then create a file in the home folder. The watcher must keep running with no stack overflow.
5. **Fallback deletes:** stop the app, delete a file, start it, then delete another file while a watcher event is missed (or wait out one 3-minute cycle). Both should disappear.
6. **New folder via fallback:** copy a folder with files into a watched root, and confirm its files become searchable.
7. **Converter overwrite:** convert `a.png` to jpg twice. The second run should produce `a (1).jpg` and leave the first untouched.
8. **Alpha to JPEG:** convert a transparent PNG to jpg. It should succeed with a white background.
9. **Lock:** convert a large TIFF and keep typing in file search. Searches should stay responsive.
10. **Config:** delete `exclude_dirs` from `config.json`. The app should still run with your hotkey, and a bad JSON file should show the error box.
11. **Index status:** on a fresh index, search should work as soon as the first rows land, and the status line should say "Index complete" at the end. If the crawl fails or the roots are empty, the "Indexing…" row must not stay forever.
12. **Hotkey:** press the hotkey when the window is visible but behind another app. It should come to the front instead of hiding.
