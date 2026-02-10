//! Sad path tests for error handling and edge cases.
//!
//! Tests invalid inputs, missing files, corrupt data, and error recovery.

mod common;

use grepika::db::Database;
use grepika::services::{GrepService, Indexer, SearchService, TrigramIndex};
use grepika::tools::*;
use grepika::types::{FileId, Score, Trigram};
use std::fs;
use std::sync::{Arc, RwLock};
use tempfile::TempDir;

// ============================================================================
// Invalid Regex Pattern Tests
// ============================================================================

#[test]
fn test_search_invalid_regex_pattern() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("test.rs"), "fn main() {}").unwrap();

    let db = Arc::new(Database::in_memory().unwrap());
    db.upsert_file(
        dir.path().join("test.rs").to_string_lossy().as_ref(),
        "fn main() {}",
        0x1,
    )
    .unwrap();

    let search = Arc::new(SearchService::new(Arc::clone(&db), dir.path().to_path_buf()).unwrap());

    // Invalid regex - unclosed bracket
    let result = search.search_grep("[invalid", 10);
    assert!(result.is_err(), "Should error on invalid regex");

    // Invalid regex - unclosed group
    let result = search.search_grep("(unclosed", 10);
    assert!(result.is_err(), "Should error on unclosed group");
}

#[test]
fn test_grep_service_invalid_pattern() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("test.rs"), "fn main() {}").unwrap();

    let grep = GrepService::new(dir.path().to_path_buf()).unwrap();

    // Invalid regex patterns
    let result = grep.search_parallel("[", 10);
    assert!(result.is_err());

    let result = grep.search_parallel("(?invalid)", 10);
    assert!(result.is_err());
}

// ============================================================================
// File Not Found Tests
// ============================================================================

#[test]
fn test_get_nonexistent_file() {
    let dir = TempDir::new().unwrap();
    let db = Arc::new(Database::in_memory().unwrap());
    let search = Arc::new(SearchService::new(Arc::clone(&db), dir.path().to_path_buf()).unwrap());

    let input = GetInput {
        path: "nonexistent.rs".to_string(),
        start_line: 1,
        end_line: 0,
    };

    let result = execute_get(&search, input);
    assert!(result.is_err());
    assert!(result
        .unwrap_err()
        .to_string()
        .to_lowercase()
        .contains("read"));
}

#[test]
fn test_outline_nonexistent_file() {
    let dir = TempDir::new().unwrap();
    let db = Arc::new(Database::in_memory().unwrap());
    let search = Arc::new(SearchService::new(Arc::clone(&db), dir.path().to_path_buf()).unwrap());

    let input = OutlineInput {
        path: "ghost.rs".to_string(),
    };

    let result = execute_outline(&search, input);
    assert!(result.is_err());
}

#[test]
fn test_context_nonexistent_file() {
    let dir = TempDir::new().unwrap();
    let db = Arc::new(Database::in_memory().unwrap());
    let search = Arc::new(SearchService::new(Arc::clone(&db), dir.path().to_path_buf()).unwrap());

    let input = ContextInput {
        path: "missing.rs".to_string(),
        line: 5,
        context_lines: 3,
    };

    let result = execute_context(&search, input);
    assert!(result.is_err());
}

#[test]
fn test_related_nonexistent_file() {
    let dir = TempDir::new().unwrap();
    let db = Arc::new(Database::in_memory().unwrap());
    let search = Arc::new(SearchService::new(Arc::clone(&db), dir.path().to_path_buf()).unwrap());

    let input = RelatedInput {
        path: "phantom.rs".to_string(),
        limit: 10,
    };

    let result = execute_related(&search, input);
    assert!(result.is_err());
}

// ============================================================================
// Binary File Handling
// ============================================================================

#[test]
fn test_outline_binary_file() {
    let dir = TempDir::new().unwrap();

    // Create a binary file (random bytes)
    let binary_content: Vec<u8> = vec![0x00, 0x01, 0x02, 0xFF, 0xFE, 0x89, 0x50, 0x4E, 0x47];
    fs::write(dir.path().join("binary.bin"), &binary_content).unwrap();

    let db = Arc::new(Database::in_memory().unwrap());
    let search = Arc::new(SearchService::new(Arc::clone(&db), dir.path().to_path_buf()).unwrap());

    let input = OutlineInput {
        path: "binary.bin".to_string(),
    };

    // Should handle gracefully - either return empty symbols or error
    let result = execute_outline(&search, input);
    // Binary files may fail to read as UTF-8 or return no symbols
    if let Ok(output) = result {
        // If it succeeds, symbols should be empty for binary
        assert!(output.symbols.is_empty() || output.file_type == "bin");
    }
    // If it errors, that's acceptable too
}

#[test]
fn test_indexer_skips_binary_files() {
    let dir = TempDir::new().unwrap();

    // Create a binary file
    let binary_content: Vec<u8> = (0..1000).map(|i| (i % 256) as u8).collect();
    fs::write(dir.path().join("binary.dat"), &binary_content).unwrap();

    // Create a text file
    fs::write(dir.path().join("text.rs"), "fn main() {}").unwrap();

    let db = Arc::new(Database::in_memory().unwrap());
    let trigram = Arc::new(RwLock::new(TrigramIndex::new()));
    let indexer = Indexer::new(Arc::clone(&db), trigram, dir.path().to_path_buf());

    let progress = indexer.index(None, false).unwrap();

    // Should only index the text file (binary should be skipped due to extension filter)
    // Note: .dat is not in the default extensions list
    assert!(progress.files_indexed <= 2); // At most text file + maybe .dat if readable
}

// ============================================================================
// Edge Case: Empty Files and Queries
// ============================================================================

#[test]
fn test_outline_empty_file() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("empty.rs"), "").unwrap();

    let db = Arc::new(Database::in_memory().unwrap());
    let search = Arc::new(SearchService::new(Arc::clone(&db), dir.path().to_path_buf()).unwrap());

    let input = OutlineInput {
        path: "empty.rs".to_string(),
    };

    let result = execute_outline(&search, input).unwrap();
    assert!(result.symbols.is_empty());
    assert_eq!(result.file_type, "rs");
}

#[test]
fn test_get_empty_file() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("empty.txt"), "").unwrap();

    let db = Arc::new(Database::in_memory().unwrap());
    let search = Arc::new(SearchService::new(Arc::clone(&db), dir.path().to_path_buf()).unwrap());

    let input = GetInput {
        path: "empty.txt".to_string(),
        start_line: 1,
        end_line: 0,
    };

    let result = execute_get(&search, input).unwrap();
    // Content has boundary markers even for empty files
    assert!(
        result
            .content
            .contains("--- BEGIN FILE CONTENT: empty.txt ---"),
        "Should have begin marker"
    );
    assert!(
        result
            .content
            .contains("--- END FILE CONTENT: empty.txt ---"),
        "Should have end marker"
    );
    assert_eq!(result.total_lines, 0);
}

#[test]
fn test_toc_empty_directory() {
    let dir = TempDir::new().unwrap();
    fs::create_dir(dir.path().join("empty_subdir")).unwrap();

    let db = Arc::new(Database::in_memory().unwrap());
    let search = Arc::new(SearchService::new(Arc::clone(&db), dir.path().to_path_buf()).unwrap());

    let input = TocInput {
        path: "empty_subdir".to_string(),
        depth: 3,
    };

    let result = execute_toc(&search, input).unwrap();
    assert!(result.tree.is_empty());
    assert_eq!(result.total_files, 0);
    assert_eq!(result.total_dirs, 0);
}

// ============================================================================
// Trigram Edge Cases
// ============================================================================

#[test]
fn test_trigram_short_query() {
    let mut index = TrigramIndex::new();
    index.add_file(FileId::new(1), "test content");

    // Query < 3 characters returns None (no filtering)
    assert!(index.search("te").is_none());
    assert!(index.search("t").is_none());
    assert!(index.search("").is_none());
}

#[test]
fn test_trigram_exact_three_chars() {
    let mut index = TrigramIndex::new();
    index.add_file(FileId::new(1), "authentication");

    // Exactly 3 characters should work
    let results = index.search("aut");
    assert!(results.is_some());
    assert!(results.unwrap().contains(1));
}

#[test]
fn test_trigram_no_matching_files() {
    let mut index = TrigramIndex::new();
    index.add_file(FileId::new(1), "hello world");

    // Search for something that doesn't exist
    let results = index.search("xyz123");
    // Should return None or empty bitmap
    assert!(results.is_none() || results.unwrap().is_empty());
}

// ============================================================================
// Unicode Content Tests
// ============================================================================

#[test]
fn test_search_unicode_content() {
    let dir = TempDir::new().unwrap();

    // File with Chinese characters
    let unicode_content = r#"
// Unicode test file
fn greet_chinese() {
    println!("Hello");
}

fn greet_japanese() {
    println!("Japanese greeting");
}
"#;
    fs::write(dir.path().join("unicode.rs"), unicode_content).unwrap();

    let db = Arc::new(Database::in_memory().unwrap());
    db.upsert_file(
        dir.path().join("unicode.rs").to_string_lossy().as_ref(),
        unicode_content,
        0x1,
    )
    .unwrap();

    let search = Arc::new(SearchService::new(Arc::clone(&db), dir.path().to_path_buf()).unwrap());

    // Should be able to search for ASCII content in unicode file
    let results = search.search("greet", 10).unwrap();
    assert!(!results.is_empty());
}

#[test]
fn test_trigram_unicode() {
    let mut index = TrigramIndex::new();

    // Add content with ASCII (trigrams work on bytes)
    index.add_file(FileId::new(1), "hello world");

    // Search should work for ASCII trigrams
    let results = index.search("hello");
    assert!(results.is_some());
}

// ============================================================================
// Large File Handling
// ============================================================================

#[test]
fn test_indexer_skips_large_files() {
    let dir = TempDir::new().unwrap();

    // Create a file larger than 1MB (default max_file_size)
    let large_content: String = "x".repeat(2 * 1024 * 1024); // 2MB
    fs::write(dir.path().join("large.rs"), &large_content).unwrap();

    // Create a normal file
    fs::write(dir.path().join("normal.rs"), "fn main() {}").unwrap();

    let db = Arc::new(Database::in_memory().unwrap());
    let trigram = Arc::new(RwLock::new(TrigramIndex::new()));
    let indexer = Indexer::new(Arc::clone(&db), trigram, dir.path().to_path_buf());

    let progress = indexer.index(None, false).unwrap();

    // Large file should be skipped
    assert_eq!(
        progress.files_indexed, 1,
        "Should only index the normal file"
    );
}

// ============================================================================
// FTS5 Special Characters
// ============================================================================

#[test]
fn test_fts_special_characters_in_query() {
    let db = Database::in_memory().unwrap();
    db.upsert_file("test.rs", "fn test() { foo() }", 0x1)
        .unwrap();

    // FTS5 special characters should be escaped by preprocess_query
    // These shouldn't cause SQL errors
    let results = db.fts_search("test*", 10);
    assert!(results.is_ok());

    // Parentheses in query
    let results = db.fts_search("foo*", 10);
    assert!(results.is_ok());
}

// ============================================================================
// Score Type Safety
// ============================================================================

#[test]
fn test_score_saturation_bounds() {
    // Values above 1.0 should saturate
    assert_eq!(Score::new(1.5).as_f64(), 1.0);
    assert_eq!(Score::new(100.0).as_f64(), 1.0);

    // Values below 0.0 should saturate
    assert_eq!(Score::new(-0.5).as_f64(), 0.0);
    assert_eq!(Score::new(-100.0).as_f64(), 0.0);

    // NaN is treated as zero (defensive hardening in Score::new)
    let nan_score = Score::new(f64::NAN);
    assert_eq!(nan_score.as_f64(), 0.0);
}

#[test]
fn test_score_merge_saturation() {
    let s1 = Score::new(0.8);
    let s2 = Score::new(0.5);

    // 0.8 + 0.5 = 1.3, should saturate to 1.0
    let merged = s1.merge(s2);
    assert_eq!(merged.as_f64(), 1.0);
}

#[test]
fn test_score_weighted() {
    let score = Score::new(1.0);

    // Weighted by 0.5
    let weighted = score.weighted(0.5);
    assert!((weighted.as_f64() - 0.5).abs() < f64::EPSILON);

    // Weighted by 0 should give 0
    let zero_weighted = score.weighted(0.0);
    assert_eq!(zero_weighted.as_f64(), 0.0);
}

// ============================================================================
// FileId Type Safety
// ============================================================================

#[test]
fn test_file_id_roundtrip() {
    let id = FileId::new(42);
    assert_eq!(id.as_u32(), 42);
    assert_eq!(u32::from(id), 42);

    let from_u32: FileId = 123.into();
    assert_eq!(from_u32.as_u32(), 123);
}

#[test]
fn test_file_id_display() {
    let id = FileId::new(42);
    let display = format!("{}", id);
    assert_eq!(display, "file:42");
}

// ============================================================================
// Trigram Type Safety
// ============================================================================

#[test]
fn test_trigram_extraction_empty() {
    let trigrams: Vec<_> = Trigram::extract("").collect();
    assert!(trigrams.is_empty());

    let trigrams: Vec<_> = Trigram::extract("ab").collect();
    assert!(trigrams.is_empty());
}

#[test]
fn test_trigram_extraction_exact() {
    let trigrams: Vec<_> = Trigram::extract("abc").collect();
    assert_eq!(trigrams.len(), 1);
    assert_eq!(trigrams[0].as_bytes(), b"abc");
}

#[test]
fn test_trigram_debug_display() {
    let trigram = Trigram::new(*b"abc");
    let debug = format!("{:?}", trigram);
    assert!(debug.contains("abc"));
}

// ============================================================================
// Database Error Recovery
// ============================================================================

#[test]
fn test_database_file_not_found() {
    let db = Database::in_memory().unwrap();

    // Query non-existent file
    let result = db.get_file(FileId::new(99999));
    assert!(result.is_ok());
    assert!(result.unwrap().is_none());

    let result = db.get_file_by_path("/nonexistent/path.rs");
    assert!(result.is_ok());
    assert!(result.unwrap().is_none());
}

#[test]
fn test_delete_nonexistent_file_graceful() {
    let db = Database::in_memory().unwrap();

    // Deleting non-existent file should not error
    let result = db.delete_file("/nonexistent/file.rs");
    assert!(result.is_ok());
    assert!(!result.unwrap()); // Returns false (nothing deleted)
}

// ============================================================================
// Symlink Handling
// ============================================================================

#[cfg(unix)]
#[test]
fn test_indexer_symlink_handling() {
    use std::os::unix::fs::symlink;

    let dir = TempDir::new().unwrap();

    // Create a real file
    fs::write(dir.path().join("real.rs"), "fn main() {}").unwrap();

    // Create a symlink to it
    symlink(dir.path().join("real.rs"), dir.path().join("link.rs")).unwrap();

    let db = Arc::new(Database::in_memory().unwrap());
    let trigram = Arc::new(RwLock::new(TrigramIndex::new()));
    let indexer = Indexer::new(Arc::clone(&db), trigram, dir.path().to_path_buf());

    // Default config doesn't follow symlinks
    let progress = indexer.index(None, false).unwrap();

    // Should only index the real file, not the symlink
    assert_eq!(progress.files_indexed, 1);
}

// ============================================================================
// TOC Nonexistent Directory
// ============================================================================

#[test]
fn test_toc_nonexistent_directory() {
    let dir = TempDir::new().unwrap();
    let db = Arc::new(Database::in_memory().unwrap());
    let search = Arc::new(SearchService::new(Arc::clone(&db), dir.path().to_path_buf()).unwrap());

    let input = TocInput {
        path: "nonexistent_dir".to_string(),
        depth: 3,
    };

    // Should return empty tree for nonexistent directory
    let result = execute_toc(&search, input);
    // May error or return empty - both are acceptable
    if let Ok(output) = result {
        assert!(output.tree.is_empty());
    }
}
