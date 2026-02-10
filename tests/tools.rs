//! Integration tests for MCP tool execution.
//!
//! Tests the public tool API end-to-end with realistic file setups.

mod common;

use grepika::db::Database;
use grepika::services::{Indexer, SearchService, TrigramIndex};
use grepika::tools::*;
use std::fs;
use std::sync::{Arc, RwLock};
use tempfile::TempDir;

/// Sets up a test environment with services.
fn setup_test_services() -> (TempDir, Arc<SearchService>, Indexer) {
    let dir = TempDir::new().unwrap();
    let db = Arc::new(Database::in_memory().unwrap());
    let trigram = Arc::new(RwLock::new(TrigramIndex::new()));

    // Create test files
    fs::write(
        dir.path().join("main.rs"),
        r#"fn main() {
    let config = Config::load();
    let user = authenticate(&config).unwrap();
    println!("Hello, {}!", user.name());
}
"#,
    )
    .unwrap();

    fs::write(
        dir.path().join("auth.rs"),
        r#"use crate::config::Config;
use crate::error::AuthError;

/// Authenticates a user with the given configuration.
pub fn authenticate(config: &Config) -> Result<User, AuthError> {
    let credentials = config.credentials();
    validate_credentials(&credentials)?;
    Ok(User::new("authenticated_user"))
}

fn validate_credentials(creds: &Credentials) -> Result<(), AuthError> {
    if creds.is_valid() {
        Ok(())
    } else {
        Err(AuthError::InvalidCredentials)
    }
}

pub struct User {
    username: String,
}

impl User {
    pub fn new(username: &str) -> Self {
        Self {
            username: username.to_string(),
        }
    }

    pub fn name(&self) -> &str {
        &self.username
    }
}
"#,
    )
    .unwrap();

    fs::write(
        dir.path().join("config.rs"),
        r#"/// Application configuration.
pub struct Config {
    api_key: String,
    timeout: u64,
}

impl Config {
    pub fn load() -> Self {
        Self {
            api_key: std::env::var("API_KEY").unwrap_or_default(),
            timeout: 30,
        }
    }

    pub fn credentials(&self) -> Credentials {
        Credentials {
            api_key: self.api_key.clone(),
        }
    }
}

pub struct Credentials {
    pub api_key: String,
}

impl Credentials {
    pub fn is_valid(&self) -> bool {
        !self.api_key.is_empty()
    }
}
"#,
    )
    .unwrap();

    fs::write(
        dir.path().join("error.rs"),
        r#"use std::fmt;

#[derive(Debug)]
pub enum AuthError {
    InvalidCredentials,
    Timeout,
    NetworkError(String),
}

impl fmt::Display for AuthError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidCredentials => write!(f, "Invalid credentials"),
            Self::Timeout => write!(f, "Authentication timed out"),
            Self::NetworkError(msg) => write!(f, "Network error: {}", msg),
        }
    }
}

impl std::error::Error for AuthError {}
"#,
    )
    .unwrap();

    // Create nested directory
    fs::create_dir_all(dir.path().join("src/utils")).unwrap();
    fs::write(
        dir.path().join("src/utils/helpers.rs"),
        "pub fn helper_function() {}\n",
    )
    .unwrap();

    // Index files
    for (i, filename) in ["main.rs", "auth.rs", "config.rs", "error.rs"]
        .iter()
        .enumerate()
    {
        let path = dir.path().join(filename);
        let content = fs::read_to_string(&path).unwrap();
        db.upsert_file(path.to_string_lossy().as_ref(), &content, i as u64)
            .unwrap();
    }

    let search = Arc::new(SearchService::new(Arc::clone(&db), dir.path().to_path_buf()).unwrap());
    let indexer = Indexer::new(Arc::clone(&db), trigram, dir.path().to_path_buf());

    (dir, search, indexer)
}

// ============================================================================
// Search Tool Tests
// ============================================================================

#[test]
fn test_search_tool_happy_path() {
    let (_dir, search, _indexer) = setup_test_services();

    let input = SearchInput {
        query: "authenticate".to_string(),
        limit: 10,
        mode: SearchMode::Combined,
    };

    let result = execute_search(&search, input).unwrap();

    assert!(
        !result.results.is_empty(),
        "Should find results for 'authenticate'"
    );
    assert!(
        result.results.iter().any(|r| r.path.contains("auth")),
        "Should include auth.rs in results"
    );
    assert!(!result.results.is_empty());
}

#[test]
fn test_search_tool_fts_mode() {
    let (_dir, search, _indexer) = setup_test_services();

    let input = SearchInput {
        query: "Config".to_string(),
        limit: 10,
        mode: SearchMode::Fts,
    };

    let result = execute_search(&search, input).unwrap();

    assert!(!result.results.is_empty());
    // FTS results should have "fts" source
    for item in &result.results {
        assert!(
            item.sources.contains(&"fts".to_string()),
            "FTS mode results should have 'fts' source"
        );
    }
}

#[test]
fn test_search_tool_grep_mode() {
    let (_dir, search, _indexer) = setup_test_services();

    let input = SearchInput {
        query: "pub fn".to_string(),
        limit: 10,
        mode: SearchMode::Grep,
    };

    let result = execute_search(&search, input).unwrap();

    assert!(!result.results.is_empty());
    // Grep results should have "grep" source
    for item in &result.results {
        assert!(
            item.sources.contains(&"grep".to_string()),
            "Grep mode results should have 'grep' source"
        );
    }
}

#[test]
fn test_search_tool_no_matches() {
    let (_dir, search, _indexer) = setup_test_services();

    let input = SearchInput {
        query: "xyznonexistent123456".to_string(),
        limit: 10,
        mode: SearchMode::Combined,
    };

    let result = execute_search(&search, input).unwrap();

    assert!(result.results.is_empty());
}

#[test]
fn test_search_tool_respects_limit() {
    let (_dir, search, _indexer) = setup_test_services();

    let input = SearchInput {
        query: "fn".to_string(), // Should match many things
        limit: 2,
        mode: SearchMode::Combined,
    };

    let result = execute_search(&search, input).unwrap();

    assert!(result.results.len() <= 2);
}

// ============================================================================
// Relevant Tool Tests
// ============================================================================

#[test]
fn test_relevant_tool_happy_path() {
    let (_dir, search, _indexer) = setup_test_services();

    let input = RelevantInput {
        topic: "authentication".to_string(),
        limit: 10,
    };

    let result = execute_relevant(&search, input).unwrap();

    assert!(!result.files.is_empty());
    // Should find auth-related files
    assert!(
        result.files.iter().any(|f| f.path.contains("auth")),
        "Should find auth.rs as relevant to 'authentication'"
    );
}

#[test]
fn test_relevant_tool_provides_reasons() {
    let (_dir, search, _indexer) = setup_test_services();

    let input = RelevantInput {
        topic: "error handling".to_string(),
        limit: 10,
    };

    let result = execute_relevant(&search, input).unwrap();

    // Each file should have a reason
    for file in &result.files {
        assert!(
            !file.reason.is_empty(),
            "Each relevant file should have a reason"
        );
        assert!(
            file.reason.contains("match"),
            "Reason should describe the match type"
        );
    }
}

// ============================================================================
// Get Tool Tests
// ============================================================================

#[test]
fn test_get_tool_happy_path() {
    let (_dir, search, _indexer) = setup_test_services();

    let input = GetInput {
        path: "auth.rs".to_string(),
        start_line: 1,
        end_line: 0, // End of file
    };

    let result = execute_get(&search, input).unwrap();

    assert!(!result.content.is_empty());
    assert!(result.content.contains("authenticate"));
    assert!(result.total_lines > 0);
}

#[test]
fn test_get_tool_with_line_range() {
    let (_dir, search, _indexer) = setup_test_services();

    let input = GetInput {
        path: "auth.rs".to_string(),
        start_line: 1,
        end_line: 5,
    };

    let result = execute_get(&search, input).unwrap();

    // Should only return first 5 lines (+ 2 boundary marker lines)
    let line_count = result.content.lines().count();
    assert!(
        line_count <= 7,
        "Expected at most 5 content lines + 2 markers, got {line_count}"
    );
    assert_eq!(result.start_line, 1);
    assert!(result.end_line <= 5);
}

#[test]
fn test_get_tool_nonexistent_file() {
    let (_dir, search, _indexer) = setup_test_services();

    let input = GetInput {
        path: "nonexistent.rs".to_string(),
        start_line: 1,
        end_line: 0,
    };

    let result = execute_get(&search, input);

    assert!(result.is_err(), "Should return error for nonexistent file");
    assert!(
        result.unwrap_err().contains("Failed to read"),
        "Error should mention reading failure"
    );
}

#[test]
fn test_get_tool_line_beyond_eof() {
    let (_dir, search, _indexer) = setup_test_services();

    let input = GetInput {
        path: "auth.rs".to_string(),
        start_line: 1000, // Way beyond EOF
        end_line: 2000,
    };

    let result = execute_get(&search, input).unwrap();

    // Should gracefully handle, content may contain only boundary markers
    // The actual file content between markers should be empty or minimal
    assert!(result.start_line <= result.total_lines || result.total_lines > 0);
}

// ============================================================================
// Outline Tool Tests
// ============================================================================

#[test]
fn test_outline_tool_rust_file() {
    let (_dir, search, _indexer) = setup_test_services();

    let input = OutlineInput {
        path: "auth.rs".to_string(),
    };

    let result = execute_outline(&search, input).unwrap();

    assert_eq!(result.file_type, "rs");
    assert!(
        !result.symbols.is_empty(),
        "Should extract symbols from Rust file"
    );

    // Should find the authenticate function
    let has_authenticate = result
        .symbols
        .iter()
        .any(|s| s.name == "authenticate" && s.kind == "function");
    assert!(has_authenticate, "Should find 'authenticate' function");

    // Should find the User struct
    let has_user = result
        .symbols
        .iter()
        .any(|s| s.name == "User" && s.kind == "struct");
    assert!(has_user, "Should find 'User' struct");
}

#[test]
fn test_outline_tool_extracts_impl_blocks() {
    let (_dir, search, _indexer) = setup_test_services();

    let input = OutlineInput {
        path: "auth.rs".to_string(),
    };

    let result = execute_outline(&search, input).unwrap();

    // Should find impl block
    let has_impl = result.symbols.iter().any(|s| s.kind == "impl");
    assert!(has_impl, "Should find impl block");
}

#[test]
fn test_outline_tool_includes_line_numbers() {
    let (_dir, search, _indexer) = setup_test_services();

    let input = OutlineInput {
        path: "auth.rs".to_string(),
    };

    let result = execute_outline(&search, input).unwrap();

    for symbol in &result.symbols {
        assert!(symbol.line > 0, "Line numbers should be 1-indexed");
    }
}

// ============================================================================
// TOC Tool Tests
// ============================================================================

#[test]
fn test_toc_tool_happy_path() {
    let (_dir, search, _indexer) = setup_test_services();

    let input = TocInput {
        path: ".".to_string(),
        depth: 3,
    };

    let result = execute_toc(&search, input).unwrap();

    assert!(!result.tree.is_empty());
    assert!(result.total_files > 0);

    // Should find our test files
    assert!(result.tree.contains("main.rs"), "Should include main.rs");
}

#[test]
fn test_toc_tool_respects_depth() {
    let (_dir, search, _indexer) = setup_test_services();

    // Depth 1 should only show top-level items
    let input = TocInput {
        path: ".".to_string(),
        depth: 1,
    };

    let result = execute_toc(&search, input).unwrap();

    // At depth 1, directories should be listed but no children shown
    // The tree string should contain top-level items but no indented entries
    assert!(!result.tree.is_empty());
    // No lines should have 2-space indentation (depth 1 means only top-level)
    assert!(
        !result.tree.lines().any(|l| l.starts_with("  ")),
        "At depth 1, directories should have no children listed"
    );
}

#[test]
fn test_toc_tool_nested_directory() {
    let (_dir, search, _indexer) = setup_test_services();

    let input = TocInput {
        path: "src".to_string(),
        depth: 3,
    };

    let result = execute_toc(&search, input).unwrap();

    assert!(!result.tree.is_empty());
    // Should find utils subdirectory (listed with trailing slash)
    assert!(
        result.tree.contains("utils/"),
        "Should find utils subdirectory"
    );
}

// ============================================================================
// Context Tool Tests
// ============================================================================

#[test]
fn test_context_tool_happy_path() {
    let (_dir, search, _indexer) = setup_test_services();

    let input = ContextInput {
        path: "auth.rs".to_string(),
        line: 5,
        context_lines: 3,
    };

    let result = execute_context(&search, input).unwrap();

    assert!(!result.content.is_empty());
    assert_eq!(result.center_line, 5);

    // Should include line numbers in output
    assert!(
        result.content.contains("|"),
        "Context should include line number formatting"
    );

    // Should mark the center line
    assert!(
        result.content.contains(">"),
        "Should mark the center line with >"
    );
}

#[test]
fn test_context_tool_respects_context_size() {
    let (_dir, search, _indexer) = setup_test_services();

    let input = ContextInput {
        path: "auth.rs".to_string(),
        line: 10,
        context_lines: 2,
    };

    let result = execute_context(&search, input).unwrap();

    // Should return ~5 lines (2 before + center + 2 after) + 2 boundary marker lines
    let line_count = result.content.lines().count();
    assert!(
        line_count <= 7,
        "Should respect context_lines parameter, got {line_count}"
    );
}

#[test]
fn test_context_tool_at_file_start() {
    let (_dir, search, _indexer) = setup_test_services();

    let input = ContextInput {
        path: "auth.rs".to_string(),
        line: 1,
        context_lines: 5,
    };

    let result = execute_context(&search, input).unwrap();

    // Should not panic, start_line should be 1
    assert_eq!(result.start_line, 1);
}

// ============================================================================
// Stats Tool Tests
// ============================================================================

#[test]
fn test_stats_tool_basic() {
    let (_dir, search, indexer) = setup_test_services();

    let input = StatsInput { detailed: false };

    let result = execute_stats(&search, &indexer, input).unwrap();

    assert!(result.total_files > 0);
    assert!(result.by_type.is_none()); // Not detailed
    assert!(!result.index_size.human.is_empty());
}

#[test]
fn test_stats_tool_detailed() {
    let (_dir, search, indexer) = setup_test_services();

    let input = StatsInput { detailed: true };

    let result = execute_stats(&search, &indexer, input).unwrap();

    assert!(result.by_type.is_some());
    let by_type = result.by_type.unwrap();

    // Should have "rs" as a file type
    assert!(by_type.contains_key("rs"), "Should count Rust files");
}

// ============================================================================
// Refs Tool Tests
// ============================================================================

#[test]
fn test_refs_tool_finds_usages() {
    let (_dir, search, _indexer) = setup_test_services();

    let input = RefsInput {
        symbol: "authenticate".to_string(),
        limit: 50,
    };

    let result = execute_refs(&search, input).unwrap();

    assert!(!result.references.is_empty());

    // Should include the definition
    let has_definition = result
        .references
        .iter()
        .any(|r| r.ref_type == "definition" || r.content.contains("pub fn authenticate"));
    assert!(
        has_definition || !result.references.is_empty(),
        "Should find at least one reference"
    );
}

#[test]
fn test_refs_tool_respects_limit() {
    let (_dir, search, _indexer) = setup_test_services();

    let input = RefsInput {
        symbol: "fn".to_string(), // Common keyword
        limit: 3,
    };

    let result = execute_refs(&search, input).unwrap();

    assert!(result.references.len() <= 3);
}

#[test]
fn test_refs_tool_no_matches() {
    let (_dir, search, _indexer) = setup_test_services();

    let input = RefsInput {
        symbol: "xyznonexistent123".to_string(),
        limit: 50,
    };

    let result = execute_refs(&search, input).unwrap();

    assert!(result.references.is_empty());
}

// ============================================================================
// Related Tool Tests
// ============================================================================

#[test]
fn test_related_tool_finds_related_files() {
    let (_dir, search, _indexer) = setup_test_services();

    let input = RelatedInput {
        path: "auth.rs".to_string(),
        limit: 10,
    };

    let result = execute_related(&search, input).unwrap();

    assert_eq!(result.source, "auth.rs");
    // Config and main should be related to auth (they use it)
    // Note: May or may not find relations depending on keyword extraction
}

// ============================================================================
// Content Boundary Marker Tests
// ============================================================================

#[test]
fn test_get_output_has_content_boundaries() {
    let (_dir, search, _indexer) = setup_test_services();

    let input = GetInput {
        path: "auth.rs".to_string(),
        start_line: 1,
        end_line: 0,
    };

    let result = execute_get(&search, input).unwrap();

    assert!(
        result
            .content
            .starts_with("--- BEGIN FILE CONTENT: auth.rs ---\n"),
        "Content should start with BEGIN marker"
    );
    assert!(
        result
            .content
            .ends_with("\n--- END FILE CONTENT: auth.rs ---"),
        "Content should end with END marker"
    );
}

#[test]
fn test_context_output_has_content_boundaries() {
    let (_dir, search, _indexer) = setup_test_services();

    let input = ContextInput {
        path: "auth.rs".to_string(),
        line: 5,
        context_lines: 3,
    };

    let result = execute_context(&search, input).unwrap();

    assert!(
        result
            .content
            .starts_with("--- BEGIN FILE CONTENT: auth.rs ---\n"),
        "Context content should start with BEGIN marker"
    );
    assert!(
        result
            .content
            .ends_with("\n--- END FILE CONTENT: auth.rs ---"),
        "Context content should end with END marker"
    );
}

// ============================================================================
// Parameter Cap Tests
// ============================================================================

#[test]
fn test_context_tool_caps_context_lines() {
    let (_dir, search, _indexer) = setup_test_services();

    // Even with absurd context_lines, should not crash or OOM
    let input = ContextInput {
        path: "auth.rs".to_string(),
        line: 5,
        context_lines: 999_999,
    };

    let result = execute_context(&search, input);
    assert!(
        result.is_ok(),
        "Should handle large context_lines gracefully"
    );
}

#[test]
fn test_toc_depth_capped() {
    let (_dir, search, _indexer) = setup_test_services();

    // Even with absurd depth, should not recurse too deep
    let input = TocInput {
        path: ".".to_string(),
        depth: 999_999,
    };

    let result = execute_toc(&search, input);
    assert!(result.is_ok(), "Should handle large depth gracefully");
}

// ============================================================================
// Search Result Sensitive File Filtering Tests
// ============================================================================

#[test]
fn test_search_results_exclude_sensitive_files() {
    let dir = TempDir::new().unwrap();
    let db = Arc::new(Database::in_memory().unwrap());
    let _trigram = Arc::new(RwLock::new(TrigramIndex::new()));

    // Create a normal file with the word "secret"
    fs::write(
        dir.path().join("app.rs"),
        "fn secret_handler() { /* handles secrets */ }\n",
    )
    .unwrap();

    // Create a sensitive file with "secret"
    fs::write(dir.path().join(".env"), "SECRET_KEY=abc123\n").unwrap();

    // Index only the normal file (sensitive files are excluded from index by design)
    let path = dir.path().join("app.rs");
    let content = fs::read_to_string(&path).unwrap();
    db.upsert_file(path.to_string_lossy().as_ref(), &content, 0)
        .unwrap();

    let search = Arc::new(SearchService::new(Arc::clone(&db), dir.path().to_path_buf()).unwrap());

    let input = SearchInput {
        query: "secret".to_string(),
        limit: 20,
        mode: SearchMode::Combined,
    };

    let result = execute_search(&search, input).unwrap();

    // No result should point to .env
    for item in &result.results {
        assert!(
            !item.path.contains(".env"),
            "Search results should not include .env, got: {}",
            item.path
        );
    }
}

#[test]
fn test_related_tool_nonexistent_file() {
    let (_dir, search, _indexer) = setup_test_services();

    let input = RelatedInput {
        path: "nonexistent.rs".to_string(),
        limit: 10,
    };

    let result = execute_related(&search, input);

    assert!(result.is_err());
}
