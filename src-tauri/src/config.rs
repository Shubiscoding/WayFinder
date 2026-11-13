
// config.rs — User configuration for WayFinder.
//
// All user-editable settings live in ONE file in the app-data directory:
//   %APPDATA%\WayFinder\config.json
//
// The launcher works fully without the config file — missing or broken
// config falls back to built-in defaults and shows an error box. Never crash.
//
// The `version` field lets us upgrade old files in the future.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// Config version
// ---------------------------------------------------------------------------

/// Bump this whenever the format changes in a backwards-incompatible way.
/// Old files with a lower version are upgraded by `load()`.
const CURRENT_VERSION: u32 = 1;

// ---------------------------------------------------------------------------
// Config struct
// ---------------------------------------------------------------------------

/// Default file extensions excluded from the search index (binaries, build
/// artifacts, temp files). Folders are never filtered by extension.
fn default_exclude_extensions() -> Vec<String> {
    [
        "dll", "sys", "drv", "ocx", "tmp", "temp", "pyc", "pyo", "class", "o", "obj", "pdb",
        "ilk", "exp", "lib", "rlib", "rmeta", "mui", "cat",
    ]
    .iter()
    .map(|s| (*s).to_string())
    .collect()
}

/// All user options. Add new fields here with a sensible `Default`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)] // any missing field falls back to Default::default()
pub struct Config {
    pub version: u32,
    pub hotkey: String,
    pub index_roots: Vec<String>,
    pub exclude_dirs: Vec<String>,
    pub exclude_extensions: Vec<String>,
    pub skip_dot_dirs: bool,
    pub fallback_interval_secs: u64,
    pub hide_on_blur: bool,
    pub autostart: bool,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            version: CURRENT_VERSION,
            hotkey: "ctrl+space".to_string(),
            index_roots: vec![], // resolved to home dir at runtime when empty
            exclude_dirs: vec!["node_modules".to_string(), ".git".to_string()],
            exclude_extensions: default_exclude_extensions(),
            skip_dot_dirs: true,
            fallback_interval_secs: 900,
            hide_on_blur: true,
            autostart: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Load / save
// ---------------------------------------------------------------------------

/// Load config from `config.json` in `dir`. Returns the config and any optional warning.
pub fn load(dir: &Path) -> (Config, Option<String>) {
    let path = dir.join("config.json");

    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let d = Config::default();
            let warn = save(dir, &d)
                .err()
                .map(|we| format!("Could not write the default config: {we}"));
            return (d, warn);
        }
        Err(e) => {
            return (
                Config::default(),
                Some(format!("Could not read {}: {e}. Using built-in defaults.", path.display())),
            );
        }
    };

    match serde_json::from_str::<Config>(&text) {
        Ok(mut cfg) => {
            let mut warn = None;
            if cfg.version < CURRENT_VERSION {
                cfg = upgrade(cfg);
                if let Err(e) = save(dir, &cfg) {
                    warn = Some(format!("Could not save the upgraded config: {e}"));
                }
            }
            (cfg, warn)
        }
        Err(e) => (
            Config::default(),
            Some(format!(
                "Your config.json ({}) is not valid JSON for WayFinder: {e}. \
                 Built-in defaults are in use until it is fixed.",
                path.display()
            )),
        ),
    }
}

/// Write `config.json` to `dir`, pretty-printed for human readability.
pub fn save(dir: &Path, cfg: &Config) -> Result<(), String> {
    let path = dir.join("config.json");
    let text = serde_json::to_string_pretty(cfg)
        .map_err(|e| format!("Could not serialise config: {e}"))?;
    std::fs::write(&path, text)
        .map_err(|e| format!("Could not write config to {}: {}", path.display(), e))
}

// ---------------------------------------------------------------------------
// Migration
// ---------------------------------------------------------------------------

/// Upgrade an old config to the current version.
/// Each version bump adds a branch here.
fn upgrade(mut cfg: Config) -> Config {
    // Currently only version 1 exists; nothing to do yet.
    cfg.version = CURRENT_VERSION;
    cfg
}

// ---------------------------------------------------------------------------
// Runtime helpers
// ---------------------------------------------------------------------------

/// Return the resolved index roots. If the user left `index_roots` empty,
/// use the home directory. Never panics.
pub fn resolved_roots(cfg: &Config) -> Vec<PathBuf> {
    if !cfg.index_roots.is_empty() {
        return cfg
            .index_roots
            .iter()
            .map(PathBuf::from)
            .collect();
    }
    // Default: home directory.
    match dirs::home_dir() {
        Some(h) => vec![h],
        None => {
            log::warn!("[config] Could not resolve home directory; no roots to index.");
            vec![]
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_legacy_config_json_uses_serde_defaults() {
        let legacy_json = r#"{
            "version": 1,
            "hotkey": "alt+space",
            "index_roots": [],
            "exclude_dirs": ["node_modules", ".git"],
            "hide_on_blur": false,
            "autostart": true
        }"#;

        let cfg: Config = serde_json::from_str(legacy_json).expect("deserialize legacy config");
        assert_eq!(cfg.version, 1);
        assert_eq!(cfg.hotkey, "alt+space");
        assert!(cfg.skip_dot_dirs);
        assert_eq!(cfg.fallback_interval_secs, 900);
        assert!(cfg.exclude_extensions.contains(&"dll".to_string()));
        assert!(!cfg.exclude_extensions.contains(&"exe".to_string()));
        assert!(!cfg.hide_on_blur);
        assert!(cfg.autostart);
    }
}
