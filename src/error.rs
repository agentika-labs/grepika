//! Error types for agentika-grep.
//!
//! Uses thiserror for ergonomic error handling with proper
//! error chain propagation.

use std::path::PathBuf;
use thiserror::Error;

/// Top-level server error.
#[derive(Error, Debug)]
pub enum ServerError {
    #[error("Database error: {0}")]
    Database(#[from] DbError),

    #[error("Search error: {0}")]
    Search(#[from] SearchError),

    #[error("Index error: {0}")]
    Index(#[from] IndexError),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("Configuration error: {0}")]
    Config(String),

    #[error("Tool error: {0}")]
    Tool(String),
}

/// Database-specific errors.
#[derive(Error, Debug)]
pub enum DbError {
    #[error("SQLite error: {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("Connection pool error: {0}")]
    Pool(#[from] r2d2::Error),

    #[error("Schema migration failed: {0}")]
    Migration(String),

    #[error("File not found in database: {path}")]
    FileNotFound { path: PathBuf },

    #[error("Database is locked")]
    Locked,
}

/// Search operation errors.
#[derive(Error, Debug)]
pub enum SearchError {
    #[error("Invalid regex pattern: {0}")]
    InvalidPattern(String),

    #[error("Grep error: {0}")]
    Grep(#[from] GrepError),

    #[error("Search timeout after {seconds}s")]
    Timeout { seconds: u64 },

    #[error("No results found")]
    NoResults,

    #[error("Search cancelled")]
    Cancelled,
}

/// Grep-specific errors.
#[derive(Error, Debug)]
pub enum GrepError {
    #[error("Regex build error: {0}")]
    RegexBuild(String),

    #[error("File read error for {path}: {source}")]
    FileRead {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("Binary file skipped: {path}")]
    BinaryFile { path: PathBuf },

    #[error("Walk error: {0}")]
    Walk(String),
}

/// Indexing errors.
#[derive(Error, Debug)]
pub enum IndexError {
    #[error("Failed to index file {path}: {reason}")]
    FileIndex { path: PathBuf, reason: String },

    #[error("Hash computation failed: {0}")]
    Hash(String),

    #[error("Trigram extraction failed: {0}")]
    Trigram(String),

    #[error("Index is stale and needs rebuild")]
    Stale,

    #[error("Index corruption detected: {0}")]
    Corruption(String),
}

/// Result type alias for server operations.
pub type Result<T> = std::result::Result<T, ServerError>;

/// Result type alias for database operations.
pub type DbResult<T> = std::result::Result<T, DbError>;

/// Result type alias for search operations.
pub type SearchResult<T> = std::result::Result<T, SearchError>;

/// Result type alias for index operations.
pub type IndexResult<T> = std::result::Result<T, IndexError>;

// Error code implementations for machine-readable error responses
impl ServerError {
    /// Returns a machine-readable error code.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::Database(e) => e.code(),
            Self::Search(e) => e.code(),
            Self::Index(e) => e.code(),
            Self::Io(_) => "IO_ERROR",
            Self::Json(_) => "JSON_ERROR",
            Self::Config(_) => "CONFIG_ERROR",
            Self::Tool(_) => "TOOL_ERROR",
        }
    }
}

impl DbError {
    /// Returns a machine-readable error code.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::Sqlite(_) => "SQLITE_ERROR",
            Self::Pool(_) => "POOL_ERROR",
            Self::Migration(_) => "MIGRATION_ERROR",
            Self::FileNotFound { .. } => "FILE_NOT_FOUND",
            Self::Locked => "DB_LOCKED",
        }
    }
}

impl SearchError {
    /// Returns a machine-readable error code.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::InvalidPattern(_) => "INVALID_PATTERN",
            Self::Grep(e) => e.code(),
            Self::Timeout { .. } => "TIMEOUT",
            Self::NoResults => "NO_RESULTS",
            Self::Cancelled => "CANCELLED",
        }
    }
}

impl GrepError {
    /// Returns a machine-readable error code.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::RegexBuild(_) => "REGEX_BUILD_ERROR",
            Self::FileRead { .. } => "FILE_READ_ERROR",
            Self::BinaryFile { .. } => "BINARY_FILE",
            Self::Walk(_) => "WALK_ERROR",
        }
    }
}

impl IndexError {
    /// Returns a machine-readable error code.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::FileIndex { .. } => "FILE_INDEX_ERROR",
            Self::Hash(_) => "HASH_ERROR",
            Self::Trigram(_) => "TRIGRAM_ERROR",
            Self::Stale => "INDEX_STALE",
            Self::Corruption(_) => "INDEX_CORRUPT",
        }
    }
}

impl From<GrepError> for ServerError {
    fn from(err: GrepError) -> Self {
        Self::Search(SearchError::Grep(err))
    }
}

// Conversion to rmcp tool errors
impl From<ServerError> for rmcp::Error {
    fn from(err: ServerError) -> Self {
        rmcp::Error::internal_error(err.to_string(), None)
    }
}
