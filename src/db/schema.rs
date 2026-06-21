//! Database schema definitions.

use crate::error::DbResult;
use rusqlite::Connection;

/// Current schema version for migrations.
/// v2: Changed hash from TEXT (SHA256 hex) to INTEGER (xxHash u64)
/// v3: Replaced 3-byte trigram keys with u64 sparse n-gram keys
/// v4: Added symbols + edges tables (tree-sitter code graph) and embeddings
/// v5: Added graph foreign keys/cascades for index lifecycle cleanup
pub const SCHEMA_VERSION: u32 = 5;

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
                DROP TABLE IF EXISTS files_fts;
                DROP TABLE IF EXISTS embeddings;
                DROP TABLE IF EXISTS edges;
                DROP TABLE IF EXISTS symbols;
                DROP TABLE IF EXISTS trigrams;
                DROP TABLE IF EXISTS files;
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

        -- Code graph: definitions extracted via tree-sitter.
        CREATE TABLE IF NOT EXISTS symbols (
            symbol_id INTEGER PRIMARY KEY AUTOINCREMENT,
            file_id INTEGER NOT NULL REFERENCES files(file_id) ON DELETE CASCADE,
            name TEXT NOT NULL,
            kind TEXT NOT NULL,
            start_line INTEGER NOT NULL,
            end_line INTEGER NOT NULL,
            start_byte INTEGER NOT NULL,
            end_byte INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_symbols_file ON symbols(file_id);
        CREATE INDEX IF NOT EXISTS idx_symbols_name ON symbols(name);

        -- Code graph edges. dst is stored by name and resolved at query time
        -- (name-based; no type scoping). src_symbol is NULL for file-level
        -- import edges. kind is 'CALLS' or 'IMPORTS'.
        CREATE TABLE IF NOT EXISTS edges (
            src_symbol INTEGER REFERENCES symbols(symbol_id) ON DELETE CASCADE,
            dst_name TEXT NOT NULL,
            kind TEXT NOT NULL CHECK (kind IN ('CALLS', 'IMPORTS')),
            file_id INTEGER NOT NULL REFERENCES files(file_id) ON DELETE CASCADE
        );
        CREATE INDEX IF NOT EXISTS idx_edges_src ON edges(src_symbol);
        CREATE INDEX IF NOT EXISTS idx_edges_dst ON edges(dst_name);
        CREATE INDEX IF NOT EXISTS idx_edges_file ON edges(file_id);

        -- Semantic embeddings (one row per symbol chunk). Populated only when
        -- the `semantic` feature is built; otherwise the table stays empty.
        CREATE TABLE IF NOT EXISTS embeddings (
            symbol_id INTEGER PRIMARY KEY REFERENCES symbols(symbol_id) ON DELETE CASCADE,
            file_id INTEGER NOT NULL REFERENCES files(file_id) ON DELETE CASCADE,
            vec BLOB NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_embeddings_file ON embeddings(file_id);

        -- Schema version tracking
        CREATE TABLE IF NOT EXISTS schema_info (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        ) WITHOUT ROWID;

        INSERT OR REPLACE INTO schema_info (key, value)
        VALUES ('version', '5');
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

    /// 6e: Verify SCHEMA_VERSION constant matches the value written to SQL.
    #[test]
    fn test_schema_version_consistency() {
        let conn = Connection::open_in_memory().unwrap();
        apply_pragmas(&conn).unwrap();
        init_schema(&conn).unwrap();

        let db_version: u32 = conn
            .query_row(
                "SELECT CAST(value AS INTEGER) FROM schema_info WHERE key = 'version'",
                [],
                |row| row.get(0),
            )
            .unwrap();

        assert_eq!(
            db_version, SCHEMA_VERSION,
            "SCHEMA_VERSION constant ({SCHEMA_VERSION}) does not match SQL-embedded version ({db_version})"
        );
    }
}
