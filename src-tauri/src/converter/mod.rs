// converter/mod.rs — File conversion (Phase 4).

pub mod image_conv;
pub mod registry;

use rusqlite::Connection;
use serde::Serialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize)]
pub struct ConversionResult {
    pub source_path: String,
    pub target_path: String,
    pub message: String,
}

/// Step 1 (needs the DB, fast): turn user text into a real file path.
pub fn resolve_source(source: &str, conn: Option<&Connection>) -> Result<PathBuf, String> {
    let trimmed = source.trim().trim_matches('"');
    if trimmed.is_empty() {
        return Err("Please provide a file to convert.".to_string());
    }

    let direct = PathBuf::from(trimmed);
    if direct.is_file() {
        return Ok(direct);
    }

    // Must match how the index stores names: Unicode lowercase.
    let file_name = direct
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(trimmed)
        .to_lowercase();

    if let Some(c) = conn {
        // Newest match first, so "image.png" resolves to the one you used last.
        if let Ok(mut stmt) = c.prepare_cached(
            "SELECT path FROM files
             WHERE is_dir = 0 AND name_lower = ?1
             ORDER BY modified_at DESC
             LIMIT 5",
        ) {
            if let Ok(rows) = stmt.query_map([&file_name], |r| r.get::<_, String>(0)) {
                for p in rows.flatten() {
                    let pb = PathBuf::from(p);
                    if pb.is_file() {
                        return Ok(pb);
                    }
                }
            }
        }
    }

    Err(format!(
        "Could not find \"{trimmed}\" on your computer. \
         Make sure the file still exists or enter its full path."
    ))
}

fn canonical_ext(e: &str) -> &str {
    match e {
        "jpeg" | "jfif" => "jpg",
        "tif" => "tiff",
        other => other,
    }
}

fn unique_destination(parent: &Path, stem: &str, ext: &str) -> Result<PathBuf, String> {
    let first = parent.join(format!("{stem}.{ext}"));
    if !first.exists() {
        return Ok(first);
    }
    for n in 1..1000 {
        let candidate = parent.join(format!("{stem} ({n}).{ext}"));
        if !candidate.exists() {
            return Ok(candidate);
        }
    }
    Err(format!(
        "Too many files named \"{stem}\" already exist next to the original. Clean some up and try again."
    ))
}

/// Step 2 (slow, no DB needed): do the actual conversion.
pub fn convert_resolved(source_path: &Path, target_ext: &str) -> Result<ConversionResult, String> {
    let from_ext = source_path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if from_ext.is_empty() {
        return Err(format!(
            "File \"{}\" has no file extension. WayFinder cannot determine its format.",
            source_path.display()
        ));
    }

    let clean_target = target_ext.trim().trim_start_matches('.').to_ascii_lowercase();
    if clean_target.is_empty() {
        return Err("Please specify a target format (e.g. \"png to jpg\").".to_string());
    }
    if canonical_ext(&from_ext) == canonical_ext(&clean_target) {
        return Err(format!(
            "\".{from_ext}\" and \".{clean_target}\" are the same format. Nothing to convert."
        ));
    }

    let converter = registry::find_converter(&from_ext, &clean_target).ok_or_else(|| {
        format!("Conversion from \".{from_ext}\" to \".{clean_target}\" is not supported yet.")
    })?;

    let stem = source_path.file_stem().and_then(|s| s.to_str()).unwrap_or("converted");
    let parent = source_path.parent().unwrap_or_else(|| Path::new("."));
    let dest_path = unique_destination(parent, stem, &clean_target)?;

    if let Err(e) = (converter.convert)(source_path, &dest_path) {
        let _ = std::fs::remove_file(&dest_path); // never leave a half-written file
        return Err(e);
    }

    let target_name = dest_path.file_name().and_then(|n| n.to_str()).unwrap_or("file");
    Ok(ConversionResult {
        source_path: source_path.display().to_string(),
        target_path: dest_path.display().to_string(),
        message: format!("Saved {target_name}"),
    })
}
