//! Parallel grep service using ripgrep internals.
//!
//! Uses a dedicated rayon ThreadPool to avoid contention with
//! tokio's blocking thread pool.

use crate::error::{GrepError, SearchError};
use crate::types::Score;
use grep_matcher::Matcher;
use grep_regex::RegexMatcher;
use grep_searcher::sinks::UTF8;
use grep_searcher::Searcher;
use ignore::WalkBuilder;
use rayon::{ThreadPool, ThreadPoolBuilder};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

/// Match found by grep.
#[derive(Debug, Clone)]
pub struct GrepMatch {
    pub path: PathBuf,
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
    /// Maximum threads for parallel grep (0 = auto-detect, capped at 8)
    pub max_threads: usize,
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
        }
    }
}

/// Parallel grep service using ripgrep internals.
pub struct GrepService {
    /// Dedicated thread pool (avoids tokio contention)
    pool: ThreadPool,
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
    /// Returns `SearchError::Grep` if thread pool creation fails.
    pub fn new(root: PathBuf) -> Result<Self, SearchError> {
        Self::with_config(root, GrepConfig::default())
    }

    /// Creates a grep service with custom configuration.
    ///
    /// # Errors
    ///
    /// Returns `SearchError::Grep` if thread pool creation fails.
    pub fn with_config(root: PathBuf, config: GrepConfig) -> Result<Self, SearchError> {
        let num_threads = if config.max_threads > 0 {
            config.max_threads.min(8)
        } else {
            num_cpus::get().min(8)
        };
        let pool = ThreadPoolBuilder::new()
            .num_threads(num_threads)
            .thread_name(|i| format!("grep-worker-{i}"))
            .build()
            .map_err(|e| SearchError::Grep(GrepError::RegexBuild(e.to_string())))?;

        Ok(Self { pool, root, config })
    }

    /// Searches for pattern in files under root directory.
    ///
    /// # Errors
    ///
    /// Returns `SearchError::InvalidPattern` if the regex pattern is invalid.
    pub fn search_parallel(
        &self,
        pattern: &str,
        limit: usize,
    ) -> Result<Vec<GrepMatch>, SearchError> {
        let matcher = RegexMatcher::new_line_matcher(pattern)
            .map_err(|e| SearchError::InvalidPattern(e.to_string()))?;

        let matches = Arc::new(Mutex::new(Vec::new()));
        let match_count = Arc::new(AtomicUsize::new(0));
        let cancelled = Arc::new(AtomicBool::new(false));
        let max_matches = if limit > 0 {
            limit
        } else {
            self.config.max_matches
        };

        // Collect files first (respects .gitignore)
        let files: Vec<PathBuf> = WalkBuilder::new(&self.root)
            .hidden(!self.config.include_hidden)
            .follow_links(self.config.follow_symlinks)
            .git_ignore(true)
            .git_global(true)
            .git_exclude(true)
            .build()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_type().is_some_and(|ft| ft.is_file()))
            .map(|entry| entry.path().to_path_buf())
            .take(self.config.max_files)
            .collect();

        // Search files in parallel
        self.pool.install(|| {
            use rayon::prelude::*;

            files.par_iter().for_each(|path| {
                // Early termination check
                if cancelled.load(Ordering::Relaxed) {
                    return;
                }

                if let Ok(file_matches) = search_file(path, &matcher) {
                    // Lock poisoning indicates a thread panicked - treat as unrecoverable
                    let mut all_matches = matches.lock().unwrap_or_else(|e| e.into_inner());
                    for m in file_matches {
                        all_matches.push(m);
                        let count = match_count.fetch_add(1, Ordering::Relaxed);
                        if count + 1 >= max_matches {
                            cancelled.store(true, Ordering::Relaxed);
                            break;
                        }
                    }
                }
            });
        });

        // Arc::try_unwrap succeeds because this is the only remaining reference after pool.install
        // Lock poisoning recovery: extract data even if a thread panicked
        let mutex = Arc::try_unwrap(matches).unwrap_or_else(|arc| {
            // Fallback: clone the data if other references somehow exist
            let guard = arc.lock().unwrap_or_else(|e| e.into_inner());
            std::sync::Mutex::new(guard.clone())
        });
        let mut results = mutex.into_inner().unwrap_or_else(|e| e.into_inner());
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
        let matches = self.search_parallel(pattern, limit * 10)?;

        // Aggregate by file and compute scores
        let mut file_counts: HashMap<PathBuf, usize> = HashMap::new();
        for m in matches {
            *file_counts.entry(m.path).or_insert(0) += 1;
        }

        // Score based on match count (logarithmic scale)
        let max_count = file_counts.values().copied().max().unwrap_or(1) as f64;
        let mut results: Vec<_> = file_counts
            .into_iter()
            .map(|(path, count)| {
                let score = Score::new((count as f64).ln_1p() / max_count.ln_1p());
                (path, score)
            })
            .collect();

        // Sort by score descending
        results.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        results.truncate(limit);

        Ok(results)
    }

    /// Gets the root directory.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }
}

/// Searches a single file for matches.
fn search_file(path: &Path, matcher: &RegexMatcher) -> Result<Vec<GrepMatch>, GrepError> {
    let mut matches = Vec::new();
    let mut searcher = Searcher::new();

    searcher
        .search_path(
            matcher,
            path,
            UTF8(|line_number, line| {
                // Find match positions in line
                matcher
                    .find(line.as_bytes())
                    .map_err(|_| std::io::Error::other("matcher error"))?;

                if let Ok(Some(m)) = matcher.find(line.as_bytes()) {
                    matches.push(GrepMatch {
                        path: path.to_path_buf(),
                        line_number,
                        line_content: line.trim_end().to_string(),
                        match_start: m.start(),
                        match_end: m.end(),
                    });
                }
                Ok(true)
            }),
        )
        .map_err(|e| GrepError::FileRead {
            path: path.to_path_buf(),
            source: std::io::Error::other(e.to_string()),
        })?;

    Ok(matches)
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
}
