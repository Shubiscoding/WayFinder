// reconcile.rs — Startup reconcile and cheap 3-minute fallback.

use crate::search_engine::crawler;
use crate::search_engine::database::{self, BatchTx, FileEntry};
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

pub fn run_startup_reconcile<F>(
    db_path: &Path,
    roots: &[PathBuf],
    exclude_dirs: &[String],
    mut on_progress: F,
) -> Result<(), String>
where
    F: FnMut(u64, u64),
{
    if roots.is_empty() {
        return Ok(()); // never purge the index just because roots failed to resolve
    }
    let conn = database::open(db_path)
        .map_err(|e| format!("Could not open database: {e}"))?;

    log::info!("[reconcile] Starting startup reconcile …");
    let live = root_status(roots);

    // Read total row count once at the start:
    let total_rows: u64 = conn
        .query_row("SELECT COUNT(*) FROM files", [], |r| r.get::<_, i64>(0))
        .unwrap_or(0)
        .max(0) as u64;

    on_progress(0, total_rows);

    // 1. Load what the index knows.
    let mut known: HashMap<String, Option<i64>> = HashMap::new();
    {
        let mut stmt = conn
            .prepare("SELECT path, modified_at FROM files")
            .map_err(|e| format!("Could not query files: {e}"))?;
        if let Ok(rows) = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<i64>>(1)?))) {
            for (p, m) in rows.flatten() {
                known.insert(p, m);
            }
        }
    }

    let mut processed: u64 = 0;

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
        processed += 1;
        if processed.is_multiple_of(50) {
            on_progress(processed, total_rows);
        }
    }
    for chunk in stale.chunks(500) {
        if let Err(e) = database::delete_missing(&conn, chunk) {
            log::error!("[reconcile] Error purging missing paths: {e}");
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
            processed += 1;
            if processed.is_multiple_of(50) {
                on_progress(processed, total_rows);
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
    on_progress(processed, total_rows);
    log::info!(
        "[reconcile] Startup reconcile complete: removed {}, updated {updated} items.",
        stale.len()
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// Fallback checks
// ---------------------------------------------------------------------------

/// Run one cheap fallback check.
///
/// Used at startup right after the watcher starts to catch any filesystem changes
/// that occurred between the startup sync's disk scan and the watcher initialization.
pub fn run_fallback_check(db_path: &Path, roots: &[PathBuf], exclude_dirs: &[String]) {
    run_cheap_fallback(db_path, roots, exclude_dirs);
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_startup_reconcile_progress_reporting() {
        let temp_dir = std::env::temp_dir().join(format!("wf_test_rec_{}", std::process::id()));
        let _ = fs::create_dir_all(&temp_dir);
        let db_path = temp_dir.join("test_index.db");

        // Create db and a dummy file entry
        let conn = database::open(&db_path).expect("open db");
        database::create_table(&conn).expect("create table");
        let fe = FileEntry {
            name: "alpha.txt".to_string(),
            path: temp_dir.join("alpha.txt").to_string_lossy().to_string(),
            extension: Some("txt".to_string()),
            is_dir: false,
            created_at: None,
            modified_at: None,
        };
        database::insert_entry(&conn, &fe).expect("insert");
        drop(conn);

        let mut progress_calls = Vec::new();
        let res = run_startup_reconcile(
            &db_path,
            &[temp_dir.clone()],
            &[],
            |processed, total| {
                progress_calls.push((processed, total));
            },
        );

        assert!(res.is_ok());
        assert!(!progress_calls.is_empty(), "Expected progress callbacks");
        // Initial call must report (0, total)
        assert_eq!(progress_calls[0].0, 0);
        assert_eq!(progress_calls[0].1, 1);

        // Also test run_fallback_check does not panic
        run_fallback_check(&db_path, &[temp_dir.clone()], &[]);

        // Cleanup
        let _ = fs::remove_dir_all(&temp_dir);
    }
}
