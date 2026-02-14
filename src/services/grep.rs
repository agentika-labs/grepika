//! Parallel grep service using ripgrep internals.
//!
//! Uses `WalkParallel` from the `ignore` crate to overlap directory
//! walking with file searching, with per-thread `Searcher` reuse.
//!
//! # Security
//!
//! This module includes ReDoS protection via pattern validation.
//! See [`crate::security::validate_regex_pattern`] for details.

use crate::error::{GrepError, SearchError};
use crate::security;
use crate::types::Score;
use grep_matcher::Matcher;
use grep_regex::RegexMatcher;
use grep_searcher::sinks::UTF8;
use grep_searcher::Searcher;
use ignore::{WalkBuilder, WalkState};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

/// Scored files with their matching line snippets.
pub type GrepSearchResult = (Vec<(PathBuf, Score)>, HashMap<Arc<Path>, Vec<GrepMatch>>);

/// Match found by grep.
#[derive(Debug, Clone)]
pub struct GrepMatch {
    pub path: Arc<Path>,
    pub line_number: u64,
    pub line_content: String,
    pub match_start: usize,
    pub match_end: usize,
}

/// Configuration for grep operations.
#[derive(Debug, Clone)]
pub struct GrepConfig {
    /// Maximum files to search (0 = unlimited)
    pub max_files: usize,
    /// Maximum matches to return (0 = unlimited)
    pub max_matches: usize,
    /// Include hidden files
    pub include_hidden: bool,
    /// Follow symlinks
    pub follow_symlinks: bool,
    /// Case insensitive search
    pub case_insensitive: bool,
    /// Context lines before match
    pub before_context: usize,
    /// Context lines after match
    pub after_context: usize,
    /// Maximum threads for parallel grep (0 = auto-detect)
    pub max_threads: usize,
    /// Upper bound on thread count (caps both auto-detected and explicit values)
    pub thread_cap: usize,
}

impl Default for GrepConfig {
    fn default() -> Self {
        Self {
            max_files: 10000,
            max_matches: 1000,
            include_hidden: false,
            follow_symlinks: false,
            case_insensitive: false,
            before_context: 0,
            after_context: 0,
            max_threads: 0, // Auto-detect
            thread_cap: 8,
        }
    }
}

/// Parallel grep service using ripgrep internals.
pub struct GrepService {
    /// Number of parallel walk+search threads
    num_threads: usize,
    /// Root directory to search
    root: PathBuf,
    /// Default configuration
    config: GrepConfig,
}

impl GrepService {
    /// Creates a new grep service.
    ///
    /// # Errors
    ///
    /// Returns `SearchError::Grep` if configuration is invalid.
    pub fn new(root: PathBuf) -> Result<Self, SearchError> {
        Self::with_config(root, GrepConfig::default())
    }

    /// Creates a grep service with custom configuration.
    ///
    /// # Errors
    ///
    /// Returns `SearchError::Grep` if configuration is invalid.
    pub fn with_config(root: PathBuf, config: GrepConfig) -> Result<Self, SearchError> {
        let cap = config.thread_cap;
        let num_threads = if config.max_threads > 0 {
            config.max_threads.min(cap)
        } else {
            std::thread::available_parallelism()
                .map_or(4, |n| n.get())
                .min(cap)
        };

        Ok(Self {
            num_threads,
            root,
            config,
        })
    }

    /// Searches for pattern in files under root directory.
    ///
    /// # Security
    ///
    /// The pattern is validated for potential ReDoS vulnerabilities before
    /// regex compilation. Patterns with nested quantifiers or excessive
    /// complexity will be rejected.
    ///
    /// # Errors
    ///
    /// Returns `SearchError::InvalidPattern` if the regex pattern is invalid
    /// or potentially dangerous.
    pub fn search_parallel(
        &self,
        pattern: &str,
        limit: usize,
    ) -> Result<Vec<GrepMatch>, SearchError> {
        self.search_parallel_filtered(pattern, limit, None)
    }

    /// Searches with an optional file filter (Phase 3: trigram pre-filtering).
    ///
    /// Uses `WalkParallel` to overlap directory traversal with file searching.
    /// Each walker thread gets its own `Searcher` instance (reused across files
    /// on that thread), avoiding per-file allocation overhead.
    ///
    /// When `file_filter` is `Some`, only files in the set are searched.
    /// This avoids scanning files the trigram index already ruled out.
    pub fn search_parallel_filtered(
        &self,
        pattern: &str,
        limit: usize,
        file_filter: Option<&HashSet<Arc<Path>>>,
    ) -> Result<Vec<GrepMatch>, SearchError> {
        // Validate pattern for ReDoS vulnerabilities
        security::validate_regex_pattern(pattern)
            .map_err(|e| SearchError::InvalidPattern(e.to_string()))?;

        let matcher = RegexMatcher::new_line_matcher(pattern)
            .map_err(|e| SearchError::InvalidPattern(e.to_string()))?;

        let max_matches = if limit > 0 {
            limit
        } else {
            self.config.max_matches
        };

        let match_count = Arc::new(AtomicUsize::new(0));
        let file_count = Arc::new(AtomicUsize::new(0));
        let results: Arc<Mutex<Vec<GrepMatch>>> = Arc::new(Mutex::new(Vec::new()));
        let matcher = Arc::new(matcher);

        let walker = WalkBuilder::new(&self.root)
            .hidden(!self.config.include_hidden)
            .follow_links(self.config.follow_symlinks)
            .git_ignore(true)
            .git_global(true)
            .git_exclude(true)
            .threads(self.num_threads)
            .build_parallel();

        let max_files = self.config.max_files;

        walker.run(|| {
            // Per-thread state: factory called once per walker thread
            let mut searcher = Searcher::new();
            let matcher = Arc::clone(&matcher);
            let mc = Arc::clone(&match_count);
            let fc = Arc::clone(&file_count);
            let res = Arc::clone(&results);

            Box::new(move |entry| {
                // Early termination: enough matches collected
                if mc.load(Ordering::Relaxed) >= max_matches {
                    return WalkState::Quit;
                }

                let entry = match entry {
                    Ok(e) => e,
                    Err(_) => return WalkState::Continue,
                };

                if !entry.file_type().is_some_and(|ft| ft.is_file()) {
                    return WalkState::Continue;
                }

                // Max files limit (non-deterministic order, acceptable — results are scored)
                if fc.fetch_add(1, Ordering::Relaxed) >= max_files {
                    return WalkState::Quit;
                }

                let path = entry.path();

                // Trigram pre-filter: skip files not in the filter set
                if let Some(filter) = file_filter {
                    if !filter.contains(path) {
                        return WalkState::Continue;
                    }
                }

                // Search with per-thread Searcher (reused across files on this thread)
                let arc_path: Arc<Path> = Arc::from(path);
                let mut file_matches = Vec::new();

                let search_ok = searcher
                    .search_path(
                        &*matcher,
                        path,
                        UTF8(|line_number, line| {
                            if let Ok(Some(m)) = matcher.find(line.as_bytes()) {
                                file_matches.push(GrepMatch {
                                    path: Arc::clone(&arc_path),
                                    line_number,
                                    line_content: line.trim_end().to_string(),
                                    match_start: m.start(),
                                    match_end: m.end(),
                                });
                            }
                            Ok(true)
                        }),
                    )
                    .is_ok();

                if search_ok && !file_matches.is_empty() {
                    let count = mc.fetch_add(file_matches.len(), Ordering::Relaxed);
                    if let Ok(mut r) = res.lock() {
                        r.extend(file_matches);
                    }
                    if count >= max_matches {
                        return WalkState::Quit;
                    }
                }

                WalkState::Continue
            })
        });

        // Safe: WalkParallel::run() uses thread::scope internally —
        // all threads are joined before run() returns.
        let mut results = Arc::try_unwrap(results)
            .map_err(|_| {
                SearchError::Grep(GrepError::Walk("walker threads still hold Arc".into()))
            })?
            .into_inner()
            .unwrap_or_else(|poisoned| {
                tracing::warn!("grep results mutex was poisoned, recovering partial results");
                poisoned.into_inner()
            });
        results.truncate(max_matches);
        Ok(results)
    }

    /// Searches and returns file-level results with scores.
    ///
    /// # Errors
    ///
    /// Returns `SearchError::InvalidPattern` if the regex pattern is invalid.
    pub fn search_files(
        &self,
        pattern: &str,
        limit: usize,
    ) -> Result<Vec<(PathBuf, Score)>, SearchError> {
        let (results, _) = self.search_files_with_matches(pattern, limit)?;
        Ok(results)
    }

    /// Searches and returns file-level results with scores plus top matches per file.
    ///
    /// Returns `(scored_files, matches_by_file)` where `matches_by_file` contains
    /// the top 3 `GrepMatch`es per file for snippet generation.
    ///
    /// # Errors
    ///
    /// Returns `SearchError::InvalidPattern` if the regex pattern is invalid.
    pub fn search_files_with_matches(
        &self,
        pattern: &str,
        limit: usize,
    ) -> Result<GrepSearchResult, SearchError> {
        self.search_files_with_matches_filtered(pattern, limit, None)
    }

    /// Like `search_files_with_matches` but with optional trigram pre-filter.
    pub fn search_files_with_matches_filtered(
        &self,
        pattern: &str,
        limit: usize,
        file_filter: Option<&HashSet<Arc<Path>>>,
    ) -> Result<GrepSearchResult, SearchError> {
        // Overcollect by ~25% to ensure enough results survive dedup/filtering
        let matches =
            self.search_parallel_filtered(pattern, (limit * 5 / 4).max(limit + 1), file_filter)?;

        // 2A: Single HashMap holding stats + snippets. Arc<Path> key =
        // cheap clone (atomic increment) instead of PathBuf heap alloc.
        // Each entry: (match_count, max_line_number, top_3_snippets)
        let mut file_agg: HashMap<Arc<Path>, (usize, u64, Vec<GrepMatch>)> = HashMap::new();

        for m in matches {
            let entry = file_agg
                .entry(Arc::clone(&m.path))
                .or_insert_with(|| (0, 0, Vec::with_capacity(3)));
            entry.0 += 1;
            entry.1 = entry.1.max(m.line_number);
            if entry.2.len() < 3 {
                entry.2.push(m);
            }
        }

        // Score blending match count and density (Q6):
        // density = matches / max_line_number rewards focused files
        let max_count = file_agg.values().map(|(c, _, _)| *c).max().unwrap_or(1) as f64;
        let max_density = file_agg
            .values()
            .map(|(count, max_line, _)| {
                if *max_line > 0 {
                    *count as f64 / *max_line as f64
                } else {
                    0.0
                }
            })
            .fold(0.0f64, f64::max)
            .max(f64::EPSILON);

        // Split into scored results + file_matches in one pass
        let mut results: Vec<(PathBuf, Score)> = Vec::with_capacity(file_agg.len());
        let mut file_matches: HashMap<Arc<Path>, Vec<GrepMatch>> =
            HashMap::with_capacity(file_agg.len().min(limit));

        for (path, (count, max_line, snippets)) in file_agg {
            let norm_count = (count as f64).ln_1p() / max_count.ln_1p();
            let density = if max_line > 0 {
                (count as f64 / max_line as f64) / max_density
            } else {
                0.0
            };
            let score = Score::new(0.6 * norm_count + 0.4 * density);

            // Temporarily store all snippets; trimmed after truncation (1F)
            if !snippets.is_empty() {
                file_matches.insert(Arc::clone(&path), snippets);
            }
            results.push((path.to_path_buf(), score));
        }

        // Sort by score descending (1E: sort_unstable avoids temp allocation)
        results.sort_unstable_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        results.truncate(limit);

        // Trim file_matches to only paths in the truncated results (1F)
        if file_matches.len() > results.len() {
            let kept_paths: HashSet<&Path> = results.iter().map(|(p, _)| p.as_path()).collect();
            file_matches.retain(|k, _| kept_paths.contains(k.as_ref()));
        }

        Ok((results, file_matches))
    }

    /// Gets the root directory.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn setup_test_dir() -> TempDir {
        let dir = TempDir::new().unwrap();
        fs::write(
            dir.path().join("test.rs"),
            "fn main() {\n    println!(\"hello\");\n}\n",
        )
        .unwrap();
        fs::write(
            dir.path().join("lib.rs"),
            "pub fn greet() {\n    println!(\"greeting\");\n}\n",
        )
        .unwrap();
        dir
    }

    #[test]
    fn test_grep_basic() {
        let dir = setup_test_dir();
        let service = GrepService::new(dir.path().to_path_buf()).unwrap();
        let matches = service.search_parallel("println", 100).unwrap();
        assert_eq!(matches.len(), 2);
    }

    #[test]
    fn test_grep_limit() {
        let dir = setup_test_dir();
        let service = GrepService::new(dir.path().to_path_buf()).unwrap();
        let matches = service.search_parallel("println", 1).unwrap();
        assert_eq!(matches.len(), 1);
    }

    #[test]
    fn test_redos_pattern_rejected() {
        let dir = setup_test_dir();
        let service = GrepService::new(dir.path().to_path_buf()).unwrap();

        // These patterns should be rejected due to ReDoS risk
        let result = service.search_parallel("(a+)+", 10);
        assert!(result.is_err());

        let result = service.search_parallel("(.*)*", 10);
        assert!(result.is_err());

        let result = service.search_parallel("(.+)+", 10);
        assert!(result.is_err());
    }

    #[test]
    fn test_safe_patterns_accepted() {
        let dir = setup_test_dir();
        let service = GrepService::new(dir.path().to_path_buf()).unwrap();

        // These patterns should be accepted
        assert!(service.search_parallel("fn\\s+\\w+", 10).is_ok());
        assert!(service.search_parallel("hello.*world", 10).is_ok());
        assert!(service.search_parallel("[a-z]+", 10).is_ok());
    }
}
