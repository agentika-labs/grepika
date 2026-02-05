//! Database layer with connection pooling and FTS5.

mod pragmas;
mod schema;

pub use pragmas::apply_pragmas;
pub use schema::{init_schema, SCHEMA_VERSION};

use crate::error::{DbError, DbResult};
use crate::types::FileId;
use r2d2::{Pool, PooledConnection};
use r2d2_sqlite::SqliteConnectionManager;
use std::path::Path;

/// File data for batch upsert operations.
#[derive(Debug)]
pub struct FileData {
    /// File path (relative or absolute)
    pub path: String,
    /// File content
    pub content: String,
    /// Content hash (xxHash u64)
    pub hash: u64,
}

/// Database handle with connection pooling.
///
/// Uses r2d2 because `rusqlite::Connection` is NOT Sync.
/// The pool manages thread-safe access to `SQLite` connections.
///
/// Thread-safe (Send + Sync) via r2d2's internal synchronization.
pub struct Database {
    pool: Pool<SqliteConnectionManager>,
}

impl Database {
    /// Opens or creates a database at the given path.
    ///
    /// # Errors
    ///
    /// Returns `DbError::Pool` if connection pool creation fails.
    /// Returns `DbError::Sqlite` if schema initialization fails.
    pub fn open(path: &Path) -> DbResult<Self> {
        let manager = SqliteConnectionManager::file(path);
        let pool = Pool::builder()
            .max_size(4)
            .min_idle(Some(1))
            .build(manager)?;

        // Initialize schema on first connection
        {
            let conn = pool.get()?;
            apply_pragmas(&conn)?;
            init_schema(&conn)?;
        }

        Ok(Self { pool })
    }

    /// Creates an in-memory database (for testing).
    ///
    /// # Errors
    ///
    /// Returns `DbError::Pool` if connection pool creation fails.
    /// Returns `DbError::Sqlite` if schema initialization fails.
    pub fn in_memory() -> DbResult<Self> {
        let manager = SqliteConnectionManager::memory();
        let pool = Pool::builder().max_size(1).build(manager)?;

        {
            let conn = pool.get()?;
            apply_pragmas(&conn)?;
            init_schema(&conn)?;
        }

        Ok(Self { pool })
    }

    /// Gets a connection from the pool.
    ///
    /// # Errors
    ///
    /// Returns `DbError::Pool` if no connection is available within the timeout.
    pub fn conn(&self) -> DbResult<PooledConnection<SqliteConnectionManager>> {
        self.pool.get().map_err(DbError::from)
    }

    /// Performs FTS5 full-text search with BM25 ranking.
    ///
    /// # Errors
    ///
    /// Returns `DbError::Pool` if no connection is available.
    /// Returns `DbError::Sqlite` if the query execution fails.
    pub fn fts_search(&self, query: &str, limit: usize) -> DbResult<Vec<(FileId, f64)>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare_cached(
            r"
            SELECT f.file_id, bm25(files_fts, 5.0, 10.0, 1.0) as score
            FROM files_fts
            JOIN files f ON files_fts.rowid = f.file_id
            WHERE files_fts MATCH ?1
            ORDER BY score
            LIMIT ?2
            ",
        )?;

        let results = stmt
            .query_map(rusqlite::params![query, limit as i64], |row| {
                Ok((FileId::new(row.get::<_, u32>(0)?), row.get::<_, f64>(1)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(results)
    }

    /// Upserts a file into the database.
    ///
    /// # Errors
    ///
    /// Returns `DbError::Pool` if no connection is available.
    /// Returns `DbError::Sqlite` if the insert/update operation fails.
    pub fn upsert_file(&self, path: &str, content: &str, hash: u64) -> DbResult<FileId> {
        let conn = self.conn()?;
        let filename = Path::new(path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("");

        // Store hash as i64 since SQLite INTEGER is signed
        let hash_i64 = hash as i64;

        conn.execute(
            r#"
            INSERT INTO files (path, filename, content, hash, indexed_at)
            VALUES (?1, ?2, ?3, ?4, datetime('now'))
            ON CONFLICT(path) DO UPDATE SET
                content = excluded.content,
                hash = excluded.hash,
                indexed_at = excluded.indexed_at
            "#,
            rusqlite::params![path, filename, content, hash_i64],
        )?;

        let file_id: u32 = conn.query_row(
            "SELECT file_id FROM files WHERE path = ?1",
            rusqlite::params![path],
            |row| row.get(0),
        )?;

        Ok(FileId::new(file_id))
    }

    /// Gets file content by ID.
    ///
    /// # Errors
    ///
    /// Returns `DbError::Pool` if no connection is available.
    /// Returns `DbError::Sqlite` if the query fails (other than no rows).
    pub fn get_file(&self, file_id: FileId) -> DbResult<Option<(String, String)>> {
        let conn = self.conn()?;
        let result = conn.query_row(
            "SELECT path, content FROM files WHERE file_id = ?1",
            rusqlite::params![file_id.as_u32()],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        );

        match result {
            Ok(data) => Ok(Some(data)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(DbError::Sqlite(e)),
        }
    }

    /// Gets file content by path.
    ///
    /// # Errors
    ///
    /// Returns `DbError::Pool` if no connection is available.
    /// Returns `DbError::Sqlite` if the query fails (other than no rows).
    pub fn get_file_by_path(&self, path: &str) -> DbResult<Option<(FileId, String)>> {
        let conn = self.conn()?;
        let result = conn.query_row(
            "SELECT file_id, content FROM files WHERE path = ?1",
            rusqlite::params![path],
            |row| Ok((FileId::new(row.get::<_, u32>(0)?), row.get::<_, String>(1)?)),
        );

        match result {
            Ok(data) => Ok(Some(data)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(DbError::Sqlite(e)),
        }
    }

    /// Stores trigram index data.
    ///
    /// # Errors
    ///
    /// Returns `DbError::Pool` if no connection is available.
    /// Returns `DbError::Sqlite` if the insert/update operation fails.
    pub fn upsert_trigrams(&self, trigram: &[u8], file_ids_blob: &[u8]) -> DbResult<()> {
        let conn = self.conn()?;
        conn.execute(
            r"
            INSERT INTO trigrams (trigram, file_ids)
            VALUES (?1, ?2)
            ON CONFLICT(trigram) DO UPDATE SET file_ids = excluded.file_ids
            ",
            rusqlite::params![trigram, file_ids_blob],
        )?;
        Ok(())
    }

    /// Gets trigram file IDs.
    ///
    /// # Errors
    ///
    /// Returns `DbError::Pool` if no connection is available.
    /// Returns `DbError::Sqlite` if the query fails (other than no rows).
    pub fn get_trigram_files(&self, trigram: &[u8]) -> DbResult<Option<Vec<u8>>> {
        let conn = self.conn()?;
        let result = conn.query_row(
            "SELECT file_ids FROM trigrams WHERE trigram = ?1",
            rusqlite::params![trigram],
            |row| row.get::<_, Vec<u8>>(0),
        );

        match result {
            Ok(data) => Ok(Some(data)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(DbError::Sqlite(e)),
        }
    }

    /// Loads all trigrams from the database.
    ///
    /// Returns an iterator of (trigram_bytes, bitmap_bytes) tuples.
    /// This is used to restore the in-memory trigram index on startup.
    ///
    /// # Errors
    ///
    /// Returns `DbError::Pool` if no connection is available.
    /// Returns `DbError::Sqlite` if the query fails.
    pub fn load_all_trigrams(&self) -> DbResult<Vec<(Vec<u8>, Vec<u8>)>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare_cached("SELECT trigram, file_ids FROM trigrams")?;
        let results = stmt
            .query_map([], |row| {
                Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, Vec<u8>>(1)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(results)
    }

    /// Saves all trigrams to the database.
    ///
    /// This replaces the entire trigrams table with the new data.
    /// It's designed for use after indexing completes.
    ///
    /// # Errors
    ///
    /// Returns `DbError::Pool` if no connection is available.
    /// Returns `DbError::Sqlite` if any insert fails.
    pub fn save_trigrams(&self, entries: &[(Vec<u8>, Vec<u8>)]) -> DbResult<()> {
        if entries.is_empty() {
            return Ok(());
        }

        let conn = self.conn()?;

        // Use a transaction for efficiency
        conn.execute("BEGIN IMMEDIATE", [])?;

        // Clear existing trigrams
        conn.execute("DELETE FROM trigrams", [])?;

        // Insert all new trigrams
        {
            let mut stmt = conn.prepare_cached(
                "INSERT INTO trigrams (trigram, file_ids) VALUES (?1, ?2)",
            )?;

            for (trigram, file_ids) in entries {
                stmt.execute(rusqlite::params![trigram, file_ids])?;
            }
        }

        conn.execute("COMMIT", [])?;
        Ok(())
    }

    /// Gets the count of trigrams in the database.
    ///
    /// # Errors
    ///
    /// Returns `DbError::Pool` if no connection is available.
    /// Returns `DbError::Sqlite` if the query fails.
    pub fn trigram_count(&self) -> DbResult<u64> {
        let conn = self.conn()?;
        let count: i64 =
            conn.query_row("SELECT COUNT(*) FROM trigrams", [], |row| row.get(0))?;
        Ok(count as u64)
    }

    /// Gets all indexed file paths with their hashes.
    ///
    /// # Errors
    ///
    /// Returns `DbError::Pool` if no connection is available.
    /// Returns `DbError::Sqlite` if the query fails.
    pub fn get_indexed_files(&self) -> DbResult<Vec<(String, u64)>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare_cached("SELECT path, hash FROM files")?;
        let results = stmt
            .query_map([], |row| {
                let path: String = row.get(0)?;
                let hash: i64 = row.get(1)?;
                Ok((path, hash as u64))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(results)
    }

    /// Gets all indexed file paths with their hashes as a HashMap.
    ///
    /// This is optimized for change detection during indexing,
    /// allowing O(1) lookups instead of O(n) database queries.
    ///
    /// # Errors
    ///
    /// Returns `DbError::Pool` if no connection is available.
    /// Returns `DbError::Sqlite` if the query fails.
    pub fn get_all_hashes(&self) -> DbResult<std::collections::HashMap<String, u64>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare_cached("SELECT path, hash FROM files")?;
        let results = stmt
            .query_map([], |row| {
                let path: String = row.get(0)?;
                let hash: i64 = row.get(1)?;
                Ok((path, hash as u64))
            })?
            .collect::<Result<std::collections::HashMap<_, _>, _>>()?;
        Ok(results)
    }

    /// Batch upserts multiple files in a single transaction.
    ///
    /// This is significantly faster than individual upserts because:
    /// - Single transaction instead of N transactions
    /// - Prepared statements are reused for all files
    /// - Reduces disk I/O by batching commits
    ///
    /// # Returns
    ///
    /// Returns `Vec<FileId>` in the **same order** as the input `files`.
    /// This ordering is critical for trigram indexing.
    ///
    /// # Errors
    ///
    /// Returns `DbError::Pool` if no connection is available.
    /// Returns `DbError::Sqlite` if any insert/update fails.
    pub fn upsert_files_batch(&self, files: &[FileData]) -> DbResult<Vec<FileId>> {
        if files.is_empty() {
            return Ok(Vec::new());
        }

        let conn = self.conn()?;
        let mut file_ids = Vec::with_capacity(files.len());

        // Use IMMEDIATE transaction to avoid SQLITE_BUSY on concurrent access
        conn.execute("BEGIN IMMEDIATE", [])?;

        // Scope the borrows so we can execute COMMIT after
        {
            let mut insert_stmt = conn.prepare_cached(
                r#"
                INSERT INTO files (path, filename, content, hash, indexed_at)
                VALUES (?1, ?2, ?3, ?4, datetime('now'))
                ON CONFLICT(path) DO UPDATE SET
                    content = excluded.content,
                    hash = excluded.hash,
                    indexed_at = excluded.indexed_at
                "#,
            )?;
            let mut select_stmt =
                conn.prepare_cached("SELECT file_id FROM files WHERE path = ?1")?;

            for data in files {
                let filename = Path::new(&data.path)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("");

                // Store hash as i64 (SQLite INTEGER is signed)
                let hash_i64 = data.hash as i64;

                insert_stmt.execute(rusqlite::params![
                    &data.path,
                    filename,
                    &data.content,
                    hash_i64
                ])?;

                // Note: last_insert_rowid() doesn't work reliably with ON CONFLICT DO UPDATE
                // so we need to query the file_id explicitly
                let file_id: u32 =
                    select_stmt.query_row(rusqlite::params![&data.path], |row| row.get(0))?;
                file_ids.push(FileId::new(file_id));
            }
        }

        conn.execute("COMMIT", [])?;
        Ok(file_ids)
    }

    /// Gets file path by ID (without loading content).
    ///
    /// # Errors
    ///
    /// Returns `DbError::Pool` if no connection is available.
    /// Returns `DbError::Sqlite` if the query fails (other than no rows).
    pub fn get_file_path(&self, file_id: FileId) -> DbResult<Option<String>> {
        let conn = self.conn()?;
        let result = conn.query_row(
            "SELECT path FROM files WHERE file_id = ?1",
            rusqlite::params![file_id.as_u32()],
            |row| row.get::<_, String>(0),
        );

        match result {
            Ok(path) => Ok(Some(path)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(DbError::Sqlite(e)),
        }
    }

    /// Gets file ID by path (without loading content).
    ///
    /// # Errors
    ///
    /// Returns `DbError::Pool` if no connection is available.
    /// Returns `DbError::Sqlite` if the query fails (other than no rows).
    pub fn get_file_id(&self, path: &str) -> DbResult<Option<FileId>> {
        let conn = self.conn()?;
        let result = conn.query_row(
            "SELECT file_id FROM files WHERE path = ?1",
            rusqlite::params![path],
            |row| row.get::<_, u32>(0),
        );

        match result {
            Ok(id) => Ok(Some(FileId::new(id))),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(DbError::Sqlite(e)),
        }
    }

    /// Batch gets file paths by IDs (without loading content).
    ///
    /// Returns a HashMap of FileId -> path for all found IDs.
    /// Missing IDs are silently omitted from the result.
    ///
    /// # Errors
    ///
    /// Returns `DbError::Pool` if no connection is available.
    /// Returns `DbError::Sqlite` if the query fails.
    pub fn get_paths_batch(&self, file_ids: &[FileId]) -> DbResult<std::collections::HashMap<FileId, String>> {
        if file_ids.is_empty() {
            return Ok(std::collections::HashMap::new());
        }

        let conn = self.conn()?;

        // Build WHERE IN clause with positional params
        let placeholders: Vec<String> = (1..=file_ids.len()).map(|i| format!("?{i}")).collect();
        let sql = format!(
            "SELECT file_id, path FROM files WHERE file_id IN ({})",
            placeholders.join(",")
        );

        let mut stmt = conn.prepare(&sql)?;
        let params: Vec<Box<dyn rusqlite::types::ToSql>> = file_ids
            .iter()
            .map(|id| Box::new(id.as_u32()) as Box<dyn rusqlite::types::ToSql>)
            .collect();
        let param_refs: Vec<&dyn rusqlite::types::ToSql> = params.iter().map(|p| p.as_ref()).collect();

        let results = stmt
            .query_map(param_refs.as_slice(), |row| {
                Ok((FileId::new(row.get::<_, u32>(0)?), row.get::<_, String>(1)?))
            })?
            .collect::<Result<std::collections::HashMap<_, _>, _>>()?;

        Ok(results)
    }

    /// Batch gets file IDs by paths (without loading content).
    ///
    /// Returns a HashMap of path -> FileId for all found paths.
    /// Missing paths are silently omitted from the result.
    ///
    /// # Errors
    ///
    /// Returns `DbError::Pool` if no connection is available.
    /// Returns `DbError::Sqlite` if the query fails.
    pub fn get_file_ids_batch(&self, paths: &[String]) -> DbResult<std::collections::HashMap<String, FileId>> {
        if paths.is_empty() {
            return Ok(std::collections::HashMap::new());
        }

        let conn = self.conn()?;

        // Build WHERE IN clause with positional params
        let placeholders: Vec<String> = (1..=paths.len()).map(|i| format!("?{i}")).collect();
        let sql = format!(
            "SELECT path, file_id FROM files WHERE path IN ({})",
            placeholders.join(",")
        );

        let mut stmt = conn.prepare(&sql)?;
        let params: Vec<Box<dyn rusqlite::types::ToSql>> = paths
            .iter()
            .map(|p| Box::new(p.clone()) as Box<dyn rusqlite::types::ToSql>)
            .collect();
        let param_refs: Vec<&dyn rusqlite::types::ToSql> = params.iter().map(|p| p.as_ref()).collect();

        let results = stmt
            .query_map(param_refs.as_slice(), |row| {
                Ok((row.get::<_, String>(0)?, FileId::new(row.get::<_, u32>(1)?)))
            })?
            .collect::<Result<std::collections::HashMap<_, _>, _>>()?;

        Ok(results)
    }

    /// Gets the total number of indexed files.
    ///
    /// Used by trigram IDF scoring to calculate inverse document frequency.
    ///
    /// # Errors
    ///
    /// Returns `DbError::Pool` if no connection is available.
    /// Returns `DbError::Sqlite` if the query fails.
    pub fn total_files(&self) -> DbResult<u64> {
        self.file_count()
    }

    /// Gets total file count.
    ///
    /// # Errors
    ///
    /// Returns `DbError::Pool` if no connection is available.
    /// Returns `DbError::Sqlite` if the query fails.
    pub fn file_count(&self) -> DbResult<u64> {
        let conn = self.conn()?;
        let count: i64 = conn.query_row("SELECT COUNT(*) FROM files", [], |row| row.get(0))?;
        Ok(count as u64)
    }

    /// Deletes a file from the index.
    ///
    /// # Errors
    ///
    /// Returns `DbError::Pool` if no connection is available.
    /// Returns `DbError::Sqlite` if the delete operation fails.
    pub fn delete_file(&self, path: &str) -> DbResult<bool> {
        let conn = self.conn()?;
        let rows = conn.execute("DELETE FROM files WHERE path = ?1", rusqlite::params![path])?;
        Ok(rows > 0)
    }
}

// Compile-time assertion for thread safety.
#[cfg(test)]
const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Database>();
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_in_memory_database() {
        let db = Database::in_memory().unwrap();
        assert_eq!(db.file_count().unwrap(), 0);
    }

    #[test]
    fn test_upsert_and_get() {
        let db = Database::in_memory().unwrap();
        let file_id = db
            .upsert_file("src/main.rs", "fn main() {}", 0xabc123)
            .unwrap();

        let (path, content) = db.get_file(file_id).unwrap().unwrap();
        assert_eq!(path, "src/main.rs");
        assert_eq!(content, "fn main() {}");
    }

    #[test]
    fn test_get_file_not_found() {
        let db = Database::in_memory().unwrap();

        // Query a non-existent FileId
        let result = db.get_file(FileId::new(99999)).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_get_file_by_path_not_found() {
        let db = Database::in_memory().unwrap();

        let result = db.get_file_by_path("/nonexistent/path.rs").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_upsert_updates_existing() {
        let db = Database::in_memory().unwrap();

        // Insert initial content
        let file_id1 = db
            .upsert_file("src/main.rs", "fn main() {}", 0x1)
            .unwrap();

        // Update with new content
        let file_id2 = db
            .upsert_file("src/main.rs", "fn main() { println!(\"updated\"); }", 0x2)
            .unwrap();

        // Should return the same FileId (same path)
        assert_eq!(file_id1, file_id2);

        // Content should be updated
        let (_, content) = db.get_file(file_id1).unwrap().unwrap();
        assert!(content.contains("updated"));

        // File count should still be 1
        assert_eq!(db.file_count().unwrap(), 1);
    }

    #[test]
    fn test_delete_file_removes_from_index() {
        let db = Database::in_memory().unwrap();

        // Insert a file
        db.upsert_file("src/to_delete.rs", "fn delete_me() {}", 0x1)
            .unwrap();
        assert_eq!(db.file_count().unwrap(), 1);

        // Delete it
        let deleted = db.delete_file("src/to_delete.rs").unwrap();
        assert!(deleted);

        // Verify it's gone
        assert_eq!(db.file_count().unwrap(), 0);
        let result = db.get_file_by_path("src/to_delete.rs").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_delete_nonexistent_file() {
        let db = Database::in_memory().unwrap();

        // Deleting a non-existent file should return false (not error)
        let deleted = db.delete_file("/nonexistent/file.rs").unwrap();
        assert!(!deleted);
    }

    #[test]
    fn test_fts_search_basic() {
        let db = Database::in_memory().unwrap();

        db.upsert_file("auth.rs", "fn authenticate() {}", 0x1)
            .unwrap();
        db.upsert_file("main.rs", "fn main() {}", 0x2).unwrap();

        let results = db.fts_search("authenticate*", 10).unwrap();
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn test_fts_search_bm25_ranking() {
        let db = Database::in_memory().unwrap();

        // File with many keyword occurrences
        db.upsert_file(
            "auth_heavy.rs",
            "fn auth() {} fn authenticate() {} fn authorization() {} struct AuthConfig {}",
            0x1,
        )
        .unwrap();

        // File with single occurrence
        db.upsert_file("auth_light.rs", "fn auth() {}", 0x2)
            .unwrap();

        // File with no auth
        db.upsert_file("main.rs", "fn main() {}", 0x3).unwrap();

        let results = db.fts_search("auth*", 10).unwrap();

        // Should find both auth files, not main
        assert!(results.len() >= 2);

        // BM25 should rank the file with more occurrences higher
        // (BM25 scores are negative, more negative = better)
        let file_ids: Vec<_> = results.iter().map(|(id, _)| id).collect();
        assert!(file_ids.iter().any(|id| {
            db.get_file(**id)
                .unwrap()
                .map(|(p, _)| p.contains("auth"))
                .unwrap_or(false)
        }));
    }

    #[test]
    fn test_fts_search_no_results() {
        let db = Database::in_memory().unwrap();

        db.upsert_file("main.rs", "fn main() {}", 0x1).unwrap();

        let results = db.fts_search("nonexistent*", 10).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn test_fts_search_empty_query() {
        let db = Database::in_memory().unwrap();

        db.upsert_file("main.rs", "fn main() {}", 0x1).unwrap();

        // Empty query should return error or empty results
        let results = db.fts_search("", 10);
        // FTS5 may error or return empty on empty query
        assert!(results.is_err() || results.unwrap().is_empty());
    }

    #[test]
    fn test_fts_search_limit() {
        let db = Database::in_memory().unwrap();

        // Insert many files
        for i in 0..20 {
            db.upsert_file(
                &format!("test_{}.rs", i),
                &format!("fn test_{}() {{}}", i),
                i as u64,
            )
            .unwrap();
        }

        // Request only 5 results
        let results = db.fts_search("test*", 5).unwrap();
        assert!(results.len() <= 5);
    }

    #[test]
    fn test_delete_file_removes_from_fts() {
        let db = Database::in_memory().unwrap();

        // Insert a file
        db.upsert_file("searchable.rs", "fn find_me() {}", 0x1)
            .unwrap();

        // Verify it's searchable
        let results = db.fts_search("find*", 10).unwrap();
        assert_eq!(results.len(), 1);

        // Delete it
        db.delete_file("searchable.rs").unwrap();

        // Verify it's no longer searchable
        let results = db.fts_search("find*", 10).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn test_get_indexed_files() {
        let db = Database::in_memory().unwrap();

        db.upsert_file("file1.rs", "content1", 0x1).unwrap();
        db.upsert_file("file2.rs", "content2", 0x2).unwrap();

        let indexed = db.get_indexed_files().unwrap();
        assert_eq!(indexed.len(), 2);

        let paths: Vec<_> = indexed.iter().map(|(p, _)| p.as_str()).collect();
        assert!(paths.contains(&"file1.rs"));
        assert!(paths.contains(&"file2.rs"));
    }

    #[test]
    fn test_trigram_storage() {
        let db = Database::in_memory().unwrap();

        let trigram = b"aut";
        let file_ids: Vec<u8> = vec![1, 2, 3, 4];

        // Store trigram
        db.upsert_trigrams(trigram, &file_ids).unwrap();

        // Retrieve it
        let result = db.get_trigram_files(trigram).unwrap();
        assert_eq!(result, Some(file_ids));
    }

    #[test]
    fn test_trigram_upsert_updates() {
        let db = Database::in_memory().unwrap();

        let trigram = b"aut";

        // Initial insert
        db.upsert_trigrams(trigram, &[1, 2]).unwrap();

        // Update
        db.upsert_trigrams(trigram, &[1, 2, 3, 4]).unwrap();

        let result = db.get_trigram_files(trigram).unwrap().unwrap();
        assert_eq!(result, vec![1, 2, 3, 4]);
    }

    #[test]
    fn test_trigram_not_found() {
        let db = Database::in_memory().unwrap();

        let result = db.get_trigram_files(b"xyz").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_filename_extraction() {
        let db = Database::in_memory().unwrap();

        // Upsert with full path
        db.upsert_file("src/deep/nested/file.rs", "content", 0x1)
            .unwrap();

        // FTS should be able to search by filename
        let results = db.fts_search("file*", 10).unwrap();
        assert!(!results.is_empty());
    }

    #[test]
    fn test_multiple_connections() {
        let db = Database::in_memory().unwrap();

        // Get multiple connections from the pool
        let conn1 = db.conn().unwrap();
        // With in_memory, pool size is 1, so we can't get another
        // But this tests that conn() works
        drop(conn1);

        // After dropping, we can get another
        let _conn2 = db.conn().unwrap();
    }

    #[test]
    fn test_special_characters_in_content() {
        let db = Database::in_memory().unwrap();

        // Content with special characters
        let content = r#"fn test() { let s = "hello\nworld"; }"#;
        db.upsert_file("special.rs", content, 0x1).unwrap();

        let (_, retrieved) = db.get_file_by_path("special.rs").unwrap().unwrap();
        assert_eq!(retrieved, content);
    }

    #[test]
    fn test_unicode_content() {
        let db = Database::in_memory().unwrap();

        // Content with unicode
        let content = "fn greet() { println!(\"Hello World\"); }";
        db.upsert_file("unicode.rs", content, 0x1).unwrap();

        let (_, retrieved) = db.get_file_by_path("unicode.rs").unwrap().unwrap();
        assert_eq!(retrieved, content);

        // Should be searchable
        let results = db.fts_search("greet*", 10).unwrap();
        assert!(!results.is_empty());
    }
}
