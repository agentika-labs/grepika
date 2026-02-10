//! Common test utilities for grepika integration tests.
//!
//! Provides `TestEnv` for setting up isolated test environments with
//! database, search service, and indexer all wired together.

#![allow(dead_code)] // Test utilities may not all be used in every test file

use grepika::db::Database;
use grepika::services::{Indexer, SearchService, TrigramIndex};
use std::fs;
use std::sync::{Arc, RwLock};
use tempfile::TempDir;

/// A complete test environment with all services wired together.
pub struct TestEnv {
    pub dir: TempDir,
    pub db: Arc<Database>,
    pub search: SearchService,
    pub trigram: Arc<RwLock<TrigramIndex>>,
}

impl TestEnv {
    /// Creates a new empty test environment.
    pub fn new() -> Self {
        let dir = TempDir::new().expect("Failed to create temp directory");
        let db = Arc::new(Database::in_memory().expect("Failed to create in-memory database"));
        let trigram = Arc::new(RwLock::new(TrigramIndex::new()));
        let search = SearchService::new(Arc::clone(&db), dir.path().to_path_buf())
            .expect("Failed to create search service");

        Self {
            dir,
            db,
            search,
            trigram,
        }
    }

    /// Creates an indexer for this environment.
    pub fn indexer(&self) -> Indexer {
        Indexer::new(
            Arc::clone(&self.db),
            Arc::clone(&self.trigram),
            self.dir.path().to_path_buf(),
        )
    }

    /// Writes a file to the test directory.
    pub fn write_file(&self, name: &str, content: &str) {
        let path = self.dir.path().join(name);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("Failed to create parent directories");
        }
        fs::write(&path, content).expect("Failed to write test file");
    }

    /// Indexes all files in the test directory.
    pub fn index_all(&self) {
        let indexer = self.indexer();
        indexer.index(None, false).expect("Failed to index files");
    }

    /// Gets the full path to a file in the test directory.
    pub fn path(&self, name: &str) -> std::path::PathBuf {
        self.dir.path().join(name)
    }
}

impl Default for TestEnv {
    fn default() -> Self {
        Self::new()
    }
}

/// Creates a test environment with pre-populated Rust source files.
pub fn rust_codebase() -> TestEnv {
    let env = TestEnv::new();

    env.write_file(
        "main.rs",
        r#"fn main() {
    let config = Config::load();
    let result = authenticate(&config);
    println!("{:?}", result);
}
"#,
    );

    env.write_file(
        "auth.rs",
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
}
"#,
    );

    env.write_file(
        "config.rs",
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
    );

    env.write_file(
        "error.rs",
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
    );

    env.write_file(
        "lib.rs",
        r#"//! A sample library for testing.

mod auth;
mod config;
mod error;

pub use auth::{authenticate, User};
pub use config::Config;
pub use error::AuthError;
"#,
    );

    env
}

/// Creates a test environment with files containing specific patterns.
pub fn pattern_test_codebase() -> TestEnv {
    let env = TestEnv::new();

    // File with many matches for testing score aggregation
    env.write_file(
        "many_matches.rs",
        r#"fn foo() { println!("foo"); }
fn bar() { println!("foo"); }
fn baz() { println!("foo"); }
fn qux() { println!("foo"); }
fn quux() { println!("foo"); }
"#,
    );

    // File with single match
    env.write_file("single_match.rs", "fn main() { println!(\"foo\"); }\n");

    // File with no matches
    env.write_file("no_match.rs", "fn main() { println!(\"bar\"); }\n");

    // File with unicode content
    env.write_file(
        "unicode.rs",
        r#"// Unicode test: \u4f60\u597d (hello in Chinese)
fn greet_chinese() {
    println!("\u4f60\u597d\u4e16\u754c"); // Hello World
}

fn greet_emoji() {
    println!("\u{1F44B} Hello!"); // Wave emoji
}
"#,
    );

    // Nested directory structure
    env.write_file("src/nested/deep.rs", "pub fn deep_function() {}\n");
    env.write_file(
        "src/nested/deeper/very_deep.rs",
        "pub fn very_deep_function() {}\n",
    );

    env
}

/// Creates a minimal test environment with just one file.
pub fn minimal_codebase() -> TestEnv {
    let env = TestEnv::new();
    env.write_file("test.rs", "fn main() {}\n");
    env
}

/// Asserts that search results contain a file with the given name.
pub fn assert_results_contain(results: &[grepika::services::SearchHit], filename: &str) {
    let found = results
        .iter()
        .any(|r| r.path.file_name().map(|n| n.to_string_lossy()) == Some(filename.into()));
    assert!(
        found,
        "Expected results to contain '{}', but got: {:?}",
        filename,
        results.iter().map(|r| &r.path).collect::<Vec<_>>()
    );
}

/// Asserts that search results do NOT contain a file with the given name.
pub fn assert_results_not_contain(results: &[grepika::services::SearchHit], filename: &str) {
    let found = results
        .iter()
        .any(|r| r.path.file_name().map(|n| n.to_string_lossy()) == Some(filename.into()));
    assert!(
        !found,
        "Expected results NOT to contain '{}', but it was found",
        filename
    );
}
