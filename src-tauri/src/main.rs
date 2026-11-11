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
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{Emitter, Manager, State};
use tauri_plugin_autostart::ManagerExt as AutostartManagerExt;
use tauri_plugin_global_shortcut::ShortcutState;
use tauri_plugin_opener::OpenerExt;

// ---------------------------------------------------------------------------
// Shared state
// ---------------------------------------------------------------------------

/// Guard used by hide-on-blur: ignores blur events fired within 150 ms of show.
struct LastShown(Arc<Mutex<Instant>>);

/// Single unified launcher activation path.
/// Records last_shown timestamp, unhides window, requests OS focus, and notifies frontend.
pub fn show_launcher(app: &tauri::AppHandle) {
    if let Some(state) = app.try_state::<LastShown>() {
        let _ = state.0.lock().map(|mut t| *t = Instant::now());
    }
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.set_focus();
        let _ = app.emit("launcher-shown", ());
    }
}

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum SyncPhase {
    Waiting,
    Crawling,
    Reconciling,
    Ready,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct SyncStatus {
    pub phase: SyncPhase,
    pub processed: u64,
    pub total: Option<u64>,
    pub elapsed_ms: u64,
    pub wait_remaining_s: Option<u32>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct SyncProgressEvent {
    pub phase: SyncPhase,
    pub processed: u64,
    pub total: Option<u64>,
    pub elapsed_ms: u64,
    pub wait_remaining_s: Option<u32>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct SyncCompleteEvent {
    pub elapsed_ms: u64,
    pub error: Option<String>,
}

pub struct SyncStateInner {
    pub phase: SyncPhase,
    pub processed: u64,
    pub total: Option<u64>,
    pub started_at: Option<Instant>,
    pub wait_remaining_s: Option<u32>,
}

#[derive(Clone)]
pub struct SyncState(pub Arc<Mutex<SyncStateInner>>);

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

/// Returns a snapshot of the current startup sync progress / state (Phase 5.6).
#[tauri::command(async)]
fn sync_status(sync: State<'_, SyncState>) -> Result<SyncStatus, String> {
    let state = sync
        .0
        .lock()
        .map_err(|e| format!("Could not read sync state: {e}"))?;
    let elapsed_ms = state
        .started_at
        .map(|t| t.elapsed().as_millis() as u64)
        .unwrap_or(0);
    Ok(SyncStatus {
        phase: state.phase,
        processed: state.processed,
        total: state.total,
        elapsed_ms,
        wait_remaining_s: state.wait_remaining_s,
    })
}

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

/// Search for installed desktop applications matching `query`. Returns up to 5 ranked results.
#[tauri::command(async)]
fn search_apps(
    query: String,
    db: State<'_, Db>,
) -> Result<Vec<search_engine::apps::AppHit>, String> {
    let conn = db
        .0
        .lock()
        .map_err(|e| format!("The file index is busy, try again: {e}"))?;
    search_engine::apps::search_apps(&conn, &query)
}

/// Launch an installed desktop application by its ID.
/// Looks up the path in the database, verifies existence, opens via ShellExecuteW,
/// records usage count for ranking, and hides the launcher window.
#[tauri::command(async)]
fn launch_app(
    id: i64,
    app: tauri::AppHandle,
    db: State<'_, Db>,
) -> Result<(), String> {
    let (name, path) = {
        let conn = db
            .0
            .lock()
            .map_err(|e| format!("The file index is busy, try again: {e}"))?;
        search_engine::apps::prepare_launch(&conn, id)?
    };

    app.opener()
        .open_path(path.clone(), None::<&str>)
        .map_err(|e| {
            format!(
                "Windows could not launch \"{name}\": {e}. Check that the shortcut target still exists."
            )
        })?;

    // Record usage count for future ranking
    if let Ok(conn) = db.0.lock() {
        let _ = search_engine::apps::record_app_usage(&conn, id);
    }

    // Hide the launcher window after a successful launch
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.hide();
    }

    Ok(())
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

/// Search for files matching `query` whose extensions can be converted.
#[tauri::command(async)]
fn search_convertible(
    query: String,
    db: State<'_, Db>,
) -> Result<Vec<search::SearchHit>, String> {
    let conn = db
        .0
        .lock()
        .map_err(|e| format!("The file index is busy, try again: {e}"))?;
    let allowed = converter::registry::supported_source_extensions();
    search::search_convertible(&conn, &query, allowed)
}

/// Supported target extensions for a given source extension.
#[tauri::command]
fn conversion_targets(ext: String) -> Vec<String> {
    converter::registry::conversion_targets(&ext)
        .into_iter()
        .map(|s| s.to_string())
        .collect()
}

/// Convert a file from one format to another using the converter registry (Phase 4 / con: rework).
///
/// Runs off the UI thread without holding the database lock.
#[tauri::command(async)]
fn convert_file(
    source_path: String,
    target_name: String,
) -> Result<converter::ConversionResult, String> {
    converter::convert_file(&source_path, &target_name)
}

/// Terminate the application process cleanly, closing all windows and child processes.
#[tauri::command]
fn quit_app(app: tauri::AppHandle) {
    app.exit(0);
}

#[derive(serde::Serialize)]
struct ConfigInfo {
    path: String,
    raw_json: String,
    error: Option<String>,
}

/// Return the raw JSON, path, and parse error (if any) of config.json for the settings view.
#[tauri::command(async)]
fn get_config(app: tauri::AppHandle) -> Result<ConfigInfo, String> {
    let data_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("Could not get app data folder: {e}"))?;
    let path = data_dir.join("config.json");
    let path_str = path.to_string_lossy().to_string();

    let raw_json = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            serde_json::to_string_pretty(&Config::default()).unwrap_or_default()
        }
        Err(e) => return Err(format!("Could not read {}: {e}", path.display())),
    };

    let parse_error = match serde_json::from_str::<Config>(&raw_json) {
        Ok(_) => None,
        Err(e) => Some(format!("JSON syntax error: {e}")),
    };

    Ok(ConfigInfo {
        path: path_str,
        raw_json,
        error: parse_error,
    })
}

/// Open config.json in the user's default text editor.
///
/// Bypasses the search-engine index constraint enforced by `open_path`.
#[tauri::command(async)]
fn open_config(app: tauri::AppHandle) -> Result<(), String> {
    let data_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("Could not get app data folder: {e}"))?;
    let path = data_dir.join("config.json");
    let path_str = path.to_string_lossy().to_string();
    app.opener()
        .open_path(path_str.clone(), None::<&str>)
        .map_err(|e| format!("Could not open config file: {e}"))
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
    let last_shown_blur = Arc::clone(&last_shown);

    tauri::Builder::default()
        // Single instance plugin must be registered FIRST (Phase 5.5.3).
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            show_launcher(app);
        }))
        .plugin(
            tauri_plugin_log::Builder::new()
                .level(log::LevelFilter::Info)
                .level_for("notify", log::LevelFilter::Warn)
                .level_for("notify_debouncer_mini", log::LevelFilter::Warn)
                .build(),
        )
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec!["--autostart"]),
        ))
        .plugin(tauri_plugin_opener::init())
        .manage(LastShown(last_shown))
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
                            show_launcher(app);
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
                log::warn!("[main] {msg}");
                warning.get_or_insert(msg);
            }
            app.manage(StartupWarning(Mutex::new(warning)));

            // ── System Tray Icon (Phase 5.5.2 & 5.5.5) ─────────────────────
            let show_i = MenuItem::with_id(app, "show", "Show WayFinder", true, None::<&str>)?;
            let settings_i = MenuItem::with_id(app, "settings", "Settings", true, None::<&str>)?;
            let quit_i = MenuItem::with_id(app, "quit", "Quit WayFinder", true, None::<&str>)?;
            let tray_menu = Menu::with_items(app, &[&show_i, &settings_i, &quit_i])?;

            if let Some(icon) = app.default_window_icon() {
                let _tray = TrayIconBuilder::new()
                    .icon(icon.clone())
                    .menu(&tray_menu)
                    .show_menu_on_left_click(false)
                    .on_menu_event(|app, event| match event.id.as_ref() {
                        "show" => {
                            show_launcher(app);
                        }
                        "settings" => {
                            show_launcher(app);
                            let _ = app.emit("open-settings", ());
                        }
                        "quit" => {
                            app.exit(0);
                        }
                        _ => {}
                    })
                    .on_tray_icon_event(|tray, event| {
                        if let TrayIconEvent::Click {
                            button: MouseButton::Left,
                            button_state: MouseButtonState::Up,
                            ..
                        } = event
                        {
                            let app = tray.app_handle();
                            show_launcher(app);
                        }
                    })
                    .build(app)?;
            } else {
                log::warn!("[main] No window icon found; system tray icon was not created.");
            }

            // ── Autostart OS sync (Phase 5.5.4) ────────────────────────────
            let autostart_mgr = app.autolaunch();
            let is_enabled = autostart_mgr.is_enabled().unwrap_or(false);
            if cfg.autostart && !is_enabled {
                if let Err(e) = autostart_mgr.enable() {
                    log::error!("[main] Could not enable autostart in OS: {e}");
                } else {
                    log::info!("[main] Autostart enabled in OS.");
                }
            } else if !cfg.autostart && is_enabled {
                if let Err(e) = autostart_mgr.disable() {
                    log::error!("[main] Could not disable autostart in OS: {e}");
                } else {
                    log::info!("[main] Autostart disabled in OS.");
                }
            }
            let is_autostart_launch = std::env::args().any(|arg| arg == "--autostart");

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
                let ls = Arc::clone(&last_shown_blur);
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
                                let _ = w.hide().map_err(|e| {
                                    log::error!(
                                        "[main] Could not hide window on focus loss: {e}"
                                    );
                                });
                            }
                        }
                    });
                }
            }

            // ── Background startup sync, watcher, and fallback (Phase 5.6) ──
            let initial_phase = if is_autostart_launch {
                SyncPhase::Waiting
            } else if needs_crawl {
                SyncPhase::Crawling
            } else {
                SyncPhase::Reconciling
            };

            let sync_state = SyncState(Arc::new(Mutex::new(SyncStateInner {
                phase: initial_phase,
                processed: 0,
                total: None,
                started_at: Some(Instant::now()),
                wait_remaining_s: if is_autostart_launch { Some(30) } else { None },
            })));
            app.manage(sync_state.clone());

            let crawl_done = Arc::new(AtomicBool::new(!needs_crawl));
            app.manage(CrawlDone(Arc::clone(&crawl_done)));

            let db_path_bg = db_path.clone();
            let index_roots_bg = index_roots.clone();
            let exclude_dirs_bg = exclude_dirs.clone();
            let done_bg = Arc::clone(&crawl_done);
            let sync_bg = Arc::clone(&sync_state.0);
            let app_h = app.handle().clone();

            std::thread::spawn(move || {
                let startup_instant = Instant::now();

                // 1. When launched via OS autostart, wait 30 seconds before sync to avoid login disk thrash.
                if is_autostart_launch {
                    log::info!("[main] Launched via autostart; delaying sync by 30 seconds.");
                    for remaining in (1..=30).rev() {
                        {
                            if let Ok(mut st) = sync_bg.lock() {
                                st.phase = SyncPhase::Waiting;
                                st.wait_remaining_s = Some(remaining);
                            }
                        }
                        let _ = app_h.emit(
                            "sync-progress",
                            SyncProgressEvent {
                                phase: SyncPhase::Waiting,
                                processed: 0,
                                total: None,
                                elapsed_ms: startup_instant.elapsed().as_millis() as u64,
                                wait_remaining_s: Some(remaining),
                            },
                        );
                        std::thread::sleep(std::time::Duration::from_secs(1));
                    }
                }

                // 2. Run ONE startup sync: crawl if empty, otherwise reconcile.
                let sync_start = Instant::now();
                let phase = if needs_crawl {
                    SyncPhase::Crawling
                } else {
                    SyncPhase::Reconciling
                };

                {
                    if let Ok(mut st) = sync_bg.lock() {
                        st.phase = phase;
                        st.started_at = Some(sync_start);
                        st.wait_remaining_s = None;
                        st.processed = 0;
                        st.total = None;
                    }
                }

                let _ = app_h.emit(
                    "sync-progress",
                    SyncProgressEvent {
                        phase,
                        processed: 0,
                        total: None,
                        elapsed_ms: 0,
                        wait_remaining_s: None,
                    },
                );

                let last_emit = Arc::new(Mutex::new(
                    Instant::now() - std::time::Duration::from_millis(300),
                ));

                let sync_result: Result<(), String> = if needs_crawl {
                    let last_emit_crawl = Arc::clone(&last_emit);
                    let app_h_crawl = app_h.clone();
                    let sync_bg_crawl = Arc::clone(&sync_bg);
                    crawler::run_crawl(
                        &db_path_bg,
                        &index_roots_bg,
                        &exclude_dirs_bg,
                        move |processed| {
                            let now = Instant::now();
                            let should_emit = match last_emit_crawl.lock() {
                                Ok(mut le) => {
                                    if now.duration_since(*le)
                                        >= std::time::Duration::from_millis(250)
                                    {
                                        *le = now;
                                        true
                                    } else {
                                        false
                                    }
                                }
                                Err(_) => false,
                            };
                            if should_emit {
                                let elapsed_ms = sync_start.elapsed().as_millis() as u64;
                                if let Ok(mut st) = sync_bg_crawl.lock() {
                                    st.processed = processed;
                                    st.total = None;
                                }
                                let _ = app_h_crawl.emit(
                                    "sync-progress",
                                    SyncProgressEvent {
                                        phase: SyncPhase::Crawling,
                                        processed,
                                        total: None,
                                        elapsed_ms,
                                        wait_remaining_s: None,
                                    },
                                );
                            }
                        },
                    )
                } else {
                    let last_emit_rec = Arc::clone(&last_emit);
                    let app_h_rec = app_h.clone();
                    let sync_bg_rec = Arc::clone(&sync_bg);
                    reconcile::run_startup_reconcile(
                        &db_path_bg,
                        &index_roots_bg,
                        &exclude_dirs_bg,
                        move |processed, total| {
                            let now = Instant::now();
                            let should_emit = match last_emit_rec.lock() {
                                Ok(mut le) => {
                                    if now.duration_since(*le)
                                        >= std::time::Duration::from_millis(250)
                                    {
                                        *le = now;
                                        true
                                    } else {
                                        false
                                    }
                                }
                                Err(_) => false,
                            };
                            if should_emit {
                                let elapsed_ms = sync_start.elapsed().as_millis() as u64;
                                if let Ok(mut st) = sync_bg_rec.lock() {
                                    st.processed = processed;
                                    st.total = Some(total);
                                }
                                let _ = app_h_rec.emit(
                                    "sync-progress",
                                    SyncProgressEvent {
                                        phase: SyncPhase::Reconciling,
                                        processed,
                                        total: Some(total),
                                        elapsed_ms,
                                        wait_remaining_s: None,
                                    },
                                );
                            }
                        },
                    )
                };

                // Also sync installed Windows Start Menu apps (fast < 20ms, no extra progress phase).
                let apps_sync_res = search_engine::apps::sync_apps_at(&db_path_bg);
                if let Err(ref e) = apps_sync_res {
                    log::error!("[sync] App discovery failed: {e}");
                }

                let total_elapsed_ms = sync_start.elapsed().as_millis() as u64;
                let sync_error = match (sync_result, apps_sync_res) {
                    (Ok(()), Ok(_)) => {
                        log::info!(
                            "[sync] Startup sync finished successfully in {total_elapsed_ms} ms."
                        );
                        None
                    }
                    (Err(e), _) => {
                        log::error!("[sync] Startup sync failed: {e}");
                        Some(e)
                    }
                    (Ok(()), Err(e)) => Some(e),
                };

                // Mark sync complete and state ready
                {
                    if let Ok(mut st) = sync_bg.lock() {
                        st.phase = SyncPhase::Ready;
                    }
                }
                done_bg.store(true, Ordering::Release);

                let _ = app_h.emit(
                    "sync-complete",
                    SyncCompleteEvent {
                        elapsed_ms: total_elapsed_ms,
                        error: sync_error,
                    },
                );

                // 3. Start real-time filesystem watcher strictly AFTER sync completes.
                watcher::start_watcher(
                    db_path_bg.clone(),
                    index_roots_bg.clone(),
                    exclude_dirs_bg.clone(),
                );

                // 4. Run one cheap fallback check to cover disk changes made between sync scan and watcher start.
                reconcile::run_fallback_check(&db_path_bg, &index_roots_bg, &exclude_dirs_bg);

                // 5. Start the periodic 3-minute fallback loop.
                reconcile::start_fallback_loop(db_path_bg, index_roots_bg, exclude_dirs_bg);
            });

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            search_files,
            search_folders,
            search_apps,
            launch_app,
            search_convertible,
            conversion_targets,
            index_status,
            sync_status,
            take_startup_warning,
            hide_window,
            open_path,
            open_url,
            convert_file,
            quit_app,
            get_config,
            open_config,
        ])
        .run(tauri::generate_context!())
        .expect("WayFinder failed to start");
}
