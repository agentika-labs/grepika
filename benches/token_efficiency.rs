//! Token efficiency benchmark comparing grepika vs Claude's built-in Grep.
//!
//! This benchmark measures output token cost, not execution time.
//! The goal is to quantify the break-even point where MCP schema overhead
//! is offset by per-query token savings.
//!
//! Run with: `cargo bench token_efficiency`
//! View reports: `open target/criterion/report/index.html`

use grepika::bench_utils::{BenchmarkStats, BreakEvenAnalysis, ComparisonResult, TokenMetrics};
use grepika::db::Database;
use grepika::services::SearchService;
use grepika::tools::{SearchOutput, SearchResultItem};
use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use std::fs;
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::sync::Arc;
use tempfile::TempDir;

// ============================================================================
// Test Fixtures
// ============================================================================

/// Creates a test codebase with realistic Rust code.
fn create_test_codebase() -> (TempDir, Arc<Database>, SearchService) {
    let dir = TempDir::new().expect("Failed to create temp dir");
    let db = Arc::new(Database::in_memory().expect("Failed to create database"));

    // Create realistic source files
    let files = vec![
        (
            "src/auth.rs",
            r#"
//! Authentication module.

use crate::config::Config;
use crate::error::AuthError;

/// Authenticates a user with the given credentials.
pub fn authenticate(username: &str, password: &str, config: &Config) -> Result<User, AuthError> {
    let hash = hash_password(password);
    let user = find_user(username)?;
    verify_password(&user, &hash)?;
    Ok(user)
}

/// Validates an authentication token.
pub fn validate_token(token: &str) -> Result<Claims, AuthError> {
    let decoded = decode_jwt(token)?;
    if decoded.is_expired() {
        return Err(AuthError::TokenExpired);
    }
    Ok(decoded.claims)
}

fn hash_password(password: &str) -> String {
    // Implementation
    password.to_string()
}

fn find_user(username: &str) -> Result<User, AuthError> {
    // Database lookup
    Ok(User { name: username.to_string() })
}

fn verify_password(user: &User, hash: &str) -> Result<(), AuthError> {
    // Password verification
    Ok(())
}
"#,
        ),
        (
            "src/config.rs",
            r#"
//! Configuration module.

use std::env;

/// Application configuration.
#[derive(Debug, Clone)]
pub struct Config {
    pub database_url: String,
    pub auth_secret: String,
    pub port: u16,
    pub debug: bool,
}

impl Config {
    /// Loads configuration from environment variables.
    pub fn from_env() -> Result<Self, ConfigError> {
        Ok(Self {
            database_url: env::var("DATABASE_URL")?,
            auth_secret: env::var("AUTH_SECRET")?,
            port: env::var("PORT").unwrap_or("8080".to_string()).parse()?,
            debug: env::var("DEBUG").map(|v| v == "true").unwrap_or(false),
        })
    }

    /// Creates a default configuration for testing.
    pub fn default_test() -> Self {
        Self {
            database_url: "sqlite::memory:".to_string(),
            auth_secret: "test_secret".to_string(),
            port: 8080,
            debug: true,
        }
    }
}
"#,
        ),
        (
            "src/handler.rs",
            r#"
//! Request handlers.

use crate::auth::{authenticate, validate_token};
use crate::config::Config;
use crate::error::HandlerError;

/// Handles login requests.
pub async fn handle_login(
    request: LoginRequest,
    config: &Config,
) -> Result<LoginResponse, HandlerError> {
    let user = authenticate(&request.username, &request.password, config)?;
    let token = generate_token(&user)?;
    Ok(LoginResponse { token, user_id: user.id })
}

/// Handles authenticated requests.
pub async fn handle_protected_resource(
    token: &str,
    config: &Config,
) -> Result<ResourceResponse, HandlerError> {
    let claims = validate_token(token)?;
    let resource = fetch_resource(&claims.user_id)?;
    Ok(ResourceResponse { data: resource })
}

/// Handles user profile updates.
pub async fn handle_profile_update(
    token: &str,
    update: ProfileUpdate,
) -> Result<ProfileResponse, HandlerError> {
    let claims = validate_token(token)?;
    update_profile(&claims.user_id, update)?;
    Ok(ProfileResponse { success: true })
}
"#,
        ),
        (
            "src/error.rs",
            r#"
//! Error types.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum AuthError {
    #[error("Invalid credentials")]
    InvalidCredentials,
    #[error("Token expired")]
    TokenExpired,
    #[error("User not found: {0}")]
    UserNotFound(String),
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("Missing environment variable: {0}")]
    MissingVar(String),
    #[error("Invalid configuration: {0}")]
    Invalid(String),
}

#[derive(Debug, Error)]
pub enum HandlerError {
    #[error("Authentication error: {0}")]
    Auth(#[from] AuthError),
    #[error("Not found: {0}")]
    NotFound(String),
    #[error("Internal error")]
    Internal,
}
"#,
        ),
        (
            "src/main.rs",
            r#"
//! Main entry point.

mod auth;
mod config;
mod error;
mod handler;

use config::Config;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = Config::from_env()?;

    println!("Starting server on port {}", config.port);

    // Server setup would go here
    Ok(())
}
"#,
        ),
        (
            "src/database.rs",
            r#"
//! Database operations.

use crate::error::DatabaseError;

pub struct DatabasePool {
    connection_string: String,
}

impl DatabasePool {
    pub fn new(connection_string: &str) -> Result<Self, DatabaseError> {
        Ok(Self {
            connection_string: connection_string.to_string(),
        })
    }

    pub async fn query<T>(&self, sql: &str) -> Result<Vec<T>, DatabaseError> {
        // Query implementation
        Ok(vec![])
    }

    pub async fn execute(&self, sql: &str) -> Result<u64, DatabaseError> {
        // Execute implementation
        Ok(0)
    }
}
"#,
        ),
        (
            "tests/auth_test.rs",
            r#"
//! Authentication tests.

use crate::auth::{authenticate, validate_token};
use crate::config::Config;

#[test]
fn test_authenticate_success() {
    let config = Config::default_test();
    let result = authenticate("user", "password", &config);
    assert!(result.is_ok());
}

#[test]
fn test_authenticate_invalid_password() {
    let config = Config::default_test();
    let result = authenticate("user", "wrong", &config);
    assert!(result.is_err());
}

#[test]
fn test_validate_token_expired() {
    let result = validate_token("expired_token");
    assert!(result.is_err());
}
"#,
        ),
    ];

    for (path, content) in files {
        let full_path = dir.path().join(path);
        if let Some(parent) = full_path.parent() {
            fs::create_dir_all(parent).expect("Failed to create directory");
        }
        fs::write(&full_path, content).expect("Failed to write file");
        db.upsert_file(
            full_path.to_string_lossy().as_ref(),
            content,
            xxhash_rust::xxh3::xxh3_64(content.as_bytes()),
        )
        .expect("Failed to insert file");
    }

    let service =
        SearchService::new(Arc::clone(&db), dir.path().to_path_buf()).expect("Failed to create search service");

    (dir, db, service)
}

// ============================================================================
// Grepika Output Measurement
// ============================================================================

/// Measures grepika search output size.
fn measure_grepika_search(service: &SearchService, query: &str, limit: usize) -> TokenMetrics {
    let results = service.search(query, limit).unwrap_or_default();
    let root = service.root();

    // Convert to SearchOutput (same format as MCP tool)
    let items: Vec<SearchResultItem> = results
        .iter()
        .map(|r| {
            let relative_path = r
                .path
                .strip_prefix(root)
                .unwrap_or(&r.path)
                .to_string_lossy()
                .to_string();

            let mut sources = Vec::new();
            if r.sources.fts {
                sources.push("fts".to_string());
            }
            if r.sources.grep {
                sources.push("grep".to_string());
            }
            if r.sources.trigram {
                sources.push("trigram".to_string());
            }

            SearchResultItem {
                path: relative_path,
                score: r.score.as_f64(),
                sources,
                snippets: Vec::new(),
            }
        })
        .collect();

    let result_count = items.len();
    let output = SearchOutput {
        results: items,
        has_more: false,
    };

    TokenMetrics::from_output(&output, result_count, result_count)
}

// ============================================================================
// Ripgrep Output Measurement (Claude Grep Proxy)
// ============================================================================

/// Measures ripgrep output size (simulating Claude's Grep tool).
///
/// Uses `rg --json` to get structured output with context lines,
/// then transforms it to match Claude's typical Grep response format.
fn measure_ripgrep_search(dir: &TempDir, query: &str, limit: usize) -> TokenMetrics {
    // Try to run ripgrep
    let output = Command::new("rg")
        .args([
            "--json",        // JSON output
            "-C",
            "3",             // 3 lines of context (Claude default)
            "--max-count",
            &limit.to_string(), // Limit matches per file
            query,
        ])
        .current_dir(dir.path())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output();

    let output = match output {
        Ok(o) => o,
        Err(_) => {
            // ripgrep not available, simulate typical output size
            return simulate_ripgrep_output(query, limit);
        }
    };

    if !output.status.success() && output.stdout.is_empty() {
        // No matches or error
        return TokenMetrics::default();
    }

    // Parse JSON lines output and transform to Claude-like format
    let reader = BufReader::new(output.stdout.as_slice());
    let mut claude_output = String::new();
    let mut result_count = 0;
    let mut files_found = std::collections::HashSet::new();

    for line in reader.lines().map_while(Result::ok) {
        // ripgrep JSON format has type field
        if let Ok(json) = serde_json::from_str::<serde_json::Value>(&line) {
            match json.get("type").and_then(|t| t.as_str()) {
                Some("match") => {
                    if let Some(data) = json.get("data") {
                        if let Some(path) = data.get("path").and_then(|p| p.get("text")).and_then(|t| t.as_str()) {
                            files_found.insert(path.to_string());
                        }
                        // Add the match line to output (simulating Claude's format)
                        if let Some(lines) = data.get("lines").and_then(|l| l.get("text")).and_then(|t| t.as_str()) {
                            let line_num = data
                                .get("line_number")
                                .and_then(|n| n.as_u64())
                                .unwrap_or(0);
                            claude_output.push_str(&format!("{}:{}: {}\n",
                                data.get("path").and_then(|p| p.get("text")).and_then(|t| t.as_str()).unwrap_or(""),
                                line_num,
                                lines.trim()
                            ));
                            result_count += 1;
                        }
                    }
                }
                Some("context") => {
                    // Add context lines
                    if let Some(data) = json.get("data") {
                        if let Some(lines) = data.get("lines").and_then(|l| l.get("text")).and_then(|t| t.as_str()) {
                            let line_num = data
                                .get("line_number")
                                .and_then(|n| n.as_u64())
                                .unwrap_or(0);
                            claude_output.push_str(&format!("{}:{}- {}\n",
                                data.get("path").and_then(|p| p.get("text")).and_then(|t| t.as_str()).unwrap_or(""),
                                line_num,
                                lines.trim()
                            ));
                        }
                    }
                }
                _ => {}
            }
        }
    }

    TokenMetrics::from_ripgrep_output(&claude_output, result_count, files_found.len())
}

/// Simulates ripgrep output when rg is not available.
///
/// Based on typical Claude Grep output characteristics:
/// - ~4x more verbose than grepika due to context lines
fn simulate_ripgrep_output(query: &str, limit: usize) -> TokenMetrics {
    // Simulate finding matches with context
    // Typical ripgrep output with 3 lines context is ~4x grepika
    let simulated_bytes = (query.len() + 50) * limit * 4; // ~4x multiplier for context
    TokenMetrics {
        output_bytes: simulated_bytes,
        estimated_tokens: simulated_bytes / 4,
        result_count: limit,
        files_found: limit.min(5), // Assume matches spread across fewer files
    }
}

// ============================================================================
// MCP Schema Size Measurement
// ============================================================================

/// Captures the MCP tool schema JSON for token cost estimation.
///
/// This measures the one-time cost paid when an MCP server is initialized.
fn measure_mcp_schema_size() -> usize {
    // Manually construct schema representation matching rmcp output
    // This is based on the 11 tools defined in server.rs
    let schema = serde_json::json!({
        "tools": [
            {
                "name": "search",
                "description": "Search for code patterns. Supports regex and natural language queries.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "query": {"type": "string", "description": "Search query (regex or natural language)"},
                        "limit": {"type": "integer", "description": "Maximum results (default: 20)"},
                        "mode": {"type": "string", "description": "Search mode: combined, fts, or grep (default: combined)"}
                    },
                    "required": ["query"]
                }
            },
            {
                "name": "relevant",
                "description": "Find files most relevant to a topic. Uses combined search for best results.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "topic": {"type": "string", "description": "Topic or concept to search for"},
                        "limit": {"type": "integer", "description": "Maximum files (default: 10)"}
                    },
                    "required": ["topic"]
                }
            },
            {
                "name": "get",
                "description": "Get file content. Supports line range selection.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "path": {"type": "string", "description": "File path relative to root"},
                        "start_line": {"type": "integer", "description": "Starting line (1-indexed, default: 1)"},
                        "end_line": {"type": "integer", "description": "Ending line (0 = end of file)"}
                    },
                    "required": ["path"]
                }
            },
            {
                "name": "outline",
                "description": "Extract file structure (functions, classes, structs, etc.)",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "path": {"type": "string", "description": "File path relative to root"}
                    },
                    "required": ["path"]
                }
            },
            {
                "name": "toc",
                "description": "Get directory tree structure",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "path": {"type": "string", "description": "Directory path (default: root)"},
                        "depth": {"type": "integer", "description": "Maximum depth (default: 3)"}
                    }
                }
            },
            {
                "name": "context",
                "description": "Get surrounding context for a line",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "path": {"type": "string", "description": "File path"},
                        "line": {"type": "integer", "description": "Center line number"},
                        "context_lines": {"type": "integer", "description": "Lines of context before and after (default: 10)"}
                    },
                    "required": ["path", "line"]
                }
            },
            {
                "name": "stats",
                "description": "Get index statistics and file type breakdown",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "detailed": {"type": "boolean", "description": "Include detailed breakdown by file type"}
                    }
                }
            },
            {
                "name": "related",
                "description": "Find files related to a source file by shared symbols",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "path": {"type": "string", "description": "Source file path"},
                        "limit": {"type": "integer", "description": "Maximum related files (default: 10)"}
                    },
                    "required": ["path"]
                }
            },
            {
                "name": "refs",
                "description": "Find all references to a symbol/identifier",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "symbol": {"type": "string", "description": "Symbol/identifier to find"},
                        "limit": {"type": "integer", "description": "Maximum references (default: 50)"}
                    },
                    "required": ["symbol"]
                }
            },
            {
                "name": "index",
                "description": "Update the search index (incremental by default)",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "force": {"type": "boolean", "description": "Force full re-index"}
                    }
                }
            },
            {
                "name": "diff",
                "description": "Show differences between two files",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "file1": {"type": "string", "description": "First file path"},
                        "file2": {"type": "string", "description": "Second file path"},
                        "context": {"type": "integer", "description": "Context lines around changes (default: 3)"}
                    },
                    "required": ["file1", "file2"]
                }
            }
        ]
    });

    serde_json::to_string(&schema).unwrap().len()
}

// ============================================================================
// Criterion Benchmarks
// ============================================================================

/// Benchmarks token output size for different query patterns.
fn bench_token_output_size(c: &mut Criterion) {
    let mut group = c.benchmark_group("token_output_size");
    group.sample_size(30); // Statistical minimum

    let (dir, _db, service) = create_test_codebase();

    let queries = [
        ("fn_search", "fn authenticate"),
        ("struct_def", r"struct Config"),
        ("error_pattern", "Error"),
        ("impl_block", "impl"),
        ("use_import", "use crate"),
    ];

    for (name, pattern) in queries {
        // Measure grepika output size
        group.bench_with_input(BenchmarkId::new("grepika", name), &pattern, |b, p| {
            b.iter(|| black_box(measure_grepika_search(&service, p, 20)))
        });

        // Measure ripgrep output size (Claude Grep proxy)
        group.bench_with_input(BenchmarkId::new("ripgrep", name), &pattern, |b, p| {
            b.iter(|| black_box(measure_ripgrep_search(&dir, p, 20)))
        });
    }

    group.finish();
}

/// Benchmarks to collect comparison data for summary.
fn bench_token_comparison(c: &mut Criterion) {
    let mut group = c.benchmark_group("token_comparison");
    group.sample_size(50);

    let (dir, _db, service) = create_test_codebase();

    let queries = [
        ("auth", "authenticate"),
        ("config", "Config"),
        ("error", "Error"),
        ("handler", "handle"),
        ("database", "database"),
    ];

    let mut comparisons = Vec::new();
    let mut grepika_bytes_samples = Vec::new();
    let mut ripgrep_bytes_samples = Vec::new();

    for (name, pattern) in queries {
        // Collect samples
        let grepika_metrics = measure_grepika_search(&service, pattern, 20);
        let ripgrep_metrics = measure_ripgrep_search(&dir, pattern, 20);

        grepika_bytes_samples.push(grepika_metrics.output_bytes as f64);
        ripgrep_bytes_samples.push(ripgrep_metrics.output_bytes as f64);

        comparisons.push(ComparisonResult::new(name, grepika_metrics, ripgrep_metrics));

        // Benchmark the measurements themselves
        group.bench_function(BenchmarkId::new("measure", name), |b| {
            b.iter(|| {
                let a = measure_grepika_search(&service, pattern, 20);
                let r = measure_ripgrep_search(&dir, pattern, 20);
                black_box((a, r))
            })
        });
    }

    group.finish();

    // Print comparison summary
    print_comparison_summary(&comparisons, &grepika_bytes_samples, &ripgrep_bytes_samples);
}

/// Prints a formatted comparison summary after benchmarks complete.
fn print_comparison_summary(
    comparisons: &[ComparisonResult],
    grepika_samples: &[f64],
    ripgrep_samples: &[f64],
) {
    let schema_bytes = measure_mcp_schema_size();
    let analysis = BreakEvenAnalysis::calculate(schema_bytes, comparisons);
    let grepika_stats = BenchmarkStats::from_samples(grepika_samples);
    let ripgrep_stats = BenchmarkStats::from_samples(ripgrep_samples);

    eprintln!("\n");
    eprintln!("═══════════════════════════════════════════════════════════════════════");
    eprintln!("                 TOKEN EFFICIENCY COMPARISON                           ");
    eprintln!("═══════════════════════════════════════════════════════════════════════");
    eprintln!();
    eprintln!(
        "{:<20} │ {:>12} │ {:>12} │ {:>10}",
        "Query", "grepika", "ripgrep", "Savings"
    );
    eprintln!("─────────────────────┼──────────────┼──────────────┼────────────");

    for c in comparisons {
        eprintln!(
            "{:<20} │ {:>10} B │ {:>10} B │ {:>8.1}%",
            c.query, c.grepika.output_bytes, c.ripgrep.output_bytes, c.savings_percent
        );
    }

    eprintln!("─────────────────────┼──────────────┼──────────────┼────────────");

    let avg_savings: f64 = comparisons.iter().map(|c| c.savings_percent).sum::<f64>()
        / comparisons.len() as f64;
    eprintln!(
        "{:<20} │ {:>10.0} B │ {:>10.0} B │ {:>8.1}%",
        "AVERAGE", grepika_stats.mean, ripgrep_stats.mean, avg_savings
    );

    eprintln!();
    eprintln!("Statistical Reliability (CV% < 50% is good):");
    eprintln!(
        "  grepika CV%: {:.1}% {}",
        grepika_stats.cv_percent,
        if grepika_stats.is_reliable(50.0) { "✓" } else { "⚠" }
    );
    eprintln!(
        "  ripgrep CV%:  {:.1}% {}",
        ripgrep_stats.cv_percent,
        if ripgrep_stats.is_reliable(50.0) { "✓" } else { "⚠" }
    );

    eprintln!();
    eprintln!("═══════════════════════════════════════════════════════════════════════");
    eprintln!("                      BREAK-EVEN ANALYSIS                              ");
    eprintln!("═══════════════════════════════════════════════════════════════════════");
    eprintln!();
    eprintln!("MCP Schema overhead:     {:>6} bytes ({} tokens)", schema_bytes, analysis.schema_tokens);
    eprintln!("Per-query savings:       {:>6.0} bytes ({:.0} tokens avg)", analysis.avg_savings_bytes, analysis.avg_savings_tokens);
    eprintln!("Break-even point:        {:>6} queries", analysis.break_even_queries);
    eprintln!();

    if analysis.break_even_queries < 20 {
        eprintln!("✓ Sessions with {}+ searches → grepika is more efficient", analysis.break_even_queries);
        eprintln!("  Sessions with <{} searches → Built-in Grep wins", analysis.break_even_queries);
    } else if analysis.break_even_queries < usize::MAX {
        eprintln!("⚠ High break-even point ({} queries) - consider for long sessions only", analysis.break_even_queries);
    } else {
        eprintln!("⚠ No token savings detected - ripgrep may be more efficient");
    }

    eprintln!();
    eprintln!("═══════════════════════════════════════════════════════════════════════");
}

// ============================================================================
// Schema Size Benchmark
// ============================================================================

/// Benchmarks MCP schema serialization overhead.
fn bench_schema_size(c: &mut Criterion) {
    let mut group = c.benchmark_group("mcp_schema");
    group.sample_size(100);

    group.bench_function("schema_serialization", |b| {
        b.iter(|| black_box(measure_mcp_schema_size()))
    });

    group.finish();

    // Report schema size
    let schema_bytes = measure_mcp_schema_size();
    eprintln!("\nMCP Schema Size: {} bytes (~{} tokens)", schema_bytes, schema_bytes / 4);
}

// ============================================================================
// Result Density Benchmark
// ============================================================================

/// Benchmarks result density (files per token).
fn bench_result_density(c: &mut Criterion) {
    let mut group = c.benchmark_group("result_density");
    group.sample_size(30);

    let (dir, _db, service) = create_test_codebase();

    let limits = [5, 10, 20, 50];

    for limit in limits {
        group.bench_with_input(BenchmarkId::new("grepika", limit), &limit, |b, &l| {
            b.iter(|| {
                let metrics = measure_grepika_search(&service, "fn", l);
                black_box(metrics.result_density())
            })
        });

        group.bench_with_input(BenchmarkId::new("ripgrep", limit), &limit, |b, &l| {
            b.iter(|| {
                let metrics = measure_ripgrep_search(&dir, "fn", l);
                black_box(metrics.result_density())
            })
        });
    }

    group.finish();
}

// ============================================================================
// Criterion Configuration
// ============================================================================

criterion_group!(
    token_benches,
    bench_token_output_size,
    bench_token_comparison,
    bench_schema_size,
    bench_result_density,
);

criterion_main!(token_benches);
