// database.rs — SQLite schema and helpers for the file index.
// All functions return Result so callers can handle errors without crashing.

use rusqlite::{params, Connection, Result};
use std::collections::HashSet;
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Bump this when the table layout changes. The index is only a cache of what
/// is on disk, so an old layout is dropped and rebuilt by a fresh crawl.
const SCHEMA_VERSION: i64 = 4;

/// One row that gets written to the `files` table.
pub struct FileEntry {
    pub name: String,
    pub path: String,
    pub extension: Option<String>,
    pub is_dir: bool,
    pub created_at: Option<SystemTime>,
    pub modified_at: Option<SystemTime>,
}

/// Open the index database. Every part of the app that needs the database
/// calls this, so settings live in one place.
/// WAL mode: one connection can write (the crawler) while another reads
/// (searches), so searching never waits for a crawl.
pub fn open(db_path: &Path) -> Result<Connection> {
    let conn = Connection::open(db_path)?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.busy_timeout(Duration::from_secs(15))?;
    Ok(conn)
}

/// Create the files and apps tables and indexes. Safe to call on every launch.
/// If the table layout is older than SCHEMA_VERSION it is dropped and rebuilt
/// (the app then crawls again, because the table is empty).
pub fn create_table(conn: &Connection) -> Result<()> {
    let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if version < SCHEMA_VERSION {
        conn.execute_batch(
            "DROP TABLE IF EXISTS files;
             DROP TABLE IF EXISTS apps;",
        )?;
    }

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
        CREATE INDEX IF NOT EXISTS idx_files_parent ON files (parent);

        CREATE TABLE IF NOT EXISTS apps (
            id          INTEGER PRIMARY KEY AUTOINCREMENT,
            name        TEXT NOT NULL,
            name_lower  TEXT NOT NULL,
            path        TEXT NOT NULL UNIQUE,
            usage_count INTEGER NOT NULL DEFAULT 0
        );
        CREATE INDEX IF NOT EXISTS idx_apps_name ON apps (name_lower);
        CREATE INDEX IF NOT EXISTS idx_apps_usage ON apps (usage_count DESC);",
    )?;

    conn.execute_batch(&format!("PRAGMA user_version = {SCHEMA_VERSION};"))?;
    Ok(())
}

fn parent_of(path: &str) -> Option<String> {
    Path::new(path).parent().map(|p| p.to_string_lossy().to_string())
}

/// Insert one entry. Duplicate paths are ignored.
/// `prepare_cached` reuses the compiled statement, which matters when the
/// crawler inserts hundreds of thousands of rows.
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

/// Delete every row whose path is in `paths` (used by startup reconcile, Phase 5).
pub fn delete_missing(conn: &Connection, paths: &[String]) -> Result<()> {
    if paths.is_empty() {
        return Ok(());
    }
    let placeholders: String = paths
        .iter()
        .enumerate()
        .map(|(i, _)| format!("?{}", i + 1))
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!("DELETE FROM files WHERE path IN ({})", placeholders);
    let params: Vec<&dyn rusqlite::ToSql> =
        paths.iter().map(|s| s as &dyn rusqlite::ToSql).collect();
    conn.execute(&sql, params.as_slice())?;
    Ok(())
}

/// Upsert one entry. If `path` already exists, update name, timestamps and flags.
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

/// Delete a single entry by exact path.
pub fn delete_by_path(conn: &Connection, path: &str) -> Result<()> {
    let mut stmt = conn.prepare_cached("DELETE FROM files WHERE path = ?1")?;
    stmt.execute(params![path])?;
    Ok(())
}

/// Delete all entries whose path starts with `dir_path` followed by a separator.
/// Used when a directory is removed or renamed so its children are also dropped.
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

/// Make `%`, `_` and `\` match literally in SQLite LIKE patterns.
pub fn escape_like(text: &str) -> String {
    text.replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

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
        let _ = stmt.query_map([folder], |r| r.get::<_, String>(0)).map(|rows| {
            out.extend(rows.flatten());
        });
    }
    out
}

fn begin_tx(conn: &Connection) -> bool {
    if let Err(e) = conn.execute_batch("BEGIN IMMEDIATE") {
        log::warn!("[database] Could not begin batch transaction: {e}");
        return false;
    }
    true
}

fn commit_tx(conn: &Connection) {
    if let Err(e) = conn.execute_batch("COMMIT") {
        log::warn!("[database] Could not commit batch transaction: {e}");
    }
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
        let open = begin_tx(conn);
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
                commit_tx(self.conn);
            }
            self.pending = 0;
            self.open = begin_tx(self.conn);
        }
    }

    pub fn finish(mut self) {
        if self.open {
            commit_tx(self.conn);
            self.open = false;
        }
    }
}

impl Drop for BatchTx<'_> {
    fn drop(&mut self) {
        if self.open {
            commit_tx(self.conn);
        }
    }
}

// --- helpers ---

fn unix_secs(t: Option<SystemTime>) -> Option<i64> {
    t.and_then(|st| st.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
}
