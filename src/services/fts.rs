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
    /// # Errors
    ///
    /// Returns `DbError` if the database query fails.
    pub fn search(&self, query: &str, limit: usize) -> DbResult<Vec<(FileId, Score)>> {
        let fts_query = preprocess_query(query);
        let results = self.db.fts_search(&fts_query, limit)?;

        // Convert BM25 scores to our Score type
        // BM25 scores are negative (lower = better match)
        let max_score = results.iter().map(|(_, s)| s.abs()).fold(0.0f64, f64::max);

        let normalized: Vec<_> = results
            .into_iter()
            .map(|(file_id, bm25)| {
                // Normalize: more negative BM25 = higher Score
                let normalized = if max_score > 0.0 {
                    bm25.abs() / max_score
                } else {
                    0.0
                };
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
/// - Escapes special FTS5 characters
/// - Converts to lowercase for consistency
/// - Adds prefix matching for partial words
fn preprocess_query(query: &str) -> String {
    let escaped = query
        .replace(['"', '\'', '(', ')', '*'], "")
        .replace(':', " ");

    // Split into words and add prefix matching
    escaped
        .split_whitespace()
        .map(|word| format!("{word}*"))
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_preprocess_query() {
        assert_eq!(preprocess_query("hello world"), "hello* world*");
        assert_eq!(preprocess_query("auth:login"), "auth* login*");
        assert_eq!(preprocess_query("test()"), "test*");
    }

    #[test]
    fn test_fts_service() {
        let db = Arc::new(Database::in_memory().unwrap());
        db.upsert_file("test.rs", "fn authenticate() {}", "hash1")
            .unwrap();
        db.upsert_file("main.rs", "fn main() {}", "hash2").unwrap();

        let fts = FtsService::new(db);
        let results = fts.search("authenticate", 10).unwrap();

        assert_eq!(results.len(), 1);
    }
}
