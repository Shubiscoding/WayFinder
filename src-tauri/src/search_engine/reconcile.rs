// reconcile.rs — Single-pass startup reconcile and configurable fallback check.

use crate::search_engine::crawler::{self, Filters};
use crate::search_engine::database::{self, BatchTx, FileEntry, RateLimitedLog};
use crate::search_engine::paths;
use std::collections::{HashMap, HashSet};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
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
        if paths::is_under_ci(path, r) {
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

fn system_time_secs(t: Option<SystemTime>) -> Option<i64> {
    t.and_then(|st| st.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
}

// ---------------------------------------------------------------------------
// Startup reconcile (single-pass mark and sweep)
// ---------------------------------------------------------------------------

enum WalkItem {
    Entry {
        path: PathBuf,
        name: String,
        extension: Option<String>,
        is_dir: bool,
        created_at: Option<SystemTime>,
        modified_at: Option<SystemTime>,
    },
    WalkError(PathBuf),
}

pub fn run_startup_reconcile<F>(
    db_path: &Path,
    roots: &[PathBuf],
    filters: &Filters,
    on_progress: F,
) -> Result<(), String>
where
    F: FnMut(u64, u64),
{
    run_reconcile_with(db_path, roots, on_progress, |root, emit| {
        let walker = WalkDir::new(root).follow_links(false).into_iter();
        for result in walker.filter_entry(|e| e.depth() == 0 || !filters.should_skip_entry(e)) {
            match result {
                Ok(entry) => {
                    if entry.depth() == 0 {
                        continue;
                    }
                    let meta = entry.metadata().ok();
                    let is_dir = entry.file_type().is_dir();
                    let extension = if is_dir {
                        None
                    } else {
                        entry.path().extension().map(|e| e.to_string_lossy().to_string())
                    };
                    emit(WalkItem::Entry {
                        path: entry.path().to_path_buf(),
                        name: entry.file_name().to_string_lossy().to_string(),
                        extension,
                        is_dir,
                        created_at: meta.as_ref().and_then(|m| m.created().ok()),
                        modified_at: meta.as_ref().and_then(|m| m.modified().ok()),
                    });
                }
                Err(err) => {
                    if let Some(p) = err.path() {
                        emit(WalkItem::WalkError(p.to_path_buf()));
                    }
                }
            }
        }
    })
}

fn run_reconcile_with<F, W>(
    db_path: &Path,
    roots: &[PathBuf],
    mut on_progress: F,
    mut walk_root: W,
) -> Result<(), String>
where
    F: FnMut(u64, u64),
    W: FnMut(&Path, &mut dyn FnMut(WalkItem)),
{
    if roots.is_empty() {
        return Ok(()); // never purge the index just because roots failed to resolve
    }
    let conn = database::open(db_path)
        .map_err(|e| format!("Could not open database: {e}"))?;

    let reconcile_start = Instant::now();
    let live = root_status(roots);

    let total_rows: u64 = conn
        .query_row("SELECT COUNT(*) FROM files", [], |r| r.get::<_, i64>(0))
        .unwrap_or(0)
        .max(0) as u64;

    log::info!("[reconcile] Starting startup reconcile ({total_rows} rows in index) …");
    on_progress(0, total_rows);

    let mut db_write_dur = Duration::ZERO;

    // 1. Load what the index knows, keyed by normalized lowercase path_key.
    let load_start = Instant::now();
    let mut known: HashMap<String, (String, Option<i64>)> = HashMap::new();
    {
        let mut stmt = conn
            .prepare("SELECT path, modified_at FROM files")
            .map_err(|e| format!("Could not query files: {e}"))?;
        if let Ok(rows) =
            stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<i64>>(1)?)))
        {
            for (p, m) in rows.flatten() {
                known.insert(paths::path_key(&p), (p, m));
            }
        }
    }
    let load_dur = load_start.elapsed();

    // 2. Walk each live root ONCE, marking seen keys by removing them from `known`
    // and upserting only new or modified entries.
    let walk_start = Instant::now();
    let tx_start = Instant::now();
    let mut tx = BatchTx::new(&conn);
    db_write_dur += tx_start.elapsed();

    let mut processed: u64 = 0;
    let mut visited: u64 = 0;
    let mut updated: u64 = 0;
    let mut upsert_failures: u64 = 0;
    let mut error_samples: Vec<String> = Vec::new();
    let mut error_dirs: Vec<PathBuf> = Vec::new();
    let mut zero_visit_roots: Vec<PathBuf> = Vec::new();

    for (root, is_live) in &live {
        if !*is_live {
            continue;
        }
        let mut visited_for_root: u64 = 0;
        walk_root(root, &mut |item| match item {
            WalkItem::WalkError(err_path) => {
                error_dirs.push(err_path);
            }
            WalkItem::Entry {
                path,
                name,
                extension,
                is_dir,
                created_at,
                modified_at,
            } => {
                visited += 1;
                visited_for_root += 1;
                processed += 1;
                if processed.is_multiple_of(50) {
                    on_progress(processed, total_rows.max(processed));
                }

                let norm_path = paths::normalize_path(&path.to_string_lossy());
                let key = paths::path_key(&norm_path);
                let prev = known.remove(&key);

                // If modified_at couldn't be read (None), only insert when unseen;
                // existing rows with unreadable metadata are kept without churn.
                let Some(mod_time) = modified_at else {
                    if prev.is_none() {
                        let fe = FileEntry::new(name, &norm_path, extension, is_dir, created_at, None);
                        let w_start = Instant::now();
                        match database::upsert_entry(tx.write_conn(), &fe) {
                            Ok(()) => {
                                tx.tick();
                                updated += 1;
                            }
                            Err(e) => {
                                upsert_failures += 1;
                                if error_samples.len() < 5 {
                                    error_samples.push(format!("{}: {e}", fe.path()));
                                }
                            }
                        }
                        db_write_dur += w_start.elapsed();
                    }
                    return;
                };

                let disk_mod = system_time_secs(Some(mod_time));
                let needs_update = match prev {
                    None => true,
                    Some((_, stored_mod)) => disk_mod != stored_mod,
                };
                if !needs_update {
                    return;
                }

                let fe = FileEntry::new(name, &norm_path, extension, is_dir, created_at, modified_at);
                let w_start = Instant::now();
                match database::upsert_entry(tx.write_conn(), &fe) {
                    Ok(()) => {
                        tx.tick();
                        updated += 1;
                    }
                    Err(e) => {
                        upsert_failures += 1;
                        if error_samples.len() < 5 {
                            error_samples.push(format!("{}: {e}", fe.path()));
                        }
                    }
                }
                db_write_dur += w_start.elapsed();
            }
        });
        if visited_for_root == 0 {
            zero_visit_roots.push(root.clone());
        }
    }

    let finish_start = Instant::now();
    tx.finish();
    db_write_dur += finish_start.elapsed();
    let walk_dur = walk_start.elapsed();

    // 3. Purge unseen rows left in `known`, respecting missing roots, errored folders,
    // and the zero-visit circuit breaker.
    let purge_start = Instant::now();
    let mut stale: Vec<String> = Vec::new();
    let mut tripped_roots: HashSet<String> = HashSet::new();

    for (_, (orig_path, _)) in known {
        let p = Path::new(&orig_path);
        match classify(p, &live) {
            Placement::UnderMissingRoot => {}
            Placement::Orphan => stale.push(orig_path),
            Placement::UnderLiveRoot => {
                if let Some(zr) = zero_visit_roots.iter().find(|r| paths::is_under_ci(p, r)) {
                    tripped_roots.insert(paths::normalize_path(&zr.to_string_lossy()));
                    continue;
                }
                if error_dirs.iter().any(|ed| paths::is_under_ci(p, ed)) {
                    continue;
                }
                stale.push(orig_path);
            }
        }
    }
    for root_str in tripped_roots {
        log::warn!(
            "[reconcile] Circuit breaker: live root {root_str} visited 0 entries while index has rows under it; skipping purge for that root."
        );
    }

    let mut purged: u64 = 0;
    if !stale.is_empty() {
        let w_start = Instant::now();
        let del_res = database::delete_missing(&conn, &stale);
        db_write_dur += w_start.elapsed();
        match del_res {
            Ok(()) => purged = stale.len() as u64,
            Err(e) => log::error!("[reconcile] Error purging missing paths: {e}"),
        }
    }
    let purge_dur = purge_start.elapsed();

    let final_total = total_rows.max(processed);
    on_progress(processed, final_total);

    if upsert_failures > 0 {
        log::warn!(
            "[reconcile] {upsert_failures} upsert failures during reconcile; first errors: {}",
            error_samples.join(" | ")
        );
    }
    log::info!(
        "[reconcile] Startup reconcile complete: start_rows={total_rows}, visited={visited}, updated={updated}, purged={purged}, upsert_failures={upsert_failures} (load={:.3}s, purge={:.3}s, walk={:.3}s, db_writes={:.3}s, total={:.3}s).",
        load_dur.as_secs_f64(),
        purge_dur.as_secs_f64(),
        walk_dur.as_secs_f64(),
        db_write_dur.as_secs_f64(),
        reconcile_start.elapsed().as_secs_f64(),
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
pub fn run_fallback_check(db_path: &Path, roots: &[PathBuf], filters: &Filters) {
    run_cheap_fallback(db_path, roots, filters);
}

// ---------------------------------------------------------------------------
// Configurable fallback loop (default 15 minutes)
// ---------------------------------------------------------------------------

pub fn start_fallback_loop(
    db_path: PathBuf,
    roots: Vec<PathBuf>,
    filters: Filters,
    interval_secs: u64,
) {
    let sleep_secs = interval_secs.max(10);
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_secs(sleep_secs));
        run_cheap_fallback(&db_path, &roots, &filters);
    });
}

fn run_cheap_fallback(db_path: &Path, roots: &[PathBuf], filters: &Filters) {
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
    let mut err_log = RateLimitedLog::new(Duration::from_secs(5));
    for (folder_str, stored_mod) in folders {
        let folder = Path::new(&folder_str);

        match classify(folder, &live) {
            Placement::UnderMissingRoot => continue,
            Placement::Orphan => {
                database::delete_entry_and_children(
                    &mut tx,
                    &folder_str,
                    true,
                    "reconcile",
                    &mut err_log,
                );
                continue;
            }
            Placement::UnderLiveRoot => {}
        }

        let meta = match std::fs::symlink_metadata(folder) {
            Ok(m) => m,
            Err(e) if e.kind() == ErrorKind::NotFound => {
                database::delete_entry_and_children(
                    &mut tx,
                    &folder_str,
                    true,
                    "reconcile",
                    &mut err_log,
                );
                continue;
            }
            Err(_) => continue, // permission / transient error: do NOT purge
        };

        let current = mtime_secs(&meta);
        if current == stored_mod {
            continue;
        }

        sync_folder(&mut tx, folder, &folder_str, roots, filters, &mut err_log);

        match tx.write_conn().execute(
            "UPDATE files SET modified_at = ?1 WHERE path = ?2",
            rusqlite::params![current, folder_str],
        ) {
            Ok(_) => tx.tick(),
            Err(e) => err_log.log_error(
                "reconcile",
                &format!("Could not update mtime for {folder_str}: {e}"),
            ),
        }
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
    filters: &Filters,
    err_log: &mut RateLimitedLog,
) {
    let read = match std::fs::read_dir(folder) {
        Ok(r) => r,
        Err(_) => return, // unreadable: keep the index as it is
    };
    let db_children: HashSet<String> = database::children_of(tx.conn(), folder_str)
        .into_iter()
        .map(|p| paths::path_key(&p))
        .collect();
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
        if filters.should_skip_path(&p, is_dir, roots) {
            continue; // not in on_disk, so an old row for it is purged below
        }
        let ps = paths::normalize_path(&p.to_string_lossy());
        let ps_key = paths::path_key(&ps);
        on_disk.insert(ps_key.clone());

        if is_dir && !db_children.contains(&ps_key) {
            // New folder: index its whole subtree now. Otherwise its contents
            // would never be scanned (its mtime would already match next cycle).
            crawler::index_tree(tx, &p, filters);
        } else {
            let fe = FileEntry::new(
                child.file_name().to_string_lossy().to_string(),
                &ps,
                if is_dir {
                    None
                } else {
                    p.extension().map(|e| e.to_string_lossy().to_string())
                },
                is_dir,
                meta.created().ok(),
                meta.modified().ok(),
            );
            match database::upsert_entry(tx.write_conn(), &fe) {
                Ok(()) => tx.tick(),
                Err(e) => err_log.log_error(
                    "reconcile",
                    &format!("Upsert failed for {}: {e}", fe.path()),
                ),
            }
        }
    }

    if complete {
        for gone in db_children.difference(&on_disk) {
            database::delete_entry_and_children(tx, gone, true, "reconcile", err_log);
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
        let fe = FileEntry::new(
            "alpha.txt".to_string(),
            &temp_dir.join("alpha.txt").to_string_lossy(),
            Some("txt".to_string()),
            false,
            None,
            None,
        );
        database::insert_entry(&conn, &fe).expect("insert");
        drop(conn);

        let mut progress_calls = Vec::new();
        let filters = Filters::default();
        let res = run_startup_reconcile(
            &db_path,
            std::slice::from_ref(&temp_dir),
            &filters,
            |processed, total| {
                progress_calls.push((processed, total));
            },
        );

        assert!(res.is_ok());
        assert!(!progress_calls.is_empty(), "Expected progress callbacks");
        assert_eq!(progress_calls[0].0, 0);
        assert_eq!(progress_calls[0].1, 1);
        assert!(
            progress_calls.iter().all(|(p, t)| p <= t),
            "processed must never exceed reported total: {progress_calls:?}"
        );

        // Also test run_fallback_check does not panic
        run_fallback_check(&db_path, std::slice::from_ref(&temp_dir), &filters);

        // Cleanup
        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_reconcile_purges_newly_filtered_files() {
        let base = std::env::temp_dir().join(format!("wf_test_rec_filter_{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let root = base.join("root");
        let _ = fs::create_dir_all(&root);
        let _ = fs::write(root.join("keep.rs"), "fn main() {}");
        let _ = fs::write(root.join("drop.tmp"), "temp data");

        let db_path = base.join("rec_filter.db");
        let conn = database::open(&db_path).expect("open");
        database::create_table(&conn).expect("create");
        drop(conn);

        let no_filter = Filters::default();
        crawler::run_crawl(&db_path, std::slice::from_ref(&root), &no_filter, |_| {})
            .expect("initial crawl");

        let conn = database::open(&db_path).expect("reopen");
        assert!(database::is_indexed(&conn, &root.join("drop.tmp").to_string_lossy()));
        drop(conn);

        let with_tmp = Filters::new(&[], &["tmp".to_string()], true);
        run_startup_reconcile(&db_path, std::slice::from_ref(&root), &with_tmp, |_, _| {})
            .expect("reconcile with tmp excluded");

        let conn = database::open(&db_path).expect("reopen");
        assert!(!database::is_indexed(&conn, &root.join("drop.tmp").to_string_lossy()));
        assert!(database::is_indexed(&conn, &root.join("keep.rs").to_string_lossy()));
        drop(conn);

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn test_reconcile_keeps_rows_under_missing_root() {
        let base = std::env::temp_dir().join(format!("wf_test_rec_missing_{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let live_root = base.join("live");
        let missing_root = base.join("offline_drive");
        let _ = fs::create_dir_all(&live_root);
        let _ = fs::write(live_root.join("present.txt"), "ok");

        let db_path = base.join("rec_missing.db");
        let conn = database::open(&db_path).expect("open");
        database::create_table(&conn).expect("create");
        for (name, p) in [
            ("present.txt", live_root.join("present.txt")),
            ("deleted.txt", live_root.join("deleted.txt")),
            ("offline.txt", missing_root.join("offline.txt")),
        ] {
            database::insert_entry(
                &conn,
                &FileEntry::new(name.to_string(), &p.to_string_lossy(), Some("txt".to_string()), false, None, None),
            )
            .expect("seed");
        }
        drop(conn);

        run_startup_reconcile(
            &db_path,
            &[live_root.clone(), missing_root.clone()],
            &Filters::default(),
            |_, _| {},
        )
        .expect("reconcile");

        let conn = database::open(&db_path).expect("reopen");
        assert!(database::is_indexed(&conn, &live_root.join("present.txt").to_string_lossy()));
        assert!(!database::is_indexed(&conn, &live_root.join("deleted.txt").to_string_lossy()));
        assert!(database::is_indexed(&conn, &missing_root.join("offline.txt").to_string_lossy()));
        drop(conn);

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn test_reconcile_keeps_rows_under_errored_folder() {
        let base = std::env::temp_dir().join(format!("wf_test_rec_errdir_{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let root = base.join("root");
        let _ = fs::create_dir_all(&root);

        let db_path = base.join("rec_errdir.db");
        let conn = database::open(&db_path).expect("open");
        database::create_table(&conn).expect("create");
        let good_keep = root.join("good").join("keep.txt");
        let good_gone = root.join("good").join("gone.txt");
        let locked_dir = root.join("locked_dir");
        let locked_secret = locked_dir.join("secret.txt");
        for (name, p, is_dir) in [
            ("good", root.join("good"), true),
            ("keep.txt", good_keep.clone(), false),
            ("gone.txt", good_gone.clone(), false),
            ("locked_dir", locked_dir.clone(), true),
            ("secret.txt", locked_secret.clone(), false),
        ] {
            database::insert_entry(
                &conn,
                &FileEntry::new(name.to_string(), &p.to_string_lossy(), None, is_dir, None, None),
            )
            .expect("seed");
        }
        drop(conn);

        let now = Some(SystemTime::now());
        run_reconcile_with(&db_path, std::slice::from_ref(&root), |_, _| {}, |_, emit| {
            emit(WalkItem::Entry {
                path: root.join("good"),
                name: "good".to_string(),
                extension: None,
                is_dir: true,
                created_at: now,
                modified_at: now,
            });
            emit(WalkItem::Entry {
                path: good_keep.clone(),
                name: "keep.txt".to_string(),
                extension: Some("txt".to_string()),
                is_dir: false,
                created_at: now,
                modified_at: now,
            });
            emit(WalkItem::Entry {
                path: locked_dir.clone(),
                name: "locked_dir".to_string(),
                extension: None,
                is_dir: true,
                created_at: now,
                modified_at: now,
            });
            emit(WalkItem::WalkError(locked_dir.clone()));
        })
        .expect("reconcile with walk error");

        let conn = database::open(&db_path).expect("reopen");
        assert!(database::is_indexed(&conn, &good_keep.to_string_lossy()));
        assert!(!database::is_indexed(&conn, &good_gone.to_string_lossy()));
        assert!(database::is_indexed(&conn, &locked_secret.to_string_lossy()));
        drop(conn);

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn test_reconcile_zero_visit_circuit_breaker_keeps_rows() {
        let base = std::env::temp_dir().join(format!("wf_test_rec_zero_{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let root = base.join("empty_live_root");
        let _ = fs::create_dir_all(&root);

        let db_path = base.join("rec_zero.db");
        let conn = database::open(&db_path).expect("open");
        database::create_table(&conn).expect("create");
        let row_path = root.join("important.txt");
        database::insert_entry(
            &conn,
            &FileEntry::new(
                "important.txt".to_string(),
                &row_path.to_string_lossy(),
                Some("txt".to_string()),
                false,
                None,
                None,
            ),
        )
        .expect("seed");
        drop(conn);

        // Live root exists on disk, has 0 children -> visits 0 entries -> must NOT purge.
        run_startup_reconcile(&db_path, std::slice::from_ref(&root), &Filters::default(), |_, _| {})
            .expect("reconcile zero-visit root");

        let conn = database::open(&db_path).expect("reopen");
        assert!(database::is_indexed(&conn, &row_path.to_string_lossy()));
        drop(conn);

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn test_reconcile_different_casing_neither_purged_nor_duplicated() {
        let base = std::env::temp_dir().join(format!("wf_test_rec_case_{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let root = base.join("root");
        let sub = root.join("SubDir");
        let _ = fs::create_dir_all(&sub);
        let disk_file = sub.join("Hello.TXT");
        let _ = fs::write(&disk_file, "hello");

        let db_path = base.join("rec_case.db");
        let conn = database::open(&db_path).expect("open");
        database::create_table(&conn).expect("create");
        let alt_case_path = root.join("subdir").join("hello.txt");
        let fe = FileEntry::new(
            "hello.txt".to_string(),
            &alt_case_path.to_string_lossy(),
            Some("txt".to_string()),
            false,
            None,
            None,
        );
        database::insert_entry(&conn, &fe).expect("seed alt case");
        drop(conn);

        run_startup_reconcile(&db_path, std::slice::from_ref(&root), &Filters::default(), |_, _| {})
            .expect("reconcile alt case");

        let conn = database::open(&db_path).expect("reopen");
        let file_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM files WHERE name_lower = 'hello.txt'",
                [],
                |r| r.get(0),
            )
            .expect("count");
        assert_eq!(file_count, 1, "must neither purge nor duplicate alt-cased file");
        drop(conn);

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn test_classify_handles_case_and_mixed_separators() {
        let roots = vec![
            (PathBuf::from("c:/Users/Alice/"), true),
            (PathBuf::from(r"D:\External"), false),
        ];
        assert!(matches!(
            classify(Path::new(r"C:\users\ALICE\Docs\note.txt"), &roots),
            Placement::UnderLiveRoot
        ));
        assert!(matches!(
            classify(Path::new("d:/external/backup.zip"), &roots),
            Placement::UnderMissingRoot
        ));
        assert!(matches!(
            classify(Path::new(r"C:\Users\Alice2\note.txt"), &roots),
            Placement::Orphan
        ));
    }
}
