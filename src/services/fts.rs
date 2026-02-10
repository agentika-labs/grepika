//! FTS5 full-text search service.
//!
//! Wraps the database's FTS5 capabilities with a cleaner interface
//! and query preprocessing.

use crate::db::Database;
use crate::error::DbResult;
use crate::types::{FileId, Score};
use std::sync::Arc;

/// FTS5 search service.
pub struct FtsService {
    db: Arc<Database>,
}

impl FtsService {
    /// Creates a new FTS service.
    pub fn new(db: Arc<Database>) -> Self {
        Self { db }
    }

    /// Searches using FTS5 with BM25 ranking.
    ///
    /// BM25 scores are normalized using a fixed reference value rather than
    /// relative to the result set. This preserves absolute relevance signals:
    /// a strong match set will have higher scores than a weak one.
    ///
    /// # Errors
    ///
    /// Returns `DbError` if the database query fails.
    pub fn search(&self, query: &str, limit: usize) -> DbResult<Vec<(FileId, Score)>> {
        let fts_query = preprocess_query(query);
        if fts_query.is_empty() {
            return Ok(Vec::new());
        }
        let results = self.db.fts_search(&fts_query, limit)?;

        // Fixed-reference BM25 normalization (Q3):
        // BM25 scores are negative (more negative = better match).
        // Using a fixed reference preserves absolute relevance,
        // unlike max-normalization which always gives top result Score(1.0).
        // Reference value of 15.0 tuned to typical BM25 range with
        // column weights (5.0, 10.0, 1.0) for code search.
        const BM25_REFERENCE: f64 = 15.0;

        let normalized: Vec<_> = results
            .into_iter()
            .map(|(file_id, bm25)| {
                let normalized = (bm25.abs() / BM25_REFERENCE).min(1.0);
                (file_id, Score::new(normalized))
            })
            .collect();

        Ok(normalized)
    }

    /// Searches with phrase matching.
    ///
    /// # Errors
    ///
    /// Returns `DbError` if the database query fails.
    pub fn search_phrase(&self, phrase: &str, limit: usize) -> DbResult<Vec<(FileId, Score)>> {
        // Wrap in quotes for exact phrase matching
        let escaped = phrase.replace('"', "");
        let fts_query = format!("\"{escaped}\"");
        self.db.fts_search(&fts_query, limit).map(|results| {
            results
                .into_iter()
                .map(|(id, score)| (id, Score::new(score.abs())))
                .collect()
        })
    }

    /// Searches by filename only.
    ///
    /// # Errors
    ///
    /// Returns `DbError` if the database query fails.
    pub fn search_filename(&self, query: &str, limit: usize) -> DbResult<Vec<(FileId, Score)>> {
        let preprocessed = preprocess_query(query);
        let fts_query = format!("filename:{preprocessed}");
        self.db.fts_search(&fts_query, limit).map(|results| {
            results
                .into_iter()
                .map(|(id, score)| (id, Score::new(score.abs())))
                .collect()
        })
    }
}

/// Preprocesses a query for FTS5.
///
/// Improved preprocessing (Q4):
/// - Preserves `"..."` for phrase matching
/// - Preserves `column:` prefix for column-qualified searches
/// - Only adds `*` suffix for words >= 4 chars (short tokens stay exact)
/// - Strips other FTS5 special characters
fn preprocess_query(query: &str) -> String {
    let trimmed = query.trim();

    // Preserve phrase queries: "exact phrase"
    if trimmed.starts_with('"') && trimmed.ends_with('"') && trimmed.len() > 2 {
        return trimmed.to_string();
    }

    // Check for column-qualified search (e.g., "filename:auth")
    if let Some((col, rest)) = trimmed.split_once(':') {
        let col_lower = col.to_lowercase();
        if col_lower == "filename" || col_lower == "path" || col_lower == "content" {
            let rest_processed = preprocess_words(rest);
            if rest_processed.is_empty() {
                return String::new();
            }
            let tokens: Vec<&str> = rest_processed.split_whitespace().collect();
            return match tokens.len() {
                1 => format!("{col_lower}:{}", tokens[0]),
                _ => tokens
                    .iter()
                    .map(|t| format!("{col_lower}:{t}"))
                    .collect::<Vec<_>>()
                    .join(" "),
            };
        }
    }

    preprocess_words(trimmed)
}

/// FTS5 boolean keywords that must be filtered out (case-sensitive, ALL-CAPS only per FTS5 spec).
const FTS5_KEYWORDS: &[&str] = &["AND", "OR", "NOT", "NEAR"];

/// Checks if a character is valid in an FTS5 bareword token.
/// Based on `sqlite3Fts5IsBareword` from `fts5_buffer.c`:
/// only non-ASCII, alphanumeric, and underscore are allowed.
fn is_fts5_bareword_char(c: char) -> bool {
    !c.is_ascii() || c.is_ascii_alphanumeric() || c == '_'
}

/// Preprocesses individual words for FTS5.
///
/// - Replaces non-bareword ASCII chars with spaces (preserves token boundaries)
/// - Filters FTS5 boolean keywords (AND, OR, NOT, NEAR)
/// - Only adds `*` suffix for words >= 4 chars
fn preprocess_words(input: &str) -> String {
    let cleaned: String = input
        .chars()
        .map(|c| if is_fts5_bareword_char(c) { c } else { ' ' })
        .collect();

    cleaned
        .split_whitespace()
        .filter(|word| !FTS5_KEYWORDS.contains(word))
        .map(|word| {
            // Short tokens (fn, if, do, etc.) stay exact for code search precision
            if word.len() >= 4 {
                format!("{word}*")
            } else {
                word.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── preprocess_words unit tests ──────────────────────────────

    #[test]
    fn test_preprocess_words_hyphens() {
        // Hyphens replaced with space, both tokens preserved
        assert_eq!(preprocess_words("auth-element"), "auth* element*");
    }

    #[test]
    fn test_preprocess_words_operators() {
        assert_eq!(preprocess_words("c++"), "c");
        assert_eq!(preprocess_words("foo:bar"), "foo bar");
        assert_eq!(preprocess_words("^start"), "start*");
        assert_eq!(preprocess_words("test{1}"), "test* 1");
        assert_eq!(preprocess_words("a.b.c"), "a b c");
    }

    #[test]
    fn test_preprocess_words_fts5_keywords() {
        assert_eq!(preprocess_words("auth AND login"), "auth* login*");
        assert_eq!(preprocess_words("read OR write"), "read* write*");
        assert_eq!(preprocess_words("NOT found"), "found*");
        // Lowercase "and"/"or" are NOT FTS5 keywords — preserved
        assert_eq!(preprocess_words("and or"), "and or");
    }

    #[test]
    fn test_preprocess_words_all_special() {
        assert_eq!(preprocess_words("---"), "");
        assert_eq!(preprocess_words("!@#$%"), "");
    }

    #[test]
    fn test_preprocess_words_syntax_error_chars() {
        assert_eq!(preprocess_words("hello@world.com"), "hello* world* com");
        assert_eq!(preprocess_words("std::string"), "std string*");
    }

    // ── preprocess_query unit tests ─────────────────────────────

    #[test]
    fn test_preprocess_query() {
        // Words >= 4 chars get * suffix, shorter stay exact
        assert_eq!(preprocess_query("hello world"), "hello* world*");
        assert_eq!(preprocess_query("fn main"), "fn main*");

        // Phrase queries preserved
        assert_eq!(preprocess_query("\"exact phrase\""), "\"exact phrase\"");

        // Column-qualified search
        assert_eq!(preprocess_query("filename:auth"), "filename:auth*");

        // Special chars stripped (existing test still passes)
        assert_eq!(preprocess_query("test()"), "test*");
    }

    #[test]
    fn test_preprocess_query_column_multi_token() {
        // Column prefix applied to all tokens when special chars split the value
        assert_eq!(
            preprocess_query("filename:auth-element"),
            "filename:auth* filename:element*"
        );
        assert_eq!(
            preprocess_query("content:std::string"),
            "content:std content:string*"
        );
    }

    #[test]
    fn test_preprocess_query_empty_after_sanitize() {
        assert_eq!(preprocess_query("---"), "");
        assert_eq!(preprocess_query("!@#"), "");
        assert_eq!(preprocess_query("filename:---"), "");
    }

    // ── Integration tests ───────────────────────────────────────

    #[test]
    fn test_fts_service() {
        let db = Arc::new(Database::in_memory().unwrap());
        db.upsert_file("test.rs", "fn authenticate() {}", 0x1)
            .unwrap();
        db.upsert_file("main.rs", "fn main() {}", 0x2).unwrap();

        let fts = FtsService::new(db);
        let results = fts.search("authenticate", 10).unwrap();

        assert_eq!(results.len(), 1);
    }

    #[test]
    fn test_fts_search_hyphenated_query() {
        let db = Arc::new(Database::in_memory().unwrap());
        db.upsert_file("auth-element.tsx", "export function AuthElement() {}", 0x1)
            .unwrap();
        let fts = FtsService::new(db);
        // Previously failed with "no such column: element"
        let results = fts.search("auth-element", 10);
        assert!(results.is_ok());
        assert!(!results.unwrap().is_empty());
    }

    #[test]
    fn test_fts_search_all_special_chars_returns_empty() {
        let db = Arc::new(Database::in_memory().unwrap());
        db.upsert_file("test.rs", "fn main() {}", 0x1).unwrap();
        let fts = FtsService::new(db);
        let results = fts.search("!@#$%", 10).unwrap();
        assert!(results.is_empty());
    }
}
