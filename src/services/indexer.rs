//! Incremental file indexer with change detection.
//!
//! Uses xxHash (xxh3_64) for fast file change detection.
//! xxHash is ~30x faster than SHA256 while providing sufficient
//! collision resistance for content hashing.

use crate::db::{Database, FileData};
use crate::error::{IndexError, ServerError};
use crate::security;
use crate::services::TrigramIndex;
use crate::types::FileId;
use ignore::WalkBuilder;
use rayon::prelude::*;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use xxhash_rust::xxh3::xxh3_64;

/// Batch size for database upserts.
/// Larger batches reduce transaction overhead but increase memory usage.
const BATCH_SIZE: usize = 500;

/// Progress callback type.
pub type ProgressCallback = Box<dyn Fn(IndexProgress) + Send + Sync>;

/// Indexing progress information.
#[derive(Debug, Clone)]
pub struct IndexProgress {
    pub files_processed: usize,
    pub files_total: usize,
    pub files_indexed: usize,
    pub files_unchanged: usize,
    pub files_deleted: usize,
    pub current_file: Option<PathBuf>,
}

/// Configuration for indexing.
#[derive(Debug, Clone)]
pub struct IndexConfig {
    /// Include hidden files
    pub include_hidden: bool,
    /// Follow symlinks
    pub follow_symlinks: bool,
    /// Maximum file size to index (bytes)
    pub max_file_size: u64,
    /// File extensions to index (empty = all text files)
    pub extensions: Vec<String>,
}

impl Default for IndexConfig {
    fn default() -> Self {
        Self {
            include_hidden: false,
            follow_symlinks: false,
            max_file_size: 1024 * 1024, // 1MB
            extensions: vec![
                "rs",
                "py",
                "js",
                "ts",
                "tsx",
                "jsx",
                "go",
                "java",
                "c",
                "cpp",
                "h",
                "hpp",
                "rb",
                "php",
                "swift",
                "kt",
                "scala",
                "cs",
                "fs",
                "ml",
                "hs",
                "clj",
                "ex",
                "exs",
                "erl",
                "lua",
                "vim",
                "sh",
                "bash",
                "zsh",
                "fish",
                "ps1",
                "bat",
                "md",
                "txt",
                "json",
                "yaml",
                "yml",
                "toml",
                "xml",
                "html",
                "css",
                "scss",
                "sql",
                "graphql",
                "proto",
                "dockerfile",
                "makefile",
                "cmake",
            ]
            .into_iter()
            .map(String::from)
            .collect(),
        }
    }
}

/// Incremental file indexer.
pub struct Indexer {
    db: Arc<Database>,
    trigram: Arc<RwLock<TrigramIndex>>,
    root: PathBuf,
    config: IndexConfig,
    /// Pre-built HashSet of lowercased extensions for O(1) lookup (P7)
    extension_set: HashSet<String>,
}

impl Indexer {
    /// Creates a new indexer.
    pub fn new(db: Arc<Database>, trigram: Arc<RwLock<TrigramIndex>>, root: PathBuf) -> Self {
        let config = IndexConfig::default();
        let extension_set = build_extension_set(&config.extensions);
        Self {
            db,
            trigram,
            root,
            config,
            extension_set,
        }
    }

    /// Creates an indexer with custom configuration.
    pub fn with_config(
        db: Arc<Database>,
        trigram: Arc<RwLock<TrigramIndex>>,
        root: PathBuf,
        config: IndexConfig,
    ) -> Self {
        let extension_set = build_extension_set(&config.extensions);
        Self {
            db,
            trigram,
            root,
            config,
            extension_set,
        }
    }

    /// Performs incremental indexing using two-phase parallel processing.
    ///
    /// **Phase 1 (Parallel):** Read files and compute hashes using rayon.
    /// This is CPU-bound work that benefits from parallelization.
    ///
    /// **Phase 2 (Sequential):** Batch insert into database and update trigrams.
    /// This is I/O-bound work where batching is more effective than parallelism.
    ///
    /// # Errors
    ///
    /// Returns `ServerError::Database` if database operations fail.
    /// Returns `ServerError::Index` if trigram indexing fails.
    pub fn index(
        &self,
        progress: Option<ProgressCallback>,
        force: bool,
    ) -> Result<IndexProgress, ServerError> {
        // Pre-load all existing hashes into memory for O(1) lookups
        // When force=true, use empty map so all files appear changed
        let existing_hashes: HashMap<String, u64> = if force {
            HashMap::new()
        } else {
            self.db.get_all_hashes()?
        };
        let existing_paths: HashSet<String> = existing_hashes.keys().cloned().collect();

        // Collect files to process
        let files: Vec<PathBuf> = self.collect_files()?;
        let total = files.len();

        // ========================================
        // PHASE 1: Parallel file reading + hashing
        // ========================================
        // This is embarrassingly parallel - no shared mutable state
        let file_data: Vec<FileData> = files
            .par_iter()
            .filter_map(|path| {
                // Read file content
                let content = fs::read_to_string(path).ok()?;

                // Compute hash
                let hash = compute_hash(&content);
                let path_str = path.to_string_lossy().to_string();

                // Check if file needs indexing (O(1) HashMap lookup)
                if existing_hashes.get(&path_str) == Some(&hash) {
                    return None; // Skip unchanged files
                }

                Some(FileData {
                    path: path_str,
                    content,
                    hash,
                })
            })
            .collect();

        // Collect all seen paths (including unchanged ones)
        let seen_paths: HashSet<String> = files
            .par_iter()
            .map(|p| p.to_string_lossy().to_string())
            .collect();

        // Calculate unchanged count
        let files_unchanged = total - file_data.len();

        // ========================================
        // PHASE 2: Sequential DB writes + trigrams
        // ========================================
        // Batching is more efficient than parallel writes due to
        // transaction overhead and lock contention

        let mut state = IndexProgress {
            files_processed: 0,
            files_total: total,
            files_indexed: 0,
            files_unchanged,
            files_deleted: 0,
            current_file: None,
        };

        // Acquire trigram write lock once for the entire Phase 2 batch loop.
        // Avoids per-file lock acquire/release overhead (atomic CAS + potential syscall).
        {
            let mut trigram_guard = self.trigram.write().unwrap_or_else(|e| e.into_inner());

            for batch in file_data.chunks(BATCH_SIZE) {
                // Report progress at batch level
                if let Some(ref cb) = progress {
                    state.current_file = batch.first().map(|f| PathBuf::from(&f.path));
                    cb(state.clone());
                }

                // Batch upsert returns FileIds in same order as input
                let file_ids = self.db.upsert_files_batch(batch)?;

                // Update trigram index for each file (guard held, no per-file lock)
                for (data, file_id) in batch.iter().zip(file_ids) {
                    trigram_guard.add_file(file_id, &data.content);
                }

                state.files_indexed += batch.len();
                state.files_processed += batch.len();
            }
        } // Drop write guard before save_trigrams (which takes a read lock)

        // Remove deleted files
        for path in existing_paths.difference(&seen_paths) {
            if self.db.delete_file(path)? {
                state.files_deleted += 1;
            }
        }

        // Persist trigram index to database if any files were indexed
        if state.files_indexed > 0 || state.files_deleted > 0 {
            let entries = {
                let trigram = self.trigram.read().unwrap_or_else(|e| e.into_inner());
                trigram.to_db_entries()
            }; // RwLock read guard dropped before DB write
            self.db.save_trigrams(&entries)?;
        }

        state.current_file = None;
        state.files_processed = total;

        if let Some(ref cb) = progress {
            cb(state.clone());
        }

        Ok(state)
    }

    /// Indexes a single file.
    ///
    /// # Errors
    ///
    /// Returns `ServerError::Index` if the file cannot be read.
    /// Returns `ServerError::Database` if database upsert fails.
    pub fn index_file(&self, path: &Path) -> Result<FileId, ServerError> {
        let content = fs::read_to_string(path).map_err(|e| IndexError::FileIndex {
            path: path.to_path_buf(),
            reason: e.to_string(),
        })?;

        let hash = compute_hash(&content);
        let path_str = path.to_string_lossy().to_string();

        let file_id = self.db.upsert_file(&path_str, &content, hash)?;
        self.index_trigrams(file_id, &content);

        Ok(file_id)
    }

    /// Collects files to index.
    fn collect_files(&self) -> Result<Vec<PathBuf>, ServerError> {
        let mut files = Vec::new();

        let walker = WalkBuilder::new(&self.root)
            .hidden(!self.config.include_hidden)
            .follow_links(self.config.follow_symlinks)
            .git_ignore(true)
            .git_global(true)
            .git_exclude(true)
            .build();

        for entry in walker.filter_map(Result::ok) {
            if !entry.file_type().is_some_and(|ft| ft.is_file()) {
                continue;
            }

            let path = entry.path();

            // Check extension using pre-built HashSet (P7: O(1) vs O(n))
            // Uses stack-buffer ASCII lowercase to avoid heap allocation per file
            if !self.extension_set.is_empty() {
                let ext_str = path.extension().and_then(|e| e.to_str()).unwrap_or("");

                let ext_matched = match ascii_lower_check(ext_str, &self.extension_set) {
                    Some(matched) => matched,
                    None => {
                        // Fallback for non-ASCII or very long extensions
                        self.extension_set.contains(&ext_str.to_lowercase())
                    }
                };

                if !ext_matched {
                    // Check for extensionless files like Makefile, Dockerfile
                    let filename_str = path.file_name().and_then(|n| n.to_str()).unwrap_or("");

                    let filename_matched =
                        match ascii_lower_check(filename_str, &self.extension_set) {
                            Some(matched) => matched,
                            None => self.extension_set.contains(&filename_str.to_lowercase()),
                        };

                    if !filename_matched {
                        continue;
                    }
                }
            }

            // Check file size
            if let Ok(metadata) = fs::metadata(path) {
                if metadata.len() > self.config.max_file_size {
                    continue;
                }
            }

            // Skip sensitive files (defense-in-depth: prevents .env, credentials, etc. from entering the index)
            if security::is_sensitive_file(path).is_some() {
                continue;
            }

            files.push(path.to_path_buf());
        }

        Ok(files)
    }

    /// Updates trigram index for a file (P8: inline to avoid content clone).
    fn index_trigrams(&self, file_id: FileId, content: &str) {
        let mut trigram = self.trigram.write().unwrap_or_else(|e| e.into_inner());
        trigram.add_file(file_id, content);
    }

    /// Gets indexing statistics.
    ///
    /// # Errors
    ///
    /// Returns `ServerError::Database` if file count query fails.
    ///
    /// # Panics
    ///
    /// Panics if the trigram `RwLock` is poisoned (another thread panicked while holding it).
    pub fn stats(&self) -> Result<IndexStats, ServerError> {
        let file_count = self.db.file_count()?;
        let trigram_count = {
            let trigram = self.trigram.read().unwrap_or_else(|e| e.into_inner());
            trigram.trigram_count()
        };

        Ok(IndexStats {
            file_count,
            trigram_count,
        })
    }
}

/// Index statistics.
#[derive(Debug, Clone)]
pub struct IndexStats {
    pub file_count: u64,
    pub trigram_count: usize,
}

/// Builds a HashSet of lowercased extensions for O(1) lookup (P7).
fn build_extension_set(extensions: &[String]) -> HashSet<String> {
    extensions.iter().map(|e| e.to_lowercase()).collect()
}

/// Checks if `s`, lowercased, is in `set` — using a stack buffer to avoid heap allocation.
///
/// Returns `Some(true/false)` if the string is ASCII and fits in the 16-byte buffer.
/// Returns `None` if the string is non-ASCII or exceeds 16 bytes (caller should fall back).
fn ascii_lower_check(s: &str, set: &HashSet<String>) -> Option<bool> {
    let bytes = s.as_bytes();
    if bytes.len() > 16 || !s.is_ascii() {
        return None;
    }
    let mut buf = [0u8; 16];
    for (i, &b) in bytes.iter().enumerate() {
        buf[i] = b.to_ascii_lowercase();
    }
    // Safety: input is ASCII, to_ascii_lowercase preserves valid UTF-8
    let lowered = std::str::from_utf8(&buf[..bytes.len()]).ok()?;
    Some(set.contains(lowered))
}

/// Computes xxHash (xxh3_64) of content.
///
/// xxHash is ~30x faster than SHA256 while providing
/// sufficient collision resistance for file change detection.
#[inline]
fn compute_hash(content: &str) -> u64 {
    xxh3_64(content.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn setup_test_env() -> (TempDir, Arc<Database>, Arc<RwLock<TrigramIndex>>) {
        let dir = TempDir::new().unwrap();
        let db = Arc::new(Database::in_memory().unwrap());
        let trigram = Arc::new(RwLock::new(TrigramIndex::new()));

        // Create test files
        fs::write(dir.path().join("main.rs"), "fn main() {}").unwrap();
        fs::write(dir.path().join("lib.rs"), "pub fn greet() {}").unwrap();

        (dir, db, trigram)
    }

    #[test]
    fn test_indexer() {
        let (dir, db, trigram) = setup_test_env();
        let indexer = Indexer::new(db.clone(), trigram, dir.path().to_path_buf());

        let progress = indexer.index(None, false).unwrap();
        assert_eq!(progress.files_indexed, 2);
        assert_eq!(db.file_count().unwrap(), 2);
    }

    #[test]
    fn test_incremental_index() {
        let (dir, db, trigram) = setup_test_env();
        let indexer = Indexer::new(db.clone(), trigram, dir.path().to_path_buf());

        // First index
        let progress1 = indexer.index(None, false).unwrap();
        assert_eq!(progress1.files_indexed, 2);

        // Second index (no changes)
        let progress2 = indexer.index(None, false).unwrap();
        assert_eq!(progress2.files_indexed, 0);
        assert_eq!(progress2.files_unchanged, 2);

        // Modify a file
        fs::write(
            dir.path().join("main.rs"),
            "fn main() { println!(\"hi\"); }",
        )
        .unwrap();

        // Third index (one change)
        let progress3 = indexer.index(None, false).unwrap();
        assert_eq!(progress3.files_indexed, 1);
        assert_eq!(progress3.files_unchanged, 1);
    }

    #[test]
    fn test_hash_computation() {
        let hash1 = compute_hash("hello");
        let hash2 = compute_hash("hello");
        let hash3 = compute_hash("world");

        assert_eq!(hash1, hash2);
        assert_ne!(hash1, hash3);
    }
}
