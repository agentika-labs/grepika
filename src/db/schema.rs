//! Database schema definitions.

use crate::error::DbResult;
use rusqlite::Connection;

/// Current schema version for migrations.
/// v2: Changed hash from TEXT (SHA256 hex) to INTEGER (xxHash u64)
pub const SCHEMA_VERSION: u32 = 2;

/// Initializes the database schema.
///
/// Handles schema versioning - if an older schema version exists,
/// drops all tables and recreates them with the new schema.
///
/// # Errors
///
/// Returns `DbError::Sqlite` if schema creation fails.
pub fn init_schema(conn: &Connection) -> DbResult<()> {
    // Check existing schema version
    let existing_version: Option<u32> = conn
        .query_row(
            "SELECT CAST(value AS INTEGER) FROM schema_info WHERE key = 'version'",
            [],
            |row| row.get(0),
        )
        .ok();

    match existing_version {
        Some(v) if v >= SCHEMA_VERSION => return Ok(()), // Already up to date
        Some(_) => {
            // Old version - drop everything and recreate
            conn.execute_batch(
                r"
                DROP TABLE IF EXISTS files;
                DROP TABLE IF EXISTS files_fts;
                DROP TABLE IF EXISTS trigrams;
                DROP TABLE IF EXISTS schema_info;
                ",
            )?;
        }
        None => {} // Fresh database
    }

    conn.execute_batch(
        r#"
        -- Main files table
        -- hash is INTEGER (xxHash u64) for fast change detection
        CREATE TABLE IF NOT EXISTS files (
            file_id INTEGER PRIMARY KEY AUTOINCREMENT,
            path TEXT NOT NULL UNIQUE,
            filename TEXT NOT NULL,
            content TEXT NOT NULL,
            hash INTEGER NOT NULL,
            indexed_at TEXT NOT NULL,
            size_bytes INTEGER GENERATED ALWAYS AS (length(content)) STORED
        );

        -- Index for path lookups
        CREATE INDEX IF NOT EXISTS idx_files_path ON files(path);

        -- Index for filename searches
        CREATE INDEX IF NOT EXISTS idx_files_filename ON files(filename);

        -- FTS5 virtual table for full-text search
        -- Using porter tokenizer for stemming (search -> search, searching)
        CREATE VIRTUAL TABLE IF NOT EXISTS files_fts USING fts5(
            path,
            filename,
            content,
            content='files',
            content_rowid='file_id',
            tokenize='porter unicode61'
        );

        -- Triggers to keep FTS in sync with files table
        CREATE TRIGGER IF NOT EXISTS files_ai AFTER INSERT ON files BEGIN
            INSERT INTO files_fts(rowid, path, filename, content)
            VALUES (new.file_id, new.path, new.filename, new.content);
        END;

        CREATE TRIGGER IF NOT EXISTS files_ad AFTER DELETE ON files BEGIN
            INSERT INTO files_fts(files_fts, rowid, path, filename, content)
            VALUES ('delete', old.file_id, old.path, old.filename, old.content);
        END;

        CREATE TRIGGER IF NOT EXISTS files_au AFTER UPDATE ON files BEGIN
            INSERT INTO files_fts(files_fts, rowid, path, filename, content)
            VALUES ('delete', old.file_id, old.path, old.filename, old.content);
            INSERT INTO files_fts(rowid, path, filename, content)
            VALUES (new.file_id, new.path, new.filename, new.content);
        END;

        -- Trigram index table for fast substring search
        -- Stores RoaringBitmap-encoded file IDs per trigram
        CREATE TABLE IF NOT EXISTS trigrams (
            trigram BLOB PRIMARY KEY,
            file_ids BLOB NOT NULL
        ) WITHOUT ROWID;

        -- Schema version tracking
        CREATE TABLE IF NOT EXISTS schema_info (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        ) WITHOUT ROWID;

        INSERT OR REPLACE INTO schema_info (key, value)
        VALUES ('version', '2');
        "#,
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::pragmas::apply_pragmas;

    #[test]
    fn test_schema_creation() {
        let conn = Connection::open_in_memory().unwrap();
        apply_pragmas(&conn).unwrap();
        init_schema(&conn).unwrap();

        // Verify tables exist
        let tables: Vec<String> = conn
            .prepare("SELECT name FROM sqlite_master WHERE type='table'")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .filter_map(Result::ok)
            .collect();

        assert!(tables.contains(&"files".to_string()));
        assert!(tables.contains(&"trigrams".to_string()));
        assert!(tables.contains(&"files_fts".to_string()));
    }
}
