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

pub mod db;
pub mod error;
pub mod server;
pub mod services;
pub mod tools;
pub mod types;

pub use error::{Result, ServerError};
pub use types::{FileId, Score, Trigram};
