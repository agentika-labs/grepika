//! `SQLite` PRAGMA configuration for optimal performance.

use crate::error::DbResult;
use rusqlite::Connection;

/// Executes a single SQL statement that may return rows (PRAGMAs, FTS5 commands).
fn exec_stmt(conn: &Connection, sql: &str) -> rusqlite::Result<()> {
    conn.prepare(sql)?.query([])?.next()?;
    Ok(())
}

/// Applies performance-tuned PRAGMA settings (raw rusqlite version).
///
/// Used by `PragmaCustomizer` to apply pragmas on every pool connection.
/// Returns raw `rusqlite::Result` for compatibility with r2d2's error types.
pub fn apply_pragmas_raw(conn: &Connection) -> rusqlite::Result<()> {
    // WAL mode enables concurrent readers during writes
    exec_stmt(conn, "PRAGMA journal_mode = WAL")?;
    // Synchronous NORMAL is safe with WAL, faster than FULL
    exec_stmt(conn, "PRAGMA synchronous = NORMAL")?;
    // 8MB page cache (2000 pages * 4KB default page size)
    exec_stmt(conn, "PRAGMA cache_size = -8000")?;
    // 64MB memory-mapped I/O for faster reads
    exec_stmt(conn, "PRAGMA mmap_size = 67108864")?;
    // 5 second busy timeout for lock contention
    exec_stmt(conn, "PRAGMA busy_timeout = 5000")?;
    // Enable foreign key constraints
    exec_stmt(conn, "PRAGMA foreign_keys = ON")?;
    // Temp tables in memory
    exec_stmt(conn, "PRAGMA temp_store = MEMORY")?;

    Ok(())
}

/// Applies performance-tuned PRAGMA settings.
///
/// These settings optimize for:
/// - Concurrent reads (WAL mode)
/// - Large working sets (8MB cache, 64MB mmap)
/// - Reliability (foreign keys, busy timeout)
///
/// # Errors
///
/// Returns `DbError::Sqlite` if any PRAGMA statement fails.
pub fn apply_pragmas(conn: &Connection) -> DbResult<()> {
    apply_pragmas_raw(conn)?;
    Ok(())
}

/// Applies pragmas optimized for bulk indexing writes.
///
/// The index is derived data (rebuildable from source files), so we trade
/// crash durability for write throughput:
/// - `synchronous=OFF`: skip fsync — ~30-50% faster writes
/// - `wal_autocheckpoint=0`: defer WAL checkpointing until after bulk writes
///
/// Also disables FTS5 automerge to batch segment merges.
///
/// Must be paired with `restore_normal_pragmas()` after indexing completes.
pub fn apply_indexing_pragmas(conn: &Connection) -> rusqlite::Result<()> {
    exec_stmt(conn, "PRAGMA synchronous = OFF")?;
    exec_stmt(conn, "PRAGMA wal_autocheckpoint = 0")?;
    // Disable FTS5 automerge during bulk inserts
    exec_stmt(
        conn,
        "INSERT INTO files_fts(files_fts, rank) VALUES('automerge', 0)",
    )?;

    Ok(())
}

/// Restores normal pragmas after indexing completes.
///
/// Re-enables crash safety and triggers an FTS5 crisis merge to
/// consolidate deferred segment merges.
pub fn restore_normal_pragmas(conn: &Connection) -> rusqlite::Result<()> {
    // FIRST: restore crash safety (non-negotiable).
    // synchronous=OFF leaks to pool if not restored. Must run before
    // FTS5 housekeeping which can fail on disk full or corruption.
    exec_stmt(conn, "PRAGMA synchronous = NORMAL")?;
    exec_stmt(conn, "PRAGMA wal_autocheckpoint = 1000")?;

    // THEN: FTS5 housekeeping (failure here doesn't compromise safety).
    // Optimize: consolidate all FTS5 segments deferred during indexing
    // into a single b-tree. Equivalent to a full merge.
    exec_stmt(conn, "INSERT INTO files_fts(files_fts) VALUES('optimize')")?;
    exec_stmt(
        conn,
        "INSERT INTO files_fts(files_fts, rank) VALUES('automerge', 8)",
    )?;

    // ANALYZE: update SQLite's query planner statistics so it picks
    // optimal index strategies for subsequent queries.
    conn.execute("ANALYZE", [])?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pragmas_apply() {
        let conn = Connection::open_in_memory().unwrap();
        apply_pragmas(&conn).unwrap();

        let journal_mode: String = conn
            .query_row("PRAGMA journal_mode", [], |row| row.get(0))
            .unwrap();
        // In-memory databases use "memory" journal mode instead of WAL
        // WAL requires a file on disk
        assert!(journal_mode.to_lowercase() == "wal" || journal_mode.to_lowercase() == "memory");
    }

    #[test]
    fn test_indexing_pragmas() {
        let db = crate::db::Database::in_memory().unwrap();
        let conn = db.conn().unwrap();
        apply_indexing_pragmas(&conn).unwrap();
        restore_normal_pragmas(&conn).unwrap();
    }
}
