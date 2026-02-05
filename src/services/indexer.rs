//! Incremental file indexer with change detection.
//!
//! Uses SHA256 hashing to detect file changes and only
//! re-indexes modified files.

use crate::db::Database;
use crate::error::{IndexError, IndexResult, ServerError};
use crate::services::TrigramIndex;
use crate::types::FileId;
use ignore::WalkBuilder;
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

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
}

impl Indexer {
    /// Creates a new indexer.
    pub fn new(db: Arc<Database>, trigram: Arc<RwLock<TrigramIndex>>, root: PathBuf) -> Self {
        Self {
            db,
            trigram,
            root,
            config: IndexConfig::default(),
        }
    }

    /// Creates an indexer with custom configuration.
    pub fn with_config(
        db: Arc<Database>,
        trigram: Arc<RwLock<TrigramIndex>>,
        root: PathBuf,
        config: IndexConfig,
    ) -> Self {
        Self {
            db,
            trigram,
            root,
            config,
        }
    }

    /// Performs incremental indexing.
    ///
    /// Returns the final progress state.
    ///
    /// # Errors
    ///
    /// Returns `ServerError::Database` if database operations fail.
    /// Returns `ServerError::Index` if trigram indexing fails.
    pub fn index(&self, progress: Option<ProgressCallback>) -> Result<IndexProgress, ServerError> {
        // Get existing indexed files
        let existing: HashSet<String> = self
            .db
            .get_indexed_files()?
            .into_iter()
            .map(|(path, _)| path)
            .collect();

        // Collect files to process
        let files: Vec<PathBuf> = self.collect_files()?;
        let total = files.len();

        let mut state = IndexProgress {
            files_processed: 0,
            files_total: total,
            files_indexed: 0,
            files_unchanged: 0,
            files_deleted: 0,
            current_file: None,
        };

        let mut seen_paths: HashSet<String> = HashSet::new();

        // Process files
        for path in files {
            state.current_file = Some(path.clone());
            state.files_processed += 1;

            if let Some(ref cb) = progress {
                cb(state.clone());
            }

            let path_str = path.to_string_lossy().to_string();
            seen_paths.insert(path_str.clone());

            // Read file content
            let content = match fs::read_to_string(&path) {
                Ok(c) => c,
                Err(_) => continue, // Skip binary/unreadable files
            };

            // Check if file needs indexing
            let hash = compute_hash(&content);
            let needs_index = match self.db.get_file_by_path(&path_str) {
                Ok(Some((_, existing_content))) => compute_hash(&existing_content) != hash,
                Ok(None) => true,
                Err(_) => true,
            };

            if needs_index {
                // Index the file
                let file_id = self.db.upsert_file(&path_str, &content, &hash)?;
                self.index_trigrams(file_id, &content).execute()?;
                state.files_indexed += 1;
            } else {
                state.files_unchanged += 1;
            }
        }

        // Remove deleted files
        for path in existing.difference(&seen_paths) {
            if self.db.delete_file(path)? {
                state.files_deleted += 1;
            }
        }

        state.current_file = None;

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

        let file_id = self.db.upsert_file(&path_str, &content, &hash)?;
        self.index_trigrams(file_id, &content).execute()?;

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

            // Check extension
            if !self.config.extensions.is_empty() {
                let ext = path
                    .extension()
                    .and_then(|e| e.to_str())
                    .unwrap_or("")
                    .to_lowercase();

                if !self
                    .config
                    .extensions
                    .iter()
                    .any(|e| e.to_lowercase() == ext)
                {
                    // Check for extensionless files like Makefile, Dockerfile
                    let filename = path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("")
                        .to_lowercase();

                    if !self
                        .config
                        .extensions
                        .iter()
                        .any(|e| e.to_lowercase() == filename)
                    {
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

            files.push(path.to_path_buf());
        }

        Ok(files)
    }

    /// Updates trigram index for a file.
    fn index_trigrams(&self, file_id: FileId, content: &str) -> IndexTrigrams {
        IndexTrigrams {
            trigram: Arc::clone(&self.trigram),
            file_id,
            content: content.to_string(),
        }
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
        let trigram = self.trigram.read().unwrap_or_else(|e| e.into_inner());
        let trigram_count = trigram.trigram_count();

        Ok(IndexStats {
            file_count,
            trigram_count,
        })
    }
}

/// Helper for async trigram indexing.
struct IndexTrigrams {
    trigram: Arc<RwLock<TrigramIndex>>,
    file_id: FileId,
    content: String,
}

impl IndexTrigrams {
    /// Executes trigram indexing for a file.
    ///
    /// # Errors
    ///
    /// Currently always returns `Ok(())`. The `IndexResult` is for future extensibility.
    fn execute(self) -> IndexResult<()> {
        // Lock poisoning recovery: continue with the inner data
        let mut trigram = self.trigram.write().unwrap_or_else(|e| e.into_inner());
        trigram.add_file(self.file_id, &self.content);
        Ok(())
    }
}

/// Index statistics.
#[derive(Debug, Clone)]
pub struct IndexStats {
    pub file_count: u64,
    pub trigram_count: usize,
}

/// Computes SHA256 hash of content.
fn compute_hash(content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    let result = hasher.finalize();
    hex::encode(result)
}

mod hex {
    pub fn encode(bytes: impl AsRef<[u8]>) -> String {
        bytes
            .as_ref()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }
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

        let progress = indexer.index(None).unwrap();
        assert_eq!(progress.files_indexed, 2);
        assert_eq!(db.file_count().unwrap(), 2);
    }

    #[test]
    fn test_incremental_index() {
        let (dir, db, trigram) = setup_test_env();
        let indexer = Indexer::new(db.clone(), trigram, dir.path().to_path_buf());

        // First index
        let progress1 = indexer.index(None).unwrap();
        assert_eq!(progress1.files_indexed, 2);

        // Second index (no changes)
        let progress2 = indexer.index(None).unwrap();
        assert_eq!(progress2.files_indexed, 0);
        assert_eq!(progress2.files_unchanged, 2);

        // Modify a file
        fs::write(
            dir.path().join("main.rs"),
            "fn main() { println!(\"hi\"); }",
        )
        .unwrap();

        // Third index (one change)
        let progress3 = indexer.index(None).unwrap();
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
