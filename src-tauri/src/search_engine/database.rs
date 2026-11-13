// database.rs — SQLite schema and helpers for the file index.
// All functions return Result so callers can handle errors without crashing.

use crate::search_engine::paths;
use rusqlite::{params, Connection, Result};
use std::collections::HashSet;
use std::path::Path;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Bump this when the table layout changes. The index is only a cache of what
/// is on disk, so an old layout is dropped and rebuilt by a fresh crawl.
const SCHEMA_VERSION: i64 = 5;

/// One row that gets written to the `files` table.
/// Path normalization is enforced in [`FileEntry::new`] so every writer stores canonical paths.
pub struct FileEntry {
    pub name: String,
    path: String,
    pub extension: Option<String>,
    pub is_dir: bool,
    pub created_at: Option<SystemTime>,
    pub modified_at: Option<SystemTime>,
}

impl FileEntry {
    pub fn new(
        name: String,
        path: &str,
        extension: Option<String>,
        is_dir: bool,
        created_at: Option<SystemTime>,
        modified_at: Option<SystemTime>,
    ) -> Self {
        Self {
            name,
            path: paths::normalize_path(path),
            extension,
            is_dir,
            created_at,
            modified_at,
        }
    }

    pub fn path(&self) -> &str {
        &self.path
    }
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
            path        TEXT NOT NULL UNIQUE COLLATE NOCASE,
            parent      TEXT COLLATE NOCASE,
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
            name_lower  TEXT NOT NULL UNIQUE,
            path        TEXT NOT NULL UNIQUE COLLATE NOCASE,
            usage_count INTEGER NOT NULL DEFAULT 0
        );
        CREATE INDEX IF NOT EXISTS idx_apps_usage ON apps (usage_count DESC);",
    )?;

    conn.execute_batch(&format!("PRAGMA user_version = {SCHEMA_VERSION};"))?;
    Ok(())
}

fn parent_of(path: &str) -> Option<String> {
    let norm = paths::normalize_path(path);
    Path::new(&norm)
        .parent()
        .map(|p| p.to_string_lossy().to_string())
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
/// Chunks at 500 parameters internally so callers never hit SQLite's bind-variable limit.
pub fn delete_missing(conn: &Connection, paths: &[String]) -> Result<()> {
    if paths.is_empty() {
        return Ok(());
    }
    for chunk in paths.chunks(500) {
        let normalized: Vec<String> = chunk.iter().map(|p| paths::normalize_path(p)).collect();
        let placeholders: String = normalized
            .iter()
            .enumerate()
            .map(|(i, _)| format!("?{}", i + 1))
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!("DELETE FROM files WHERE path IN ({})", placeholders);
        let params: Vec<&dyn rusqlite::ToSql> =
            normalized.iter().map(|s| s as &dyn rusqlite::ToSql).collect();
        conn.execute(&sql, params.as_slice())?;
    }
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
    let norm = paths::normalize_path(path);
    let mut stmt = conn.prepare_cached("DELETE FROM files WHERE path = ?1")?;
    stmt.execute(params![norm])?;
    Ok(())
}

/// Delete all entries whose path starts with `dir_path` followed by a separator.
/// Used when a directory is removed or renamed so its children are also dropped.
pub fn delete_by_path_prefix(conn: &Connection, dir_path: &str) -> Result<()> {
    let normalized = paths::normalize_path(dir_path);
    if normalized.is_empty() {
        return Ok(());
    }
    let escaped = escape_like(&normalized);
    // Drive roots (`C:\`) and bare root (`\`) already end with `\`.
    let pattern = if normalized.ends_with('\\') {
        format!("{escaped}%")
    } else {
        format!("{escaped}\\\\%")
    };
    let mut stmt = conn.prepare_cached("DELETE FROM files WHERE path LIKE ?1 ESCAPE '\\'")?;
    stmt.execute(params![pattern])?;
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
    let norm = paths::normalize_path(path);
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM files WHERE path = ?1)",
        [norm.as_str()],
        |r| r.get::<_, i64>(0),
    )
    .map(|v| v != 0)
    .unwrap_or(false)
}

/// Paths of the direct children of `folder` (empty set on any error).
pub fn children_of(conn: &Connection, folder: &str) -> HashSet<String> {
    let norm = paths::normalize_path(folder);
    let mut out = HashSet::new();
    if let Ok(mut stmt) = conn.prepare_cached("SELECT path FROM files WHERE parent = ?1") {
        let _ = stmt.query_map([norm.as_str()], |r| r.get::<_, String>(0)).map(|rows| {
            out.extend(rows.flatten());
        });
    }
    out
}

pub(crate) fn begin_immediate(conn: &Connection) -> Result<()> {
    if !conn.is_autocommit() {
        return Ok(());
    }
    conn.execute_batch("BEGIN IMMEDIATE")
}

pub(crate) fn rollback_tx(conn: &Connection) {
    if !conn.is_autocommit()
        && let Err(rb_err) = conn.execute_batch("ROLLBACK")
    {
        log::warn!("[database] Rollback failed: {rb_err}");
    }
}

/// Attempt to commit an open transaction, retrying once after a short backoff
/// on contention. If the retry still fails, rolls back and logs lost writes.
pub(crate) fn commit_or_rollback_res(conn: &Connection, pending: u32) -> Result<()> {
    if let Err(first_err) = conn.execute_batch("COMMIT") {
        log::warn!(
            "[database] Commit failed ({first_err}); retrying once ({pending} pending writes)..."
        );
        std::thread::sleep(Duration::from_millis(50));
        if let Err(second_err) = conn.execute_batch("COMMIT") {
            log::error!(
                "[database] Commit retry failed ({second_err}); rolling back and losing {pending} pending writes."
            );
            rollback_tx(conn);
            return Err(second_err);
        }
    }
    Ok(())
}

fn begin_tx(conn: &Connection) -> bool {
    if let Err(e) = begin_immediate(conn) {
        log::warn!("[database] Could not begin batch transaction: {e}");
        return false;
    }
    true
}

fn commit_or_rollback(conn: &Connection, pending: u32) {
    let _ = commit_or_rollback_res(conn, pending);
}

/// Rate-limits repeated error logs so a storm cannot flood the log file.
pub struct RateLimitedLog {
    interval: Duration,
    last_logged: Option<Instant>,
    suppressed: u64,
}

impl RateLimitedLog {
    pub fn new(interval: Duration) -> Self {
        Self {
            interval,
            last_logged: None,
            suppressed: 0,
        }
    }

    /// Log an error if the cooldown interval has elapsed, otherwise count it as suppressed.
    pub fn log_error(&mut self, tag: &str, msg: &str) {
        let now = Instant::now();
        let should_log = match self.last_logged {
            None => true,
            Some(prev) => now.duration_since(prev) >= self.interval,
        };
        if should_log {
            if self.suppressed > 0 {
                log::error!(
                    "[{tag}] {msg} ({} similar errors suppressed)",
                    self.suppressed
                );
                self.suppressed = 0;
            } else {
                log::error!("[{tag}] {msg}");
            }
            self.last_logged = Some(now);
        } else {
            self.suppressed += 1;
        }
    }
}

/// Delete `path` (and optionally its subtree) inside `tx`, logging failures
/// through `err_log` instead of silently discarding errors.
pub fn delete_entry_and_children(
    tx: &mut BatchTx,
    path: &str,
    include_children: bool,
    tag: &str,
    err_log: &mut RateLimitedLog,
) {
    let mut any_ok = false;
    match delete_by_path(tx.write_conn(), path) {
        Ok(()) => any_ok = true,
        Err(e) => err_log.log_error(tag, &format!("Delete failed for {path}: {e}")),
    }
    if include_children {
        match delete_by_path_prefix(tx.write_conn(), path) {
            Ok(()) => any_ok = true,
            Err(e) => err_log.log_error(tag, &format!("Delete subtree failed for {path}: {e}")),
        }
    }
    if any_ok {
        tx.tick();
    }
}

/// Commits every `BATCH` writes so a long job never holds the write lock for
/// minutes (which made watcher writes time out). Opens lazily on first write
/// and commits on drop.
pub struct BatchTx<'a> {
    conn: &'a Connection,
    pending: u32,
    open: bool,
}

const BATCH: u32 = 500;

impl<'a> BatchTx<'a> {
    pub fn new(conn: &'a Connection) -> Self {
        BatchTx {
            conn,
            pending: 0,
            open: false,
        }
    }

    /// Read-only access to the underlying connection without starting a write transaction.
    pub fn conn(&self) -> &Connection {
        self.conn
    }

    /// Write access to the underlying connection; opens `BEGIN IMMEDIATE` lazily on first write.
    pub fn write_conn(&mut self) -> &Connection {
        if !self.open {
            self.open = begin_tx(self.conn);
        }
        self.conn
    }

    #[cfg(test)]
    pub fn is_open(&self) -> bool {
        self.open
    }

    fn flush(&mut self) {
        if self.open {
            commit_or_rollback(self.conn, self.pending);
            // Sync `open` with SQLite's actual transaction state so tick()/finish() never drift.
            self.open = !self.conn.is_autocommit();
        }
        self.pending = 0;
    }

    /// Call after each write.
    pub fn tick(&mut self) {
        self.pending += 1;
        if self.pending >= BATCH {
            self.flush();
        }
    }

    pub fn finish(mut self) {
        self.flush();
    }
}

impl Drop for BatchTx<'_> {
    fn drop(&mut self) {
        self.flush();
    }
}

// --- helpers ---

fn unix_secs(t: Option<SystemTime>) -> Option<i64> {
    t.and_then(|st| st.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_batch_never_opens_transaction() {
        let temp_dir = std::env::temp_dir().join(format!("wf_test_empty_tx_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&temp_dir);
        let db_path = temp_dir.join("empty_tx.db");

        let conn1 = open(&db_path).expect("open conn1");
        create_table(&conn1).expect("create table");
        let conn2 = open(&db_path).expect("open conn2");

        let batch = BatchTx::new(&conn1);
        assert!(!batch.is_open());
        assert!(conn1.is_autocommit());

        // Because batch has not taken a write lock, conn2 can write immediately.
        conn2.execute_batch("BEGIN IMMEDIATE; COMMIT;").expect("conn2 should acquire write lock");

        batch.finish();
        assert!(conn1.is_autocommit());

        drop(conn2);
        drop(conn1);
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_failed_commit_rolls_back_and_keeps_open_in_sync() {
        let conn = Connection::open_in_memory().expect("open in-memory db");
        conn.execute_batch(
            "PRAGMA foreign_keys = ON;
             CREATE TABLE parent (id INTEGER PRIMARY KEY);
             CREATE TABLE child (
                 id INTEGER PRIMARY KEY,
                 pid INTEGER REFERENCES parent(id) DEFERRABLE INITIALLY DEFERRED
             );",
        )
        .expect("create fk tables");

        // 1. finish() on a failing COMMIT must retry, rollback, and close the batch.
        {
            let mut batch = BatchTx::new(&conn);
            batch
                .write_conn()
                .execute("INSERT INTO child (id, pid) VALUES (1, 999)", [])
                .expect("deferred FK insert succeeds until commit");
            assert!(batch.is_open());
            assert!(!conn.is_autocommit());

            batch.finish();
            assert!(conn.is_autocommit(), "transaction must be closed after rollback");
            let count: i64 = conn
                .query_row("SELECT COUNT(*) FROM child", [], |r| r.get(0))
                .expect("count rows");
            assert_eq!(count, 0, "failed write must be rolled back");
        }

        // 2. tick() hitting BATCH on a failing COMMIT must also leave `open` in sync (false).
        {
            let mut batch = BatchTx::new(&conn);
            batch
                .write_conn()
                .execute("INSERT INTO child (id, pid) VALUES (2, 999)", [])
                .expect("deferred FK insert succeeds until commit");
            for _ in 0..BATCH {
                batch.tick();
            }
            assert!(!batch.is_open(), "batch must be closed after failed tick commit");
            assert!(conn.is_autocommit(), "SQLite must be back in autocommit");
            batch.finish();
            assert!(conn.is_autocommit());
        }
    }

    #[test]
    fn test_schema_version_is_5_and_upgrades_from_v4() {
        let conn = Connection::open_in_memory().expect("open in-memory db");
        conn.execute_batch(
            "PRAGMA user_version = 4;
             CREATE TABLE files (id INTEGER PRIMARY KEY, legacy TEXT);
             CREATE TABLE apps (id INTEGER PRIMARY KEY, legacy TEXT);",
        )
        .expect("seed v4 schema");

        create_table(&conn).expect("upgrade to v5");

        let ver: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .expect("read user_version");
        assert_eq!(ver, 5);

        let idx_apps_name_exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'index' AND name = 'idx_apps_name'",
                [],
                |r| r.get(0),
            )
            .expect("check idx_apps_name");
        assert_eq!(idx_apps_name_exists, 0);

        // apps.name_lower is unique and drops duplicate app names on conflict test
        conn.execute(
            "INSERT INTO apps (name, name_lower, path) VALUES ('Code', 'code', 'C:\\\\A\\\\Code.lnk')",
            [],
        )
        .expect("insert first app");
        let dup_res = conn.execute(
            "INSERT INTO apps (name, name_lower, path) VALUES ('Code', 'code', 'C:\\\\B\\\\Code.lnk')",
            [],
        );
        assert!(dup_res.is_err(), "apps.name_lower must be unique");
    }

    #[test]
    fn test_case_insensitive_and_normalized_path_operations() {
        let conn = Connection::open_in_memory().expect("open in-memory db");
        create_table(&conn).expect("create schema 5");

        let fe1 = FileEntry::new(
            "a.TXT".to_string(),
            r"C:\Users\X\Dir\a.TXT",
            Some("txt".to_string()),
            false,
            None,
            None,
        );
        assert_eq!(fe1.path(), r"C:\Users\X\Dir\a.TXT");
        insert_entry(&conn, &fe1).expect("insert fe1");

        // Same path with different casing, drive case, forward slashes, and trailing slash
        let fe2 = FileEntry::new(
            "A.txt".to_string(),
            "c:/users/x/dir/A.txt/",
            Some("txt".to_string()),
            false,
            None,
            None,
        );
        upsert_entry(&conn, &fe2).expect("upsert fe2");

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM files", [], |r| r.get(0))
            .expect("count files");
        assert_eq!(count, 1, "COLLATE NOCASE + normalize_path must update single row");

        assert!(is_indexed(&conn, "c:/Users/X/DIR/a.txt/"));
        let children = children_of(&conn, "c:/users/x/dir/");
        assert_eq!(children.len(), 1);

        // Test subtree prefix deletion without matching sibling directory `Dir2`
        let sub = FileEntry::new("Sub".to_string(), r"C:\Users\X\Dir\Sub", None, true, None, None);
        let child = FileEntry::new(
            "child.txt".to_string(),
            r"C:\USERS\X\DIR\SUB\child.txt",
            Some("txt".to_string()),
            false,
            None,
            None,
        );
        let sibling = FileEntry::new(
            "keep.txt".to_string(),
            r"C:\Users\X\Dir2\keep.txt",
            Some("txt".to_string()),
            false,
            None,
            None,
        );
        insert_entry(&conn, &sub).expect("insert sub");
        insert_entry(&conn, &child).expect("insert child");
        insert_entry(&conn, &sibling).expect("insert sibling");

        delete_by_path(&conn, "c:/users/x/dir/a.txt").expect("delete single file");
        assert!(!is_indexed(&conn, r"C:\Users\X\Dir\a.TXT"));

        delete_by_path_prefix(&conn, "c:/users/x/dir/").expect("delete prefix");
        assert!(!is_indexed(&conn, r"C:\Users\X\Dir\Sub"));
        assert!(!is_indexed(&conn, r"C:\Users\X\Dir\Sub\child.txt"));
        assert!(is_indexed(&conn, r"C:\Users\X\Dir2\keep.txt"));
    }

    #[test]
    fn test_delete_missing_chunks_5000_paths() {
        let conn = Connection::open_in_memory().expect("open in-memory db");
        create_table(&conn).expect("create schema 5");

        conn.execute_batch("BEGIN IMMEDIATE;").expect("begin");
        for i in 0..5000 {
            let fe = FileEntry::new(
                format!("item_{i}.txt"),
                &format!(r"C:\TestRoot\item_{i}.txt"),
                Some("txt".to_string()),
                false,
                None,
                None,
            );
            insert_entry(&conn, &fe).expect("insert item");
        }
        conn.execute_batch("COMMIT;").expect("commit");

        let delete_paths: Vec<String> = (0..5000)
            .map(|i| format!("c:/testroot/ITEM_{i}.txt"))
            .collect();
        delete_missing(&conn, &delete_paths).expect("delete_missing 5000 rows");

        let remaining: i64 = conn
            .query_row("SELECT COUNT(*) FROM files", [], |r| r.get(0))
            .expect("count remaining");
        assert_eq!(remaining, 0);
    }
}
