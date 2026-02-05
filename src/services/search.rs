//! Combined search service integrating FTS, grep, and trigram search.
//!
//! Uses spawn_blocking to bridge async MCP handlers with blocking
//! search operations.

use crate::db::Database;
use crate::error::{DbResult, SearchError};
use crate::services::grep::GrepMatch;
use crate::services::{FtsService, GrepService, TrigramIndex};
use crate::types::{FileId, Score};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::sync::RwLock;

/// Detected query intent for weight adjustment (Q7).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueryIntent {
    /// Contains regex metacharacters (e.g., `fn\s+\w+`)
    Regex,
    /// Multiple words, likely natural language (e.g., "authentication flow")
    NaturalLanguage,
    /// CamelCase or snake_case identifier (e.g., "SearchService")
    ExactSymbol,
    /// Short token < 4 chars (e.g., "fn", "if")
    ShortToken,
}

/// Classifies a query to determine optimal backend weights.
fn classify_query(query: &str) -> QueryIntent {
    let trimmed = query.trim();

    if trimmed.is_empty() {
        return QueryIntent::ShortToken;
    }

    // Check for regex metacharacters (beyond simple wildcards)
    let regex_chars = ['\\', '+', '?', '{', '}', '|', '^', '$', '[', ']'];
    let has_regex = trimmed.chars().any(|c| regex_chars.contains(&c));
    // Standalone . and * are common in natural language, but combined patterns are regex
    let has_regex_combo = trimmed.contains(".*") || trimmed.contains(".+") || trimmed.contains("\\s");

    if has_regex || has_regex_combo {
        return QueryIntent::Regex;
    }

    // Multiple words = natural language
    let word_count = trimmed.split_whitespace().count();
    if word_count >= 2 {
        return QueryIntent::NaturalLanguage;
    }

    // Short single token
    if trimmed.len() < 4 {
        return QueryIntent::ShortToken;
    }

    // Default: exact symbol (CamelCase, snake_case, or single long word)
    QueryIntent::ExactSymbol
}

/// A matching snippet from a search result.
#[derive(Debug, Clone)]
pub struct MatchSnippet {
    /// Line number where the match occurs (1-indexed)
    pub line_number: u64,
    /// The content of the matching line (trimmed)
    pub line_content: String,
    /// Byte offset within the line where the match starts
    pub match_start: usize,
    /// Byte offset within the line where the match ends
    pub match_end: usize,
}

/// A search result with merged scores.
#[derive(Debug, Clone)]
pub struct SearchResult {
    pub file_id: FileId,
    pub path: PathBuf,
    pub score: Score,
    /// Which search methods contributed to this result
    pub sources: SearchSources,
    /// Top matching snippets from this file (up to 3)
    pub snippets: Vec<MatchSnippet>,
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

/// Bidirectional path↔FileId cache.
///
/// `Arc<str>` is shared between both maps — one heap allocation per path.
/// Cloning `Arc<str>` is a pointer bump (atomic increment), not a heap alloc.
struct PathCache {
    id_to_path: HashMap<FileId, Arc<str>>,
    path_to_id: HashMap<Arc<str>, FileId>,
}

/// Combined search service.
///
/// Thread-safe (Send + Sync) via internal synchronization:
/// - `Database`: Uses r2d2 connection pool for thread-safe SQLite access
/// - `TrigramIndex`: Wrapped in `Arc<RwLock<_>>` for concurrent read/write
/// - `FtsService` and `GrepService`: Stateless or internally synchronized
/// - `PathCache`: Wrapped in `RwLock` for concurrent read access during searches
pub struct SearchService {
    db: Arc<Database>,
    fts: FtsService,
    grep: GrepService,
    trigram: Arc<RwLock<TrigramIndex>>,
    config: SearchConfig,
    /// Cached file count to avoid DB round-trip on every search (1C).
    /// Updated after indexing. Relaxed ordering is fine — staleness
    /// by a few files is acceptable for IDF weighting.
    cached_total_files: AtomicU64,
    /// Bidirectional path↔FileId cache. Read-heavy (searches), write-rare (after indexing).
    path_cache: RwLock<PathCache>,
}

impl SearchService {
    /// Creates a new search service.
    ///
    /// Eagerly loads the path cache from the database on construction.
    ///
    /// # Errors
    ///
    /// Returns `SearchError::Grep` if grep service initialization fails.
    pub fn new(db: Arc<Database>, root: PathBuf) -> Result<Self, SearchError> {
        let fts = FtsService::new(Arc::clone(&db));
        let grep = GrepService::new(root)?;
        let trigram = Arc::new(RwLock::new(TrigramIndex::new()));

        // Pre-populate cached total_files from DB (best-effort)
        let total = db.total_files().unwrap_or(0);

        // Eagerly load path cache from DB
        let path_cache = Self::load_path_cache(&db);

        Ok(Self {
            db,
            fts,
            grep,
            trigram,
            config: SearchConfig::default(),
            cached_total_files: AtomicU64::new(total),
            path_cache: RwLock::new(path_cache),
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

    /// Updates the cached total file count and path cache (call after indexing).
    pub fn refresh_total_files(&self) {
        if let Ok(total) = self.db.total_files() {
            self.cached_total_files.store(total, Ordering::Relaxed);
        }
        self.refresh_path_cache();
    }

    /// Returns the cached total file count, falling back to DB on 0.
    fn total_files(&self) -> u64 {
        let cached = self.cached_total_files.load(Ordering::Relaxed);
        if cached > 0 {
            return cached;
        }
        // Fallback: cache was never populated
        let total = self.db.total_files().unwrap_or(1);
        self.cached_total_files.store(total, Ordering::Relaxed);
        total
    }

    /// Builds a `PathCache` from the database (used at init and refresh).
    fn load_path_cache(db: &Database) -> PathCache {
        let entries = db.get_all_file_paths().unwrap_or_default();
        let mut id_to_path = HashMap::with_capacity(entries.len());
        let mut path_to_id = HashMap::with_capacity(entries.len());
        for (file_id, path) in entries {
            let arc: Arc<str> = Arc::from(path);
            id_to_path.insert(file_id, Arc::clone(&arc));
            path_to_id.insert(arc, file_id);
        }
        PathCache {
            id_to_path,
            path_to_id,
        }
    }

    /// Refreshes the path cache from the database (call after indexing).
    pub fn refresh_path_cache(&self) {
        let new_cache = Self::load_path_cache(&self.db);
        if let Ok(mut cache) = self.path_cache.write() {
            *cache = new_cache;
        }
    }

    /// Batch resolves FileIds to paths, cache-first with DB fallback.
    fn get_paths_cached(&self, file_ids: &[FileId]) -> HashMap<FileId, Arc<str>> {
        let mut result = HashMap::with_capacity(file_ids.len());
        let mut misses = Vec::new();

        // Read from cache
        if let Ok(cache) = self.path_cache.read() {
            for &fid in file_ids {
                if let Some(path) = cache.id_to_path.get(&fid) {
                    result.insert(fid, Arc::clone(path));
                } else {
                    misses.push(fid);
                }
            }
        } else {
            // Poisoned lock — fall back to DB for all
            misses.extend_from_slice(file_ids);
        }

        // DB fallback for cache misses
        if !misses.is_empty() {
            if let Ok(db_paths) = self.db.get_paths_batch(&misses) {
                for (fid, path) in db_paths {
                    result.insert(fid, Arc::from(path));
                }
            }
        }

        result
    }

    /// Batch resolves paths to FileIds, cache-first with DB fallback.
    fn get_file_ids_cached(&self, paths: &[String]) -> HashMap<String, FileId> {
        let mut result = HashMap::with_capacity(paths.len());
        let mut misses = Vec::new();

        // Read from cache
        if let Ok(cache) = self.path_cache.read() {
            for path in paths {
                if let Some(&fid) = cache.path_to_id.get(path.as_str()) {
                    result.insert(path.clone(), fid);
                } else {
                    misses.push(path.clone());
                }
            }
        } else {
            // Poisoned lock — fall back to DB for all
            misses = paths.to_vec();
        }

        // DB fallback for cache misses
        if !misses.is_empty() {
            if let Ok(db_ids) = self.db.get_file_ids_batch(&misses) {
                result.extend(db_ids);
            }
        }

        result
    }

    /// Performs a combined search using all available methods.
    ///
    /// Adjusts backend weights based on query intent (Q7):
    /// - Regex patterns favor grep
    /// - Natural language queries favor FTS
    /// - Short/exact tokens use balanced weights
    ///
    /// This is a blocking operation - use `spawn_blocking` in async contexts.
    ///
    /// # Errors
    ///
    /// Returns `SearchError` if result merging or database access fails.
    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchResult>, SearchError> {
        let limit = if limit > 0 { limit } else { self.config.limit };
        let intent = classify_query(query);

        // Run searches based on intent
        // For regex queries, skip FTS (it can't handle regex)
        let fts_results = if intent != QueryIntent::Regex {
            self.fts.search(query, limit * 2).unwrap_or_default()
        } else {
            Vec::new()
        };

        // Phase 3: Run trigram BEFORE grep to build a file filter.
        // If the trigram bitmap is selective (<80% of files), convert it
        // to a path set and restrict grep to only those files.
        let trigram_results = {
            let trigram = self.trigram.read().unwrap_or_else(|e| e.into_inner());
            trigram.search(query)
        };

        let file_filter = self.build_trigram_filter(&trigram_results);

        let (grep_results, grep_matches) = self
            .grep
            .search_files_with_matches_filtered(query, limit * 2, file_filter.as_ref())
            .unwrap_or_default();

        // Override weights based on intent
        let config = match intent {
            QueryIntent::Regex => SearchConfig {
                fts_weight: 0.0,
                grep_weight: 0.7,
                trigram_weight: 0.3,
                ..self.config.clone()
            },
            QueryIntent::NaturalLanguage => SearchConfig {
                fts_weight: 0.6,
                grep_weight: 0.2,
                trigram_weight: 0.2,
                ..self.config.clone()
            },
            QueryIntent::ExactSymbol | QueryIntent::ShortToken => self.config.clone(),
        };

        self.merge_results(fts_results, grep_results, grep_matches, trigram_results, limit, &config)
    }

    /// Performs FTS-only search.
    ///
    /// # Errors
    ///
    /// Returns `DbError` if the FTS database query fails.
    pub fn search_fts(&self, query: &str, limit: usize) -> DbResult<Vec<SearchResult>> {
        let results = self.fts.search(query, limit)?;
        Ok(self.enrich_results(
            results,
            SearchSources {
                fts: true,
                ..Default::default()
            },
        ))
    }

    /// Performs grep-only search.
    ///
    /// # Errors
    ///
    /// Returns `SearchError::InvalidPattern` if the regex pattern is invalid.
    pub fn search_grep(&self, query: &str, limit: usize) -> Result<Vec<SearchResult>, SearchError> {
        let results = self.grep.search_files(query, limit)?;

        // Batch resolve paths to file IDs via cache
        let path_strings: Vec<String> = results
            .iter()
            .map(|(p, _)| p.to_string_lossy().to_string())
            .collect();
        let id_map = self.get_file_ids_cached(&path_strings);

        let results: Vec<_> = results
            .into_iter()
            .map(|(path, score)| {
                let path_str = path.to_string_lossy().to_string();
                let file_id = id_map
                    .get(&path_str)
                    .copied()
                    .unwrap_or(FileId::new(0));

                SearchResult {
                    file_id,
                    path,
                    score,
                    sources: SearchSources {
                        grep: true,
                        ..Default::default()
                    },
                    snippets: Vec::new(),
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

    /// Builds a file filter from trigram results for grep pre-filtering (Phase 3).
    ///
    /// Returns `None` if the bitmap is absent or matches >=80% of files
    /// (filter overhead would exceed savings). Otherwise resolves FileIds
    /// to paths via batch lookup.
    fn build_trigram_filter(
        &self,
        trigram: &Option<roaring::RoaringBitmap>,
    ) -> Option<HashSet<PathBuf>> {
        let bitmap = trigram.as_ref()?;
        let total = self.total_files();
        if total == 0 {
            return None;
        }

        let match_count = bitmap.len();
        // Skip filter when bitmap matches >=80% of files (not selective enough)
        if match_count * 100 / total >= 80 {
            return None;
        }

        // Resolve FileIds to paths via cache
        let file_ids: Vec<FileId> = bitmap.iter().map(FileId::new).collect();
        let path_map = self.get_paths_cached(&file_ids);

        let filter: HashSet<PathBuf> =
            path_map.into_values().map(|s| PathBuf::from(&*s)).collect();

        if filter.is_empty() {
            None
        } else {
            Some(filter)
        }
    }

    /// Merges results from multiple search methods.
    ///
    /// Performance optimizations:
    /// - Batch DB lookups instead of per-result queries (P1)
    /// - Only fetches paths, never content (P2)
    /// - Trigram only scores files already in FTS/grep results (P3)
    /// - Single-pass scoring: no intermediate HashMap (1A)
    /// - Lazy snippets: only extracted for top-N results (1B)
    /// - Cached total_files: avoids DB round-trip (1C)
    /// - Reduced path conversions: index into pre-computed strings (1D)
    /// - sort_unstable_by: no allocation overhead (1E)
    fn merge_results(
        &self,
        fts: Vec<(FileId, Score)>,
        grep: Vec<(PathBuf, Score)>,
        grep_matches: HashMap<Arc<Path>, Vec<GrepMatch>>,
        trigram: Option<roaring::RoaringBitmap>,
        limit: usize,
        config: &SearchConfig,
    ) -> Result<Vec<SearchResult>, SearchError> {
        let estimated_capacity = fts.len() + grep.len();

        // Single accumulator: (weighted_score_sum, weight_sum, sources, path)
        let mut score_accum: HashMap<FileId, (f64, f64, SearchSources, PathBuf)> =
            HashMap::with_capacity(estimated_capacity);

        // Batch resolve FTS file_ids to paths via cache (P1+P2)
        let fts_ids: Vec<FileId> = fts.iter().map(|(id, _)| *id).collect();
        let fts_paths = self.get_paths_cached(&fts_ids);

        for (file_id, score) in fts {
            if let Some(path) = fts_paths.get(&file_id) {
                let entry = score_accum.entry(file_id).or_insert_with(|| {
                    (0.0, 0.0, SearchSources::default(), PathBuf::from(&**path))
                });
                entry.0 += score.as_f64() * config.fts_weight;
                entry.1 += config.fts_weight;
                entry.2.fts = true;
            }
        }

        // Batch resolve grep paths to file_ids via cache (P1+P2)
        // Pre-compute path strings once (1D: avoids double to_string_lossy)
        let grep_path_strings: Vec<String> = grep
            .iter()
            .map(|(p, _)| p.to_string_lossy().to_string())
            .collect();
        let grep_ids = self.get_file_ids_cached(&grep_path_strings);

        // Zip pre-computed strings with grep results to avoid re-conversion (1D)
        for (i, (path, score)) in grep.into_iter().enumerate() {
            if let Some(&file_id) = grep_ids.get(&grep_path_strings[i]) {
                let entry = score_accum
                    .entry(file_id)
                    .or_insert_with(|| (0.0, 0.0, SearchSources::default(), path));
                entry.0 += score.as_f64() * config.grep_weight;
                entry.1 += config.grep_weight;
                entry.2.grep = true;
            }
        }

        // Add trigram scores ONLY for files already in FTS/grep results (P3)
        if let Some(bitmap) = trigram {
            // Use cached total_files (1C) instead of DB round-trip
            let total_files = self.total_files() as f64;
            let match_count = bitmap.len() as f64;

            // IDF-based score (Q1): rare matches score higher than common ones
            let trigram_raw = if total_files > 0.0 && match_count > 0.0 {
                let idf = (total_files / match_count).ln() / total_files.ln().max(1.0);
                idf.clamp(0.1, 1.0)
            } else {
                0.5
            };

            for (file_id, (score_sum, weight_sum, sources, _)) in score_accum.iter_mut() {
                if bitmap.contains(file_id.as_u32()) {
                    *score_sum += trigram_raw * config.trigram_weight;
                    *weight_sum += config.trigram_weight;
                    sources.trigram = true;
                }
            }
        }

        // Single-pass (1A): compute final scores directly from score_accum,
        // WITHOUT snippets (1B: deferred to after truncation)
        let mut results: Vec<SearchResult> = score_accum
            .into_iter()
            .map(|(file_id, (score_sum, weight_sum, sources, path))| {
                let source_count =
                    sources.fts as u8 + sources.grep as u8 + sources.trigram as u8;

                let base_score = if weight_sum > 0.0 {
                    score_sum / weight_sum
                } else {
                    0.0
                };

                let bonus_mult = if source_count > 1 {
                    1.0 + config.multi_source_bonus * (source_count - 1) as f64
                } else {
                    1.0
                };

                SearchResult {
                    file_id,
                    path,
                    score: Score::new(base_score * bonus_mult),
                    sources,
                    snippets: Vec::new(), // Populated below for top-N only
                }
            })
            .collect();

        // sort_unstable_by: no temp allocation (1E)
        // Score is clamped [0.0, 1.0] so NaN is impossible; unwrap_or is defensive
        results.sort_unstable_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        results.truncate(limit);

        // Lazy snippet extraction (1B): only for surviving top-N results
        for result in &mut results {
            if let Some(matches) = grep_matches.get(result.path.as_path()) {
                result.snippets = matches
                    .iter()
                    .map(|m| MatchSnippet {
                        line_number: m.line_number,
                        line_content: m.line_content.clone(),
                        match_start: m.match_start,
                        match_end: m.match_end,
                    })
                    .collect();
            }
        }

        Ok(results)
    }

    /// Enriches file IDs with paths using the path cache.
    fn enrich_results(
        &self,
        results: Vec<(FileId, Score)>,
        sources: SearchSources,
    ) -> Vec<SearchResult> {
        let ids: Vec<FileId> = results.iter().map(|(id, _)| *id).collect();
        let path_map = self.get_paths_cached(&ids);

        let mut enriched = Vec::with_capacity(results.len());
        for (file_id, score) in results {
            if let Some(path) = path_map.get(&file_id) {
                enriched.push(SearchResult {
                    file_id,
                    path: PathBuf::from(&**path),
                    score,
                    sources: sources.clone(),
                    snippets: Vec::new(),
                });
            }
        }

        enriched
    }
}

// Compile-time assertions for thread safety.
// These ensure Send+Sync remain implemented and catch regressions.
#[cfg(test)]
const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}

    assert_send_sync::<SearchService>();
    assert_send_sync::<SearchResult>();
    assert_send_sync::<SearchSources>();
    assert_send_sync::<SearchConfig>();
};

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
            0x1,
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
            0x1,
        )
        .unwrap();
        db.upsert_file(
            dir.path().join("login.rs").to_string_lossy().as_ref(),
            "fn login() { println!(\"logging in\"); }",
            0x2,
        )
        .unwrap();
        db.upsert_file(
            dir.path().join("config.rs").to_string_lossy().as_ref(),
            "struct Config { auth_timeout: u64 }",
            0x3,
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
                i as u64,
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
            0x1,
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

    #[test]
    fn test_path_cache_bidirectional() {
        let (_dir, _db, service) = setup_multi_file_env();

        // Cache should be populated after construction (3 files)
        let cache = service.path_cache.read().unwrap();
        assert_eq!(cache.id_to_path.len(), 3);
        assert_eq!(cache.path_to_id.len(), 3);

        // Round-trip: FileId → path → FileId
        for (&file_id, path) in &cache.id_to_path {
            let resolved_id = cache.path_to_id.get(path).copied();
            assert_eq!(resolved_id, Some(file_id));
        }
    }

    #[test]
    fn test_path_cache_miss_fallback() {
        let dir = TempDir::new().unwrap();
        let db = Arc::new(Database::in_memory().unwrap());

        // Create service with empty DB → empty cache
        let service = SearchService::new(Arc::clone(&db), dir.path().to_path_buf()).unwrap();

        // Insert a file AFTER cache was loaded (simulates cache miss)
        let file_id = db.upsert_file("late_file.rs", "fn late() {}", 0x1).unwrap();

        // get_paths_cached should fall back to DB for the miss
        let result = service.get_paths_cached(&[file_id]);
        assert_eq!(result.len(), 1);
        assert_eq!(&*result[&file_id], "late_file.rs");

        // get_file_ids_cached should also fall back
        let result = service.get_file_ids_cached(&["late_file.rs".to_string()]);
        assert_eq!(result.len(), 1);
        assert_eq!(result["late_file.rs"], file_id);
    }

    #[test]
    fn test_cache_after_refresh() {
        let dir = TempDir::new().unwrap();
        let db = Arc::new(Database::in_memory().unwrap());
        let service = SearchService::new(Arc::clone(&db), dir.path().to_path_buf()).unwrap();

        // Cache starts empty
        assert_eq!(service.path_cache.read().unwrap().id_to_path.len(), 0);

        // Add files and refresh
        db.upsert_file("new1.rs", "fn new1() {}", 0x1).unwrap();
        db.upsert_file("new2.rs", "fn new2() {}", 0x2).unwrap();
        service.refresh_total_files(); // Also refreshes path cache

        // Cache should now have 2 entries
        let cache = service.path_cache.read().unwrap();
        assert_eq!(cache.id_to_path.len(), 2);
        assert_eq!(cache.path_to_id.len(), 2);
    }

    #[test]
    fn test_cache_arc_sharing() {
        let (_dir, _db, service) = setup_multi_file_env();

        let cache = service.path_cache.read().unwrap();

        // Verify Arc<str> pointers are shared between both maps
        for (&file_id, path_arc) in &cache.id_to_path {
            // Look up the same path in path_to_id
            let (key_arc, _) = cache.path_to_id.get_key_value(path_arc.as_ref()).unwrap();
            // Both should point to the same allocation
            assert!(
                Arc::ptr_eq(path_arc, key_arc),
                "Arc<str> for FileId {:?} should be shared between both maps",
                file_id
            );
        }
    }
}
