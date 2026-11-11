// watcher.rs — Real-time filesystem watcher (notify-debouncer-mini).

use crate::search_engine::apps;
use crate::search_engine::crawler;
use crate::search_engine::database::{self, BatchTx, FileEntry};
use notify_debouncer_mini::{new_debouncer, notify::RecursiveMode};
use std::path::{Path, PathBuf};
use std::sync::mpsc::channel;
use std::time::Duration;

fn is_under_dir_ci(path: &Path, dir: &Path) -> bool {
    let mut p_comps = path.components();
    for d_comp in dir.components() {
        match p_comps.next() {
            Some(p_comp) => {
                let p_s = p_comp.as_os_str().to_string_lossy().to_lowercase();
                let d_s = d_comp.as_os_str().to_string_lossy().to_lowercase();
                if p_s != d_s {
                    return false;
                }
            }
            None => return false,
        }
    }
    true
}

pub fn start_watcher(db_path: PathBuf, roots: Vec<PathBuf>, exclude_dirs: Vec<String>) {
    std::thread::spawn(move || {
        run_watcher(&db_path, &roots, &exclude_dirs);
    });
}

fn run_watcher(db_path: &Path, roots: &[PathBuf], exclude_dirs: &[String]) {
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
        if dir.exists() && !roots.iter().any(|r| is_under_dir_ci(dir, r)) {
            if let Err(e) = debouncer.watcher().watch(dir, RecursiveMode::Recursive) {
                log::warn!("[watcher] Could not watch Start Menu {}: {e}", dir.display());
            } else {
                log::info!("[watcher] Watching Start Menu {} for app changes", dir.display());
            }
        }
    }

    while let Ok(event_result) = rx.recv() {
        match event_result {
            Ok(events) => {
                let mut app_change = false;
                {
                    // One transaction per debounced batch instead of one fsync per event.
                    let mut batch = BatchTx::new(&conn);
                    for event in events {
                        if app_dirs.iter().any(|d| is_under_dir_ci(&event.path, d)) {
                            app_change = true;
                            continue;
                        }
                        handle_path_change(&mut batch, &event.path, roots, exclude_dirs);
                    }
                    batch.finish();
                }
                if app_change
                    && let Err(e) = apps::sync_apps(&conn)
                {
                    log::error!("[watcher] Error syncing apps after Start Menu change: {e}");
                }
            }
            Err(e) => log::error!("[watcher] Watch error: {e:?}"),
        }
    }
}

fn remove_subtree(tx: &mut BatchTx, path_str: &str, is_dir: bool) {
    // Only query/delete if the path actually exists in the index.
    // This avoids thousands of useless LIKE queries and lock contention
    // for files in hidden/excluded trees (e.g. AppData, .git, temp files).
    if !database::is_indexed(tx.conn(), path_str) {
        return;
    }
    let _ = database::delete_by_path(tx.conn(), path_str);
    if is_dir {
        let _ = database::delete_by_path_prefix(tx.conn(), path_str);
    }
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
            if database::is_indexed(tx.conn(), &path_str) {
                let _ = database::delete_by_path(tx.conn(), &path_str);
                let _ = database::delete_by_path_prefix(tx.conn(), &path_str);
                tx.tick();
            }
            return;
        }
        Err(_) => return, // permission etc.: leave the index untouched
    };
    let is_dir = meta.is_dir();

    if crawler::is_excluded_in_roots(path, is_dir, roots, exclude_dirs)
        || crawler::is_hidden_in_roots(path, roots)
    {
        // Previously indexed but now hidden/excluded: drop it.
        // If it was never indexed (e.g. EBWebView/AppData), do nothing.
        remove_subtree(tx, &path_str, is_dir);
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
        Err(e) => log::error!("[watcher] Upsert error for {path_str}: {e}"),
    }
}
