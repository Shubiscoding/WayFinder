// search.rs — Ranked search over the SQLite file index (Phase 5).
//
// How it works:
//   1. Prefix matches come from the (is_dir, name_lower) index. Fast, and an
//      exact name sorts first alphabetically, so it is always fetched.
//   2. If that gives fewer than MAX_RESULTS, we run a "contains" scan.
//   3. If still fewer than MAX_RESULTS and query has >= 2 characters, we run
//      a fuzzy subsequence query scored with `fuzzy-matcher` (SkimMatcherV2).
//   4. Rust scores candidates (100 exact, 90 no-ext, 80 prefix, 70 word,
//      60 contains, 1-50 fuzzy) + recency bonus (+5 for <24h, +2 for <7d).
//   5. Returns the top MAX_RESULTS (5 suggestions).

use fuzzy_matcher::skim::SkimMatcherV2;
use fuzzy_matcher::FuzzyMatcher;
use rusqlite::{params, Connection, Row};
use std::time::{SystemTime, UNIX_EPOCH};

/// Maximum suggestions shown in the launcher.
const MAX_RESULTS: usize = 5;
/// How many prefix candidates to fetch before scoring.
const PREFIX_FETCH: i64 = 50;
/// How many "contains" candidates to fetch before scoring.
const CONTAINS_FETCH: i64 = 200;
/// How many fuzzy candidates to fetch when needed.
const FUZZY_FETCH: i64 = 50;

/// One search result returned to the frontend.
#[derive(serde::Serialize)]
pub struct SearchHit {
    pub name: String,
    pub path: String,
    pub extension: Option<String>,
    pub is_dir: bool,
}

/// Search for files (is_dir = 0) matching `query`.
pub fn search_files(conn: &Connection, query: &str) -> Result<Vec<SearchHit>, String> {
    search_internal(conn, query, false)
}

/// Search for folders (is_dir = 1) matching `query`.
pub fn search_folders(conn: &Connection, query: &str) -> Result<Vec<SearchHit>, String> {
    search_internal(conn, query, true)
}

// --- internal helpers ---

struct Candidate {
    name: String,
    path: String,
    extension: Option<String>,
    is_dir: bool,
    modified_at: Option<i64>,
}

fn search_internal(
    conn: &Connection,
    query: &str,
    is_dir: bool,
) -> Result<Vec<SearchHit>, String> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return Ok(vec![]);
    }

    let mut candidates = fetch_prefix(conn, &q, is_dir)?;

    // If prefix yields fewer than MAX_RESULTS, scan for substring matches.
    if candidates.len() < MAX_RESULTS {
        for extra in fetch_contains(conn, &q, is_dir)? {
            let already_have = candidates.iter().any(|c| c.path == extra.path);
            if !already_have {
                candidates.push(extra);
            }
        }
    }

    // If still fewer than MAX_RESULTS, fetch fuzzy candidates.
    if candidates.len() < MAX_RESULTS && q.chars().count() >= 2 && q.chars().count() <= 12 {
        for extra in fetch_fuzzy(conn, &q, is_dir)? {
            let already_have = candidates.iter().any(|c| c.path == extra.path);
            if !already_have {
                candidates.push(extra);
            }
        }
    }

    let fuzzy_engine = SkimMatcherV2::default();

    let mut scored: Vec<(i32, Candidate)> = candidates
        .into_iter()
        .filter_map(|c| {
            let base_score = score_match(&c.name, &q, &fuzzy_engine);
            if base_score > 0 {
                let recency = recency_bonus(c.modified_at);
                Some((base_score + recency, c))
            } else {
                None
            }
        })
        .collect();

    // Best score first. On a tie the shorter name wins ("report.docx" before
    // "report_final_v2_copy.docx").
    scored.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then(a.1.name.len().cmp(&b.1.name.len()))
    });

    let hits: Vec<SearchHit> = scored
        .into_iter()
        .take(MAX_RESULTS)
        .map(|(_, c)| SearchHit {
            name: c.name,
            path: c.path,
            extension: c.extension,
            is_dir: c.is_dir,
        })
        .collect();

    Ok(hits)
}

fn row_to_candidate(row: &Row) -> rusqlite::Result<Candidate> {
    Ok(Candidate {
        name: row.get(0)?,
        path: row.get(1)?,
        extension: row.get(2)?,
        is_dir: row.get::<_, i64>(3)? != 0,
        modified_at: row.get(4)?,
    })
}

/// Names that START with `q`. Uses the index through a range query:
/// everything >= "q" and < "q" + the highest possible character.
fn fetch_prefix(conn: &Connection, q: &str, is_dir: bool) -> Result<Vec<Candidate>, String> {
    let upper = format!("{q}\u{10FFFF}");

    let mut stmt = conn
        .prepare_cached(
            "SELECT name, path, extension, is_dir, modified_at
             FROM files
             WHERE is_dir = ?1 AND name_lower >= ?2 AND name_lower < ?3
             ORDER BY length(name_lower), name_lower
             LIMIT ?4",
        )
        .map_err(|e| format!("Could not prepare the file search: {e}"))?;

    let rows = stmt
        .query_map(params![is_dir as i64, q, upper, PREFIX_FETCH], row_to_candidate)
        .map_err(|e| format!("Could not run the file search: {e}"))?;

    let found: Vec<Candidate> = rows.filter_map(|r| r.ok()).collect();
    Ok(found)
}

/// Names that CONTAIN `q` anywhere. Slower (scans the table), so it only runs
/// when the prefix search found too little.
fn fetch_contains(conn: &Connection, q: &str, is_dir: bool) -> Result<Vec<Candidate>, String> {
    let pattern = format!("%{}%", escape_like(q));

    let mut stmt = conn
        .prepare_cached(
            "SELECT name, path, extension, is_dir, modified_at
             FROM files
             WHERE is_dir = ?1 AND name_lower LIKE ?2 ESCAPE '\\'
             ORDER BY modified_at DESC
             LIMIT ?3",
        )
        .map_err(|e| format!("Could not prepare the file search: {e}"))?;

    let rows = stmt
        .query_map(params![is_dir as i64, pattern, CONTAINS_FETCH], row_to_candidate)
        .map_err(|e| format!("Could not run the file search: {e}"))?;

    let found: Vec<Candidate> = rows.filter_map(|r| r.ok()).collect();
    Ok(found)
}

/// Fetch candidates containing characters of `q` in sequence for fuzzy matching.
fn fetch_fuzzy(conn: &Connection, q: &str, is_dir: bool) -> Result<Vec<Candidate>, String> {
    let pattern = format!(
        "%{}%",
        q.chars()
            .map(|c| escape_like(&c.to_string()))
            .collect::<Vec<_>>()
            .join("%")
    );

    let mut stmt = conn
        .prepare_cached(
            "SELECT name, path, extension, is_dir, modified_at
             FROM files
             WHERE is_dir = ?1 AND name_lower LIKE ?2 ESCAPE '\\'
             ORDER BY modified_at DESC
             LIMIT ?3",
        )
        .map_err(|e| format!("Could not prepare fuzzy search: {e}"))?;

    let rows = stmt
        .query_map(params![is_dir as i64, pattern, FUZZY_FETCH], row_to_candidate)
        .map_err(|e| format!("Could not run fuzzy search: {e}"))?;

    let found: Vec<Candidate> = rows.filter_map(|r| r.ok()).collect();
    Ok(found)
}

/// Make `%`, `_` and `\` in the user's text match literally in a LIKE query.
fn escape_like(text: &str) -> String {
    text.replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

/// Assign a score to a candidate name against the query.
/// Exact: 100
/// Exact without ext: 90
/// Starts with: 80
/// Word starts with: 70
/// Contains: 60
/// Fuzzy (SkimMatcherV2): 1-50
fn score_match(name: &str, query: &str, fuzzy_engine: &SkimMatcherV2) -> i32 {
    let name_lower = name.to_lowercase();

    // Name without its extension, for the "exact without extension" tier.
    let stem = name_lower
        .rsplit_once('.')
        .map(|(s, _)| s)
        .unwrap_or(&name_lower);

    if name_lower == query {
        return 100;
    }
    if stem == query {
        return 90;
    }
    if name_lower.starts_with(query) {
        return 80;
    }
    let word_match = name_lower
        .split(|c: char| !c.is_alphanumeric())
        .any(|word| !word.is_empty() && word.starts_with(query));
    if word_match {
        return 70;
    }
    if name_lower.contains(query) {
        return 60;
    }

    // Fuzzy matching fallback using SkimMatcherV2
    if let Some(raw_score) = fuzzy_engine.fuzzy_match(&name_lower, query) {
        // Clamp to 1..=50 so fuzzy matches never beat a substring match (60).
        let scaled = (raw_score.clamp(1, 100) / 2) as i32;
        return scaled.clamp(1, 50);
    }

    0
}

/// Compute a subtle recency bonus based on file modification timestamp.
fn recency_bonus(modified_at: Option<i64>) -> i32 {
    if let Some(mod_time) = modified_at {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let age = now.saturating_sub(mod_time);
        if age < 86400 {
            // Modified in the last 24 hours
            return 5;
        } else if age < 7 * 86400 {
            // Modified in the last 7 days
            return 2;
        }
    }
    0
}
