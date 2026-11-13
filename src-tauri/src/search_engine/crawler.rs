// crawler.rs — Walks the user's home folder and inserts every file and
// directory into the SQLite index. Runs on a background thread so the UI
// stays responsive.
//
// Rules enforced here:
//  • No unwrap / expect — every error is logged and skipped.
//  • Symlinks and junctions are listed but not entered (`follow_links(false)`);
//    their rows are stored with `is_dir = false` so they are never descended into.
//  • Skips hidden items (Windows hidden attribute, top-down from root) and configured Filters.
//  • Skips permission errors and continues.
//  • All inserts happen inside batch transactions for speed and concurrency.
//  • Root dirs and filters come from Config — nothing is hardcoded.

use crate::config::Config;
use crate::search_engine::database::{self, BatchTx, FileEntry};
use crate::search_engine::paths;
use std::collections::HashSet;
use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};
use walkdir::WalkDir;

#[cfg(windows)]
use std::os::windows::fs::MetadataExt;

/// Shared indexing filters built once from `Config` at startup and passed to
/// `run_crawl`, `index_tree`, reconcile, `sync_folder`, and the watcher.
#[derive(Debug, Clone)]
pub struct Filters {
    pub exclude_dirs: HashSet<String>,
    pub exclude_extensions: HashSet<String>,
    pub skip_dot_dirs: bool,
}

impl Default for Filters {
    fn default() -> Self {
        Self {
            exclude_dirs: HashSet::new(),
            exclude_extensions: HashSet::new(),
            skip_dot_dirs: true,
        }
    }
}

impl Filters {
    pub fn new(
        exclude_dirs: &[String],
        exclude_extensions: &[String],
        skip_dot_dirs: bool,
    ) -> Self {
        let dirs = exclude_dirs
            .iter()
            .map(|s| s.trim().to_lowercase())
            .filter(|s| !s.is_empty())
            .collect();
        let exts = exclude_extensions
            .iter()
            .map(|s| s.trim().trim_start_matches('.').to_lowercase())
            .filter(|s| !s.is_empty())
            .collect();
        Self {
            exclude_dirs: dirs,
            exclude_extensions: exts,
            skip_dot_dirs,
        }
    }

    pub fn from_config(cfg: &Config) -> Self {
        Self::new(&cfg.exclude_dirs, &cfg.exclude_extensions, cfg.skip_dot_dirs)
    }

    pub fn is_excluded_dir_name(&self, name: &str) -> bool {
        if name.is_empty() {
            return false;
        }
        if self.skip_dot_dirs && name.starts_with('.') {
            return true;
        }
        self.exclude_dirs.contains(&name.to_lowercase())
    }

    pub fn is_excluded_extension(&self, ext: &str) -> bool {
        let clean = ext.trim().trim_start_matches('.');
        !clean.is_empty() && self.exclude_extensions.contains(&clean.to_lowercase())
    }

    /// Walk-time filter for `WalkDir::filter_entry`.
    /// Order: excluded dir name -> dot-dir -> extension (files only) -> hidden attribute (costliest last).
    pub fn should_skip_entry(&self, entry: &walkdir::DirEntry) -> bool {
        if entry.file_type().is_dir() {
            let name = entry.file_name().to_string_lossy();
            if self.is_excluded_dir_name(&name) {
                return true;
            }
        } else if let Some(ext) = entry.path().extension().and_then(|e| e.to_str())
            && self.is_excluded_extension(ext)
        {
            return true;
        }
        is_hidden(entry)
    }

    /// Cheap in-memory check (no disk stats) for whether any strict ancestor
    /// below the matching root is an excluded or dot directory.
    pub fn is_under_excluded_ancestor(&self, path: &Path, roots: &[PathBuf]) -> bool {
        if self.exclude_dirs.is_empty() && !self.skip_dot_dirs {
            return false;
        }
        let Some((_, rel_comps)) = relative_components_ci(path, roots) else {
            return false;
        };
        let strict_len = rel_comps.len().saturating_sub(1);
        rel_comps[..strict_len]
            .iter()
            .any(|c| self.is_excluded_dir_name(&c.to_string_lossy()))
    }

    /// Path-time filter for watcher `handle_path_change` and `reconcile::sync_folder`.
    /// Order: components below root only (excluded dir names, dot-dirs) -> leaf extension (files only) -> hidden ancestors top-down.
    pub fn should_skip_path(&self, path: &Path, is_dir: bool, roots: &[PathBuf]) -> bool {
        let Some((_, rel_comps)) = relative_components_ci(path, roots) else {
            return true; // outside every configured root
        };
        let count = rel_comps.len();
        for (i, comp) in rel_comps.iter().enumerate() {
            if i + 1 == count && !is_dir {
                break; // files are never matched against dir exclusion rules
            }
            if self.is_excluded_dir_name(&comp.to_string_lossy()) {
                return true;
            }
        }
        if !is_dir
            && let Some(ext) = path.extension().and_then(|e| e.to_str())
            && self.is_excluded_extension(ext)
        {
            return true;
        }
        is_hidden_in_roots(path, roots)
    }
}

fn find_matching_root_ci(path: &Path, roots: &[PathBuf]) -> Option<PathBuf> {
    let mut best: Option<(usize, PathBuf)> = None;
    for r in roots {
        if paths::is_under_ci(path, r) {
            let norm = paths::normalize_path(&r.to_string_lossy());
            let len = norm.len();
            if best.as_ref().is_none_or(|(b_len, _)| len > *b_len) {
                best = Some((len, PathBuf::from(norm)));
            }
        }
    }
    best.map(|(_, p)| p)
}

fn relative_components_ci(path: &Path, roots: &[PathBuf]) -> Option<(PathBuf, Vec<OsString>)> {
    let norm_root = find_matching_root_ci(path, roots)?;
    let norm_path = PathBuf::from(paths::normalize_path(&path.to_string_lossy()));
    let root_comp_count = norm_root.components().count();
    let rel: Vec<OsString> = norm_path
        .components()
        .skip(root_comp_count)
        .filter_map(|c| match c {
            Component::Normal(s) => Some(s.to_os_string()),
            _ => None,
        })
        .collect();
    Some((norm_root, rel))
}

/// Returns true if the filesystem entry is marked hidden.
pub fn is_hidden(entry: &walkdir::DirEntry) -> bool {
    // Windows hidden attribute (FILE_ATTRIBUTE_HIDDEN = 0x2).
    #[cfg(windows)]
    {
        if let Ok(meta) = entry.metadata() {
            return meta.file_attributes() & 0x2 != 0;
        }
    }

    // Non-Windows fallback: treat a leading dot as hidden.
    #[cfg(not(windows))]
    {
        return entry
            .file_name()
            .to_str()
            .map(|s| s.starts_with('.'))
            .unwrap_or(false);
    }

    #[allow(unreachable_code)]
    false
}

/// Returns true if the path itself is marked hidden on Windows (never follows symlinks/junctions).
pub fn is_path_hidden(path: &Path) -> bool {
    #[cfg(windows)]
    {
        if let Ok(meta) = std::fs::symlink_metadata(path) {
            return meta.file_attributes() & 0x2 != 0;
        }
    }
    #[cfg(not(windows))]
    {
        if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
            return name.starts_with('.');
        }
    }
    false
}

/// True if `path` or any component below its root is hidden, checking top-down
/// from the root and stopping at the first hidden component (using `symlink_metadata`).
pub fn is_hidden_in_roots(path: &Path, roots: &[PathBuf]) -> bool {
    let Some((norm_root, rel_comps)) = relative_components_ci(path, roots) else {
        return is_path_hidden(path);
    };
    let mut cur = norm_root;
    for comp in rel_comps {
        cur.push(comp);
        if is_path_hidden(&cur) {
            return true;
        }
    }
    false
}

/// Index `start` and everything under it. Never follows links, so it cannot
/// loop on symlink/junction cycles. Returns the number of rows written.
pub fn index_tree(tx: &mut BatchTx, start: &Path, filters: &Filters) -> u64 {
    let mut n = 0;
    let walker = WalkDir::new(start).follow_links(false).into_iter();
    for result in walker.filter_entry(|e| e.depth() == 0 || !filters.should_skip_entry(e)) {
        let entry = match result {
            Ok(e) => e,
            Err(_) => continue,
        };
        let meta = match entry.metadata() {
            Ok(m) => m,
            Err(_) => continue,
        };
        let is_dir = entry.file_type().is_dir();
        let path_str = entry.path().to_string_lossy();
        let fe = FileEntry::new(
            entry.file_name().to_string_lossy().to_string(),
            &path_str,
            if is_dir {
                None
            } else {
                entry.path().extension().map(|e| e.to_string_lossy().to_string())
            },
            is_dir,
            meta.created().ok(),
            meta.modified().ok(),
        );
        if database::upsert_entry(tx.write_conn(), &fe).is_ok() {
            tx.tick();
            n += 1;
        }
    }
    n
}

/// Walk `roots`, inserting every visible file and directory into the database.
/// All inserts are wrapped in batch transactions for performance.
///
/// Periodically reports the number of inserted files via `on_progress`.
/// Errors from individual entries are logged and skipped — the crawl never panics.
pub fn run_crawl<F>(
    db_path: &Path,
    roots: &[PathBuf],
    filters: &Filters,
    mut on_progress: F,
) -> Result<(), String>
where
    F: FnMut(u64),
{
    if roots.is_empty() {
        log::info!("[crawler] No roots to index; crawl skipped.");
        return Ok(());
    }

    // The crawler gets its OWN connection. Sharing the app's connection
    // would lock out every search until the crawl finished (the freeze).
    let conn = database::open(db_path)
        .map_err(|e| format!("Could not open the index database: {e}"))?;

    let mut tx = BatchTx::new(&conn);
    let mut count: u64 = 0;

    for root in roots {
        log::info!("[crawler] Indexing {:?} …", root);

        let walker = WalkDir::new(root)
            .follow_links(false) // never follow symlinks / junctions
            .into_iter();

        for result in walker.filter_entry(|e| e.depth() == 0 || !filters.should_skip_entry(e)) {
            let entry = match result {
                Ok(e) => e,
                Err(e) => {
                    // Permission denied and similar — log and continue.
                    log::debug!("[crawler] Skipping: {e}");
                    continue;
                }
            };

            // Skip the root itself.
            if entry.depth() == 0 {
                continue;
            }

            let path_str = entry.path().to_string_lossy().to_string();
            let name_str = entry.file_name().to_string_lossy().to_string();
            let is_dir = entry.file_type().is_dir();

            let extension = if is_dir {
                None
            } else {
                entry
                    .path()
                    .extension()
                    .map(|e| e.to_string_lossy().to_string())
            };

            let (created_at, modified_at) = match entry.metadata() {
                Ok(meta) => (meta.created().ok(), meta.modified().ok()),
                Err(_) => (None, None),
            };

            let file_entry = FileEntry::new(
                name_str,
                &path_str,
                extension,
                is_dir,
                created_at,
                modified_at,
            );

            if let Err(e) = database::insert_entry(tx.write_conn(), &file_entry) {
                log::error!("[crawler] Insert error: {e}");
            } else {
                tx.tick();
                count += 1;
                if count.is_multiple_of(50) {
                    on_progress(count);
                }
            }
        }
    }

    tx.finish();
    on_progress(count);
    log::info!("[crawler] Done. Indexed {count} entries.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_crawler_progress_reporting() {
        let base_dir = std::env::temp_dir().join(format!("wf_test_crawl_{}", std::process::id()));
        let scan_dir = base_dir.join("to_scan");
        let db_dir = base_dir.join("db_storage");
        let _ = fs::create_dir_all(&scan_dir);
        let _ = fs::create_dir_all(&db_dir);

        let sample_file = scan_dir.join("sample.txt");
        let _ = fs::write(&sample_file, "hello");
        let db_path = db_dir.join("crawl_test.db");

        let conn = database::open(&db_path).expect("open db");
        database::create_table(&conn).expect("create table");
        drop(conn);

        let mut counts = Vec::new();
        let filters = Filters::default();
        let res = run_crawl(&db_path, &[scan_dir], &filters, |count| {
            counts.push(count);
        });

        assert!(res.is_ok());
        assert!(!counts.is_empty(), "Progress callback should be called at least at completion");
        assert_eq!(*counts.last().unwrap(), 1);

        let _ = fs::remove_dir_all(&base_dir);
    }

    #[test]
    fn test_filters_extensions_dot_dirs_dot_files_and_ancestors() {
        let base = std::env::temp_dir().join(format!("wf_test_filters_{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        // Ancestor above root literally named "target" (must NOT disable indexing under root).
        let root = base.join("target").join("MyRoot");
        let _ = fs::create_dir_all(root.join("src"));
        let _ = fs::create_dir_all(root.join("x.dll")); // folder named x.dll stays!
        let _ = fs::create_dir_all(root.join(".cargo").join("bin"));
        let _ = fs::create_dir_all(root.join("node_modules").join("pkg"));

        let _ = fs::write(root.join("app.dll"), "bin");
        let _ = fs::write(root.join("UPPER.DLL"), "bin");
        let _ = fs::write(root.join("src").join("main.rs"), "fn main() {}");
        let _ = fs::write(root.join("Makefile"), "all:");
        let _ = fs::write(root.join(".gitignore"), "target/\n");
        let _ = fs::write(root.join("x.dll").join("keep.txt"), "inside dir");
        let _ = fs::write(root.join(".cargo").join("config.toml"), "net = {}");
        let _ = fs::write(root.join("node_modules").join("pkg").join("index.js"), "");

        let db_path = base.join("filters.db");
        let conn = database::open(&db_path).expect("open");
        database::create_table(&conn).expect("create");
        drop(conn);

        let filters = Filters::new(
            &["node_modules".to_string(), ".git".to_string()],
            &["dll".to_string(), "tmp".to_string()],
            true,
        );
        let roots = vec![root.clone()];

        run_crawl(&db_path, &roots, &filters, |_| {}).expect("crawl with skip_dot_dirs=true");

        let conn = database::open(&db_path).expect("reopen");
        assert!(!database::is_indexed(&conn, &root.join("app.dll").to_string_lossy()));
        assert!(!database::is_indexed(&conn, &root.join("UPPER.DLL").to_string_lossy()));
        assert!(database::is_indexed(&conn, &root.join("src").join("main.rs").to_string_lossy()));
        assert!(database::is_indexed(&conn, &root.join("Makefile").to_string_lossy()));
        assert!(database::is_indexed(&conn, &root.join("x.dll").to_string_lossy()));
        assert!(database::is_indexed(&conn, &root.join("x.dll").join("keep.txt").to_string_lossy()));
        assert!(database::is_indexed(&conn, &root.join(".gitignore").to_string_lossy()));
        assert!(!database::is_indexed(&conn, &root.join(".cargo").to_string_lossy()));
        assert!(!database::is_indexed(&conn, &root.join(".cargo").join("config.toml").to_string_lossy()));
        assert!(!database::is_indexed(&conn, &root.join("node_modules").to_string_lossy()));

        // Path-time filter check parity:
        assert!(filters.should_skip_path(&root.join("app.dll"), false, &roots));
        assert!(filters.should_skip_path(&root.join("UPPER.DLL"), false, &roots));
        assert!(!filters.should_skip_path(&root.join("x.dll"), true, &roots));
        assert!(!filters.should_skip_path(&root.join(".gitignore"), false, &roots));
        assert!(filters.should_skip_path(&root.join(".cargo"), true, &roots));
        assert!(filters.should_skip_path(&root.join(".cargo").join("config.toml"), false, &roots));
        assert!(!filters.should_skip_path(&root.join("src").join("main.rs"), false, &roots));

        // When skip_dot_dirs = false, `.cargo` IS indexed during crawl:
        let filters_dots = Filters::new(
            &["node_modules".to_string()],
            &["dll".to_string()],
            false,
        );
        assert!(!filters_dots.should_skip_path(&root.join(".cargo"), true, &roots));
        drop(conn);

        let db_path_dots = base.join("filters_dots.db");
        let conn_dots = database::open(&db_path_dots).expect("open dots db");
        database::create_table(&conn_dots).expect("create dots db");
        drop(conn_dots);
        run_crawl(&db_path_dots, &roots, &filters_dots, |_| {})
            .expect("crawl with skip_dot_dirs=false");
        let conn_dots = database::open(&db_path_dots).expect("reopen dots db");
        assert!(database::is_indexed(&conn_dots, &root.join(".cargo").to_string_lossy()));
        assert!(database::is_indexed(
            &conn_dots,
            &root.join(".cargo").join("config.toml").to_string_lossy()
        ));
        drop(conn_dots);

        let _ = fs::remove_dir_all(&base);
    }

    #[cfg(windows)]
    #[test]
    fn test_ntfs_junction_is_listed_as_non_dir_and_not_entered() {
        let base = std::env::temp_dir().join(format!("wf_test_junction_{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let root = base.join("root");
        let external_target = base.join("target_dir");
        let _ = fs::create_dir_all(&root);
        let _ = fs::create_dir_all(&external_target);
        let _ = fs::write(root.join("normal.txt"), "ok");
        let _ = fs::write(external_target.join("inside_target.txt"), "should not crawl");

        // Junction from root\my_junction -> external_target (and a cyclic junction back to root).
        let junction_path = root.join("my_junction");
        let cycle_path = root.join("cycle_to_root");
        let status1 = std::process::Command::new("cmd")
            .args([
                "/C",
                "mklink",
                "/J",
                &junction_path.to_string_lossy(),
                &external_target.to_string_lossy(),
            ])
            .status()
            .expect("run mklink /J");
        let status2 = std::process::Command::new("cmd")
            .args([
                "/C",
                "mklink",
                "/J",
                &cycle_path.to_string_lossy(),
                &root.to_string_lossy(),
            ])
            .status()
            .expect("run cyclic mklink /J");
        assert!(status1.success() && status2.success(), "mklink /J must succeed");

        let db_path = base.join("junction.db");
        let conn = database::open(&db_path).expect("open");
        database::create_table(&conn).expect("create");
        drop(conn);

        run_crawl(&db_path, std::slice::from_ref(&root), &Filters::default(), |_| {})
            .expect("crawl");

        let conn = database::open(&db_path).expect("reopen");
        // Junction row itself is listed, stored as is_dir = 0 (symlink/reparse point is not a folder),
        // and nothing inside the target or cyclic loop is entered.
        let j_is_dir: i64 = conn
            .query_row(
                "SELECT is_dir FROM files WHERE path = ?1",
                [&paths::normalize_path(&junction_path.to_string_lossy())],
                |r| r.get(0),
            )
            .expect("junction row should exist");
        assert_eq!(j_is_dir, 0);
        assert!(!database::is_indexed(
            &conn,
            &junction_path.join("inside_target.txt").to_string_lossy()
        ));
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM files", [], |r| r.get(0))
            .expect("count rows");
        assert_eq!(count, 3, "expected normal.txt + my_junction + cycle_to_root only");
        drop(conn);

        let _ = fs::remove_dir_all(&base);
    }
}
