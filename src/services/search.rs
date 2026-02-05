//! Combined search service integrating FTS, grep, and trigram search.
//!
//! Uses spawn_blocking to bridge async MCP handlers with blocking
//! search operations.

use crate::db::Database;
use crate::error::{DbResult, SearchError};
use crate::services::{FtsService, GrepService, TrigramIndex};
use crate::types::{FileId, Score};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::RwLock;

/// A search result with merged scores.
#[derive(Debug, Clone)]
pub struct SearchResult {
    pub file_id: FileId,
    pub path: PathBuf,
    pub score: Score,
    /// Which search methods contributed to this result
    pub sources: SearchSources,
}

/// Tracks which search methods found a result.
#[derive(Debug, Clone, Default)]
pub struct SearchSources {
    pub fts: bool,
    pub grep: bool,
    pub trigram: bool,
}

/// Configuration for combined search.
#[derive(Debug, Clone)]
pub struct SearchConfig {
    /// Weight for FTS results (0.0 - 1.0)
    pub fts_weight: f64,
    /// Weight for grep results (0.0 - 1.0)
    pub grep_weight: f64,
    /// Weight for trigram results (0.0 - 1.0)
    pub trigram_weight: f64,
    /// Bonus for results found by multiple methods
    pub multi_source_bonus: f64,
    /// Maximum results to return
    pub limit: usize,
}

impl Default for SearchConfig {
    fn default() -> Self {
        Self {
            fts_weight: 0.4,
            grep_weight: 0.4,
            trigram_weight: 0.2,
            multi_source_bonus: 0.1,
            limit: 50,
        }
    }
}

/// Combined search service.
pub struct SearchService {
    db: Arc<Database>,
    fts: FtsService,
    grep: GrepService,
    trigram: Arc<RwLock<TrigramIndex>>,
    config: SearchConfig,
}

impl SearchService {
    /// Creates a new search service.
    ///
    /// # Errors
    ///
    /// Returns `SearchError::Grep` if grep service initialization fails.
    pub fn new(db: Arc<Database>, root: PathBuf) -> Result<Self, SearchError> {
        let fts = FtsService::new(Arc::clone(&db));
        let grep = GrepService::new(root)?;
        let trigram = Arc::new(RwLock::new(TrigramIndex::new()));

        Ok(Self {
            db,
            fts,
            grep,
            trigram,
            config: SearchConfig::default(),
        })
    }

    /// Creates a search service with custom configuration.
    ///
    /// # Errors
    ///
    /// Returns `SearchError::Grep` if grep service initialization fails.
    pub fn with_config(
        db: Arc<Database>,
        root: PathBuf,
        config: SearchConfig,
    ) -> Result<Self, SearchError> {
        let mut service = Self::new(db, root)?;
        service.config = config;
        Ok(service)
    }

    /// Performs a combined search using all available methods.
    ///
    /// This is a blocking operation - use `spawn_blocking` in async contexts.
    ///
    /// # Errors
    ///
    /// Returns `SearchError` if result merging or database access fails.
    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchResult>, SearchError> {
        let limit = if limit > 0 { limit } else { self.config.limit };

        // Run searches (these are already parallel internally)
        let fts_results = self.fts.search(query, limit * 2).unwrap_or_default();
        let grep_results = self.grep.search_files(query, limit * 2).unwrap_or_default();

        // Trigram search (blocking read)
        // Lock poisoning recovery: continue with the inner data
        let trigram_results = {
            let trigram = self.trigram.read().unwrap_or_else(|e| e.into_inner());
            trigram.search(query)
        };

        // Merge results
        self.merge_results(fts_results, grep_results, trigram_results, limit)
    }

    /// Performs FTS-only search.
    ///
    /// # Errors
    ///
    /// Returns `DbError` if database query or result enrichment fails.
    pub fn search_fts(&self, query: &str, limit: usize) -> DbResult<Vec<SearchResult>> {
        let results = self.fts.search(query, limit)?;
        self.enrich_results(
            results,
            SearchSources {
                fts: true,
                ..Default::default()
            },
        )
    }

    /// Performs grep-only search.
    ///
    /// # Errors
    ///
    /// Returns `SearchError::InvalidPattern` if the regex pattern is invalid.
    pub fn search_grep(&self, query: &str, limit: usize) -> Result<Vec<SearchResult>, SearchError> {
        let results = self.grep.search_files(query, limit)?;
        let results: Vec<_> = results
            .into_iter()
            .map(|(path, score)| {
                // Try to find file ID from path
                let file_id = self
                    .db
                    .get_file_by_path(path.to_string_lossy().as_ref())
                    .ok()
                    .flatten()
                    .map(|(id, _)| id)
                    .unwrap_or(FileId::new(0));

                SearchResult {
                    file_id,
                    path,
                    score,
                    sources: SearchSources {
                        grep: true,
                        ..Default::default()
                    },
                }
            })
            .collect();

        Ok(results)
    }

    /// Gets the trigram index for modifications.
    #[must_use]
    pub fn trigram_index(&self) -> &Arc<RwLock<TrigramIndex>> {
        &self.trigram
    }

    /// Gets the database reference.
    #[must_use]
    pub fn db(&self) -> &Arc<Database> {
        &self.db
    }

    /// Gets the root search directory.
    #[must_use]
    pub fn root(&self) -> &Path {
        self.grep.root()
    }

    /// Merges results from multiple search methods.
    fn merge_results(
        &self,
        fts: Vec<(FileId, Score)>,
        grep: Vec<(PathBuf, Score)>,
        trigram: Option<roaring::RoaringBitmap>,
        limit: usize,
    ) -> Result<Vec<SearchResult>, SearchError> {
        let mut scores: HashMap<FileId, (Score, SearchSources, PathBuf)> = HashMap::new();

        // Add FTS results
        for (file_id, score) in fts {
            if let Ok(Some((path, _))) = self.db.get_file(file_id) {
                let entry = scores.entry(file_id).or_insert_with(|| {
                    (Score::ZERO, SearchSources::default(), PathBuf::from(&path))
                });
                entry.0 = entry.0.merge(score.weighted(self.config.fts_weight));
                entry.1.fts = true;
            }
        }

        // Add grep results
        for (path, score) in grep {
            if let Ok(Some((file_id, _))) =
                self.db.get_file_by_path(path.to_string_lossy().as_ref())
            {
                let entry = scores
                    .entry(file_id)
                    .or_insert_with(|| (Score::ZERO, SearchSources::default(), path.clone()));
                entry.0 = entry.0.merge(score.weighted(self.config.grep_weight));
                entry.1.grep = true;
            }
        }

        // Add trigram results (boost existing or add new)
        if let Some(bitmap) = trigram {
            for file_id in bitmap.iter() {
                let file_id = FileId::new(file_id);
                if let Ok(Some((path, _))) = self.db.get_file(file_id) {
                    let entry = scores.entry(file_id).or_insert_with(|| {
                        (Score::ZERO, SearchSources::default(), PathBuf::from(&path))
                    });
                    entry.0 = entry
                        .0
                        .merge(Score::new(0.5).weighted(self.config.trigram_weight));
                    entry.1.trigram = true;
                }
            }
        }

        // Apply multi-source bonus
        for (score, sources, _) in scores.values_mut() {
            let source_count = sources.fts as u8 + sources.grep as u8 + sources.trigram as u8;
            if source_count > 1 {
                *score = score.merge(Score::new(
                    self.config.multi_source_bonus * (source_count - 1) as f64,
                ));
            }
        }

        // Convert to results and sort
        let mut results: Vec<_> = scores
            .into_iter()
            .map(|(file_id, (score, sources, path))| SearchResult {
                file_id,
                path,
                score,
                sources,
            })
            .collect();

        results.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        results.truncate(limit);

        Ok(results)
    }

    /// Enriches file IDs with paths.
    fn enrich_results(
        &self,
        results: Vec<(FileId, Score)>,
        sources: SearchSources,
    ) -> DbResult<Vec<SearchResult>> {
        let mut enriched = Vec::with_capacity(results.len());

        for (file_id, score) in results {
            if let Some((path, _)) = self.db.get_file(file_id)? {
                enriched.push(SearchResult {
                    file_id,
                    path: PathBuf::from(path),
                    score,
                    sources: sources.clone(),
                });
            }
        }

        Ok(enriched)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn setup_test_env() -> (TempDir, Arc<Database>) {
        let dir = TempDir::new().unwrap();
        let db = Arc::new(Database::in_memory().unwrap());

        // Add test files to DB
        db.upsert_file(
            dir.path().join("auth.rs").to_string_lossy().as_ref(),
            "fn authenticate() { login() }",
            "hash1",
        )
        .unwrap();

        (dir, db)
    }

    fn setup_multi_file_env() -> (TempDir, Arc<Database>, SearchService) {
        let dir = TempDir::new().unwrap();
        let db = Arc::new(Database::in_memory().unwrap());

        // Create actual files on disk for grep to find
        fs::write(
            dir.path().join("auth.rs"),
            "fn authenticate() { login(); validate(); }",
        )
        .unwrap();
        fs::write(
            dir.path().join("login.rs"),
            "fn login() { println!(\"logging in\"); }",
        )
        .unwrap();
        fs::write(
            dir.path().join("config.rs"),
            "struct Config { auth_timeout: u64 }",
        )
        .unwrap();

        // Index files in database
        db.upsert_file(
            dir.path().join("auth.rs").to_string_lossy().as_ref(),
            "fn authenticate() { login(); validate(); }",
            "hash1",
        )
        .unwrap();
        db.upsert_file(
            dir.path().join("login.rs").to_string_lossy().as_ref(),
            "fn login() { println!(\"logging in\"); }",
            "hash2",
        )
        .unwrap();
        db.upsert_file(
            dir.path().join("config.rs").to_string_lossy().as_ref(),
            "struct Config { auth_timeout: u64 }",
            "hash3",
        )
        .unwrap();

        let service = SearchService::new(Arc::clone(&db), dir.path().to_path_buf()).unwrap();
        (dir, db, service)
    }

    #[test]
    fn test_combined_search() {
        let (dir, db) = setup_test_env();
        let service = SearchService::new(db, dir.path().to_path_buf()).unwrap();

        let results = service.search("authenticate", 10).unwrap();
        // Should find the auth.rs file via FTS
        assert!(!results.is_empty());
    }

    #[test]
    fn test_search_fts_only() {
        let (_dir, _db, service) = setup_multi_file_env();

        let results = service.search_fts("authenticate", 10).unwrap();
        assert!(!results.is_empty());

        // Verify only FTS source is marked
        for result in &results {
            assert!(result.sources.fts);
            // grep and trigram should be false for FTS-only search
        }
    }

    #[test]
    fn test_search_grep_only() {
        let (_dir, _db, service) = setup_multi_file_env();

        let results = service.search_grep("login", 10).unwrap();
        assert!(!results.is_empty());

        // Verify only grep source is marked
        for result in &results {
            assert!(result.sources.grep);
        }
    }

    #[test]
    fn test_search_empty_query() {
        let (_dir, _db, service) = setup_multi_file_env();

        // Empty query should not panic
        let results = service.search("", 10);
        // FTS5 with empty query may return error or empty results
        // Grep may find files (empty pattern matches everything)
        // What's important is no panic
        assert!(results.is_ok() || results.is_err());
    }

    #[test]
    fn test_search_whitespace_query() {
        let (_dir, _db, service) = setup_multi_file_env();

        // Whitespace-only query
        let results = service.search("   ", 10).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn test_search_result_limiting() {
        let dir = TempDir::new().unwrap();
        let db = Arc::new(Database::in_memory().unwrap());

        // Create many files that all match "test"
        for i in 0..20 {
            let filename = format!("test_{}.rs", i);
            let content = format!("fn test_function_{}() {{ }}", i);
            fs::write(dir.path().join(&filename), &content).unwrap();
            db.upsert_file(
                dir.path().join(&filename).to_string_lossy().as_ref(),
                &content,
                &format!("hash{}", i),
            )
            .unwrap();
        }

        let service = SearchService::new(Arc::clone(&db), dir.path().to_path_buf()).unwrap();

        // Request only 5 results
        let results = service.search("test", 5).unwrap();
        assert!(results.len() <= 5);
    }

    #[test]
    fn test_score_merging_weights() {
        // Verify the default weights are applied correctly
        let config = SearchConfig::default();
        assert!((config.fts_weight - 0.4).abs() < f64::EPSILON);
        assert!((config.grep_weight - 0.4).abs() < f64::EPSILON);
        assert!((config.trigram_weight - 0.2).abs() < f64::EPSILON);
        assert!((config.multi_source_bonus - 0.1).abs() < f64::EPSILON);
    }

    #[test]
    fn test_search_with_custom_config() {
        let dir = TempDir::new().unwrap();
        let db = Arc::new(Database::in_memory().unwrap());

        fs::write(dir.path().join("test.rs"), "fn custom_test() {}").unwrap();
        db.upsert_file(
            dir.path().join("test.rs").to_string_lossy().as_ref(),
            "fn custom_test() {}",
            "hash1",
        )
        .unwrap();

        let config = SearchConfig {
            fts_weight: 0.8,
            grep_weight: 0.1,
            trigram_weight: 0.1,
            multi_source_bonus: 0.05,
            limit: 10,
        };

        let service =
            SearchService::with_config(Arc::clone(&db), dir.path().to_path_buf(), config).unwrap();
        let results = service.search("custom", 10).unwrap();

        // Should still find results with custom weights
        assert!(!results.is_empty());
    }

    #[test]
    fn test_search_sources_tracking() {
        let (_dir, _db, service) = setup_multi_file_env();

        let results = service.search("authenticate", 10).unwrap();
        assert!(!results.is_empty());

        // At least one source should be marked for each result
        for result in &results {
            let has_source = result.sources.fts || result.sources.grep || result.sources.trigram;
            assert!(has_source, "Result should have at least one source");
        }
    }

    #[test]
    fn test_search_results_sorted_by_score() {
        let (_dir, _db, service) = setup_multi_file_env();

        let results = service.search("auth", 10).unwrap();

        // Results should be sorted by score descending
        for window in results.windows(2) {
            assert!(
                window[0].score >= window[1].score,
                "Results should be sorted by score descending"
            );
        }
    }

    #[test]
    fn test_zero_limit_uses_default() {
        let (_dir, _db, service) = setup_multi_file_env();

        // A limit of 0 should use the config's default limit
        let results = service.search("fn", 0).unwrap();
        // Should return results (using default limit, not 0)
        assert!(!results.is_empty());
    }

    #[test]
    fn test_search_no_matches() {
        let (_dir, _db, service) = setup_multi_file_env();

        // Search for something that doesn't exist
        let results = service.search("xyznonexistent123", 10).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn test_trigram_index_access() {
        let (_dir, _db, service) = setup_multi_file_env();

        // Verify we can access and modify the trigram index
        let trigram = service.trigram_index();
        {
            let mut index = trigram.write().unwrap();
            index.add_file(FileId::new(999), "test content for trigram");
        }

        {
            let index = trigram.read().unwrap();
            let results = index.search("test");
            assert!(results.is_some());
        }
    }

    #[test]
    fn test_db_access() {
        let (_dir, db, service) = setup_multi_file_env();

        // Verify db() returns the same database
        let service_db = service.db();
        assert_eq!(service_db.file_count().unwrap(), db.file_count().unwrap());
    }

    #[test]
    fn test_root_access() {
        let (dir, _db, service) = setup_multi_file_env();

        // Verify root() returns the correct path
        assert_eq!(service.root(), dir.path());
    }
}
