//! Security integration tests for MCP tool execution.
//!
//! Tests that security measures are properly enforced at the tool level:
//! - Path traversal protection
//! - Sensitive file blocking
//! - ReDoS pattern rejection

mod common;

use agentika_grep::db::Database;
use agentika_grep::services::{Indexer, SearchService, TrigramIndex};
use agentika_grep::tools::*;
use std::fs;
use std::sync::{Arc, RwLock};
use tempfile::TempDir;

/// Sets up a test environment with services.
fn setup_test_services() -> (TempDir, Arc<SearchService>, Indexer) {
    let dir = TempDir::new().unwrap();
    let db = Arc::new(Database::in_memory().unwrap());
    let trigram = Arc::new(RwLock::new(TrigramIndex::new()));

    // Create test files
    fs::write(dir.path().join("main.rs"), "fn main() {}\n").unwrap();
    fs::write(dir.path().join("lib.rs"), "pub fn hello() {}\n").unwrap();

    // Create a nested directory
    fs::create_dir_all(dir.path().join("src")).unwrap();
    fs::write(dir.path().join("src/app.rs"), "pub fn run() {}\n").unwrap();

    // Create sensitive files (these should be blocked)
    fs::write(dir.path().join(".env"), "SECRET_KEY=abc123\n").unwrap();
    fs::write(
        dir.path().join(".env.production"),
        "DATABASE_URL=postgres://...\n",
    )
    .unwrap();
    fs::write(dir.path().join("credentials.json"), "{\"key\": \"secret\"}\n").unwrap();

    // Index files for FTS
    for filename in ["main.rs", "lib.rs", "src/app.rs"] {
        let path = dir.path().join(filename);
        let content = fs::read_to_string(&path).unwrap();
        db.upsert_file(
            path.to_string_lossy().as_ref(),
            &content,
            &format!("hash_{filename}"),
        )
        .unwrap();
    }

    let service = Arc::new(
        SearchService::new(Arc::clone(&db), dir.path().to_path_buf()).unwrap(),
    );

    let indexer = Indexer::new(Arc::clone(&db), trigram, dir.path().to_path_buf());

    (dir, service, indexer)
}

// =============================================================================
// Path Traversal Tests
// =============================================================================

#[test]
fn test_get_tool_blocks_path_traversal() {
    let (_dir, service, _indexer) = setup_test_services();

    // Direct traversal attempts
    let result = execute_get(
        &service,
        GetInput {
            path: "../etc/passwd".to_string(),
            start_line: 1,
            end_line: 0,
        },
    );
    assert!(result.is_err(), "Should block ../etc/passwd");
    let err = result.unwrap_err();
    assert!(
        err.contains("traversal"),
        "Error should mention traversal, got: {err}"
    );

    // Absolute path attempt
    let result = execute_get(
        &service,
        GetInput {
            path: "/etc/passwd".to_string(),
            start_line: 1,
            end_line: 0,
        },
    );
    assert!(result.is_err(), "Should block absolute paths");

    // Hidden traversal (go into directory then back out)
    let result = execute_get(
        &service,
        GetInput {
            path: "src/../../etc/passwd".to_string(),
            start_line: 1,
            end_line: 0,
        },
    );
    assert!(result.is_err(), "Should block hidden traversal");
}

#[test]
fn test_outline_tool_blocks_path_traversal() {
    let (_dir, service, _indexer) = setup_test_services();

    let result = execute_outline(
        &service,
        OutlineInput {
            path: "../../etc/passwd".to_string(),
        },
    );
    assert!(result.is_err(), "Should block path traversal");
}

#[test]
fn test_context_tool_blocks_path_traversal() {
    let (_dir, service, _indexer) = setup_test_services();

    let result = execute_context(
        &service,
        ContextInput {
            path: "../../../etc/shadow".to_string(),
            line: 1,
            context_lines: 5,
        },
    );
    assert!(result.is_err(), "Should block path traversal");
}

#[test]
fn test_toc_tool_blocks_path_traversal() {
    let (_dir, service, _indexer) = setup_test_services();

    let result = execute_toc(
        &service,
        TocInput {
            path: "../../..".to_string(),
            depth: 3,
        },
    );
    assert!(result.is_err(), "Should block path traversal");
}

#[test]
fn test_diff_tool_blocks_path_traversal() {
    let (_dir, service, _indexer) = setup_test_services();

    // First file is traversal
    let result = execute_diff(
        &service,
        DiffInput {
            file1: "../etc/passwd".to_string(),
            file2: "main.rs".to_string(),
            context: 3,
        },
    );
    assert!(result.is_err(), "Should block path traversal in file1");

    // Second file is traversal
    let result = execute_diff(
        &service,
        DiffInput {
            file1: "main.rs".to_string(),
            file2: "../etc/shadow".to_string(),
            context: 3,
        },
    );
    assert!(result.is_err(), "Should block path traversal in file2");
}

#[test]
fn test_related_tool_blocks_path_traversal() {
    let (_dir, service, _indexer) = setup_test_services();

    let result = execute_related(
        &service,
        RelatedInput {
            path: "../../etc/passwd".to_string(),
            limit: 10,
        },
    );
    assert!(result.is_err(), "Should block path traversal");
}

// =============================================================================
// Sensitive File Blocking Tests
// =============================================================================

#[test]
fn test_get_tool_blocks_sensitive_files() {
    let (_dir, service, _indexer) = setup_test_services();

    // .env file
    let result = execute_get(
        &service,
        GetInput {
            path: ".env".to_string(),
            start_line: 1,
            end_line: 0,
        },
    );
    assert!(result.is_err(), "Should block .env file");
    assert!(
        result.unwrap_err().contains("sensitive"),
        "Error should mention sensitive file"
    );

    // .env.production file
    let result = execute_get(
        &service,
        GetInput {
            path: ".env.production".to_string(),
            start_line: 1,
            end_line: 0,
        },
    );
    assert!(result.is_err(), "Should block .env.production file");

    // credentials.json
    let result = execute_get(
        &service,
        GetInput {
            path: "credentials.json".to_string(),
            start_line: 1,
            end_line: 0,
        },
    );
    assert!(result.is_err(), "Should block credentials.json");
}

#[test]
fn test_outline_tool_blocks_sensitive_files() {
    let (_dir, service, _indexer) = setup_test_services();

    let result = execute_outline(
        &service,
        OutlineInput {
            path: ".env".to_string(),
        },
    );
    assert!(result.is_err(), "Should block .env file");
}

#[test]
fn test_context_tool_blocks_sensitive_files() {
    let (_dir, service, _indexer) = setup_test_services();

    let result = execute_context(
        &service,
        ContextInput {
            path: "credentials.json".to_string(),
            line: 1,
            context_lines: 5,
        },
    );
    assert!(result.is_err(), "Should block credentials.json");
}

#[test]
fn test_diff_tool_blocks_sensitive_files() {
    let (_dir, service, _indexer) = setup_test_services();

    // Sensitive file as file1
    let result = execute_diff(
        &service,
        DiffInput {
            file1: ".env".to_string(),
            file2: "main.rs".to_string(),
            context: 3,
        },
    );
    assert!(result.is_err(), "Should block .env in file1");

    // Sensitive file as file2
    let result = execute_diff(
        &service,
        DiffInput {
            file1: "main.rs".to_string(),
            file2: ".env.production".to_string(),
            context: 3,
        },
    );
    assert!(result.is_err(), "Should block .env.production in file2");
}

#[test]
fn test_related_tool_blocks_sensitive_files() {
    let (_dir, service, _indexer) = setup_test_services();

    let result = execute_related(
        &service,
        RelatedInput {
            path: ".env".to_string(),
            limit: 10,
        },
    );
    assert!(result.is_err(), "Should block .env file");
}

// =============================================================================
// Valid Path Tests (ensure we don't over-block)
// =============================================================================

#[test]
fn test_valid_paths_still_work() {
    let (_dir, service, _indexer) = setup_test_services();

    // Normal file access should work
    let result = execute_get(
        &service,
        GetInput {
            path: "main.rs".to_string(),
            start_line: 1,
            end_line: 0,
        },
    );
    assert!(result.is_ok(), "Should allow normal file access");

    // Nested file access should work
    let result = execute_get(
        &service,
        GetInput {
            path: "src/app.rs".to_string(),
            start_line: 1,
            end_line: 0,
        },
    );
    assert!(result.is_ok(), "Should allow nested file access");

    // Path with ./ should work (it normalizes)
    let result = execute_get(
        &service,
        GetInput {
            path: "./main.rs".to_string(),
            start_line: 1,
            end_line: 0,
        },
    );
    assert!(result.is_ok(), "Should allow ./ prefix");

    // Path with redundant components should work
    let result = execute_get(
        &service,
        GetInput {
            path: "src/../main.rs".to_string(),
            start_line: 1,
            end_line: 0,
        },
    );
    assert!(result.is_ok(), "Should allow path that stays within root");
}

#[test]
fn test_toc_valid_paths_work() {
    let (_dir, service, _indexer) = setup_test_services();

    // Root directory
    let result = execute_toc(
        &service,
        TocInput {
            path: ".".to_string(),
            depth: 2,
        },
    );
    assert!(result.is_ok(), "Should allow root directory");

    // Subdirectory
    let result = execute_toc(
        &service,
        TocInput {
            path: "src".to_string(),
            depth: 2,
        },
    );
    assert!(result.is_ok(), "Should allow subdirectory");
}

// =============================================================================
// ReDoS Pattern Tests
// =============================================================================

#[test]
fn test_search_tool_blocks_redos_patterns() {
    let (_dir, service, _indexer) = setup_test_services();

    // Dangerous nested quantifiers
    let result = execute_search(
        &service,
        SearchInput {
            query: "(a+)+".to_string(),
            limit: 10,
            mode: "grep".to_string(),
        },
    );
    assert!(result.is_err(), "Should block (a+)+ pattern");

    let result = execute_search(
        &service,
        SearchInput {
            query: "(.*)*".to_string(),
            limit: 10,
            mode: "grep".to_string(),
        },
    );
    assert!(result.is_err(), "Should block (.*)* pattern");

    let result = execute_search(
        &service,
        SearchInput {
            query: "(.+)+".to_string(),
            limit: 10,
            mode: "grep".to_string(),
        },
    );
    assert!(result.is_err(), "Should block (.+)+ pattern");
}

#[test]
fn test_search_tool_allows_safe_patterns() {
    let (_dir, service, _indexer) = setup_test_services();

    // Safe patterns should work
    let result = execute_search(
        &service,
        SearchInput {
            query: "fn\\s+\\w+".to_string(),
            limit: 10,
            mode: "grep".to_string(),
        },
    );
    assert!(result.is_ok(), "Should allow fn\\s+\\w+ pattern");

    let result = execute_search(
        &service,
        SearchInput {
            query: "hello.*world".to_string(),
            limit: 10,
            mode: "grep".to_string(),
        },
    );
    assert!(result.is_ok(), "Should allow hello.*world pattern");
}

// =============================================================================
// Combined Attack Scenarios
// =============================================================================

#[test]
fn test_traversal_to_sensitive_file() {
    let (_dir, service, _indexer) = setup_test_services();

    // Try to traverse to a common sensitive location
    let result = execute_get(
        &service,
        GetInput {
            path: "../.ssh/id_rsa".to_string(),
            start_line: 1,
            end_line: 0,
        },
    );
    assert!(result.is_err(), "Should block traversal to SSH key");

    let result = execute_get(
        &service,
        GetInput {
            path: "../../.aws/credentials".to_string(),
            start_line: 1,
            end_line: 0,
        },
    );
    assert!(result.is_err(), "Should block traversal to AWS credentials");
}

#[test]
fn test_edge_cases() {
    let (_dir, service, _indexer) = setup_test_services();

    // Empty path
    let result = execute_get(
        &service,
        GetInput {
            path: "".to_string(),
            start_line: 1,
            end_line: 0,
        },
    );
    // Empty path joins to root directory, which should fail to read as file
    assert!(result.is_err());

    // Just dots
    let result = execute_get(
        &service,
        GetInput {
            path: "...".to_string(),
            start_line: 1,
            end_line: 0,
        },
    );
    assert!(result.is_err());

    // Unicode path (potential bypass attempt)
    let result = execute_get(
        &service,
        GetInput {
            path: "..%2F..%2Fetc%2Fpasswd".to_string(), // URL encoded
            start_line: 1,
            end_line: 0,
        },
    );
    // Should fail - no URL decoding
    assert!(result.is_err());
}
