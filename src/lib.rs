//! agentika-grep: Token-efficient MCP server for code search.
//!
//! This library provides a high-performance code search server using:
//! - Trigram indexing for fast substring search
//! - FTS5 full-text search with BM25 ranking
//! - Parallel grep with ripgrep internals
//! - Combined scoring from multiple search backends
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────┐
//! │              MCP Server (rmcp)              │
//! │         JSON-RPC over stdin/stdout          │
//! └─────────────────┬───────────────────────────┘
//!                   │
//! ┌─────────────────▼───────────────────────────┐
//! │               Tool Router                    │
//! │  search, relevant, get, stats, outline...   │
//! └─────────────────┬───────────────────────────┘
//!                   │
//! ┌─────────────────▼───────────────────────────┐
//! │            Search Service                    │
//! │     (spawn_blocking for async bridge)       │
//! └───────┬─────────┬─────────┬─────────────────┘
//!         │         │         │
//!    ┌────▼───┐ ┌───▼───┐ ┌───▼────┐
//!    │  FTS5  │ │ Grep  │ │Trigram │
//!    │ BM25   │ │rayon  │ │ Index  │
//!    └────┬───┘ └───┬───┘ └───┬────┘
//!         │         │         │
//!    ┌────▼─────────▼─────────▼────┐
//!    │     SQLite Database          │
//!    │   (r2d2 connection pool)     │
//!    └──────────────────────────────┘
//! ```

#[doc(hidden)]
pub mod bench_utils;
pub mod db;
pub mod error;
pub mod security;
pub mod server;
pub mod services;
pub mod tools;
pub mod types;

pub use error::{Result, ServerError};
pub use types::{FileId, Score, Trigram};

use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// Computes the default database path for a given root directory.
///
/// The path is `~/.cache/agentika-grep/<hash>.db` where `<hash>` is the first
/// 16 hex characters of the SHA256 hash of the canonical root path.
///
/// This decouples index storage from the indexed directory, preventing:
/// - Pollution of git repositories with index files
/// - Need for `.gitignore` modifications
/// - Write permission requirements in the indexed directory
///
/// # Panics
///
/// Panics if the cache directory cannot be created.
#[must_use]
pub fn default_db_path(root: &Path) -> PathBuf {
    let mut hasher = Sha256::new();
    hasher.update(root.to_string_lossy().as_bytes());
    let result = hasher.finalize();
    // Use first 8 bytes = 16 hex characters for uniqueness
    let hash: String = result[..8].iter().map(|b| format!("{b:02x}")).collect();

    let cache_dir = dirs::cache_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("agentika-grep");

    cache_dir.join(format!("{hash}.db"))
}
