//! Error types for grepika.
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
    #[error("SQLite error: {0}. Try running 'index' with force=true to rebuild.")]
    Sqlite(#[from] rusqlite::Error),

    #[error("Connection pool error: {0}. Try again in a moment, or restart the server.")]
    Pool(#[from] r2d2::Error),

    #[error("Schema migration failed: {0}")]
    Migration(String),

    #[error("File not found in database: {path}. Run 'index' to add new files, or check the path is relative to workspace root.")]
    FileNotFound { path: PathBuf },

    #[error("Database is locked. Another process may be indexing. Wait a moment and retry.")]
    Locked,
}

/// Search operation errors.
#[derive(Error, Debug)]
pub enum SearchError {
    #[error("Invalid regex pattern: {0}. Check your pattern syntax or use mode=fts for natural language search.")]
    InvalidPattern(String),

    #[error("Grep error: {0}")]
    Grep(#[from] GrepError),

    #[error("Search timeout after {seconds}s")]
    Timeout { seconds: u64 },

    #[error("No results found for '{query}'. Try broader terms, a different mode (fts/grep), or run 'index' to refresh.")]
    NoResults { query: String },

    #[error("Search cancelled")]
    Cancelled,
}

/// Grep-specific errors.
#[derive(Error, Debug)]
pub enum GrepError {
    #[error("Invalid regex: {0}. Check your pattern syntax or use mode=fts for natural language search.")]
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

    #[error("Index is stale. Run 'index' with force=true to rebuild.")]
    Stale,

    #[error(
        "Index corruption: {0}. Delete the index file and run 'index' to rebuild from scratch."
    )]
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

/// Trait for error types that provide machine-readable codes.
pub trait ErrorCode {
    /// Returns a machine-readable error code string.
    fn code(&self) -> &'static str;
}

impl ErrorCode for ServerError {
    fn code(&self) -> &'static str {
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

impl ErrorCode for DbError {
    fn code(&self) -> &'static str {
        match self {
            Self::Sqlite(_) => "SQLITE_ERROR",
            Self::Pool(_) => "POOL_ERROR",
            Self::Migration(_) => "MIGRATION_ERROR",
            Self::FileNotFound { .. } => "FILE_NOT_FOUND",
            Self::Locked => "DB_LOCKED",
        }
    }
}

impl ErrorCode for SearchError {
    fn code(&self) -> &'static str {
        match self {
            Self::InvalidPattern(_) => "INVALID_PATTERN",
            Self::Grep(e) => e.code(),
            Self::Timeout { .. } => "TIMEOUT",
            Self::NoResults { .. } => "NO_RESULTS",
            Self::Cancelled => "CANCELLED",
        }
    }
}

impl ErrorCode for GrepError {
    fn code(&self) -> &'static str {
        match self {
            Self::RegexBuild(_) => "REGEX_BUILD_ERROR",
            Self::FileRead { .. } => "FILE_READ_ERROR",
            Self::BinaryFile { .. } => "BINARY_FILE",
            Self::Walk(_) => "WALK_ERROR",
        }
    }
}

impl ErrorCode for IndexError {
    fn code(&self) -> &'static str {
        match self {
            Self::FileIndex { .. } => "FILE_INDEX_ERROR",
            Self::Hash(_) => "HASH_ERROR",
            Self::Trigram(_) => "TRIGRAM_ERROR",
            Self::Stale => "INDEX_STALE",
            Self::Corruption(_) => "INDEX_CORRUPT",
        }
    }
}

impl ServerError {
    /// Returns true if the LLM client can fix this error (bad input, not found, etc.)
    pub fn is_client_fixable(&self) -> bool {
        matches!(
            self,
            Self::Search(SearchError::InvalidPattern(_))
                | Self::Search(SearchError::NoResults { .. })
                | Self::Database(DbError::FileNotFound { .. })
                | Self::Config(_)
                | Self::Tool(_)
        )
    }
}

impl From<GrepError> for ServerError {
    fn from(err: GrepError) -> Self {
        Self::Search(SearchError::Grep(err))
    }
}

impl From<crate::security::SecurityError> for ServerError {
    fn from(err: crate::security::SecurityError) -> Self {
        Self::Tool(err.to_string())
    }
}

// Conversion to rmcp tool errors.
// Maps client-fixable errors to invalid_params (-32602) and server faults to internal_error (-32603).
// The machine-readable code from `.code()` is preserved in `data.code` for MCP clients.
impl From<ServerError> for rmcp::ErrorData {
    fn from(err: ServerError) -> Self {
        let code = err.code();
        let data = Some(serde_json::json!({ "code": code }));
        let message = err.to_string();

        match &err {
            // Client-fixable errors → invalid_params (-32602)
            ServerError::Search(SearchError::InvalidPattern(_))
            | ServerError::Search(SearchError::NoResults { .. })
            | ServerError::Database(DbError::FileNotFound { .. })
            | ServerError::Config(_)
            | ServerError::Tool(_) => rmcp::ErrorData::invalid_params(message, data),
            // Server-side faults → internal_error (-32603)
            _ => rmcp::ErrorData::internal_error(message, data),
        }
    }
}
