//! grepika: Token-efficient MCP server for code search.
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
pub mod fmt;
pub mod profiling;
pub mod security;
pub mod server;
pub mod services;
pub mod tools;
pub mod types;

pub use error::{Result, ServerError};
pub use types::{FileId, NgramKey, Score, Trigram};

use std::path::{Path, PathBuf};

/// Computes the default database path for a given root directory.
///
/// The path is `~/.cache/grepika/<hash>.db` where `<hash>` is the
/// 16 hex characters of the xxh3_64 hash of the canonical root path.
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
    let hash = xxhash_rust::xxh3::xxh3_64(root.to_string_lossy().as_bytes());
    let hash = format!("{hash:016x}");

    let cache_dir = dirs::cache_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("grepika");

    cache_dir.join(format!("{hash}.db"))
}
