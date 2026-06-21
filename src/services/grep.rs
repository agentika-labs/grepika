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
use grep_regex::{RegexMatcher, RegexMatcherBuilder};
use grep_searcher::sinks::UTF8;
use grep_searcher::Searcher;
use ignore::{WalkBuilder, WalkState};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

/// Scored files with their matching line snippets.
pub type GrepSearchResult = (Vec<(PathBuf, Score)>, HashMap<Arc<Path>, Vec<GrepMatch>>);

/// Search results only need one proof snippet per file; callers can use
/// `context` for deeper reading.
const MAX_SNIPPETS_PER_FILE: usize = 1;

/// File-level grep ranking needs enough match counts to distinguish dense files
/// without letting one dense file consume the whole result budget.
const MAX_MATCHES_PER_FILE_FOR_RANKING: usize = 64;

/// Match found by grep.
#[derive(Debug, Clone)]
pub struct GrepMatch {
    pub path: Arc<Path>,
    pub line_number: u64,
    pub line_content: String,
    pub match_start: usize,
    pub match_end: usize,
}

#[derive(Debug, Default)]
struct GrepFileStats {
    count: usize,
    max_line_number: u64,
    snippets: Vec<GrepMatch>,
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

        let matcher = self.line_matcher(pattern)?;

        let max_matches = self.effective_max_matches(limit);

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

                let path = entry.path();

                if security::is_sensitive_file(path).is_some() {
                    return WalkState::Continue;
                }

                // Trigram pre-filter: skip files not in the filter set
                if let Some(filter) = file_filter {
                    if !path_matches_filter(path, filter) {
                        return WalkState::Continue;
                    }
                }

                // Max files searched limit. Apply after the optional filter so
                // skipped files do not consume the filtered search budget.
                if max_files > 0 && fc.fetch_add(1, Ordering::Relaxed) >= max_files {
                    return WalkState::Quit;
                }

                let local_match_budget = max_matches.saturating_sub(mc.load(Ordering::Relaxed));
                if local_match_budget == 0 {
                    return WalkState::Quit;
                }

                // Search with per-thread Searcher (reused across files on this thread)
                let arc_path: Arc<Path> = Arc::from(path);
                let mut file_matches = Vec::new();

                let search_ok = searcher
                    .search_path(
                        &*matcher,
                        path,
                        UTF8(|line_number, line| {
                            if file_matches.len() >= local_match_budget {
                                return Ok(false);
                            }
                            if let Ok(Some(m)) = matcher.find(line.as_bytes()) {
                                file_matches.push(GrepMatch {
                                    path: Arc::clone(&arc_path),
                                    line_number,
                                    line_content: line.trim_end().to_string(),
                                    match_start: m.start(),
                                    match_end: m.end(),
                                });
                                if file_matches.len() >= local_match_budget {
                                    return Ok(false);
                                }
                            }
                            Ok(true)
                        }),
                    )
                    .is_ok();

                if search_ok && !file_matches.is_empty() {
                    let file_match_count = file_matches.len();
                    let previous = mc.fetch_add(file_match_count, Ordering::Relaxed);
                    let remaining = max_matches.saturating_sub(previous);
                    if remaining > 0 {
                        file_matches.truncate(remaining);
                        if let Ok(mut r) = res.lock() {
                            r.extend(file_matches);
                        }
                    }
                    if previous.saturating_add(file_match_count) >= max_matches {
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
    /// compact proof snippets per file for result output.
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
        let match_limit = ranked_match_limit(limit);
        let file_agg = self.search_file_stats_filtered(pattern, match_limit, file_filter)?;
        Ok(score_file_stats(file_agg, limit))
    }

    /// Searches a known candidate set directly instead of walking the full tree.
    ///
    /// This is useful after an index has already narrowed the search to a small
    /// set of paths. Candidate paths are still constrained to the service root
    /// and must be regular files.
    pub fn search_files_with_matches_candidates(
        &self,
        pattern: &str,
        limit: usize,
        candidates: &[Arc<Path>],
    ) -> Result<GrepSearchResult, SearchError> {
        let match_limit = ranked_match_limit(limit);
        let file_agg = self.search_file_stats_candidates(pattern, match_limit, candidates)?;
        Ok(score_file_stats(file_agg, limit))
    }

    fn effective_max_matches(&self, limit: usize) -> usize {
        let max_matches = if limit > 0 {
            limit
        } else {
            self.config.max_matches
        };
        if max_matches == 0 {
            usize::MAX
        } else {
            max_matches
        }
    }

    fn search_file_stats_filtered(
        &self,
        pattern: &str,
        limit: usize,
        file_filter: Option<&HashSet<Arc<Path>>>,
    ) -> Result<HashMap<Arc<Path>, GrepFileStats>, SearchError> {
        security::validate_regex_pattern(pattern)
            .map_err(|e| SearchError::InvalidPattern(e.to_string()))?;

        let matcher = self.line_matcher(pattern)?;

        let max_ranked_files = self.effective_max_matches(limit);
        let per_file_match_limit = per_file_ranking_match_limit(max_ranked_files);

        let matched_file_count = Arc::new(AtomicUsize::new(0));
        let file_count = Arc::new(AtomicUsize::new(0));
        let results: Arc<Mutex<HashMap<Arc<Path>, GrepFileStats>>> =
            Arc::new(Mutex::new(HashMap::new()));
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
            let mut searcher = Searcher::new();
            let matcher = Arc::clone(&matcher);
            let matched_files = Arc::clone(&matched_file_count);
            let fc = Arc::clone(&file_count);
            let res = Arc::clone(&results);

            Box::new(move |entry| {
                if matched_files.load(Ordering::Relaxed) >= max_ranked_files {
                    return WalkState::Quit;
                }

                let entry = match entry {
                    Ok(e) => e,
                    Err(_) => return WalkState::Continue,
                };

                if !entry.file_type().is_some_and(|ft| ft.is_file()) {
                    return WalkState::Continue;
                }

                let path = entry.path();

                if security::is_sensitive_file(path).is_some() {
                    return WalkState::Continue;
                }

                if let Some(filter) = file_filter {
                    if !path_matches_filter(path, filter) {
                        return WalkState::Continue;
                    }
                }

                if max_files > 0 && fc.fetch_add(1, Ordering::Relaxed) >= max_files {
                    return WalkState::Quit;
                }

                if per_file_match_limit == 0 {
                    return WalkState::Quit;
                }

                let arc_path: Arc<Path> = Arc::from(path);
                if let Some(stats) = collect_file_stats(
                    &mut searcher,
                    &matcher,
                    path,
                    &arc_path,
                    per_file_match_limit,
                ) {
                    let previous = matched_files.fetch_add(1, Ordering::Relaxed);
                    if previous < max_ranked_files {
                        if let Ok(mut r) = res.lock() {
                            r.insert(arc_path, stats);
                        }
                    }
                    if previous.saturating_add(1) >= max_ranked_files {
                        return WalkState::Quit;
                    }
                }

                WalkState::Continue
            })
        });

        let results = Arc::try_unwrap(results)
            .map_err(|_| {
                SearchError::Grep(GrepError::Walk("walker threads still hold Arc".into()))
            })?
            .into_inner()
            .unwrap_or_else(|poisoned| {
                tracing::warn!("grep results mutex was poisoned, recovering partial results");
                poisoned.into_inner()
            });

        Ok(results)
    }

    fn search_file_stats_candidates(
        &self,
        pattern: &str,
        limit: usize,
        candidates: &[Arc<Path>],
    ) -> Result<HashMap<Arc<Path>, GrepFileStats>, SearchError> {
        security::validate_regex_pattern(pattern)
            .map_err(|e| SearchError::InvalidPattern(e.to_string()))?;

        let matcher = self.line_matcher(pattern)?;

        let max_ranked_files = self.effective_max_matches(limit);
        let per_file_match_limit = per_file_ranking_match_limit(max_ranked_files);
        let mut matched_file_count = 0usize;
        let mut file_count = 0usize;
        let mut searcher = Searcher::new();
        let mut results = HashMap::with_capacity(candidates.len().min(limit));

        for candidate in candidates {
            if matched_file_count >= max_ranked_files {
                break;
            }

            let Some(search_path) = self.candidate_search_path(candidate.as_ref()) else {
                continue;
            };

            if self.config.max_files > 0 && file_count >= self.config.max_files {
                break;
            }
            file_count += 1;

            if per_file_match_limit == 0 {
                break;
            }

            if let Some(stats) = collect_file_stats(
                &mut searcher,
                &matcher,
                &search_path,
                candidate,
                per_file_match_limit,
            ) {
                matched_file_count = matched_file_count.saturating_add(1);
                results.insert(Arc::clone(candidate), stats);
            }
        }

        Ok(results)
    }

    fn candidate_search_path(&self, path: &Path) -> Option<PathBuf> {
        if path
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
        {
            return None;
        }

        let root = self.root.canonicalize().ok()?;
        let search_path = if path.is_absolute() || path.starts_with(&self.root) {
            path.to_path_buf()
        } else {
            self.root.join(path)
        };

        let metadata = if self.config.follow_symlinks {
            fs::metadata(&search_path).ok()?
        } else {
            let metadata = fs::symlink_metadata(&search_path).ok()?;
            if metadata.file_type().is_symlink() {
                return None;
            }
            metadata
        };

        let canonical_search_path = search_path.canonicalize().ok()?;
        if !canonical_search_path.starts_with(root) {
            return None;
        }

        metadata.is_file().then_some(search_path)
    }

    fn line_matcher(&self, pattern: &str) -> Result<RegexMatcher, SearchError> {
        let mut builder = RegexMatcherBuilder::new();
        builder
            .line_terminator(Some(b'\n'))
            .case_insensitive(self.config.case_insensitive);
        builder
            .build(pattern)
            .map_err(|e| SearchError::InvalidPattern(e.to_string()))
    }

    /// Gets the root directory.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }
}

/// Returns true when `path` matches a canonicalized trigram prefilter entry.
fn path_matches_filter(path: &Path, filter: &HashSet<Arc<Path>>) -> bool {
    if filter.contains(path) {
        return true;
    }
    let Ok(canon) = dunce::canonicalize(path) else {
        return false;
    };
    filter.contains(canon.as_path())
}

#[inline]
fn ranked_match_limit(limit: usize) -> usize {
    limit
        .saturating_mul(5)
        .saturating_div(4)
        .max(limit.saturating_add(1))
}

#[inline]
fn per_file_ranking_match_limit(max_ranked_files: usize) -> usize {
    max_ranked_files.clamp(1, MAX_MATCHES_PER_FILE_FOR_RANKING)
}

#[inline]
fn collect_file_stats(
    searcher: &mut Searcher,
    matcher: &RegexMatcher,
    path: &Path,
    arc_path: &Arc<Path>,
    max_matches: usize,
) -> Option<GrepFileStats> {
    if max_matches == 0 {
        return None;
    }

    let mut stats = GrepFileStats {
        count: 0,
        max_line_number: 0,
        snippets: Vec::with_capacity(MAX_SNIPPETS_PER_FILE),
    };

    let search_ok = searcher
        .search_path(
            matcher,
            path,
            UTF8(|line_number, line| {
                if stats.count >= max_matches {
                    return Ok(false);
                }
                if let Ok(Some(m)) = matcher.find(line.as_bytes()) {
                    stats.count += 1;
                    stats.max_line_number = stats.max_line_number.max(line_number);
                    if stats.snippets.len() < MAX_SNIPPETS_PER_FILE {
                        stats.snippets.push(GrepMatch {
                            path: Arc::clone(arc_path),
                            line_number,
                            line_content: line.trim_end().to_string(),
                            match_start: m.start(),
                            match_end: m.end(),
                        });
                    }
                    if stats.count >= max_matches {
                        return Ok(false);
                    }
                }
                Ok(true)
            }),
        )
        .is_ok();

    (search_ok && stats.count > 0).then_some(stats)
}

fn score_file_stats(file_agg: HashMap<Arc<Path>, GrepFileStats>, limit: usize) -> GrepSearchResult {
    // Score blending match count and density (Q6):
    // density = matches / max_line_number rewards focused files
    let max_count = file_agg
        .values()
        .map(|stats| stats.count)
        .max()
        .unwrap_or(1) as f64;
    let max_density = file_agg
        .values()
        .map(|stats| {
            if stats.max_line_number > 0 {
                stats.count as f64 / stats.max_line_number as f64
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

    for (path, stats) in file_agg {
        let norm_count = (stats.count as f64).ln_1p() / max_count.ln_1p();
        let density = if stats.max_line_number > 0 {
            (stats.count as f64 / stats.max_line_number as f64) / max_density
        } else {
            0.0
        };
        let score = Score::new(0.6 * norm_count + 0.4 * density);

        // Temporarily store all snippets; trimmed after truncation (1F)
        if !stats.snippets.is_empty() {
            file_matches.insert(Arc::clone(&path), stats.snippets);
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

    (results, file_matches)
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
    fn test_ranked_grep_keeps_compact_snippets() {
        let dir = TempDir::new().unwrap();
        let dense: String = (0..20)
            .map(|i| format!("let needle_{i} = call();\n"))
            .collect();
        fs::write(dir.path().join("dense.rs"), dense).unwrap();

        let service = GrepService::new(dir.path().to_path_buf()).unwrap();
        let (results, matches_by_file) = service.search_files_with_matches("needle", 10).unwrap();

        assert_eq!(results.len(), 1);
        assert!(matches_by_file
            .values()
            .all(|matches| { !matches.is_empty() && matches.len() <= MAX_SNIPPETS_PER_FILE }));
    }

    #[test]
    fn test_raw_grep_preserves_multiple_matches() {
        let dir = TempDir::new().unwrap();
        fs::write(
            dir.path().join("many.rs"),
            "needle one\nneedle two\nneedle three\n",
        )
        .unwrap();

        let service = GrepService::new(dir.path().to_path_buf()).unwrap();
        let matches = service.search_parallel("needle", 10).unwrap();

        assert_eq!(matches.len(), 3);
    }

    #[test]
    fn test_raw_grep_dense_file_respects_limit() {
        let dir = TempDir::new().unwrap();
        let dense = dir.path().join("dense.rs");
        let dense_content: String = (0..5000).map(|i| format!("needle dense_{i}\n")).collect();
        fs::write(&dense, dense_content).unwrap();

        let config = GrepConfig {
            max_threads: 1,
            thread_cap: 1,
            ..Default::default()
        };
        let service = GrepService::with_config(dir.path().to_path_buf(), config).unwrap();
        let matches = service.search_parallel("needle", 10).unwrap();

        assert_eq!(matches.len(), 10);
        assert!(matches.iter().all(|m| m.path.as_ref() == dense.as_path()));
        assert_eq!(matches[0].line_number, 1);
        assert_eq!(matches[9].line_number, 10);
    }

    #[test]
    fn test_zero_max_matches_is_unlimited() {
        let dir = setup_test_dir();
        let config = GrepConfig {
            max_matches: 0,
            max_threads: 1,
            thread_cap: 1,
            ..Default::default()
        };
        let service = GrepService::with_config(dir.path().to_path_buf(), config).unwrap();

        let matches = service.search_parallel("println", 0).unwrap();

        assert_eq!(matches.len(), 2);
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

    #[test]
    fn test_case_insensitive_config_matches_mixed_case() {
        let dir = TempDir::new().unwrap();
        fs::write(
            dir.path().join("auth.rs"),
            "fn auth() { let token = PasswordResetToken::new(); }\n",
        )
        .unwrap();

        let sensitive = GrepService::new(dir.path().to_path_buf()).unwrap();
        assert!(sensitive
            .search_parallel("passwordresettoken", 10)
            .unwrap()
            .is_empty());

        let insensitive = GrepService::with_config(
            dir.path().to_path_buf(),
            GrepConfig {
                case_insensitive: true,
                max_threads: 1,
                thread_cap: 1,
                ..Default::default()
            },
        )
        .unwrap();
        let matches = insensitive
            .search_parallel("passwordresettoken", 10)
            .unwrap();

        assert_eq!(matches.len(), 1);
        assert_eq!(
            matches[0].path.as_ref(),
            dir.path().join("auth.rs").as_path()
        );
    }

    #[test]
    fn test_file_filter_applies_before_max_files_limit() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("aaa_skip.rs"), "fn skip() {}\n").unwrap();
        let target = dir.path().join("zzz_target.rs");
        fs::write(&target, "fn target() { let needle = true; }\n").unwrap();

        let config = GrepConfig {
            max_files: 1,
            max_threads: 1,
            thread_cap: 1,
            ..Default::default()
        };
        let service = GrepService::with_config(dir.path().to_path_buf(), config).unwrap();
        let filter = HashSet::from([Arc::<Path>::from(target.as_path())]);

        let matches = service
            .search_parallel_filtered("needle", 10, Some(&filter))
            .unwrap();

        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].path.as_ref(), target.as_path());
    }

    #[test]
    fn test_candidate_search_only_reads_candidates() {
        let dir = TempDir::new().unwrap();
        let target = dir.path().join("target.rs");
        let skipped = dir.path().join("skipped.rs");
        fs::write(&target, "fn target() { let needle = true; }\n").unwrap();
        fs::write(&skipped, "fn skipped() { let needle = true; }\n").unwrap();

        let service = GrepService::new(dir.path().to_path_buf()).unwrap();
        let candidates = [Arc::<Path>::from(target.as_path())];

        let (results, matches_by_file) = service
            .search_files_with_matches_candidates("needle", 10, &candidates)
            .unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, target);
        assert!(matches_by_file.contains_key(target.as_path()));
        assert!(!matches_by_file.contains_key(skipped.as_path()));
    }

    #[test]
    fn test_candidate_file_ranking_budget_is_per_file() {
        let dir = TempDir::new().unwrap();
        let dense = dir.path().join("dense.rs");
        let sparse_a = dir.path().join("sparse_a.rs");
        let sparse_b = dir.path().join("sparse_b.rs");
        let skipped = dir.path().join("skipped.rs");
        let dense_content: String = (0..100).map(|i| format!("needle dense_{i}\n")).collect();
        fs::write(&dense, dense_content).unwrap();
        fs::write(&sparse_a, "needle sparse_a\n").unwrap();
        fs::write(&sparse_b, "needle sparse_b\n").unwrap();
        fs::write(&skipped, "needle skipped\n").unwrap();

        let service = GrepService::new(dir.path().to_path_buf()).unwrap();
        let candidates = [
            Arc::<Path>::from(dense.as_path()),
            Arc::<Path>::from(sparse_a.as_path()),
            Arc::<Path>::from(sparse_b.as_path()),
        ];

        let (results, matches_by_file) = service
            .search_files_with_matches_candidates("needle", 3, &candidates)
            .unwrap();

        assert_eq!(results.len(), 3);
        assert!(results.iter().any(|(path, _)| path == &dense));
        assert!(results.iter().any(|(path, _)| path == &sparse_a));
        assert!(results.iter().any(|(path, _)| path == &sparse_b));
        assert!(!results.iter().any(|(path, _)| path == &skipped));
        assert!(matches_by_file
            .values()
            .all(|matches| !matches.is_empty() && matches.len() <= MAX_SNIPPETS_PER_FILE));
    }

    #[test]
    fn test_candidate_search_rejects_paths_outside_root() {
        let dir = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        let outside_file = outside.path().join("outside.rs");
        fs::write(&outside_file, "fn outside() { let needle = true; }\n").unwrap();

        let service = GrepService::new(dir.path().to_path_buf()).unwrap();
        let candidates = [Arc::<Path>::from(outside_file.as_path())];

        let (results, matches_by_file) = service
            .search_files_with_matches_candidates("needle", 10, &candidates)
            .unwrap();

        assert!(results.is_empty());
        assert!(matches_by_file.is_empty());
    }

    #[test]
    fn test_candidate_search_rejects_absolute_outside_path_with_relative_root() {
        let outside = TempDir::new().unwrap();
        let outside_file = outside.path().join("outside.rs");
        fs::write(&outside_file, "fn outside() { let needle = true; }\n").unwrap();

        let service = GrepService::new(PathBuf::from("src")).unwrap();
        let candidates = [Arc::<Path>::from(outside_file.as_path())];

        let (results, matches_by_file) = service
            .search_files_with_matches_candidates("needle", 10, &candidates)
            .unwrap();

        assert!(results.is_empty());
        assert!(matches_by_file.is_empty());
    }

    #[test]
    fn test_candidate_search_accepts_canonical_absolute_paths() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("src/file_31.rs");
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(
            &file,
            "pub fn zero_repeat() { let without_zero_marker = true; }\n",
        )
        .unwrap();
        let canonical = dunce::canonicalize(&file).unwrap();

        let service = GrepService::new(dir.path().to_path_buf()).unwrap();
        let candidates = [Arc::<Path>::from(canonical.as_path())];
        let (results, matches_by_file) = service
            .search_files_with_matches_candidates(r"(token)*without_zero_marker", 10, &candidates)
            .unwrap();

        assert_eq!(results.len(), 1, "canonical candidate path should match");
        assert!(matches_by_file.contains_key(canonical.as_path()));
    }

    #[test]
    fn test_candidate_search_rejects_parent_dir_components() {
        let dir = TempDir::new().unwrap();
        let service = GrepService::new(dir.path().to_path_buf()).unwrap();
        let candidates = [Arc::<Path>::from(Path::new("../outside.rs"))];

        let (results, matches_by_file) = service
            .search_files_with_matches_candidates("needle", 10, &candidates)
            .unwrap();

        assert!(results.is_empty());
        assert!(matches_by_file.is_empty());
    }

    #[test]
    fn test_zero_max_files_is_unlimited() {
        let dir = setup_test_dir();
        let config = GrepConfig {
            max_files: 0,
            max_threads: 1,
            thread_cap: 1,
            ..Default::default()
        };
        let service = GrepService::with_config(dir.path().to_path_buf(), config).unwrap();

        let matches = service.search_parallel("println", 10).unwrap();

        assert_eq!(matches.len(), 2);
    }
}
