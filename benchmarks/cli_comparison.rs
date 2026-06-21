//! CLI comparison benchmarks.
//!
//! Contains three benchmark categories:
//! 1. **Library search** (Criterion) — stable, CI-friendly, the primary benchmark
//! 2. **Subprocess comparison** (standalone) — `grepika search` vs `rg`, opt-in via `BENCH_SUBPROCESS=1`
//! 3. **Incremental index** (Criterion) — measures the "no changes" fast path
//!
//! Run with: `cargo bench --bench cli_comparison`
//! Full subprocess comparison: `BENCH_SUBPROCESS=1 cargo bench --bench cli_comparison`

use criterion::{black_box, criterion_group, BenchmarkId, Criterion};
use grepika::bench_utils::BenchmarkStats;
use grepika::db::Database;
use grepika::services::{Indexer, SearchService, TrigramIndex};
use grepika::tools::{SearchInput, SearchMode};
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Arc, RwLock};
use std::time::Instant;
use tempfile::TempDir;

// ============================================================================
// Shared Setup
// ============================================================================

/// Benchmark queries covering all QueryIntent categories.
const BENCHMARK_QUERIES: &[(&str, &str)] = &[
    ("SearchService", "SearchService"),
    ("fn", "fn"),
    ("error handling", "error handling"),
    (r"fn\s+\w+", r"fn\s+\w+"),
];

/// Sets up a real codebase with indexing for benchmarks.
fn setup_real_codebase() -> (PathBuf, Arc<Database>, Arc<SearchService>) {
    let root = std::env::var("BENCH_REPO_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(env!("CARGO_MANIFEST_DIR")));

    let db_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("bench_cache");
    fs::create_dir_all(&db_dir).expect("bench cache dir");
    let db_path = db_dir.join("cli_comparison.db");
    let db = Arc::new(Database::open(&db_path).expect("open DB"));

    let trigram = Arc::new(RwLock::new(TrigramIndex::new()));
    let indexer = Indexer::new(Arc::clone(&db), Arc::clone(&trigram), root.clone());
    indexer.index(None, false).expect("index");
    load_trigrams_if_empty(&db, &trigram);

    let search = Arc::new(
        SearchService::with_trigram(Arc::clone(&db), root.clone(), Arc::clone(&trigram))
            .expect("search service"),
    );
    (root, db, search)
}

/// Returns an indexed temporary git repo plus one tracked file to mutate.
fn setup_git_changed_file_fixture(
    file_count: usize,
) -> (TempDir, Arc<Database>, Arc<RwLock<TrigramIndex>>, PathBuf) {
    let dir = TempDir::new().expect("git fixture dir");
    let repo = dir.path().join("repo");
    fs::create_dir(&repo).expect("git fixture repo");
    Command::new("git")
        .args(["init"])
        .current_dir(&repo)
        .output()
        .expect("git init");
    Command::new("git")
        .args(["config", "user.email", "bench@example.com"])
        .current_dir(&repo)
        .output()
        .expect("git config email");
    Command::new("git")
        .args(["config", "user.name", "Bench"])
        .current_dir(&repo)
        .output()
        .expect("git config name");

    for i in 0..file_count {
        fs::write(
            repo.join(format!("file_{i}.rs")),
            format!("fn file_{i}() {{ let value = {i}; }}\n"),
        )
        .expect("fixture file");
    }

    Command::new("git")
        .args(["add", "."])
        .current_dir(&repo)
        .output()
        .expect("git add");
    Command::new("git")
        .args(["commit", "-m", "initial"])
        .current_dir(&repo)
        .output()
        .expect("git commit");

    let db = Arc::new(Database::open(&dir.path().join("index.db")).expect("open fixture DB"));
    let trigram = Arc::new(RwLock::new(TrigramIndex::new()));
    let indexer = Indexer::new(Arc::clone(&db), Arc::clone(&trigram), repo.clone());
    indexer.index(None, false).expect("initial fixture index");
    load_trigrams_if_empty(&db, &trigram);

    let target = repo.join("file_0.rs");
    (dir, db, trigram, target)
}

fn load_trigrams_if_empty(db: &Database, trigram: &Arc<RwLock<TrigramIndex>>) {
    let mut guard = trigram.write().unwrap_or_else(|e| e.into_inner());
    if guard.trigram_count() == 0 {
        *guard = TrigramIndex::from_db_entries(db.load_all_trigrams().expect("load trigrams"));
    }
}

// ============================================================================
// Phase 3a: Library-level search benchmarks (Criterion)
// ============================================================================

fn bench_library_search(c: &mut Criterion) {
    let mut group = c.benchmark_group("library_search");
    group.sample_size(50);

    let (_root, _db, search) = setup_real_codebase();

    for &(name, query) in BENCHMARK_QUERIES {
        let preflight = search.search(query, 20).expect("combined search preflight");
        assert!(!preflight.is_empty(), "combined search returned no results");
        // Combined mode (default)
        group.bench_function(BenchmarkId::new("combined", name), |b| {
            b.iter(|| {
                let input = SearchInput {
                    query: query.to_string(),
                    limit: 20,
                    mode: SearchMode::Combined,
                };
                black_box(grepika::tools::execute_search(&search, input).expect("combined search"))
            })
        });
    }

    assert!(
        !search
            .search_fts("SearchService", 20)
            .expect("fts preflight")
            .is_empty(),
        "fts preflight returned no results"
    );
    // Also benchmark individual modes for "SearchService" query
    group.bench_function("fts_only/SearchService", |b| {
        b.iter(|| {
            let input = SearchInput {
                query: "SearchService".to_string(),
                limit: 20,
                mode: SearchMode::Fts,
            };
            black_box(grepika::tools::execute_search(&search, input).expect("fts search"))
        })
    });

    assert!(
        !search
            .search_grep("SearchService", 20)
            .expect("grep preflight")
            .is_empty(),
        "grep preflight returned no results"
    );
    group.bench_function("grep_only/SearchService", |b| {
        b.iter(|| {
            let input = SearchInput {
                query: "SearchService".to_string(),
                limit: 20,
                mode: SearchMode::Grep,
            };
            black_box(grepika::tools::execute_search(&search, input).expect("grep search"))
        })
    });

    group.finish();
}

// ============================================================================
// Phase 3b: Subprocess comparison (standalone, NOT Criterion)
// ============================================================================

fn rg_available() -> bool {
    Command::new("rg").arg("--version").output().is_ok()
}

fn grepika_binary() -> Option<PathBuf> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("release")
        .join("grepika");
    path.exists().then_some(path)
}

/// Runs the subprocess comparison. Called from main() only when BENCH_SUBPROCESS is set.
fn run_subprocess_comparison() {
    let root =
        std::env::var("BENCH_REPO_PATH").unwrap_or_else(|_| env!("CARGO_MANIFEST_DIR").to_string());
    let db_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("bench_cache")
        .join("cli_comparison_subprocess.db");
    let db_path = db_path.to_string_lossy().to_string();

    let grepika_bin = match grepika_binary() {
        Some(p) => p,
        None => {
            eprintln!("\n⚠ Skipping subprocess comparison: ./target/release/grepika not found");
            eprintln!("  Build with: cargo build --release");
            return;
        }
    };

    let index_output = Command::new(&grepika_bin)
        .args(["--root", root.as_str(), "--db", db_path.as_str(), "index"])
        .output()
        .unwrap_or_else(|err| panic!("failed to index subprocess benchmark DB: {err}"));
    if !index_output.status.success() {
        panic!(
            "failed to index subprocess benchmark DB: status={} stderr={}",
            index_output.status,
            String::from_utf8_lossy(&index_output.stderr).trim()
        );
    }

    let has_rg = rg_available();
    if !has_rg {
        eprintln!("\n⚠ ripgrep not found — subprocess comparison will show grepika only");
    }

    const N: usize = 20;

    eprintln!();
    eprintln!("═══════════════════════════════════════════════════════════════════════");
    eprintln!("                CLI vs RIPGREP COMPARISON (N={N})");
    eprintln!("═══════════════════════════════════════════════════════════════════════");
    eprintln!();

    if has_rg {
        eprintln!(
            "{:<20} │ {:>14} │ {:>14} │ {:>13} │ {:>10}",
            "Query", "grepika (ms)", "rg (ms)", "grepika (B)", "rg (B)"
        );
        eprintln!(
            "─────────────────────┼────────────────┼────────────────┼───────────────┼────────────"
        );
    } else {
        eprintln!(
            "{:<20} │ {:>14} │ {:>13}",
            "Query", "grepika (ms)", "grepika (B)"
        );
        eprintln!("─────────────────────┼────────────────┼───────────────");
    }

    let mut all_grepika_cv = Vec::new();
    let mut all_rg_cv = Vec::new();

    for &(name, query) in BENCHMARK_QUERIES {
        // Measure grepika
        let mut grepika_times = Vec::new();
        let mut grepika_bytes = 0usize;
        for _ in 0..N {
            let start = Instant::now();
            let output = Command::new(&grepika_bin)
                .args([
                    "--root",
                    root.as_str(),
                    "--db",
                    db_path.as_str(),
                    "search",
                    "--json",
                    query,
                ])
                .output()
                .unwrap_or_else(|err| panic!("failed to run grepika for '{name}': {err}"));
            let elapsed = start.elapsed();
            if !output.status.success() {
                panic!(
                    "grepika failed for '{}': status={} stderr={}",
                    name,
                    output.status,
                    String::from_utf8_lossy(&output.stderr).trim()
                );
            }
            grepika_times.push(elapsed.as_secs_f64() * 1000.0);
            grepika_bytes = output.stdout.len();
        }
        let grepika_stats = BenchmarkStats::from_samples(&grepika_times);
        all_grepika_cv.push(grepika_stats.cv_percent);

        if has_rg {
            // Measure ripgrep
            let mut rg_times = Vec::new();
            let mut rg_bytes = 0usize;
            for _ in 0..N {
                let start = Instant::now();
                let output = Command::new("rg")
                    .args(["--json", query])
                    .current_dir(&root)
                    .output();
                let elapsed = start.elapsed();
                if let Ok(o) = output {
                    rg_times.push(elapsed.as_secs_f64() * 1000.0);
                    rg_bytes = o.stdout.len();
                }
            }
            let rg_stats = BenchmarkStats::from_samples(&rg_times);
            all_rg_cv.push(rg_stats.cv_percent);

            eprintln!(
                "{:<20} │ {:>6.1} ± {:<5.1} │ {:>6.1} ± {:<5.1} │ {:>11} │ {:>8}",
                name,
                grepika_stats.median,
                grepika_stats.std_dev,
                rg_stats.median,
                rg_stats.std_dev,
                format_bytes(grepika_bytes),
                format_bytes(rg_bytes),
            );
        } else {
            eprintln!(
                "{:<20} │ {:>6.1} ± {:<5.1} │ {:>11}",
                name,
                grepika_stats.median,
                grepika_stats.std_dev,
                format_bytes(grepika_bytes),
            );
        }
    }

    // Statistical reliability
    eprintln!();
    eprintln!("Statistical reliability (CV% < 30% is good):");
    let avg_grepika_cv: f64 =
        all_grepika_cv.iter().sum::<f64>() / all_grepika_cv.len().max(1) as f64;
    let grepika_reliable = avg_grepika_cv < 30.0;
    eprintln!(
        "  grepika CV%: {:.1}% {}",
        avg_grepika_cv,
        if grepika_reliable { "✓" } else { "⚠" }
    );

    if has_rg && !all_rg_cv.is_empty() {
        let avg_rg_cv: f64 = all_rg_cv.iter().sum::<f64>() / all_rg_cv.len() as f64;
        let rg_reliable = avg_rg_cv < 30.0;
        eprintln!(
            "  ripgrep CV%: {:.1}% {}",
            avg_rg_cv,
            if rg_reliable { "✓" } else { "⚠" }
        );
    }

    eprintln!();
    eprintln!("═══════════════════════════════════════════════════════════════════════");
}

fn format_bytes(bytes: usize) -> String {
    if bytes >= 1_000_000 {
        format!("{:.1}MB", bytes as f64 / 1_000_000.0)
    } else if bytes >= 1_000 {
        format!("{:.1}KB", bytes as f64 / 1_000.0)
    } else {
        format!("{bytes}B")
    }
}

// ============================================================================
// Phase 3c: Incremental index benchmark (Criterion)
// ============================================================================

fn bench_incremental_index(c: &mut Criterion) {
    let mut group = c.benchmark_group("incremental_index");
    group.sample_size(20); // Indexing touches disk

    let (_git_dir, git_db, git_trigram, changed_file) = setup_git_changed_file_fixture(1_000);
    let git_indexer = Indexer::new(
        Arc::clone(&git_db),
        Arc::clone(&git_trigram),
        changed_file.parent().unwrap().to_path_buf(),
    );
    let (_many_dir, many_db, many_trigram, many_first_file) = setup_git_changed_file_fixture(1_000);
    let many_root = many_first_file.parent().unwrap().to_path_buf();
    let many_indexer = Indexer::new(
        Arc::clone(&many_db),
        Arc::clone(&many_trigram),
        many_root.clone(),
    );
    let many_changed_files: Vec<PathBuf> = (0..100)
        .map(|i| many_root.join(format!("file_{i}.rs")))
        .collect();
    let (_force_dir, force_db, force_trigram, force_file) = setup_git_changed_file_fixture(100);
    let force_indexer = Indexer::new(
        Arc::clone(&force_db),
        Arc::clone(&force_trigram),
        force_file.parent().unwrap().to_path_buf(),
    );
    let mut toggle = false;
    let mut many_toggle = false;

    // Benchmark: no-op re-index (all files unchanged, xxHash match)
    group.bench_function("no_changes", |b| {
        b.iter(|| black_box(git_indexer.index(None, false).expect("no-change index")))
    });

    group.bench_function("one_changed_file_git_fast_path", |b| {
        b.iter(|| {
            toggle = !toggle;
            let version = if toggle { "a" } else { "b" };
            fs::write(
                &changed_file,
                format!("fn file_0() {{ let changed_value = \"{version}\"; }}\n"),
            )
            .expect("toggle changed file");
            black_box(
                git_indexer
                    .index(None, false)
                    .expect("one changed file git fast path"),
            )
        })
    });

    group.bench_function("many_changed_files_git_fast_path", |b| {
        b.iter(|| {
            many_toggle = !many_toggle;
            let version = if many_toggle { "a" } else { "b" };
            for (i, path) in many_changed_files.iter().enumerate() {
                fs::write(
                    path,
                    format!(
                        "fn file_{i}() {{ file_999(); let changed_value_{i} = \"{version}\"; }}\n"
                    ),
                )
                .expect("toggle changed file batch");
            }
            black_box(
                many_indexer
                    .index(None, false)
                    .expect("many changed files git fast path"),
            )
        })
    });

    // Benchmark: force full re-index
    group.bench_function("force_reindex", |b| {
        b.iter(|| black_box(force_indexer.index(None, true).expect("force reindex")))
    });

    group.finish();
}

// ============================================================================
// Criterion Configuration + main
// ============================================================================

criterion_group!(search_benches, bench_library_search);
criterion_group!(index_benches, bench_incremental_index);

fn main() {
    // Criterion benchmarks first (library-level)
    let mut criterion = Criterion::default().configure_from_args();
    bench_library_search(&mut criterion);
    bench_incremental_index(&mut criterion);
    criterion.final_summary();

    // Standalone subprocess comparison (only if BENCH_SUBPROCESS is set)
    if std::env::var("BENCH_SUBPROCESS").is_ok() {
        run_subprocess_comparison();
    }
}
