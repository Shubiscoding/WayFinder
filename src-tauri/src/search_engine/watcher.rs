// watcher.rs — Real-time filesystem watcher (notify-debouncer-mini).

use crate::search_engine::apps;
use crate::search_engine::crawler::{self, Filters};
use crate::search_engine::database::{self, BatchTx, FileEntry, RateLimitedLog};
use crate::search_engine::paths;
use notify_debouncer_mini::{new_debouncer, notify::RecursiveMode};
use rusqlite::Connection;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc::channel;
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EventClass {
    Ignored,
    AppChange,
    NeedsDbWork,
}

#[derive(Debug, Default, PartialEq, Eq)]
struct BatchOutcome {
    app_change: bool,
    batch_created: bool,
    ignored_count: usize,
    db_work_count: usize,
}

/// Classify a filesystem event path using cheap in-memory checks only (no disk stat or DB access).
fn classify_event(
    path: &Path,
    roots: &[PathBuf],
    app_dirs: &[PathBuf],
    ignore_dirs: &[PathBuf],
    filters: &Filters,
) -> EventClass {
    // 1. Ignore the app's own folders FIRST (DB, logs, WebView2 cache).
    if ignore_dirs.iter().any(|d| paths::is_under_ci(path, d)) {
        return EventClass::Ignored;
    }

    // 2. Start Menu app folders trigger an app sync, not file-index work.
    if app_dirs.iter().any(|d| paths::is_under_ci(path, d)) {
        return EventClass::AppChange;
    }

    // 3. Paths equal to a root or outside every configured root are ignored.
    if roots.iter().any(|r| paths::is_same_path_ci(path, r))
        || !roots.iter().any(|r| paths::is_under_ci(path, r))
    {
        return EventClass::Ignored;
    }

    // 4. Paths nested inside an excluded or dot directory below a root are ignored cheaply.
    if filters.is_under_excluded_ancestor(path, roots) {
        return EventClass::Ignored;
    }

    EventClass::NeedsDbWork
}

fn process_event_paths(
    conn: &Connection,
    paths: &[PathBuf],
    roots: &[PathBuf],
    app_dirs: &[PathBuf],
    ignore_dirs: &[PathBuf],
    filters: &Filters,
    err_log: &mut RateLimitedLog,
) -> BatchOutcome {
    let mut outcome = BatchOutcome::default();
    let mut needs_db_work: Vec<&PathBuf> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    for path in paths {
        match classify_event(path, roots, app_dirs, ignore_dirs, filters) {
            EventClass::Ignored => {
                outcome.ignored_count += 1;
            }
            EventClass::AppChange => {
                outcome.app_change = true;
            }
            EventClass::NeedsDbWork => {
                let key = paths::path_key(&path.to_string_lossy());
                if seen.insert(key) {
                    needs_db_work.push(path);
                }
            }
        }
    }

    outcome.db_work_count = needs_db_work.len();

    // Only construct BatchTx when there is actual file-index work to inspect/write.
    if !needs_db_work.is_empty() {
        outcome.batch_created = true;
        let mut batch = BatchTx::new(conn);
        for path in needs_db_work {
            handle_path_change(&mut batch, path, roots, filters, err_log);
        }
        batch.finish();
    }

    outcome
}

pub fn start_watcher(
    db_path: PathBuf,
    roots: Vec<PathBuf>,
    ignore_dirs: Vec<PathBuf>,
    filters: Filters,
) {
    std::thread::spawn(move || {
        run_watcher(&db_path, &roots, &ignore_dirs, &filters);
    });
}

fn run_watcher(
    db_path: &Path,
    roots: &[PathBuf],
    ignore_dirs: &[PathBuf],
    filters: &Filters,
) {
    let app_dirs = apps::start_menu_dirs();
    if roots.is_empty() && app_dirs.is_empty() {
        return;
    }

    let conn = match database::open(db_path) {
        Ok(c) => c,
        Err(e) => {
            log::error!("[watcher] Could not open database connection: {e}");
            return;
        }
    };

    let (tx, rx) = channel();
    let mut debouncer = match new_debouncer(Duration::from_millis(500), tx) {
        Ok(d) => d,
        Err(e) => {
            log::error!("[watcher] Could not initialize filesystem debouncer: {e}");
            return;
        }
    };

    for root in roots {
        if root.exists() {
            if let Err(e) = debouncer.watcher().watch(root, RecursiveMode::Recursive) {
                log::warn!("[watcher] Could not watch {}: {e}", root.display());
            } else {
                log::info!("[watcher] Watching {} for changes", root.display());
            }
        }
    }

    for dir in &app_dirs {
        if dir.exists() && !roots.iter().any(|r| paths::is_under_ci(dir, r)) {
            if let Err(e) = debouncer.watcher().watch(dir, RecursiveMode::Recursive) {
                log::warn!("[watcher] Could not watch Start Menu {}: {e}", dir.display());
            } else {
                log::info!("[watcher] Watching Start Menu {} for app changes", dir.display());
            }
        }
    }

    let mut err_log = RateLimitedLog::new(Duration::from_secs(5));

    while let Ok(event_result) = rx.recv() {
        match event_result {
            Ok(events) => {
                let paths: Vec<PathBuf> = events.into_iter().map(|e| e.path).collect();
                let outcome = process_event_paths(
                    &conn,
                    &paths,
                    roots,
                    &app_dirs,
                    ignore_dirs,
                    filters,
                    &mut err_log,
                );
                // BatchTx is finished/dropped before running apps::sync_apps.
                if outcome.app_change
                    && let Err(e) = apps::sync_apps(&conn)
                {
                    log::error!("[watcher] Error syncing apps after Start Menu change: {e}");
                }
            }
            Err(e) => log::error!("[watcher] Watch error: {e:?}"),
        }
    }
}

fn remove_subtree(
    tx: &mut BatchTx,
    path_str: &str,
    is_dir: bool,
    err_log: &mut RateLimitedLog,
) {
    // Only query/delete if the path actually exists in the index.
    // This avoids thousands of useless LIKE queries and lock contention
    // for files in hidden/excluded trees (e.g. AppData, .git, temp files).
    if !database::is_indexed(tx.conn(), path_str) {
        return;
    }
    database::delete_entry_and_children(tx, path_str, is_dir, "watcher", err_log);
}

fn handle_path_change(
    tx: &mut BatchTx,
    path: &Path,
    roots: &[PathBuf],
    filters: &Filters,
    err_log: &mut RateLimitedLog,
) {
    // The root itself is never an index row.
    if roots.iter().any(|r| paths::is_same_path_ci(path, r)) {
        return;
    }
    let path_str = paths::normalize_path(&path.to_string_lossy());

    // symlink_metadata: never follow links (matches the crawler's rule and
    // makes symlink cycles impossible).
    let meta = match std::fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            if database::is_indexed(tx.conn(), &path_str) {
                database::delete_entry_and_children(tx, &path_str, true, "watcher", err_log);
            }
            return;
        }
        Err(_) => return, // permission etc.: leave the index untouched
    };
    let is_dir = meta.is_dir();

    if filters.should_skip_path(path, is_dir, roots) {
        // Previously indexed but now hidden/excluded/filtered: drop it.
        // If it was never indexed (e.g. EBWebView/AppData), do nothing.
        remove_subtree(tx, &path_str, is_dir, err_log);
        return;
    }

    // A directory we have never seen (created, copied in, renamed into place)
    // is indexed with one safe walk. Known directories only get a row update,
    // so every mtime change does not re-walk the whole subtree.
    if is_dir && !database::is_indexed(tx.conn(), &path_str) {
        crawler::index_tree(tx, path, filters);
        return;
    }

    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| path_str.clone());
    let entry = FileEntry::new(
        name,
        &path_str,
        if is_dir {
            None
        } else {
            path.extension().map(|e| e.to_string_lossy().to_string())
        },
        is_dir,
        meta.created().ok(),
        meta.modified().ok(),
    );
    match database::upsert_entry(tx.write_conn(), &entry) {
        Ok(()) => tx.tick(),
        Err(e) => err_log.log_error("watcher", &format!("Upsert error for {path_str}: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_empty_and_all_ignored_batches_never_start_transaction() {
        let temp_dir = std::env::temp_dir().join(format!("wf_test_watcher_{}", std::process::id()));
        let root_dir = temp_dir.join("UserHome");
        let app_data_dir = root_dir.join("AppData").join("Roaming").join("com.wayfinder.app");
        let local_data_dir = root_dir.join("AppData").join("Local").join("com.wayfinder.app");
        let log_dir = local_data_dir.join("logs");
        let app_menu_dir = root_dir.join("StartMenu");
        let _ = fs::create_dir_all(&app_data_dir);
        let _ = fs::create_dir_all(local_data_dir.join("EBWebView").join("Default"));
        let _ = fs::create_dir_all(&log_dir);
        let _ = fs::create_dir_all(&app_menu_dir);

        let db_path = temp_dir.join("watcher_test.db");
        let conn1 = database::open(&db_path).expect("open conn1");
        database::create_table(&conn1).expect("create table");
        let conn2 = database::open(&db_path).expect("open conn2");

        // Hold an exclusive SQLite write lock on conn2 to prove conn1 never tries BEGIN IMMEDIATE.
        conn2.execute_batch("BEGIN IMMEDIATE;").expect("conn2 write lock");

        let roots = vec![root_dir.clone()];
        let ignore_dirs = vec![app_data_dir.clone(), local_data_dir.clone(), log_dir.clone()];
        let app_dirs = vec![app_menu_dir.clone()];
        let filters = Filters::new(
            &["node_modules".to_string(), ".git".to_string()],
            &["dll".to_string()],
            true,
        );
        let mut err_log = RateLimitedLog::new(Duration::from_secs(5));

        // 1. Empty batch
        let empty_out = process_event_paths(
            &conn1,
            &[],
            &roots,
            &app_dirs,
            &ignore_dirs,
            &filters,
            &mut err_log,
        );
        assert!(!empty_out.batch_created);
        assert!(!empty_out.app_change);
        assert!(conn1.is_autocommit());

        // 2. All-ignored batch: own folders (including mixed case), root itself, outside root, excluded/dot ancestor.
        let mixed_case_webview = PathBuf::from(
            local_data_dir
                .join("EBWebView")
                .join("Default")
                .join("Cache")
                .to_string_lossy()
                .to_lowercase(),
        );
        let ignored_paths = vec![
            app_data_dir.join("index.db-wal"),
            mixed_case_webview,
            log_dir.join("WayFinder.log"),
            root_dir.clone(),
            temp_dir.join("OutsideRoot").join("file.txt"),
            root_dir.join("proj").join("node_modules").join("pkg").join("index.js"),
            root_dir.join(".cargo").join("registry").join("crate.rs"),
        ];
        let ignored_out = process_event_paths(
            &conn1,
            &ignored_paths,
            &roots,
            &app_dirs,
            &ignore_dirs,
            &filters,
            &mut err_log,
        );
        assert!(!ignored_out.batch_created);
        assert!(!ignored_out.app_change);
        assert_eq!(ignored_out.ignored_count, ignored_paths.len());
        assert!(conn1.is_autocommit());

        // 3. App-change-only batch must also never open a BatchTx.
        let app_out = process_event_paths(
            &conn1,
            &[app_menu_dir.join("Editor.lnk")],
            &roots,
            &app_dirs,
            &ignore_dirs,
            &filters,
            &mut err_log,
        );
        assert!(!app_out.batch_created);
        assert!(app_out.app_change);
        assert!(conn1.is_autocommit());

        conn2.execute_batch("COMMIT;").expect("release conn2 write lock");
        drop(conn2);
        drop(conn1);
        let _ = fs::remove_dir_all(&temp_dir);
    }
}
