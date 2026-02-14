//! Token efficiency benchmark comparing grepika vs Claude's built-in Grep.
//!
//! This benchmark measures output token cost, not execution time.
//! The goal is to quantify the break-even point where MCP schema overhead
//! is offset by per-query token savings.
//!
//! Uses the **real grepika codebase** (or `BENCH_REPO_PATH`) with queries
//! covering all 4 `QueryIntent` categories.
//!
//! Run with: `cargo bench --bench token_efficiency`
//! View reports: `open target/criterion/report/index.html`

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use grepika::bench_utils::{BenchmarkStats, BreakEvenAnalysis, ComparisonResult, TokenMetrics};
use grepika::db::Database;
use grepika::server::GrepikaServer;
use grepika::services::{Indexer, SearchService, TrigramIndex};
use grepika::tools::{MatchSnippetOutput, SearchOutput, SearchResultItem};
use rmcp::model::{CallToolResult, RawContent};
use std::collections::HashSet;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{Arc, RwLock};

// ============================================================================
// Real Codebase Setup
// ============================================================================

/// Sets up a real codebase for benchmarking.
///
/// Uses the grepika repository itself by default, or `BENCH_REPO_PATH` if set.
/// Index is stored in `target/bench_cache/` for fast incremental reindexing.
fn setup_real_codebase() -> (PathBuf, Arc<Database>, SearchService) {
    let root = std::env::var("BENCH_REPO_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(env!("CARGO_MANIFEST_DIR")));

    let db_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("bench_cache");
    fs::create_dir_all(&db_dir).expect("bench cache dir");
    let db_path = db_dir.join("token_efficiency.db");
    let db = Arc::new(Database::open(&db_path).expect("open DB"));

    // Index (incremental — fast on rerun)
    let trigram = Arc::new(RwLock::new(TrigramIndex::new()));
    let indexer = Indexer::new(Arc::clone(&db), Arc::clone(&trigram), root.clone());
    indexer.index(None, false).expect("index");

    let search = SearchService::new(Arc::clone(&db), root.clone()).expect("search service");
    (root, db, search)
}

// ============================================================================
// Query Patterns (all 4 QueryIntent categories)
// ============================================================================

/// Benchmark queries covering all QueryIntent categories.
///
/// Format: (name, query, expected_intent_description)
const BENCHMARK_QUERIES: &[(&str, &str, &str)] = &[
    // ExactSymbol (single word >= 4 chars)
    ("exact_SearchService", "SearchService", "ExactSymbol"),
    ("exact_Score", "Score", "ExactSymbol"),
    ("exact_Database", "Database", "ExactSymbol"),
    // ShortToken (single word < 4 chars)
    ("short_fn", "fn", "ShortToken"),
    ("short_use", "use", "ShortToken"),
    // NaturalLanguage (multiple words)
    ("natural_search_flow", "search service", "NaturalLanguage"),
    (
        "natural_error_handling",
        "error handling",
        "NaturalLanguage",
    ),
    // Regex (metacharacters)
    ("regex_fn_def", r"fn\s+\w+", "Regex"),
    ("regex_impl_for", r"impl.*for", "Regex"),
];

// ============================================================================
// Grepika Output Measurement
// ============================================================================

/// Measures grepika search output size using the real search service.
fn measure_grepika_search(service: &SearchService, query: &str, limit: usize) -> TokenMetrics {
    let results = service.search(query, limit).unwrap_or_default();
    let root = service.root();

    let items: Vec<SearchResultItem> = results
        .iter()
        .map(|r| {
            let relative_path = r
                .path
                .strip_prefix(root)
                .unwrap_or(&r.path)
                .to_string_lossy()
                .to_string();

            let sources = r.sources.to_compact();

            let snippets: Vec<MatchSnippetOutput> = r
                .snippets
                .iter()
                .map(|s| MatchSnippetOutput {
                    line: s.line_number,
                    text: s.line_content.clone(),
                    highlight_start: s.match_start,
                    highlight_end: s.match_end,
                })
                .collect();

            SearchResultItem {
                path: relative_path,
                score: r.score.as_f64(),
                sources,
                snippets,
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
// Ripgrep Output Measurement
// ============================================================================

/// Returns true if ripgrep is available on the system.
fn rg_available() -> bool {
    Command::new("rg").arg("--version").output().is_ok()
}

/// Measures ripgrep output size (simulating Claude's Grep tool content mode).
///
/// Returns None if `rg` is not installed.
fn measure_ripgrep_search(root: &PathBuf, query: &str, limit: usize) -> Option<TokenMetrics> {
    let output = Command::new("rg")
        .args(["--json", "--max-count", &limit.to_string(), query])
        .current_dir(root)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .ok()?;

    if !output.status.success() && output.stdout.is_empty() {
        return Some(TokenMetrics::default());
    }

    let reader = BufReader::new(output.stdout.as_slice());
    let mut claude_output = String::new();
    let mut result_count = 0;
    let mut files_found = HashSet::new();

    for line in reader.lines().map_while(Result::ok) {
        if let Ok(json) = serde_json::from_str::<serde_json::Value>(&line) {
            if json.get("type").and_then(|t| t.as_str()) == Some("match") {
                if let Some(data) = json.get("data") {
                    if let Some(path) = data
                        .get("path")
                        .and_then(|p| p.get("text"))
                        .and_then(|t| t.as_str())
                    {
                        files_found.insert(path.to_string());
                    }
                    if let Some(lines) = data
                        .get("lines")
                        .and_then(|l| l.get("text"))
                        .and_then(|t| t.as_str())
                    {
                        let line_num = data
                            .get("line_number")
                            .and_then(|n| n.as_u64())
                            .unwrap_or(0);
                        claude_output.push_str(&format!(
                            "{}:{}: {}\n",
                            data.get("path")
                                .and_then(|p| p.get("text"))
                                .and_then(|t| t.as_str())
                                .unwrap_or(""),
                            line_num,
                            lines.trim()
                        ));
                        result_count += 1;
                    }
                }
            }
        }
    }

    Some(TokenMetrics::from_ripgrep_output(
        &claude_output,
        result_count,
        files_found.len(),
    ))
}

// ============================================================================
// Output Format Measurement (CLI, JSON, MCP JSON-RPC)
// ============================================================================

/// Measures a grepika search result in all three output formats.
///
/// Returns (cli_formatted_bytes, cli_json_bytes, mcp_jsonrpc_bytes).
fn measure_output_formats(
    service: &SearchService,
    query: &str,
    limit: usize,
) -> (usize, usize, usize) {
    let results = service.search(query, limit).unwrap_or_default();
    let root = service.root();

    let items: Vec<SearchResultItem> = results
        .iter()
        .map(|r| {
            let relative_path = r
                .path
                .strip_prefix(root)
                .unwrap_or(&r.path)
                .to_string_lossy()
                .to_string();

            let sources = r.sources.to_compact();

            let snippets: Vec<MatchSnippetOutput> = r
                .snippets
                .iter()
                .map(|s| MatchSnippetOutput {
                    line: s.line_number,
                    text: s.line_content.clone(),
                    highlight_start: s.match_start,
                    highlight_end: s.match_end,
                })
                .collect();

            SearchResultItem {
                path: relative_path,
                score: r.score.as_f64(),
                sources,
                snippets,
            }
        })
        .collect();

    let search_output = SearchOutput {
        results: items,
        has_more: false,
    };

    // 1. CLI formatted output (via grepika::fmt::fmt_search)
    let mut cli_buf = Vec::new();
    grepika::fmt::fmt_search(&mut cli_buf, &search_output, false).expect("fmt_search");
    let cli_bytes = cli_buf.len();

    // 2. CLI JSON output (what --json produces)
    let cli_json = serde_json::to_string(&search_output).expect("json serialize");
    let cli_json_bytes = cli_json.len();

    // 3. MCP JSON-RPC — exact production path from spawn_tool (server.rs)
    let json = serde_json::to_string(&search_output).expect("json serialize mcp");
    let result = CallToolResult::success(vec![rmcp::model::Content::text(json)]);
    let mcp_bytes: usize = result
        .content
        .iter()
        .map(|c| match &c.raw {
            RawContent::Text(t) => t.text.len(),
            _ => 0,
        })
        .sum();

    (cli_bytes, cli_json_bytes, mcp_bytes)
}

// ============================================================================
// MCP Schema Size Measurement (live extraction)
// ============================================================================

/// Extracts the actual MCP tool schema from GrepikaServer.
///
/// Stays in sync with schema changes automatically — no hand-crafted JSON.
fn measure_mcp_schema_size() -> usize {
    let server = GrepikaServer::new_empty(None);
    let tools = server.tool_schemas();
    serde_json::to_string(&tools).unwrap().len()
}

// ============================================================================
// Criterion Benchmarks
// ============================================================================

/// Benchmarks token output size for all QueryIntent categories.
fn bench_token_output_size(c: &mut Criterion) {
    let mut group = c.benchmark_group("token_output_size");
    group.sample_size(30);

    let (root, _db, service) = setup_real_codebase();
    let has_rg = rg_available();
    if !has_rg {
        eprintln!("\nripgrep not found — skipping rg comparison benchmarks");
    }

    for &(name, query, _intent) in BENCHMARK_QUERIES {
        group.bench_with_input(BenchmarkId::new("grepika", name), &query, |b, q| {
            b.iter(|| black_box(measure_grepika_search(&service, q, 20)))
        });

        if has_rg {
            group.bench_with_input(BenchmarkId::new("ripgrep", name), &query, |b, q| {
                b.iter(|| black_box(measure_ripgrep_search(&root, q, 20)))
            });
        }
    }

    group.finish();
}

/// Benchmarks and collects comparison data, then prints summary.
fn bench_token_comparison(c: &mut Criterion) {
    let mut group = c.benchmark_group("token_comparison");
    group.sample_size(50);

    let (root, _db, service) = setup_real_codebase();
    let has_rg = rg_available();

    let mut comparisons = Vec::new();
    let mut grepika_bytes_samples = Vec::new();
    let mut ripgrep_bytes_samples = Vec::new();

    for &(name, query, _intent) in BENCHMARK_QUERIES {
        let grepika_metrics = measure_grepika_search(&service, query, 20);
        grepika_bytes_samples.push(grepika_metrics.output_bytes as f64);

        if has_rg {
            if let Some(ripgrep_metrics) = measure_ripgrep_search(&root, query, 20) {
                ripgrep_bytes_samples.push(ripgrep_metrics.output_bytes as f64);
                comparisons.push(ComparisonResult::new(
                    name,
                    grepika_metrics,
                    ripgrep_metrics,
                ));
            }
        }

        group.bench_function(BenchmarkId::new("measure", name), |b| {
            b.iter(|| black_box(measure_grepika_search(&service, query, 20)))
        });
    }

    group.finish();

    // Print comparison summary (grepika-only if rg unavailable)
    print_comparison_summary(&comparisons, &grepika_bytes_samples, &ripgrep_bytes_samples);

    // Print output format comparison
    print_output_format_comparison(&service);
}

/// Benchmarks MCP schema serialization overhead using live extraction.
fn bench_schema_size(c: &mut Criterion) {
    let mut group = c.benchmark_group("mcp_schema");
    group.sample_size(100);

    group.bench_function("schema_serialization", |b| {
        b.iter(|| black_box(measure_mcp_schema_size()))
    });

    group.finish();

    let schema_bytes = measure_mcp_schema_size();
    eprintln!(
        "\nMCP Schema Size: {} bytes (~{} tokens)",
        schema_bytes,
        (schema_bytes + 2) / 4
    );
}

/// Benchmarks result density (files per token).
fn bench_result_density(c: &mut Criterion) {
    let mut group = c.benchmark_group("result_density");
    group.sample_size(30);

    let (_root, _db, service) = setup_real_codebase();

    let limits = [5, 10, 20, 50];

    for limit in limits {
        group.bench_with_input(BenchmarkId::new("grepika", limit), &limit, |b, &l| {
            b.iter(|| {
                let metrics = measure_grepika_search(&service, "fn", l);
                black_box(metrics.result_density())
            })
        });
    }

    group.finish();
}

// ============================================================================
// Summary Printing
// ============================================================================

fn print_comparison_summary(
    comparisons: &[ComparisonResult],
    grepika_samples: &[f64],
    ripgrep_samples: &[f64],
) {
    let schema_bytes = measure_mcp_schema_size();
    let grepika_stats = BenchmarkStats::from_samples(grepika_samples);

    eprintln!("\n");
    eprintln!("═══════════════════════════════════════════════════════════════════════");
    eprintln!("                 TOKEN EFFICIENCY COMPARISON                           ");
    eprintln!("═══════════════════════════════════════════════════════════════════════");
    eprintln!();

    if comparisons.is_empty() {
        eprintln!("ripgrep not available — showing grepika-only metrics");
        eprintln!();
        eprintln!("{:<24} │ {:>12}", "Query", "grepika (B)");
        eprintln!("─────────────────────────┼──────────────");
        for (i, sample) in grepika_samples.iter().enumerate() {
            let name = BENCHMARK_QUERIES.get(i).map(|q| q.0).unwrap_or("?");
            eprintln!("{:<24} │ {:>10.0} B", name, sample);
        }
    } else {
        let ripgrep_stats = BenchmarkStats::from_samples(ripgrep_samples);

        eprintln!(
            "{:<24} │ {:>12} │ {:>12} │ {:>10}",
            "Query", "grepika", "ripgrep", "Savings"
        );
        eprintln!("─────────────────────────┼──────────────┼──────────────┼────────────");

        for c in comparisons {
            eprintln!(
                "{:<24} │ {:>10} B │ {:>10} B │ {:>8.1}%",
                c.query, c.grepika.output_bytes, c.ripgrep.output_bytes, c.savings_percent
            );
        }

        eprintln!("─────────────────────────┼──────────────┼──────────────┼────────────");

        let avg_savings: f64 =
            comparisons.iter().map(|c| c.savings_percent).sum::<f64>() / comparisons.len() as f64;
        eprintln!(
            "{:<24} │ {:>10.0} B │ {:>10.0} B │ {:>8.1}%",
            "AVERAGE", grepika_stats.mean, ripgrep_stats.mean, avg_savings
        );

        eprintln!();
        eprintln!("Statistical Reliability (CV% < 50% is good):");
        eprintln!(
            "  grepika CV%: {:.1}% {}",
            grepika_stats.cv_percent,
            if grepika_stats.is_reliable(50.0) {
                "✓"
            } else {
                "⚠"
            }
        );
        eprintln!(
            "  ripgrep CV%: {:.1}% {}",
            ripgrep_stats.cv_percent,
            if ripgrep_stats.is_reliable(50.0) {
                "✓"
            } else {
                "⚠"
            }
        );

        // Break-even analysis (only meaningful with rg comparison data)
        let analysis = BreakEvenAnalysis::calculate(schema_bytes, comparisons);

        eprintln!();
        eprintln!("═══════════════════════════════════════════════════════════════════════");
        eprintln!("                      BREAK-EVEN ANALYSIS                              ");
        eprintln!("═══════════════════════════════════════════════════════════════════════");
        eprintln!();
        eprintln!(
            "MCP Schema overhead:     {:>6} bytes ({} tokens)",
            schema_bytes, analysis.schema_tokens
        );
        eprintln!(
            "Per-query savings:       {:>6.0} bytes ({:.0} tokens avg)",
            analysis.avg_savings_bytes, analysis.avg_savings_tokens
        );
        eprintln!(
            "Break-even (raw):        {:>6} queries",
            analysis.break_even_queries
        );
        eprintln!(
            "Break-even (cached):     {:>6} queries  (90% prompt cache discount)",
            analysis.cached_break_even_queries
        );
        eprintln!();

        if analysis.break_even_queries < 20 {
            eprintln!(
                "✓ Sessions with {}+ searches → grepika is more efficient",
                analysis.break_even_queries
            );
            eprintln!(
                "  With prompt caching: {}+ searches",
                analysis.cached_break_even_queries
            );
        } else if analysis.break_even_queries < usize::MAX {
            eprintln!(
                "⚠ High break-even point ({} queries) - consider for long sessions only",
                analysis.break_even_queries
            );
        } else {
            eprintln!("⚠ No token savings detected - ripgrep may be more efficient");
        }
    }

    eprintln!();
    eprintln!("═══════════════════════════════════════════════════════════════════════");
}

/// Prints a comparison of output sizes across CLI, JSON, and MCP formats.
fn print_output_format_comparison(service: &SearchService) {
    eprintln!();
    eprintln!("═══════════════════════════════════════════════════════════════════════");
    eprintln!("                  OUTPUT FORMAT COMPARISON                             ");
    eprintln!("═══════════════════════════════════════════════════════════════════════");
    eprintln!();
    eprintln!(
        "{:<24} │ {:>10} │ {:>10} │ {:>10} │ {:>10}",
        "Query", "CLI fmt", "CLI JSON", "MCP JSON", "MCP overhead"
    );
    eprintln!("─────────────────────────┼────────────┼────────────┼────────────┼────────────");

    for &(name, query, _intent) in BENCHMARK_QUERIES {
        let (cli, json, mcp) = measure_output_formats(service, query, 20);
        let overhead = if json > 0 {
            ((mcp as f64 - json as f64) / json as f64) * 100.0
        } else {
            0.0
        };
        eprintln!(
            "{:<24} │ {:>8} B │ {:>8} B │ {:>8} B │ {:>8.1}%",
            name, cli, json, mcp, overhead
        );
    }

    eprintln!();
    eprintln!("═══════════════════════════════════════════════════════════════════════");
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
