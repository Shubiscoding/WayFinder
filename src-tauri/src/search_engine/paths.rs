// paths.rs — Single path normalizer and case-insensitive path helpers for Windows.
// Every writer into SQLite and every path comparison routes through here so mixed
// drive-letter casing, forward/back slashes, or trailing separators never split
// one disk item into duplicate rows.

use std::path::Path;

/// Normalize a path string into canonical Windows-style form without touching disk:
/// - converts `/` to `\`
/// - strips Windows verbatim prefixes (`\\?\` and `\\?\UNC\`)
/// - collapses repeated `\` (preserving a leading UNC `\\`)
/// - uppercases an ASCII drive letter (`c:` -> `C:`)
/// - strips trailing `\` except on bare root (`\`), UNC prefix (`\\`), or drive roots (`C:\`)
pub fn normalize_path(path: &str) -> String {
    if path.is_empty() {
        return String::new();
    }

    let replaced = path.replace('/', "\\");

    let (is_unc, rest) = if let Some(after) = strip_prefix_ascii_ci(&replaced, "\\\\?\\UNC\\") {
        (true, after)
    } else if let Some(after) = replaced.strip_prefix("\\\\?\\") {
        (false, after)
    } else if replaced.starts_with("\\\\") {
        (true, replaced.trim_start_matches('\\'))
    } else {
        (false, replaced.as_str())
    };

    let mut out = String::with_capacity(replaced.len());
    if is_unc {
        out.push_str("\\\\");
    }

    let mut prev_sep = false;
    for ch in rest.chars() {
        if ch == '\\' {
            if !prev_sep {
                out.push('\\');
                prev_sep = true;
            }
        } else {
            out.push(ch);
            prev_sep = false;
        }
    }

    if !is_unc {
        let bytes = out.as_bytes();
        if bytes.len() >= 2 && bytes[0].is_ascii_lowercase() && bytes[1] == b':' {
            let upper = bytes[0].to_ascii_uppercase() as char;
            out.replace_range(0..1, &upper.to_string());
        }
    }

    while out.len() > 1 && out.ends_with('\\') {
        if out == "\\\\" || is_drive_root(&out) {
            break;
        }
        out.pop();
    }

    out
}

/// Canonical lowercase key used for in-memory path maps/sets on Windows.
pub fn path_key(path: &str) -> String {
    normalize_path(path).to_lowercase()
}

/// True if `path` equals `dir` or is a descendant of `dir` (case-insensitive,
/// mixed-separator and trailing-slash safe).
pub fn is_under_ci(path: &Path, dir: &Path) -> bool {
    let p = path_key(&path.to_string_lossy());
    let d = path_key(&dir.to_string_lossy());
    if p.is_empty() || d.is_empty() {
        return false;
    }
    if p == d {
        return true;
    }
    if d.ends_with('\\') {
        p.starts_with(&d)
    } else {
        p.starts_with(&d) && p.as_bytes().get(d.len()) == Some(&b'\\')
    }
}

/// True if `a` and `b` denote the same path after normalization and lowercasing.
pub fn is_same_path_ci(a: &Path, b: &Path) -> bool {
    let ka = path_key(&a.to_string_lossy());
    let kb = path_key(&b.to_string_lossy());
    !ka.is_empty() && ka == kb
}

fn is_drive_root(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && b[2] == b'\\'
}

fn strip_prefix_ascii_ci<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    if s.len() < prefix.len() {
        return None;
    }
    let (head, tail) = s.split_at(prefix.len());
    if head.eq_ignore_ascii_case(prefix) {
        Some(tail)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_separators_drive_case_and_trailing_slash() {
        assert_eq!(normalize_path("c:/users/Alice//Docs/"), r"C:\users\Alice\Docs");
        assert_eq!(normalize_path(r"c:\users\Alice\Docs\"), r"C:\users\Alice\Docs");
        assert_eq!(normalize_path(r"\\?\c:\Users\Alice\"), r"C:\Users\Alice");
    }

    #[test]
    fn test_normalize_preserves_drive_root_and_unc() {
        assert_eq!(normalize_path("c:/"), r"C:\");
        assert_eq!(normalize_path(r"c:\\\"), r"C:\");
        assert_eq!(normalize_path(r"C:\"), r"C:\");
        assert_eq!(normalize_path("//server/share//dir/"), r"\\server\share\dir");
        assert_eq!(normalize_path(r"\\?\UNC\server\share\dir\"), r"\\server\share\dir");
        assert_eq!(normalize_path(""), "");
    }

    #[test]
    fn test_path_key_case_folds_normalized_paths() {
        assert_eq!(
            path_key("c:/Users/Alice/File.TXT/"),
            path_key(r"C:\users\alice\file.txt")
        );
    }

    #[test]
    fn test_is_under_ci_handles_casing_separators_and_sibling_prefixes() {
        let dir = Path::new("c:/Users/Alice/");
        assert!(is_under_ci(Path::new(r"C:\users\ALICE\docs\a.txt"), dir));
        assert!(is_under_ci(Path::new(r"C:\users\alice"), dir));
        assert!(!is_under_ci(Path::new(r"C:\users\Alice2\a.txt"), dir));
        assert!(!is_under_ci(Path::new(r"C:\users\Alice"), Path::new("")));

        let root = Path::new("c:/");
        assert!(is_under_ci(Path::new(r"C:\Windows\System32"), root));
        assert!(!is_under_ci(Path::new(r"D:\Windows\System32"), root));
    }

    #[test]
    fn test_is_same_path_ci() {
        assert!(is_same_path_ci(
            Path::new("c:/Users/Alice/"),
            Path::new(r"C:\users\ALICE")
        ));
        assert!(!is_same_path_ci(
            Path::new(r"C:\Users\Alice"),
            Path::new(r"C:\Users\Bob")
        ));
        assert!(!is_same_path_ci(Path::new(""), Path::new("")));
    }
}

