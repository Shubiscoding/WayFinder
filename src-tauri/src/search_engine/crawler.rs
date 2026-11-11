// crawler.rs — Walks the user's home folder and inserts every file and
// directory into the SQLite index. Runs on a background thread so the UI
// stays responsive.
//
// Rules enforced here:
//  • No unwrap / expect — every error is logged and skipped.
//  • Never follows symlinks, junctions, or other reparse points.
//  • Skips hidden items (Windows hidden attribute, not just a leading dot).
//  • Skips permission errors and continues.
//  • All inserts happen inside batch transactions for speed and concurrency.
//  • Root dirs and excludes come from Config — nothing is hardcoded.

use crate::search_engine::database::{self, BatchTx, FileEntry};
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

#[cfg(windows)]
use std::os::windows::fs::MetadataExt;

/// Returns true if the entry should be skipped entirely (not descended into
/// when it is a directory, not inserted when it is a file).
fn should_skip(entry: &walkdir::DirEntry, exclude_dirs: &[String]) -> bool {
    let name = entry.file_name().to_string_lossy();

    // Skip user-configured and built-in excluded directory names.
    if entry.file_type().is_dir() {
        for excl in exclude_dirs {
            if name.eq_ignore_ascii_case(excl) {
                return true;
            }
        }
    }

    // Skip hidden items. On Windows, check the hidden file attribute.
    is_hidden(entry)
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

/// Returns true if the path is marked hidden on Windows (or starts with dot).
pub fn is_path_hidden(path: &Path) -> bool {
    #[cfg(windows)]
    {
        if let Ok(meta) = std::fs::metadata(path) {
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

/// Returns true if the directory name matches any entry in `exclude_dirs`.
pub fn is_excluded_name(name: &str, exclude_dirs: &[String]) -> bool {
    exclude_dirs.iter().any(|excl| name.eq_ignore_ascii_case(excl))
}

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

/// Walk `roots`, inserting every visible file and directory into the database.
/// All inserts are wrapped in batch transactions for performance.
///
/// Periodically reports the number of inserted files via `on_progress`.
/// Errors from individual entries are logged and skipped — the crawl never panics.
pub fn run_crawl<F>(
    db_path: &Path,
    roots: &[PathBuf],
    exclude_dirs: &[String],
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

        for result in walker.filter_entry(|e| e.depth() == 0 || !should_skip(e, exclude_dirs)) {
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

            let file_entry = FileEntry {
                name: name_str,
                path: path_str,
                extension,
                is_dir,
                created_at,
                modified_at,
            };

            if let Err(e) = database::insert_entry(tx.conn(), &file_entry) {
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
        let res = run_crawl(&db_path, &[scan_dir], &[], |count| {
            counts.push(count);
        });

        assert!(res.is_ok());
        assert!(!counts.is_empty(), "Progress callback should be called at least at completion");
        assert_eq!(*counts.last().unwrap(), 1);

        let _ = fs::remove_dir_all(&base_dir);
    }
}
