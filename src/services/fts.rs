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
            return format!("{col_lower}:{rest_processed}");
        }
    }

    preprocess_words(trimmed)
}

/// Preprocesses individual words for FTS5.
/// Only adds `*` suffix for words >= 4 chars.
fn preprocess_words(input: &str) -> String {
    let escaped = input.replace(['"', '\'', '(', ')', '*'], "");

    escaped
        .split_whitespace()
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

    #[test]
    fn test_preprocess_query() {
        // Words >= 4 chars get * suffix, shorter stay exact
        assert_eq!(preprocess_query("hello world"), "hello* world*");
        assert_eq!(preprocess_query("fn main"), "fn main*");

        // Phrase queries preserved
        assert_eq!(preprocess_query("\"exact phrase\""), "\"exact phrase\"");

        // Column-qualified search
        assert_eq!(preprocess_query("filename:auth"), "filename:auth*");

        // Special chars stripped
        assert_eq!(preprocess_query("test()"), "test*");
    }

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
}
