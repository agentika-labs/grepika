//! Concurrent access tests for thread safety verification.
//!
//! Tests that the search service and database handle concurrent
//! access correctly without panics or data corruption.

mod common;

use grepika::db::Database;
use grepika::services::{Indexer, SearchService, TrigramIndex};
use grepika::types::FileId;
use std::fs;
use std::sync::{Arc, RwLock};
use std::thread;
use tempfile::TempDir;

/// Sets up a test environment with multiple files.
fn setup_concurrent_env() -> (TempDir, Arc<Database>, Arc<SearchService>) {
    let dir = TempDir::new().unwrap();
    let db = Arc::new(Database::in_memory().unwrap());

    // Create and index multiple test files
    for i in 0..10 {
        let filename = format!("file_{}.rs", i);
        let content = format!(
            r#"
// File {i}
fn function_{i}() {{
    println!("Hello from file {i}");
}}

struct Struct{i} {{
    field: i32,
}}
"#,
            i = i
        );
        fs::write(dir.path().join(&filename), &content).unwrap();
        db.upsert_file(
            dir.path().join(&filename).to_string_lossy().as_ref(),
            &content,
            i as u64,
        )
        .unwrap();
    }

    let search = Arc::new(SearchService::new(Arc::clone(&db), dir.path().to_path_buf()).unwrap());

    (dir, db, search)
}

// ============================================================================
// Concurrent Search Tests
// ============================================================================

#[test]
fn test_concurrent_searches() {
    let (_dir, _db, search) = setup_concurrent_env();

    let handles: Vec<_> = (0..8)
        .map(|i| {
            let search = Arc::clone(&search);
            thread::spawn(move || {
                // Each thread performs multiple searches
                for _ in 0..10 {
                    let query = format!("function_{}", i % 10);
                    let results = search.search(&query, 20);
                    assert!(results.is_ok(), "Search should not panic");
                }
            })
        })
        .collect();

    for handle in handles {
        handle.join().expect("Thread should not panic");
    }
}

#[test]
fn test_concurrent_fts_searches() {
    let (_dir, _db, search) = setup_concurrent_env();

    let handles: Vec<_> = (0..4)
        .map(|i| {
            let search = Arc::clone(&search);
            thread::spawn(move || {
                for j in 0..20 {
                    let query = format!("file_{}", (i + j) % 10);
                    let results = search.search_fts(&query, 10);
                    assert!(results.is_ok(), "FTS search should not panic");
                }
            })
        })
        .collect();

    for handle in handles {
        handle.join().expect("Thread should not panic");
    }
}

#[test]
fn test_concurrent_grep_searches() {
    let (_dir, _db, search) = setup_concurrent_env();

    let handles: Vec<_> = (0..4)
        .map(|i| {
            let search = Arc::clone(&search);
            thread::spawn(move || {
                for j in 0..10 {
                    let query = format!("Struct{}", (i + j) % 10);
                    let results = search.search_grep(&query, 10);
                    assert!(results.is_ok(), "Grep search should not panic");
                }
            })
        })
        .collect();

    for handle in handles {
        handle.join().expect("Thread should not panic");
    }
}

#[test]
fn test_concurrent_mixed_operations() {
    let (_dir, _db, search) = setup_concurrent_env();

    // Mix of different search types
    let handles: Vec<_> = (0..6)
        .map(|i| {
            let search = Arc::clone(&search);
            thread::spawn(move || {
                for j in 0..10 {
                    match i % 3 {
                        0 => {
                            let _ = search.search(&format!("function_{}", j % 10), 10);
                        }
                        1 => {
                            let _ = search.search_fts(&format!("file_{}", j % 10), 10);
                        }
                        _ => {
                            let _ = search.search_grep(&format!("Struct{}", j % 10), 10);
                        }
                    }
                }
            })
        })
        .collect();

    for handle in handles {
        handle.join().expect("Thread should not panic");
    }
}

// ============================================================================
// Concurrent Database Access Tests
// ============================================================================

#[test]
fn test_concurrent_database_reads() {
    let db = Arc::new(Database::in_memory().unwrap());

    // Insert initial data
    for i in 0..10 {
        db.upsert_file(&format!("file_{}.rs", i), "content", i as u64)
            .unwrap();
    }

    let handles: Vec<_> = (0..8)
        .map(|_| {
            let db = Arc::clone(&db);
            thread::spawn(move || {
                for _ in 0..100 {
                    let _ = db.file_count();
                    let _ = db.get_indexed_files();
                    let _ = db.fts_search("content*", 10);
                }
            })
        })
        .collect();

    for handle in handles {
        handle.join().expect("Thread should not panic");
    }
}

#[test]
fn test_concurrent_database_file_lookups() {
    let db = Arc::new(Database::in_memory().unwrap());

    // Insert files
    let mut file_ids = Vec::new();
    for i in 0..10 {
        let id = db
            .upsert_file(&format!("file_{}.rs", i), "content", i as u64)
            .unwrap();
        file_ids.push(id);
    }

    let handles: Vec<_> = (0..8)
        .map(|thread_i| {
            let db = Arc::clone(&db);
            let ids = file_ids.clone();
            thread::spawn(move || {
                for i in 0..50 {
                    let id = ids[(thread_i + i) % ids.len()];
                    let result = db.get_file(id);
                    assert!(result.is_ok());
                }
            })
        })
        .collect();

    for handle in handles {
        handle.join().expect("Thread should not panic");
    }
}

// ============================================================================
// Concurrent Trigram Index Access Tests
// ============================================================================

#[test]
fn test_concurrent_trigram_reads() {
    let trigram = Arc::new(RwLock::new(TrigramIndex::new()));

    // Populate the index
    {
        let mut index = trigram.write().unwrap();
        for i in 0..100 {
            index.add_file(
                FileId::new(i),
                &format!("content_{} authentication authorization", i),
            );
        }
    }

    // Concurrent reads
    let handles: Vec<_> = (0..8)
        .map(|_| {
            let trigram = Arc::clone(&trigram);
            thread::spawn(move || {
                for _ in 0..100 {
                    let index = trigram.read().unwrap();
                    let _ = index.search("auth");
                    let _ = index.search("content");
                    let _ = index.trigram_count();
                }
            })
        })
        .collect();

    for handle in handles {
        handle.join().expect("Thread should not panic");
    }
}

#[test]
fn test_concurrent_trigram_read_write() {
    let trigram = Arc::new(RwLock::new(TrigramIndex::new()));

    // Initial population
    {
        let mut index = trigram.write().unwrap();
        for i in 0..50 {
            index.add_file(FileId::new(i), "initial content");
        }
    }

    // Reader threads
    let reader_handles: Vec<_> = (0..4)
        .map(|_| {
            let trigram = Arc::clone(&trigram);
            thread::spawn(move || {
                for _ in 0..50 {
                    let index = trigram.read().unwrap();
                    let _ = index.search("content");
                    drop(index);
                    thread::yield_now();
                }
            })
        })
        .collect();

    // Writer thread
    let writer_trigram = Arc::clone(&trigram);
    let writer_handle = thread::spawn(move || {
        for i in 50..100 {
            let mut index = writer_trigram.write().unwrap();
            index.add_file(FileId::new(i), &format!("new content {}", i));
            drop(index);
            thread::yield_now();
        }
    });

    for handle in reader_handles {
        handle.join().expect("Reader thread should not panic");
    }
    writer_handle
        .join()
        .expect("Writer thread should not panic");

    // Verify final state
    let index = trigram.read().unwrap();
    assert!(index.trigram_count() > 0);
}

// ============================================================================
// Read During Index Tests
// ============================================================================

#[test]
fn test_search_during_indexing() {
    let dir = TempDir::new().unwrap();
    let db = Arc::new(Database::in_memory().unwrap());
    let trigram = Arc::new(RwLock::new(TrigramIndex::new()));

    // Create some initial files
    for i in 0..5 {
        let filename = format!("initial_{}.rs", i);
        let content = format!("fn initial_{}() {{}}", i);
        fs::write(dir.path().join(&filename), &content).unwrap();
        db.upsert_file(
            dir.path().join(&filename).to_string_lossy().as_ref(),
            &content,
            i as u64,
        )
        .unwrap();
    }

    let search = Arc::new(SearchService::new(Arc::clone(&db), dir.path().to_path_buf()).unwrap());

    // Create additional files for indexing
    for i in 5..20 {
        let filename = format!("new_{}.rs", i);
        let content = format!("fn new_function_{}() {{}}", i);
        fs::write(dir.path().join(&filename), &content).unwrap();
    }

    // Start indexing in a separate thread
    let indexer_db = Arc::clone(&db);
    let indexer_trigram = Arc::clone(&trigram);
    let indexer_dir = dir.path().to_path_buf();
    let indexer_handle = thread::spawn(move || {
        let indexer = Indexer::new(indexer_db, indexer_trigram, indexer_dir);
        indexer.index(None, false).unwrap()
    });

    // Perform searches while indexing
    for _ in 0..20 {
        let _ = search.search("initial", 10);
        let _ = search.search("function", 10);
        thread::yield_now();
    }

    indexer_handle.join().expect("Indexer should complete");
}

// ============================================================================
// Lock Poisoning Recovery Tests
// ============================================================================

#[test]
fn test_rwlock_poisoning_recovery() {
    let trigram = Arc::new(RwLock::new(TrigramIndex::new()));

    // Add initial data
    {
        let mut index = trigram.write().unwrap();
        index.add_file(FileId::new(1), "test content");
    }

    // Poison the lock by panicking while holding a write guard
    let trigram_clone = Arc::clone(&trigram);
    let handle = thread::spawn(move || {
        let _guard = trigram_clone.write().unwrap();
        panic!("intentional panic to poison the lock");
    });
    let _ = handle.join(); // Thread panicked, lock is now poisoned

    // Verify the lock is actually poisoned
    assert!(trigram.read().is_err(), "Lock should be poisoned");

    // Verify recovery via unwrap_or_else(|e| e.into_inner()) — the pattern used in server.rs
    let index = trigram.read().unwrap_or_else(|e| e.into_inner());
    assert!(
        index.trigram_count() > 0,
        "Data should be intact after poisoning recovery"
    );
}

// ============================================================================
// Stress Tests
// ============================================================================

#[test]
fn test_high_concurrency_stress() {
    let (_dir, _db, search) = setup_concurrent_env();

    // Many threads with many operations
    let handles: Vec<_> = (0..16)
        .map(|i| {
            let search = Arc::clone(&search);
            thread::spawn(move || {
                for j in 0..50 {
                    let query = format!("function_{}", (i * j) % 10);
                    let _ = search.search(&query, 5);
                }
            })
        })
        .collect();

    for handle in handles {
        handle.join().expect("Thread should not panic under stress");
    }
}

#[test]
fn test_database_connection_pool_under_load() {
    let db = Arc::new(Database::in_memory().unwrap());

    // Insert test data
    for i in 0..100 {
        db.upsert_file(
            &format!("stress_file_{}.rs", i),
            &format!("fn stress_{}() {{}}", i),
            i as u64,
        )
        .unwrap();
    }

    // Hammer the connection pool
    let handles: Vec<_> = (0..20)
        .map(|_| {
            let db = Arc::clone(&db);
            thread::spawn(move || {
                for i in 0..100 {
                    // Mix of read operations
                    let _ = db.file_count();
                    let _ = db.fts_search(&format!("stress_{}*", i % 100), 10);
                    let _ = db.get_file_by_path(&format!("stress_file_{}.rs", i % 100));
                }
            })
        })
        .collect();

    for handle in handles {
        handle
            .join()
            .expect("Thread should not fail under pool stress");
    }
}

// ============================================================================
// Concurrent Tool Execution Tests
// ============================================================================

#[test]
fn test_concurrent_tool_execution() {
    use grepika::tools::*;

    let dir = TempDir::new().unwrap();
    let db = Arc::new(Database::in_memory().unwrap());
    let _trigram = Arc::new(RwLock::new(TrigramIndex::new()));

    // Create and index files
    for i in 0..5 {
        let filename = format!("tool_test_{}.rs", i);
        let content = format!("pub fn tool_function_{}() {{}}", i);
        fs::write(dir.path().join(&filename), &content).unwrap();
        db.upsert_file(
            dir.path().join(&filename).to_string_lossy().as_ref(),
            &content,
            i as u64,
        )
        .unwrap();
    }

    let search = Arc::new(SearchService::new(Arc::clone(&db), dir.path().to_path_buf()).unwrap());

    // Execute multiple tools concurrently
    let handles: Vec<_> = (0..8)
        .map(|i| {
            let search = Arc::clone(&search);
            thread::spawn(move || {
                for _ in 0..10 {
                    match i % 4 {
                        0 => {
                            let input = SearchInput {
                                query: "function".to_string(),
                                limit: 10,
                                mode: SearchMode::Combined,
                            };
                            let _ = execute_search(&search, input);
                        }
                        1 => {
                            let input = SearchInput {
                                query: "tool".to_string(),
                                limit: 5,
                                mode: SearchMode::Fts,
                            };
                            let _ = execute_search(&search, input);
                        }
                        2 => {
                            let input = TocInput {
                                path: ".".to_string(),
                                depth: 2,
                            };
                            let _ = execute_toc(&search, input);
                        }
                        _ => {
                            let input = GetInput {
                                path: "tool_test_0.rs".to_string(),
                                start_line: 1,
                                end_line: 10,
                            };
                            let _ = execute_get(&search, input);
                        }
                    }
                }
            })
        })
        .collect();

    for handle in handles {
        handle.join().expect("Tool execution should not panic");
    }
}
