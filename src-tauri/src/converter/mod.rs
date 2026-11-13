// converter/mod.rs — File conversion (Phase 4 / con: rework).

pub mod image_conv;
pub mod registry;

use serde::Serialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize)]
pub struct ConversionResult {
    pub source_path: String,
    pub target_path: String,
    pub message: String,
}

fn canonical_ext(e: &str) -> &str {
    match e {
        "jpeg" | "jfif" => "jpg",
        "tif" => "tiff",
        other => other,
    }
}

/// Convert a file given its exact full source path and the desired target file name.
///
/// Rules:
/// - The source is always an exact full path.
/// - The destination is the source's folder plus `target_name`.
/// - If the destination already exists, refuses with an error (never overwrites).
/// - If conversion fails, removes only a partial file created by this call.
/// - The target file name must be a bare name: rejects `\`, `/` and `:`, and cannot equal the source.
/// - The original file is never modified or removed.
pub fn convert_file(source_path: &str, target_name: &str) -> Result<ConversionResult, String> {
    let trimmed_src = source_path.trim().trim_matches('"');
    if trimmed_src.is_empty() {
        return Err("Please provide a source file to convert.".to_string());
    }
    let src = PathBuf::from(trimmed_src);
    if !src.is_file() {
        return Err(format!(
            "Source file \"{}\" does not exist or was moved.",
            src.display()
        ));
    }

    let parent = match src.parent() {
        Some(p) => p,
        None => return Err("Could not determine the folder of the source file.".to_string()),
    };

    let trimmed_target = target_name.trim().trim_matches('"');
    if trimmed_target.is_empty() {
        return Err("Please provide a target file name.".to_string());
    }

    // Bare file name validation: reject slashes, backslashes, and colons
    if trimmed_target.contains('\\') || trimmed_target.contains('/') || trimmed_target.contains(':') {
        return Err("Target file name must be a file name only, without folder paths or drive letters.".to_string());
    }

    let src_file_name = src
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("");
    if trimmed_target.eq_ignore_ascii_case(src_file_name) {
        return Err("Target file name must be different from the source file.".to_string());
    }

    let from_ext = src
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if from_ext.is_empty() {
        return Err(format!(
            "Source file \"{}\" has no extension. WayFinder cannot determine its format.",
            src.display()
        ));
    }

    let target_path = Path::new(trimmed_target);
    let target_ext = match target_path.extension().and_then(|e| e.to_str()) {
        Some(e) if !e.trim().is_empty() => e.trim().to_ascii_lowercase(),
        _ => {
            return Err(format!(
                "Target file name \"{trimmed_target}\" must include an extension (e.g. \".png\")."
            ));
        }
    };

    if canonical_ext(&from_ext) == canonical_ext(&target_ext) {
        return Err(format!(
            "\".{from_ext}\" and \".{target_ext}\" are the same format. Nothing to convert."
        ));
    }

    let converter = registry::find_converter(&from_ext, &target_ext).ok_or_else(|| {
        let targets = registry::conversion_targets(&from_ext).join(", ");
        format!(
            "Conversion from \".{from_ext}\" to \".{target_ext}\" is not supported. Supported targets for \".{from_ext}\": {targets}."
        )
    })?;

    let dest_path = parent.join(trimmed_target);
    if dest_path.exists() {
        return Err(format!(
            "\"{trimmed_target}\" already exists in {}. WayFinder will not overwrite existing files.",
            parent.display()
        ));
    }

    if let Err(e) = (converter.convert)(&src, &dest_path) {
        let _ = std::fs::remove_file(&dest_path); // never leave a half-written file
        return Err(e);
    }

    Ok(ConversionResult {
        source_path: src.display().to_string(),
        target_path: dest_path.display().to_string(),
        message: format!("Saved {trimmed_target} in {}", parent.display()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_missing_source() {
        let res = convert_file("C:/does_not_exist_file_12345.png", "output.jpg");
        assert!(res.is_err());
        assert!(res.unwrap_err().contains("does not exist or was moved"));
    }

    #[test]
    fn test_bare_name_validation() {
        let temp_dir = std::env::temp_dir().join("wf_test_bare");
        let _ = fs::create_dir_all(&temp_dir);
        let src = temp_dir.join("test.png");
        let _ = fs::write(&src, b"dummy");

        let res_slash = convert_file(src.to_str().unwrap(), "sub/out.jpg");
        assert!(res_slash.is_err());
        assert!(res_slash.unwrap_err().contains("Target file name must be a file name only"));

        let res_bslash = convert_file(src.to_str().unwrap(), "sub\\out.jpg");
        assert!(res_bslash.is_err());
        assert!(res_bslash.unwrap_err().contains("Target file name must be a file name only"));

        let res_colon = convert_file(src.to_str().unwrap(), "D:out.jpg");
        assert!(res_colon.is_err());
        assert!(res_colon.unwrap_err().contains("Target file name must be a file name only"));

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_same_file_name_rejected() {
        let temp_dir = std::env::temp_dir().join("wf_test_samename");
        let _ = fs::create_dir_all(&temp_dir);
        let src = temp_dir.join("image.png");
        let _ = fs::write(&src, b"dummy");

        let res = convert_file(src.to_str().unwrap(), "image.png");
        assert!(res.is_err());
        assert!(res.unwrap_err().contains("Target file name must be different from the source file"));

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_same_format_canonical_rejected() {
        let temp_dir = std::env::temp_dir().join("wf_test_samefmt");
        let _ = fs::create_dir_all(&temp_dir);
        let src = temp_dir.join("photo.jfif");
        let _ = fs::write(&src, b"dummy");

        let res = convert_file(src.to_str().unwrap(), "photo.jpg");
        assert!(res.is_err());
        assert!(res.unwrap_err().contains("are the same format. Nothing to convert."));

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_unsupported_extension_rejected() {
        let temp_dir = std::env::temp_dir().join("wf_test_unsupported");
        let _ = fs::create_dir_all(&temp_dir);
        let src = temp_dir.join("image.png");
        let _ = fs::write(&src, b"dummy");

        let res = convert_file(src.to_str().unwrap(), "image.docx");
        assert!(res.is_err());
        assert!(res.unwrap_err().contains("is not supported"));

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_refuse_overwrite_existing() {
        let temp_dir = std::env::temp_dir().join("wf_test_overwrite");
        let _ = fs::create_dir_all(&temp_dir);
        let src = temp_dir.join("orig.png");
        let dest = temp_dir.join("target.jpg");
        let _ = fs::write(&src, b"orig");
        let _ = fs::write(&dest, b"target");

        let res = convert_file(src.to_str().unwrap(), "target.jpg");
        assert!(res.is_err());
        assert!(res.unwrap_err().contains("already exists"));

        // Original and target must remain untouched
        assert_eq!(fs::read(&dest).unwrap(), b"target");
        assert_eq!(fs::read(&src).unwrap(), b"orig");

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_successful_conversion_and_multi_folder() {
        let sample = Path::new(env!("CARGO_MANIFEST_DIR")).join("images").join("Lemon.jfif");
        if !sample.exists() {
            return;
        }

        let temp_root = std::env::temp_dir().join("wf_test_conv");
        let folder_a = temp_root.join("folderA");
        let folder_b = temp_root.join("folderB");
        let _ = fs::create_dir_all(&folder_a);
        let _ = fs::create_dir_all(&folder_b);

        let src_a = folder_a.join("Lemon.jfif");
        let src_b = folder_b.join("Lemon.jfif");
        fs::copy(&sample, &src_a).unwrap();
        fs::copy(&sample, &src_b).unwrap();
        let orig_size_a = fs::metadata(&src_a).unwrap().len();

        // Convert in folderA
        let res_a = convert_file(src_a.to_str().unwrap(), "Lemon.png").unwrap();
        let dest_a = PathBuf::from(&res_a.target_path);
        assert!(dest_a.exists());
        assert_eq!(dest_a.parent().unwrap(), folder_a.as_path());
        assert_eq!(fs::metadata(&src_a).unwrap().len(), orig_size_a); // original untouched

        // Convert in folderB
        let res_b = convert_file(src_b.to_str().unwrap(), "Lemon.png").unwrap();
        let dest_b = PathBuf::from(&res_b.target_path);
        assert!(dest_b.exists());
        assert_eq!(dest_b.parent().unwrap(), folder_b.as_path());

        let _ = fs::remove_dir_all(&temp_root);
    }
}
