// registry.rs — Table of all supported file conversions (Phase 4).
//
// Each conversion is defined by ONE ConverterEntry. Adding a new conversion
// type in the future (e.g. ffmpeg sidecar in Phase 6) means adding one entry
// to the `CONVERTERS` slice — never editing if/else chains elsewhere.

use super::image_conv;
use std::path::Path;

/// Signature for a conversion function. Takes source path and destination path.
pub type ConvertFn = fn(source: &Path, destination: &Path) -> Result<(), String>;

/// One conversion capability registered in WayFinder.
pub struct ConverterEntry {
    /// Supported input file extensions (lowercase, without dot).
    pub from_extensions: &'static [&'static str],
    /// Supported output file extensions (lowercase, without dot).
    pub to_extensions: &'static [&'static str],
    /// Descriptive name of the underlying tool or crate (e.g. "image crate").
    #[allow(dead_code)]
    pub tool_label: &'static str,
    /// Function executing the conversion.
    pub convert: ConvertFn,
}

impl ConverterEntry {
    /// True if this entry can convert from `from_ext` to `to_ext`.
    pub fn matches(&self, from_ext: &str, to_ext: &str) -> bool {
        let from_clean = from_ext.trim_start_matches('.').to_ascii_lowercase();
        let to_clean = to_ext.trim_start_matches('.').to_ascii_lowercase();

        self.from_extensions
            .iter()
            .any(|&e| e.eq_ignore_ascii_case(&from_clean))
            && self
                .to_extensions
                .iter()
                .any(|&e| e.eq_ignore_ascii_case(&to_clean))
    }
}

/// The registry table.
pub static CONVERTERS: &[ConverterEntry] = &[
    ConverterEntry {
        from_extensions: &["png", "jpg", "jpeg", "jfif", "webp", "bmp", "tiff", "tif"],
        to_extensions: &["png", "jpg", "jpeg", "jfif", "webp", "bmp", "tiff", "tif"],
        tool_label: "image crate (pure Rust)",
        convert: image_conv::convert_image,
    },
];

/// Look up a matching converter in the registry.
pub fn find_converter(from_ext: &str, to_ext: &str) -> Option<&'static ConverterEntry> {
    CONVERTERS.iter().find(|entry| entry.matches(from_ext, to_ext))
}

/// Returns true if a conversion between these extensions is supported.
#[allow(dead_code)]
pub fn is_supported(from_ext: &str, to_ext: &str) -> bool {
    find_converter(from_ext, to_ext).is_some()
}

/// All extensions supported as conversion sources (lowercase, without dot).
pub fn supported_source_extensions() -> &'static [&'static str] {
    &["png", "jpg", "jpeg", "jfif", "webp", "bmp", "tiff", "tif"]
}

/// Supported target extensions for a given source extension.
pub fn conversion_targets(from_ext: &str) -> Vec<&'static str> {
    let from_clean = from_ext.trim_start_matches('.').to_ascii_lowercase();
    let mut targets = Vec::new();
    for entry in CONVERTERS {
        if entry
            .from_extensions
            .iter()
            .any(|&e| e.eq_ignore_ascii_case(&from_clean))
        {
            for &t in entry.to_extensions {
                if !targets.contains(&t) {
                    targets.push(t);
                }
            }
        }
    }
    targets
}

