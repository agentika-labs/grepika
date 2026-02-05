//! `SQLite` PRAGMA configuration for optimal performance.

use crate::error::DbResult;
use rusqlite::Connection;

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
    // Use prepare + step pattern which handles both void and result-returning statements

    // WAL mode enables concurrent readers during writes
    conn.prepare("PRAGMA journal_mode = WAL")?
        .query([])?
        .next()?;

    // Synchronous NORMAL is safe with WAL, faster than FULL
    conn.prepare("PRAGMA synchronous = NORMAL")?
        .query([])?
        .next()?;

    // 8MB page cache (2000 pages * 4KB default page size)
    conn.prepare("PRAGMA cache_size = -8000")?
        .query([])?
        .next()?;

    // 64MB memory-mapped I/O for faster reads
    conn.prepare("PRAGMA mmap_size = 67108864")?
        .query([])?
        .next()?;

    // 5 second busy timeout for lock contention
    conn.prepare("PRAGMA busy_timeout = 5000")?
        .query([])?
        .next()?;

    // Enable foreign key constraints
    conn.prepare("PRAGMA foreign_keys = ON")?
        .query([])?
        .next()?;

    // Temp tables in memory
    conn.prepare("PRAGMA temp_store = MEMORY")?
        .query([])?
        .next()?;

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
}
