//! Incremental file indexer with change detection.
//!
//! Uses xxHash (xxh3_64) for fast file change detection.
//! xxHash is ~30x faster than SHA256 while providing sufficient
//! collision resistance for content hashing.

use crate::db::Database;
use crate::db::FileData;
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

        let files: Vec<PathBuf> = self.collect_files()?;
        let total = files.len();

        // Phase 1: parallel file reading + hashing
        let (file_data, seen_paths) = self.phase1_read_and_hash(&files, &existing_hashes);
        let files_unchanged = total - file_data.len();

        // Phase 2: sequential DB writes + trigrams + deletions
        let mut state = IndexProgress {
            files_processed: 0,
            files_total: total,
            files_indexed: 0,
            files_unchanged,
            files_deleted: 0,
            current_file: None,
        };

        // Get a dedicated connection with indexing pragmas applied.
        let indexing_conn = self.db.enter_indexing_mode()?;

        // Wrap indexing in a closure so exit_indexing_mode() runs even on error.
        // enter_indexing_mode() sets synchronous=OFF — must not leak to pool.
        let result = (|| -> Result<IndexProgress, ServerError> {
            {
                let mut trigram_guard = self.trigram.write().unwrap_or_else(|e| e.into_inner());

                self.phase2_batch_write(
                    &file_data,
                    &indexing_conn,
                    &mut trigram_guard,
                    &progress,
                    &mut state,
                )?;

                self.handle_deletions(
                    &indexing_conn,
                    &existing_paths,
                    &seen_paths,
                    &mut trigram_guard,
                    &mut state,
                )?;
            } // Drop write guard before save_trigrams (which takes a read lock)

            self.persist_trigrams(&indexing_conn, &state, force)?;
            Ok(state)
        })();

        // Always restore normal pragmas, even on error
        if let Err(e) = self.db.exit_indexing_mode(&indexing_conn) {
            tracing::error!("Failed to restore normal pragmas after indexing: {e}");
        }

        let mut state = result?;
        state.current_file = None;
        state.files_processed = total;

        if let Some(ref cb) = progress {
            cb(state.clone());
        }

        Ok(state)
    }

    /// Phase 1: Parallel file reading and hash computation.
    ///
    /// Returns changed files (needing indexing) and the set of all seen paths.
    fn phase1_read_and_hash(
        &self,
        files: &[PathBuf],
        existing_hashes: &HashMap<String, u64>,
    ) -> (Vec<FileData>, HashSet<String>) {
        // Embarrassingly parallel — no shared mutable state
        let file_data: Vec<FileData> = files
            .par_iter()
            .filter_map(|path| {
                let content = fs::read_to_string(path).ok()?;
                let hash = compute_hash(&content);
                let path_str = path.to_string_lossy().to_string();

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

        // Collect all seen paths (including unchanged ones).
        // Uses iter() not par_iter(): to_string_lossy() is pure allocation,
        // not CPU-bound work — rayon overhead exceeds benefit here.
        let seen_paths: HashSet<String> = files
            .iter()
            .map(|p| p.to_string_lossy().to_string())
            .collect();

        (file_data, seen_paths)
    }

    /// Phase 2: Sequential batch upserts and trigram updates.
    fn phase2_batch_write(
        &self,
        file_data: &[FileData],
        conn: &rusqlite::Connection,
        trigram_guard: &mut TrigramIndex,
        progress: &Option<ProgressCallback>,
        state: &mut IndexProgress,
    ) -> Result<(), ServerError> {
        for batch in file_data.chunks(BATCH_SIZE) {
            if let Some(ref cb) = progress {
                state.current_file = batch.first().map(|f| PathBuf::from(&f.path));
                cb(state.clone());
            }

            let file_ids = Database::upsert_files_batch_on(conn, batch)?;

            for (data, file_id) in batch.iter().zip(file_ids) {
                trigram_guard.add_file(file_id, &data.content);
            }

            state.files_indexed += batch.len();
            state.files_processed += batch.len();
        }
        Ok(())
    }

    /// Removes files that were previously indexed but no longer exist on disk.
    fn handle_deletions(
        &self,
        conn: &rusqlite::Connection,
        existing_paths: &HashSet<String>,
        seen_paths: &HashSet<String>,
        trigram_guard: &mut TrigramIndex,
        state: &mut IndexProgress,
    ) -> Result<(), ServerError> {
        for path in existing_paths.difference(seen_paths) {
            if let Ok(Some(file_id)) = Database::get_file_id_on(conn, path) {
                trigram_guard.remove_file(file_id);
            }
            if Database::delete_file_on(conn, path)? {
                state.files_deleted += 1;
            }
        }
        Ok(())
    }

    /// Persists the trigram index to the database if changes were made.
    fn persist_trigrams(
        &self,
        conn: &rusqlite::Connection,
        state: &IndexProgress,
        force: bool,
    ) -> Result<(), ServerError> {
        if state.files_indexed == 0 && state.files_deleted == 0 {
            return Ok(());
        }

        if force {
            let entries = {
                let trigram = self.trigram.read().unwrap_or_else(|e| e.into_inner());
                trigram.to_db_entries()
            };
            Database::save_trigrams_on(conn, &entries)?;
        } else {
            let (upserts, deletes) = {
                let mut trigram = self.trigram.write().unwrap_or_else(|e| e.into_inner());
                trigram.take_dirty_entries()
            };
            Database::save_dirty_trigrams_on(conn, &upserts, &deletes)?;
        }

        Ok(())
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
            .max_filesize(Some(self.config.max_file_size))
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
    fn test_index_removes_deleted_files_from_trigram() {
        let dir = TempDir::new().unwrap();
        let db = Arc::new(Database::in_memory().unwrap());
        let trigram = Arc::new(RwLock::new(TrigramIndex::new()));

        // Create 3 files with unique content
        fs::write(dir.path().join("alpha.rs"), "fn unique_alpha_function() {}").unwrap();
        fs::write(dir.path().join("beta.rs"), "fn unique_beta_function() {}").unwrap();
        fs::write(dir.path().join("gamma.rs"), "fn unique_gamma_function() {}").unwrap();

        let indexer = Indexer::new(db.clone(), trigram.clone(), dir.path().to_path_buf());

        // Index all 3 files
        let progress = indexer.index(None, false).unwrap();
        assert_eq!(progress.files_indexed, 3);

        // Delete beta.rs from disk
        fs::remove_file(dir.path().join("beta.rs")).unwrap();

        // Re-index — should detect deletion
        let progress = indexer.index(None, false).unwrap();
        assert_eq!(progress.files_deleted, 1);

        // Verify trigram index no longer contains the deleted file's content
        let tri = trigram.read().unwrap();
        let results = tri.search("unique_beta_function");
        // Should be empty (or None): deleted file's trigrams are cleaned up
        match results {
            None => {} // Fine: no trigrams match
            Some(bitmap) => assert!(
                bitmap.is_empty(),
                "Deleted file's FileId should not appear in trigram results"
            ),
        }

        // Verify that the other files' trigrams still work
        let alpha_results = tri.search("unique_alpha_function");
        assert!(alpha_results.is_some());
        assert!(!alpha_results.unwrap().is_empty());
    }

    #[test]
    fn test_incremental_index_deletion() {
        let dir = TempDir::new().unwrap();
        let db = Arc::new(Database::in_memory().unwrap());
        let trigram = Arc::new(RwLock::new(TrigramIndex::new()));

        fs::write(dir.path().join("keep.rs"), "fn keep() {}").unwrap();
        fs::write(dir.path().join("remove.rs"), "fn remove() {}").unwrap();

        let indexer = Indexer::new(db.clone(), trigram.clone(), dir.path().to_path_buf());

        let progress = indexer.index(None, false).unwrap();
        assert_eq!(progress.files_indexed, 2);
        assert_eq!(db.file_count().unwrap(), 2);

        // Delete one file and re-index
        fs::remove_file(dir.path().join("remove.rs")).unwrap();
        let progress = indexer.index(None, false).unwrap();
        assert_eq!(progress.files_deleted, 1);
        assert_eq!(db.file_count().unwrap(), 1);
    }

    #[test]
    fn test_hash_computation() {
        let hash1 = compute_hash("hello");
        let hash2 = compute_hash("hello");
        let hash3 = compute_hash("world");

        assert_eq!(hash1, hash2);
        assert_ne!(hash1, hash3);
    }

    #[test]
    fn test_incremental_trigram_persistence() {
        let dir = TempDir::new().unwrap();
        let db = Arc::new(Database::in_memory().unwrap());
        let trigram = Arc::new(RwLock::new(TrigramIndex::new()));

        // Create files with distinct trigram content
        fs::write(dir.path().join("alpha.rs"), "fn unique_alpha() {}").unwrap();
        fs::write(dir.path().join("beta.rs"), "fn unique_beta() {}").unwrap();

        let indexer = Indexer::new(db.clone(), trigram.clone(), dir.path().to_path_buf());

        // First index (force=false): uses dirty persistence
        let progress = indexer.index(None, false).unwrap();
        assert_eq!(progress.files_indexed, 2);

        // Verify trigrams were persisted to DB
        let db_trigrams = db.load_all_trigrams().unwrap();
        assert!(!db_trigrams.is_empty());
        let initial_trigram_count = db_trigrams.len();

        // Dirty set should be clear after persistence
        {
            let tri = trigram.read().unwrap();
            assert_eq!(tri.dirty_count(), 0);
        }

        // Add a new file and reindex incrementally
        fs::write(dir.path().join("gamma.rs"), "fn unique_gamma_xyz() {}").unwrap();
        let progress = indexer.index(None, false).unwrap();
        assert_eq!(progress.files_indexed, 1);
        assert_eq!(progress.files_unchanged, 2);

        // Verify trigram count grew (new trigrams from gamma.rs content)
        let db_trigrams_after = db.load_all_trigrams().unwrap();
        assert!(db_trigrams_after.len() >= initial_trigram_count);

        // Verify search still works for all files
        let tri = trigram.read().unwrap();
        assert!(tri.search("unique_alpha").is_some());
        assert!(!tri.search("unique_alpha").unwrap().is_empty());
        assert!(tri.search("unique_gamma").is_some());
        assert!(!tri.search("unique_gamma").unwrap().is_empty());
    }
}
