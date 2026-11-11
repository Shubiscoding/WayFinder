// apps.rs — Start Menu application discovery, storage, ranking, and launching (Phase 5.7).
//
// Responsibilities:
//  • Recursively discover classic Windows desktop app shortcuts (.lnk) in Start Menu folders.
//  • Deduplicate by lowercase name (user shortcuts take precedence over all-users shortcuts).
//  • Filter junk shortcuts (Uninstall, Readme, Help, Website, etc.).
//  • Store apps in the dedicated `apps` table in index.db.
//  • Search and rank apps using tiered matching, SkimMatcherV2, and usage frequency bonus.
//  • Launch apps safely via ShellExecuteW (no cmd, no path from frontend).

use crate::search_engine::crawler;
use crate::search_engine::database::escape_like;
use crate::search_engine::search::score_match;
use fuzzy_matcher::skim::SkimMatcherV2;
use rusqlite::{params, Connection, Row};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Instant;
use walkdir::WalkDir;

/// Maximum app suggestions shown in the launcher.
const MAX_APP_RESULTS: usize = 5;

#[derive(Debug, Clone, serde::Serialize)]
pub struct AppHit {
    pub id: i64,
    pub name: String,
    pub path: String,
    pub usage_count: i64,
}

/// Start Menu directories to scan:
///
/// 1. Per-user (%APPDATA%\Microsoft\Windows\Start Menu\Programs)
/// 2. All-users (%ProgramData%\Microsoft\Windows\Start Menu\Programs)
///
/// User folder is listed first so user shortcuts win when deduplicating.
pub fn start_menu_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();

    #[cfg(windows)]
    {
        if let Some(appdata) = std::env::var_os("APPDATA") {
            let p = PathBuf::from(appdata)
                .join("Microsoft")
                .join("Windows")
                .join("Start Menu")
                .join("Programs");
            if p.exists() {
                dirs.push(p);
            }
        }
        if let Some(progdata) = std::env::var_os("ProgramData") {
            let p = PathBuf::from(progdata)
                .join("Microsoft")
                .join("Windows")
                .join("Start Menu")
                .join("Programs");
            if p.exists() {
                dirs.push(p);
            }
        }
    }

    #[cfg(not(windows))]
    {
        // For non-Windows testing / fallback
        if let Some(home) = dirs::home_dir() {
            let p = home.join(".local").join("share").join("applications");
            if p.exists() {
                dirs.push(p);
            }
        }
    }

    dirs
}

/// Returns true if a shortcut name matches common utility / junk entries
/// that should not appear in application search.
pub fn is_junk_shortcut(stem: &str) -> bool {
    let lower = stem.to_lowercase();
    const JUNK_SUBSTRINGS: &[&str] = &[
        "uninstall",
        "help",
        "readme",
        "read me",
        "documentation",
        "release notes",
        "what's new",
        "license",
        "website",
        "visit website",
        "homepage",
        "web site",
        "support",
        "troubleshoot",
    ];
    JUNK_SUBSTRINGS.iter().any(|&junk| lower.contains(junk))
}

pub struct DiscoveredApp {
    pub name: String,
    pub path: String,
}

/// Internal scanner taking custom directories for testability.
pub fn scan_dirs(dirs: &[PathBuf]) -> Vec<DiscoveredApp> {
    let mut map: HashMap<String, DiscoveredApp> = HashMap::new();

    for dir in dirs {
        if !dir.exists() {
            continue;
        }
        let walker = WalkDir::new(dir).follow_links(false).into_iter();
        for result in walker.filter_entry(|e| !crawler::is_hidden(e)) {
            let entry = match result {
                Ok(e) => e,
                Err(_) => continue,
            };
            if !entry.file_type().is_file() {
                continue;
            }
            let path = entry.path();
            let is_shortcut = path
                .extension()
                .and_then(|ext| ext.to_str())
                .map(|ext| ext.eq_ignore_ascii_case("lnk"))
                .unwrap_or(false);

            if !is_shortcut {
                continue;
            }

            let stem = match path.file_stem().and_then(|s| s.to_str()) {
                Some(s) if !s.trim().is_empty() => s.trim(),
                _ => continue,
            };

            if is_junk_shortcut(stem) {
                continue;
            }

            let lower = stem.to_lowercase();
            // First one encountered wins (per-user dir is checked before all-users dir)
            map.entry(lower).or_insert_with(|| DiscoveredApp {
                name: stem.to_string(),
                path: path.to_string_lossy().to_string(),
            });
        }
    }

    let mut apps: Vec<DiscoveredApp> = map.into_values().collect();
    apps.sort_by_key(|a| a.name.to_lowercase());
    apps
}

/// Synchronize discovered Start Menu shortcuts with the `apps` table in SQLite.
/// Opens a dedicated connection to `db_path`.
pub fn sync_apps_at(db_path: &Path) -> Result<usize, String> {
    let conn = crate::search_engine::database::open(db_path)
        .map_err(|e| format!("Could not open database for app discovery: {e}"))?;
    sync_apps(&conn)
}

/// Synchronize discovered Start Menu shortcuts with the `apps` table in SQLite.
pub fn sync_apps(conn: &Connection) -> Result<usize, String> {
    sync_apps_from_dirs(conn, &start_menu_dirs())
}

/// Synchronize shortcuts from `dirs` into the `apps` table, preserving `usage_count`.
pub fn sync_apps_from_dirs(conn: &Connection, dirs: &[PathBuf]) -> Result<usize, String> {
    let start = Instant::now();
    let discovered = scan_dirs(dirs);
    let scan_dur = start.elapsed();
    let count = discovered.len();

    conn.execute_batch("BEGIN")
        .map_err(|e| format!("Could not begin apps transaction: {e}"))?;

    let mut paths_set = std::collections::HashSet::new();

    let res = (|| -> Result<(), rusqlite::Error> {
        {
            let mut insert_stmt = conn.prepare_cached(
                "INSERT INTO apps (name, name_lower, path, usage_count)
                 VALUES (?1, ?2, ?3, 0)
                 ON CONFLICT(path) DO UPDATE SET
                    name = excluded.name,
                    name_lower = excluded.name_lower",
            )?;

            for app in &discovered {
                paths_set.insert(app.path.clone());
                let name_lower = app.name.to_lowercase();
                insert_stmt.execute(params![app.name, name_lower, app.path])?;
            }
        }

        // Clean up shortcuts no longer present on disk
        {
            let mut select_stmt = conn.prepare_cached("SELECT id, path FROM apps")?;

            let db_apps: Vec<(i64, String)> = select_stmt
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
                .filter_map(|r| r.ok())
                .collect();

            let mut delete_stmt = conn.prepare_cached("DELETE FROM apps WHERE id = ?1")?;

            for (id, path) in db_apps {
                if !paths_set.contains(&path) {
                    let _ = delete_stmt.execute(params![id]);
                }
            }
        }
        Ok(())
    })();

    match res {
        Ok(()) => {
            conn.execute_batch("COMMIT")
                .map_err(|e| format!("Could not commit apps transaction: {e}"))?;
        }
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK");
            return Err(format!("Could not sync apps: {e}"));
        }
    }

    let total_dur = start.elapsed();
    log::info!(
        "[apps] Discovered and synced {count} applications in {total_dur:?} (scan: {scan_dur:?})."
    );
    Ok(count)
}

struct AppCandidate {
    id: i64,
    name: String,
    path: String,
    usage_count: i64,
}

fn row_to_app(row: &Row) -> rusqlite::Result<AppCandidate> {
    Ok(AppCandidate {
        id: row.get(0)?,
        name: row.get(1)?,
        path: row.get(2)?,
        usage_count: row.get(3)?,
    })
}

/// Search for installed applications matching `query`.
/// An empty query returns the most used apps (if usage data exists).
pub fn search_apps(conn: &Connection, query: &str) -> Result<Vec<AppHit>, String> {
    let q = query.trim().to_lowercase();

    if q.is_empty() {
        // Empty query shows the most used apps (if usage data exists)
        let mut stmt = conn
            .prepare_cached(
                "SELECT id, name, path, usage_count
                 FROM apps
                 WHERE usage_count > 0
                 ORDER BY usage_count DESC, name_lower ASC
                 LIMIT ?1",
            )
            .map_err(|e| format!("Could not prepare app search: {e}"))?;

        let rows = stmt
            .query_map(params![MAX_APP_RESULTS as i64], row_to_app)
            .map_err(|e| format!("Could not query apps: {e}"))?;

        let hits: Vec<AppHit> = rows
            .filter_map(|r| r.ok())
            .map(|a| AppHit {
                id: a.id,
                name: a.name,
                path: a.path,
                usage_count: a.usage_count,
            })
            .collect();

        return Ok(hits);
    }

    // 1. Prefix query
    let upper = format!("{q}\u{10FFFF}");
    let mut stmt = conn
        .prepare_cached(
            "SELECT id, name, path, usage_count
             FROM apps
             WHERE name_lower >= ?1 AND name_lower < ?2
             ORDER BY length(name_lower), name_lower
             LIMIT 50",
        )
        .map_err(|e| format!("Could not prepare app prefix search: {e}"))?;

    let mut candidates: Vec<AppCandidate> = stmt
        .query_map(params![q, upper], row_to_app)
        .map_err(|e| format!("Could not run app prefix search: {e}"))?
        .filter_map(|r| r.ok())
        .collect();

    // 2. Substring query if fewer than MAX_APP_RESULTS
    if candidates.len() < MAX_APP_RESULTS {
        let pattern = format!("%{}%", escape_like(&q));
        let mut stmt = conn
            .prepare_cached(
                "SELECT id, name, path, usage_count
                 FROM apps
                 WHERE name_lower LIKE ?1 ESCAPE '\\'
                 ORDER BY usage_count DESC, length(name_lower)
                 LIMIT 50",
            )
            .map_err(|e| format!("Could not prepare app contains search: {e}"))?;

        let contains_rows = stmt
            .query_map(params![pattern], row_to_app)
            .map_err(|e| format!("Could not run app contains search: {e}"))?;

        for extra in contains_rows.filter_map(|r| r.ok()) {
            if !candidates.iter().any(|c| c.id == extra.id) {
                candidates.push(extra);
            }
        }
    }

    // 3. Fuzzy query if still fewer than MAX_APP_RESULTS and query length >= 2
    if candidates.len() < MAX_APP_RESULTS && q.chars().count() >= 2 && q.chars().count() <= 12 {
        let pattern = format!(
            "%{}%",
            q.chars()
                .map(|c| escape_like(&c.to_string()))
                .collect::<Vec<_>>()
                .join("%")
        );
        let mut stmt = conn
            .prepare_cached(
                "SELECT id, name, path, usage_count
                 FROM apps
                 WHERE name_lower LIKE ?1 ESCAPE '\\'
                 ORDER BY usage_count DESC, length(name_lower)
                 LIMIT 50",
            )
            .map_err(|e| format!("Could not prepare app fuzzy search: {e}"))?;

        let fuzzy_rows = stmt
            .query_map(params![pattern], row_to_app)
            .map_err(|e| format!("Could not run app fuzzy search: {e}"))?;

        for extra in fuzzy_rows.filter_map(|r| r.ok()) {
            if !candidates.iter().any(|c| c.id == extra.id) {
                candidates.push(extra);
            }
        }
    }

    let fuzzy_engine = SkimMatcherV2::default();

    let mut scored: Vec<(i32, AppCandidate)> = candidates
        .into_iter()
        .filter_map(|c| {
            let base = score_match(&c.name, &q, &fuzzy_engine);
            if base > 0 {
                // Usage bonus: up to +15 points for frequently launched apps
                let freq_bonus = (c.usage_count.min(15)) as i32;
                Some((base + freq_bonus, c))
            } else {
                None
            }
        })
        .collect();

    // Sort by score DESC, then shorter name, then usage DESC
    scored.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then(a.1.name.len().cmp(&b.1.name.len()))
            .then(b.1.usage_count.cmp(&a.1.usage_count))
    });

    let hits: Vec<AppHit> = scored
        .into_iter()
        .take(MAX_APP_RESULTS)
        .map(|(_, c)| AppHit {
            id: c.id,
            name: c.name,
            path: c.path,
            usage_count: c.usage_count,
        })
        .collect();

    Ok(hits)
}

/// Lookup application name and path by ID, verifying that the file still exists on disk.
pub fn prepare_launch(conn: &Connection, id: i64) -> Result<(String, String), String> {
    let mut stmt = conn
        .prepare_cached("SELECT name, path FROM apps WHERE id = ?1")
        .map_err(|e| format!("Could not query application: {e}"))?;

    let (name, path) = stmt
        .query_row(params![id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        .map_err(|_| "Application was not found in the index. Try searching again.".to_string())?;

    if !Path::new(&path).exists() {
        return Err(format!(
            "Windows could not find \"{name}\" at \"{path}\". The shortcut may have been uninstalled or moved."
        ));
    }

    Ok((name, path))
}

/// Increment launch frequency count for an application.
pub fn record_app_usage(conn: &Connection, id: i64) -> Result<(), String> {
    let mut stmt = conn
        .prepare_cached("UPDATE apps SET usage_count = usage_count + 1 WHERE id = ?1")
        .map_err(|e| format!("Could not record app usage: {e}"))?;

    stmt.execute(params![id])
        .map_err(|e| format!("Could not update app usage: {e}"))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_junk_shortcut_detection() {
        assert!(is_junk_shortcut("Uninstall App"));
        assert!(is_junk_shortcut("Visual Studio Help"));
        assert!(is_junk_shortcut("Readme"));
        assert!(is_junk_shortcut("Read Me.txt"));
        assert!(is_junk_shortcut("Documentation"));
        assert!(is_junk_shortcut("Release Notes"));
        assert!(is_junk_shortcut("Visit Website"));
        assert!(!is_junk_shortcut("Visual Studio Code"));
        assert!(!is_junk_shortcut("Google Chrome"));
        assert!(!is_junk_shortcut("Spotify"));
        assert!(!is_junk_shortcut("Settings"));
    }

    #[test]
    fn test_scan_and_sync_apps() {
        let temp_dir = std::env::temp_dir().join(format!("wf_test_apps_{}", std::process::id()));
        let user_apps = temp_dir.join("user_apps");
        let sys_apps = temp_dir.join("sys_apps");
        let db_dir = temp_dir.join("db");
        let _ = fs::create_dir_all(&user_apps);
        let _ = fs::create_dir_all(&sys_apps);
        let _ = fs::create_dir_all(&db_dir);

        // Create sample shortcuts
        let _ = fs::write(user_apps.join("Code Editor.lnk"), "fake shortcut");
        let _ = fs::write(user_apps.join("Uninstall Code Editor.lnk"), "junk shortcut");
        let _ = fs::write(sys_apps.join("Calculator.lnk"), "fake shortcut");
        // Same name in sys_apps: user version should take precedence
        let _ = fs::write(sys_apps.join("Code Editor.lnk"), "duplicate sys shortcut");

        let scanned = scan_dirs(&[user_apps.clone(), sys_apps.clone()]);
        assert_eq!(scanned.len(), 2);
        let names: Vec<&str> = scanned.iter().map(|a| a.name.as_str()).collect();
        assert!(names.contains(&"Code Editor"));
        assert!(names.contains(&"Calculator"));
        assert!(!names.contains(&"Uninstall Code Editor"));

        // User shortcut path must be selected
        let code_app = scanned.iter().find(|a| a.name == "Code Editor").unwrap();
        assert_eq!(code_app.path, user_apps.join("Code Editor.lnk").to_string_lossy().to_string());

        // Test SQLite sync
        let db_path = db_dir.join("test_apps.db");
        let conn = crate::search_engine::database::open(&db_path).unwrap();
        crate::search_engine::database::create_table(&conn).unwrap();

        // Initial sync from test directories
        let inserted = sync_apps_from_dirs(&conn, &[user_apps.clone(), sys_apps.clone()]).unwrap();
        assert_eq!(inserted, 2);

        // Test search
        let hits = search_apps(&conn, "code").unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].name, "Code Editor");

        // Test launch prep
        let app_id = hits[0].id;
        let (name, path) = prepare_launch(&conn, app_id).unwrap();
        assert_eq!(name, "Code Editor");
        assert_eq!(path, user_apps.join("Code Editor.lnk").to_string_lossy().to_string());

        // Test usage recording
        record_app_usage(&conn, app_id).unwrap();
        record_app_usage(&conn, app_id).unwrap();

        // Empty query search should now return Code Editor as most used
        let empty_hits = search_apps(&conn, "").unwrap();
        assert_eq!(empty_hits.len(), 1);
        assert_eq!(empty_hits[0].name, "Code Editor");
        assert_eq!(empty_hits[0].usage_count, 2);

        // Fuzzy search ("clcltr" -> "Calculator")
        let fuzzy_hits = search_apps(&conn, "clctr").unwrap();
        assert_eq!(fuzzy_hits.len(), 1);
        assert_eq!(fuzzy_hits[0].name, "Calculator");

        // Re-sync must preserve usage_count and drop uninstalled shortcuts
        let _ = fs::remove_file(sys_apps.join("Calculator.lnk"));
        let resynced = sync_apps_from_dirs(&conn, &[user_apps.clone(), sys_apps.clone()]).unwrap();
        assert_eq!(resynced, 1);

        let after_hits = search_apps(&conn, "").unwrap();
        assert_eq!(after_hits.len(), 1);
        assert_eq!(after_hits[0].usage_count, 2);
        assert!(search_apps(&conn, "Calculator").unwrap().is_empty());

        // Broken/missing shortcut on disk returns a plain-English error
        let _ = fs::remove_file(user_apps.join("Code Editor.lnk"));
        let launch_err = prepare_launch(&conn, app_id).unwrap_err();
        assert!(launch_err.contains("could not find"));

        let _ = fs::remove_dir_all(&temp_dir);
    }
}
