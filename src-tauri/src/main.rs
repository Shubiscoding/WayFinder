// main.rs — Tauri entry point for WayFinder.
//
// Responsibilities:
//  • Load user config (config.json in the app-data dir).
//  • Register plugins (opener, global-shortcut) and the hotkey from config.
//  • Open (or create) the SQLite database in the app-data directory.
//  • On first run, spawn the file-system crawler on a background thread.
//  • Expose Tauri commands that the frontend calls with `invoke`.
//
// Threading rule: any command that touches the disk or the database uses
// #[tauri::command(async)] so it runs OFF the main (UI) thread. A plain
// command runs on the main thread and freezes the window while it works.

mod config;
mod converter;
mod search_engine;

use config::Config;
use search_engine::{crawler, database, reconcile, search, watcher};

use rusqlite::Connection;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tauri::{Emitter, Manager, State};
use tauri_plugin_global_shortcut::ShortcutState;
use tauri_plugin_opener::OpenerExt;

// ---------------------------------------------------------------------------
// Shared state
// ---------------------------------------------------------------------------

/// The connection used by searches. The crawler has its own connection.
struct Db(Mutex<Connection>);

/// Loaded config, available to commands that need it later.
#[allow(dead_code)]
struct AppConfig(Config);

/// Set to true when the first crawl (or startup reconcile) has finished.
struct CrawlDone(Arc<AtomicBool>);

/// A one-shot message the frontend shows in the error box at startup.
struct StartupWarning(Mutex<Option<String>>);

#[derive(serde::Serialize)]
struct IndexStatus {
    has_rows: bool,
    complete: bool,
}

// ---------------------------------------------------------------------------
// Tauri commands (called from JS with `invoke`)
// ---------------------------------------------------------------------------

/// Search for files matching `query`. Returns up to 5 ranked results.
#[tauri::command(async)]
fn search_files(
    query: String,
    db: State<'_, Db>,
) -> Result<Vec<search::SearchHit>, String> {
    let conn = db
        .0
        .lock()
        .map_err(|e| format!("The file index is busy, try again: {e}"))?;
    search::search_files(&conn, &query)
}

/// Search for folders matching `query`. Returns up to 5 ranked results.
#[tauri::command(async)]
fn search_folders(
    query: String,
    db: State<'_, Db>,
) -> Result<Vec<search::SearchHit>, String> {
    let conn = db
        .0
        .lock()
        .map_err(|e| format!("The file index is busy, try again: {e}"))?;
    search::search_folders(&conn, &query)
}

#[tauri::command(async)]
fn index_status(db: State<'_, Db>, done: State<'_, CrawlDone>) -> Result<IndexStatus, String> {
    let conn = db
        .0
        .lock()
        .map_err(|e| format!("The file index is busy, try again: {e}"))?;
    let has_rows: i64 = conn
        .query_row("SELECT EXISTS(SELECT 1 FROM files)", [], |r| r.get(0))
        .map_err(|e| format!("Could not check the file index: {e}"))?;
    Ok(IndexStatus {
        has_rows: has_rows != 0,
        complete: done.0.load(Ordering::Acquire),
    })
}

#[tauri::command]
fn take_startup_warning(w: State<'_, StartupWarning>) -> Option<String> {
    w.0.lock().ok().and_then(|mut g| g.take())
}

/// Hide the launcher window (Escape or click outside). The global hotkey
/// shows it again.
#[tauri::command]
fn hide_window(window: tauri::WebviewWindow) -> Result<(), String> {
    window
        .hide()
        .map_err(|e| format!("Could not hide the launcher window: {e}"))
}

/// Open a file or folder with its default app (same as double-clicking it).
///
/// SECURITY: only opens paths verified to exist in the SQLite index, preventing
/// injected strings from opening arbitrary files, and hands the path to Windows
/// ShellExecuteW without shell text parsing.
#[tauri::command(async)]
fn open_path(app: tauri::AppHandle, db: State<'_, Db>, path: String) -> Result<(), String> {
    {
        let conn = db
            .0
            .lock()
            .map_err(|e| format!("The file index is busy, try again: {e}"))?;
        if !search_engine::database::is_indexed(&conn, &path) {
            return Err(format!(
                "\"{path}\" is not in the file index, so WayFinder will not open it."
            ));
        }
    } // lock released before launching

    app.opener()
        .open_path(path.clone(), None::<&str>)
        .map_err(|e| {
            format!(
                "Windows could not open \"{path}\": {e}. \
                 Check that it still exists and that you can open it."
            )
        })
}

#[tauri::command]
fn open_url(app: tauri::AppHandle, url: String) -> Result<(), String> {
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return Err("Only web addresses (http/https) can be opened.".to_string());
    }
    app.opener()
        .open_url(url, None::<&str>)
        .map_err(|e| format!("Could not open your web browser: {e}"))
}

/// Convert a file from one format to another using the converter registry (Phase 4).
///
/// Runs off the UI thread and releases the database lock before performing encoding.
#[tauri::command(async)]
fn convert_file(
    source: String,
    target_ext: String,
    db: State<'_, Db>,
) -> Result<converter::ConversionResult, String> {
    let source_path = {
        let guard = db.0.lock().ok();
        converter::resolve_source(&source, guard.as_deref())?
    };
    converter::convert_resolved(&source_path, &target_ext)
}

// ---------------------------------------------------------------------------
// App setup
// ---------------------------------------------------------------------------

fn main() {
    // Guard used by the hide-on-blur feature: ignore blur events that arrive
    // within 150 ms of the window being shown (the OS sometimes fires blur
    // immediately after show, which would cause an instant re-hide).
    // Initialised 1 s in the past so the very first blur is never suppressed.
    let last_shown: Arc<Mutex<Instant>> = Arc::new(Mutex::new(
        Instant::now() - std::time::Duration::from_secs(1),
    ));
    // Clone shared into the shortcut handler (runs before setup).
    let last_shown_shortcut = Arc::clone(&last_shown);
    // Clone shared into setup (for the window-event handler).
    let last_shown_setup = Arc::clone(&last_shown);

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(move |app, _shortcut, event| {
                    // Only react on key-down, not key-up.
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
                .build(),
        )
        .setup(move |app| {
            // App data folder (created if missing). Holds config + index.
            let data_dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&data_dir)?;

            // Config always loads (falls back to defaults), never crashes.
            let (cfg, cfg_warning) = config::load(&data_dir);
            let mut warning = cfg_warning;

            // Register the global hotkey from the config. If another program
            // owns it, log a plain message and keep running.
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

            let exclude_dirs = cfg.exclude_dirs.clone();
            let index_roots = config::resolved_roots(&cfg);
            // Stash hide_on_blur before cfg is moved into AppConfig.
            let hide_on_blur = cfg.hide_on_blur;
            app.manage(AppConfig(cfg));

            // Open (or create) the index database for searches.
            let db_path = data_dir.join("index.db");
            let conn = database::open(&db_path).map_err(|e| {
                format!(
                    "Could not open the file index at {}: {}",
                    db_path.display(),
                    e
                )
            })?;
            database::create_table(&conn)
                .map_err(|e| format!("Could not set up the database schema: {e}"))?;

            // An empty table means we need the first crawl.
            let needs_crawl: bool = conn
                .query_row("SELECT EXISTS(SELECT 1 FROM files)", [], |r| {
                    r.get::<_, i64>(0)
                })
                .map(|has_rows| has_rows == 0)
                .unwrap_or(true);

            app.manage(Db(Mutex::new(conn)));

            // ── Hide-on-blur ────────────────────────────────────────────────
            // When the launcher window loses focus we hide it, so clicking
            // anywhere outside (another app, the desktop) closes it. The
            // 150 ms guard prevents an instant re-hide on show (some versions
            // of Windows fire Focused(false) immediately after show()).
            if hide_on_blur {
                let app_h = app.handle().clone();
                let ls = Arc::clone(&last_shown_setup);
                if let Some(win) = app.get_webview_window("main") {
                    win.on_window_event(move |event| {
                        if let tauri::WindowEvent::Focused(false) = event {
                            let elapsed = ls
                                .lock()
                                .map(|t| t.elapsed())
                                .unwrap_or(std::time::Duration::from_secs(1));
                            if elapsed < std::time::Duration::from_millis(150) {
                                // Spurious blur right after show — ignore it.
                                return;
                            }
                            if let Some(w) = app_h.get_webview_window("main") {
                                if let Err(e) = w.hide() {
                                    eprintln!(
                                        "[main] Could not hide window on focus loss: {e}"
                                    );
                                }
                            }
                        }
                    });
                }
            }

            // ── Background indexing, watching, and reconcile (Phase 5) ─────
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
                done_bg.store(true, Ordering::Release);
            });

            // Real-time filesystem watcher with debouncing (Phase 5).
            watcher::start_watcher(db_path.clone(), index_roots.clone(), exclude_dirs.clone());

            // 3-minute lightweight periodic fallback check (Phase 5).
            reconcile::start_fallback_loop(db_path, index_roots, exclude_dirs);

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            search_files,
            search_folders,
            index_status,
            take_startup_warning,
            hide_window,
            open_path,
            open_url,
            convert_file,
        ])
        .run(tauri::generate_context!())
        .expect("WayFinder failed to start");
}
