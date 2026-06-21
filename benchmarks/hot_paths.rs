//! Performance benchmarks for grepika hot paths.
//!
//! Measures the core operations that dominate runtime:
//! - Trigram index search and addition
//! - Score merging from multiple sources
//! - FTS5 search performance
//! - Grep parallel search
//! - Structural ast-grep search
//!
//! Run with: `cargo bench`
//! View reports: `open target/criterion/report/index.html`

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use grepika::db::{Database, FileGraphBatchItem, SymbolRow};
use grepika::services::ast::RawCall;
use grepika::services::indexer::index_path_key;
use grepika::services::{GrepService, Indexer, SearchService, TrigramIndex};
use grepika::tools::{
    execute_structural_search, StructuralLanguage, StructuralQuery, StructuralSearchInput,
    STRUCTURAL_DEFAULT_TIMEOUT_MS,
};
use grepika::types::{FileId, Score};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::Duration;
use tempfile::TempDir;

// ============================================================================
// Trigram Index Benchmarks
// ============================================================================

/// Benchmarks trigram search at different index sizes.
///
/// This measures the core hot path of intersecting RoaringBitmaps
/// across multiple trigrams.
fn bench_trigram_search(c: &mut Criterion) {
    let mut group = c.benchmark_group("trigram_search");

    for size in [100, 1_000, 10_000, 100_000] {
        // Setup: populate index with `size` files
        let mut index = TrigramIndex::new();
        for i in 0..size {
            // Realistic code content with common patterns
            let content = format!(
                r#"
                fn function_{i}() {{
                    let config = Config::load();
                    authenticate(&config)?;
                    authorize(&config)?;
                    println!("Processing item {i}");
                }}
                "#,
                i = i
            );
            index.add_file(FileId::new(i as u32), &content);
        }

        group.throughput(Throughput::Elements(1));
        group.bench_with_input(BenchmarkId::from_parameter(size), &index, |b, index| {
            b.iter(|| {
                // Search for "auth" - common pattern, matches many files
                black_box(index.search("auth"))
            })
        });
    }

    group.finish();
}

/// Benchmarks trigram search with queries of varying length.
///
/// Longer queries produce more trigrams to intersect, which
/// can affect performance.
fn bench_trigram_query_length(c: &mut Criterion) {
    let mut group = c.benchmark_group("trigram_query_length");

    // Setup: create index with realistic content
    let mut index = TrigramIndex::new();
    for i in 0..1000 {
        let content = format!(
            r#"
            fn authenticate_user_{i}() {{
                let authorization_token = get_token();
                validate_authentication(&authorization_token);
            }}
            "#,
            i = i
        );
        index.add_file(FileId::new(i), &content);
    }

    // Test different query lengths
    let queries = [
        ("3_chars", "aut"),
        ("5_chars", "authe"),
        ("10_chars", "authentica"),
        ("15_chars", "authentication_"),
    ];

    for (name, query) in queries {
        group.bench_with_input(
            BenchmarkId::new("query", name),
            &(query, &index),
            |b, (q, idx)| b.iter(|| black_box(idx.search(q))),
        );
    }

    group.finish();
}

/// Benchmarks adding files to the trigram index.
///
/// This is the indexing hot path - called for every file during
/// initial index build.
fn bench_trigram_add_file(c: &mut Criterion) {
    let mut group = c.benchmark_group("trigram_add_file");

    // Different content sizes
    let small_content = "fn main() { println!(\"hello\"); }";
    let medium_content = r#"
        use std::collections::HashMap;

        fn process_data(data: &[u8]) -> Result<Output, Error> {
            let mut map: HashMap<String, Value> = HashMap::new();
            for chunk in data.chunks(1024) {
                let parsed = parse_chunk(chunk)?;
                map.insert(parsed.key, parsed.value);
            }
            Ok(Output { data: map })
        }
    "#;
    let large_content: String = (0..100)
        .map(|i| {
            format!(
                "fn function_{i}() {{ let x = {i}; println!(\"value: {{}}\", x); }}\n",
                i = i
            )
        })
        .collect();

    let contents = [
        ("small_50b", small_content.to_string()),
        ("medium_500b", medium_content.to_string()),
        ("large_5kb", large_content),
    ];

    for (name, content) in contents {
        group.throughput(Throughput::Bytes(content.len() as u64));
        group.bench_with_input(
            BenchmarkId::new("content_size", name),
            &content,
            |b, content| {
                b.iter_batched(
                    TrigramIndex::new,
                    |mut index| {
                        index.add_file(FileId::new(1), black_box(content));
                        index
                    },
                    criterion::BatchSize::SmallInput,
                )
            },
        );
    }

    group.finish();
}

/// Benchmarks updating an existing file in the trigram index.
///
/// Incremental indexing frequently revisits files that changed only slightly,
/// so unchanged n-gram postings should not be dirtied or rewritten.
fn bench_trigram_update_file(c: &mut Criterion) {
    let mut group = c.benchmark_group("trigram_update_file");

    let original = "authentication authorization password reset workflow ".repeat(40);
    let edited = format!("{original}newtoken");

    group.bench_function("readd_identical", |b| {
        b.iter_batched(
            || {
                let mut index = TrigramIndex::new();
                index.add_file(FileId::new(1), &original);
                let _ = index.take_dirty_entries();
                index
            },
            |mut index| {
                index.add_file(FileId::new(1), black_box(&original));
                black_box(index.dirty_count())
            },
            criterion::BatchSize::SmallInput,
        )
    });

    group.bench_function("tiny_edit", |b| {
        b.iter_batched(
            || {
                let mut index = TrigramIndex::new();
                index.add_file(FileId::new(1), &original);
                let _ = index.take_dirty_entries();
                index
            },
            |mut index| {
                index.add_file(FileId::new(1), black_box(&edited));
                black_box(index.dirty_count())
            },
            criterion::BatchSize::SmallInput,
        )
    });

    group.finish();
}

// ============================================================================
// Score Merging Benchmarks
// ============================================================================

/// Benchmarks score merging and weighting operations.
///
/// This is called during result combination from FTS, grep, and trigram
/// search results.
fn bench_score_operations(c: &mut Criterion) {
    let mut group = c.benchmark_group("score_operations");

    // Benchmark score creation
    group.bench_function("new", |b| b.iter(|| Score::new(black_box(0.75))));

    // Benchmark score merging
    group.bench_function("merge", |b| {
        let s1 = Score::new(0.4);
        let s2 = Score::new(0.3);
        b.iter(|| black_box(s1).merge(black_box(s2)))
    });

    // Benchmark weighted score
    group.bench_function("weighted", |b| {
        let score = Score::new(0.8);
        b.iter(|| black_box(score).weighted(black_box(0.4)))
    });

    // Benchmark combined operation (typical merge flow)
    group.bench_function("merge_weighted_chain", |b| {
        let s1 = Score::new(0.7);
        let s2 = Score::new(0.5);
        let s3 = Score::new(0.3);
        b.iter(|| {
            black_box(s1)
                .weighted(0.4)
                .merge(black_box(s2).weighted(0.4))
                .merge(black_box(s3).weighted(0.2))
        })
    });

    group.finish();
}

/// Benchmarks simulated result merging across multiple sources.
///
/// This simulates the `merge_results` function in SearchService.
fn bench_result_merging(c: &mut Criterion) {
    let mut group = c.benchmark_group("result_merging");

    for result_count in [10, 100, 1000] {
        // Simulate FTS results
        let fts_results: Vec<(FileId, Score)> = (0..result_count)
            .map(|i| (FileId::new(i as u32), Score::new(0.8 - (i as f64 * 0.001))))
            .collect();

        // Simulate grep results (partial overlap)
        let grep_results: Vec<(FileId, Score)> = (result_count / 2..result_count * 3 / 2)
            .map(|i| (FileId::new(i as u32), Score::new(0.7 - (i as f64 * 0.001))))
            .collect();

        // Simulate trigram file IDs (bitmap intersection result)
        let trigram_ids: Vec<u32> = (result_count / 4..result_count).map(|i| i as u32).collect();

        group.throughput(Throughput::Elements(result_count as u64));
        group.bench_with_input(
            BenchmarkId::from_parameter(result_count),
            &(fts_results, grep_results, trigram_ids),
            |b, (fts, grep, trigram)| {
                b.iter(|| {
                    // Simulate merging logic
                    let mut scores: std::collections::HashMap<u32, Score> =
                        std::collections::HashMap::new();

                    // Add FTS results with weight
                    for (id, score) in fts.iter() {
                        let entry = scores.entry(id.as_u32()).or_insert(Score::ZERO);
                        *entry = entry.merge(score.weighted(0.4));
                    }

                    // Add grep results with weight
                    for (id, score) in grep.iter() {
                        let entry = scores.entry(id.as_u32()).or_insert(Score::ZERO);
                        *entry = entry.merge(score.weighted(0.4));
                    }

                    // Add trigram boost
                    for id in trigram.iter() {
                        let entry = scores.entry(*id).or_insert(Score::ZERO);
                        *entry = entry.merge(Score::new(0.5).weighted(0.2));
                    }

                    black_box(scores)
                })
            },
        );
    }

    group.finish();
}

// ============================================================================
// FTS Search Benchmarks
// ============================================================================

/// Benchmarks FTS5 full-text search.
///
/// Tests BM25 ranking performance at different database sizes.
fn bench_fts_search(c: &mut Criterion) {
    let mut group = c.benchmark_group("fts_search");

    for file_count in [100, 500, 1000] {
        let db = Database::in_memory().expect("Failed to create database");

        // Populate with realistic content
        for i in 0..file_count {
            let content = format!(
                r#"
                // File {i}
                fn function_{i}() {{
                    authenticate();
                    authorize();
                    process();
                }}

                struct Config_{i} {{
                    api_key: String,
                    timeout: u64,
                }}
                "#,
                i = i
            );
            db.upsert_file(&format!("file_{}.rs", i), &content, i as u64)
                .expect("Failed to insert file");
        }

        group.throughput(Throughput::Elements(1));
        group.bench_with_input(BenchmarkId::from_parameter(file_count), &db, |b, db| {
            b.iter(|| {
                // Search for common pattern
                black_box(db.fts_search("authenticate*", 20))
            })
        });
    }

    group.finish();
}

/// Benchmarks FTS search with different query complexities.
fn bench_fts_query_complexity(c: &mut Criterion) {
    let mut group = c.benchmark_group("fts_query_complexity");

    let db = Database::in_memory().expect("Failed to create database");

    // Populate with varied content
    for i in 0..500 {
        let content = format!(
            r#"
            fn authenticate_{i}() {{ process_user(); }}
            fn authorize_{i}() {{ check_permissions(); }}
            fn process_data_{i}() {{ handle_request(); }}
            struct Config_{i} {{ api_key: String, timeout: u64 }}
            "#,
            i = i
        );
        db.upsert_file(&format!("file_{}.rs", i), &content, i as u64)
            .expect("Failed to insert file");
    }

    let queries = [
        ("single_term", "authenticate*"),
        ("two_terms", "authenticate* OR authorize*"),
        ("phrase", "\"process_user\""),
    ];

    for (name, query) in queries {
        group.bench_with_input(
            BenchmarkId::new("query", name),
            &(query, &db),
            |b, (q, db)| b.iter(|| black_box(db.fts_search(q, 20))),
        );
    }

    group.finish();
}

// ============================================================================
// Combined Search Service Benchmarks
// ============================================================================

/// Creates benchmark files on disk and inserts them into the database.
///
/// Each file contains realistic Rust code with common patterns like
/// `authenticate`, `Config`, and `Handler` — used by search benchmarks.
fn setup_bench_files(dir: &std::path::Path, db: &Database, count: usize) -> TrigramIndex {
    let mut trigram = TrigramIndex::new();

    for i in 0..count {
        let content = format!(
            r#"
            // Source file {i}
            fn main_function_{i}() {{
                let config = Config::new();
                authenticate(&config)?;
                process_request(&config)?;
                Ok(())
            }}

            pub struct Handler_{i} {{
                state: State,
            }}

            impl Handler_{i} {{
                pub fn new() -> Self {{
                    Self {{ state: State::default() }}
                }}
            }}
            "#,
            i = i
        );

        let filename = format!("file_{}.rs", i);
        let path = dir.join(&filename);
        fs::write(&path, &content).expect("Failed to write file");
        let path_key = index_path_key(dir, &path);
        let file_id = db
            .upsert_file(&path_key, &content, i as u64)
            .expect("Failed to insert file");
        trigram.add_file(file_id, &content);
    }

    trigram
}

fn bench_search_service(dir: &std::path::Path, db: Arc<Database>, count: usize) -> SearchService {
    let trigram = Arc::new(RwLock::new(setup_bench_files(dir, &db, count)));
    SearchService::with_trigram(db, dir.to_path_buf(), trigram)
        .expect("Failed to create search service")
}

fn assert_nonempty_search(search: &SearchService, query: &str, limit: usize) {
    let results = search
        .search(query, limit)
        .expect("combined search preflight");
    assert!(!results.is_empty(), "combined search returned no results");
}

fn assert_nonempty_fts(search: &SearchService, query: &str, limit: usize) {
    let results = search
        .search_fts(query, limit)
        .expect("fts search preflight");
    assert!(!results.is_empty(), "fts search returned no results");
}

fn assert_nonempty_grep(search: &SearchService, query: &str, limit: usize) {
    let results = search
        .search_grep(query, limit)
        .expect("grep search preflight");
    assert!(!results.is_empty(), "grep search returned no results");
}

fn assert_direct_candidate_sources(search: &SearchService) {
    let results = search
        .search("unique_direct_candidate_token_1999", 20)
        .expect("direct candidate search preflight");
    assert!(
        results
            .iter()
            .any(|result| result.sources.trigram && result.sources.grep),
        "combined search did not include trigram+grep results"
    );
}

fn setup_direct_candidate_files(
    dir: &std::path::Path,
    db: &Database,
    count: usize,
) -> TrigramIndex {
    let mut trigram = TrigramIndex::new();
    for i in 0..count {
        let content = if i + 1 == count {
            format!("fn target_{i}() {{ let unique_direct_candidate_token_{i} = true; }}\n")
        } else {
            format!("fn file_{i}() {{ let common_value = {i}; }}\n")
        };
        let path = dir.join(format!("file_{i}.rs"));
        fs::write(&path, &content).expect("Failed to write file");
        let path_key = index_path_key(dir, &path);
        let file_id = db
            .upsert_file(&path_key, &content, i as u64)
            .expect("Failed to insert file");
        trigram.add_file(file_id, &content);
    }
    trigram
}

/// Benchmarks the full combined search pipeline at 200 files.
///
/// This is the most realistic benchmark - it measures end-to-end
/// search performance including FTS, grep, and result merging.
fn bench_combined_search(c: &mut Criterion) {
    let mut group = c.benchmark_group("combined_search");
    group.sample_size(50); // Reduce sample size for slower benchmarks

    let dir = TempDir::new().expect("Failed to create temp dir");
    let db = Arc::new(Database::in_memory().expect("Failed to create database"));
    let search = bench_search_service(dir.path(), Arc::clone(&db), 200);
    assert_nonempty_search(&search, "authenticate", 20);
    assert_nonempty_fts(&search, "authenticate", 20);
    assert_nonempty_grep(&search, "authenticate", 20);

    group.throughput(Throughput::Elements(1));
    group.bench_function("combined_200_files", |b| {
        b.iter(|| {
            // Search for common pattern
            black_box(search.search("authenticate", 20).expect("combined search"))
        })
    });

    // Also benchmark individual modes
    group.bench_function("fts_only_200_files", |b| {
        b.iter(|| black_box(search.search_fts("authenticate", 20).expect("fts search")))
    });

    group.bench_function("grep_only_200_files", |b| {
        b.iter(|| black_box(search.search_grep("authenticate", 20).expect("grep search")))
    });

    group.finish();
}

/// Benchmarks the full combined search pipeline at 2000 files.
///
/// At this scale, the walk phase takes ~2ms (vs ~200μs at 200 files),
/// making WalkParallel walk+search overlap measurable. Searcher reuse
/// and Arc<Path> savings also become significant (~225μs combined).
fn bench_combined_search_2k(c: &mut Criterion) {
    let mut group = c.benchmark_group("combined_search_2k");
    group.sample_size(20); // Slower iterations need fewer samples

    let dir = TempDir::new().expect("Failed to create temp dir");
    let db = Arc::new(Database::in_memory().expect("Failed to create database"));
    let search = bench_search_service(dir.path(), Arc::clone(&db), 2000);
    assert_nonempty_search(&search, "authenticate", 20);
    assert_nonempty_grep(&search, "authenticate", 20);

    let direct_dir = TempDir::new().expect("Failed to create temp dir");
    let direct_db = Arc::new(Database::in_memory().expect("Failed to create database"));
    let direct_trigram = Arc::new(RwLock::new(setup_direct_candidate_files(
        direct_dir.path(),
        &direct_db,
        2000,
    )));
    let direct_search = SearchService::with_trigram(
        Arc::clone(&direct_db),
        direct_dir.path().to_path_buf(),
        direct_trigram,
    )
    .expect("direct candidate search service");
    assert_direct_candidate_sources(&direct_search);

    group.throughput(Throughput::Elements(1));
    group.bench_function("combined_2000_files", |b| {
        b.iter(|| black_box(search.search("authenticate", 20).expect("combined search")))
    });

    group.bench_function("direct_candidate_unique_2000_files", |b| {
        b.iter(|| {
            black_box(
                direct_search
                    .search("unique_direct_candidate_token_1999", 20)
                    .expect("direct candidate search"),
            )
        })
    });

    group.bench_function("grep_only_2000_files", |b| {
        b.iter(|| black_box(search.search_grep("authenticate", 20).expect("grep search")))
    });

    group.finish();
}

// ============================================================================
// Database Operation Benchmarks
// ============================================================================

/// Benchmarks database upsert operations.
fn bench_db_upsert(c: &mut Criterion) {
    let mut group = c.benchmark_group("db_upsert");

    let content = r#"
        fn example_function() {
            let config = Config::load();
            authenticate(&config)?;
            process_data(&config)?;
            Ok(())
        }
    "#;

    group.throughput(Throughput::Elements(1));
    group.bench_function("single_file", |b| {
        let db = Database::in_memory().expect("Failed to create database");
        b.iter(|| black_box(db.upsert_file("test.rs", content, 0x1)))
    });

    // Benchmark upsert to existing file (update path)
    group.bench_function("update_existing", |b| {
        let db = Database::in_memory().expect("Failed to create database");
        db.upsert_file("test.rs", content, 0x1)
            .expect("Failed to insert");

        let updated_content = format!("{}\n// Updated", content);
        b.iter(|| black_box(db.upsert_file("test.rs", &updated_content, 0x2)))
    });

    group.finish();
}

/// Benchmarks database read operations.
fn bench_db_read(c: &mut Criterion) {
    let mut group = c.benchmark_group("db_read");

    let db = Database::in_memory().expect("Failed to create database");

    // Insert test files
    let mut file_ids = Vec::new();
    for i in 0..100 {
        let id = db
            .upsert_file(
                &format!("file_{}.rs", i),
                &format!("fn function_{}() {{}}", i),
                i as u64,
            )
            .expect("Failed to insert");
        file_ids.push(id);
    }

    group.bench_function("get_file_by_id", |b| {
        b.iter(|| black_box(db.get_file(file_ids[50])))
    });

    group.bench_function("get_file_by_path", |b| {
        b.iter(|| black_box(db.get_file_by_path("file_50.rs")))
    });

    group.bench_function("file_count", |b| b.iter(|| black_box(db.file_count())));

    group.bench_function("get_indexed_files", |b| {
        b.iter(|| black_box(db.get_indexed_files()))
    });

    group.finish();
}

/// Benchmarks graph read queries over a wide call/import fan-out.
fn bench_graph_queries(c: &mut Criterion) {
    let mut group = c.benchmark_group("graph_queries");
    group.sample_size(50);

    let db = Database::in_memory().expect("graph db");
    let caller = db
        .upsert_file("caller.rs", "fn caller() { /* generated calls */ }", 1)
        .expect("caller file");
    let caller_symbol = [SymbolRow {
        name: "caller".to_string(),
        kind: "fn".to_string(),
        start_line: 1,
        end_line: 1,
        start_byte: 0,
        end_byte: 64,
    }];
    let calls: Vec<RawCall> = (0..500)
        .map(|i| RawCall {
            name: format!("target_{i}"),
            byte: 14,
        })
        .collect();
    Database::replace_file_graph_on(
        &db.conn().expect("conn"),
        caller,
        &caller_symbol,
        &calls,
        &[],
    )
    .expect("caller graph");

    for i in 0..500 {
        let path = format!("targets/target_{i}.rs");
        let file_id = db
            .upsert_file(&path, &format!("fn target_{i}() {{}}"), i as u64 + 2)
            .expect("target file");
        let symbol = [SymbolRow {
            name: format!("target_{i}"),
            kind: "fn".to_string(),
            start_line: 1,
            end_line: 1,
            start_byte: 0,
            end_byte: 16,
        }];
        let imports = vec!["crate::shared::Thing".to_string()];
        Database::replace_file_graph_on(&db.conn().expect("conn"), file_id, &symbol, &[], &imports)
            .expect("target graph");
    }

    assert_eq!(db.callees("caller").expect("callees").len(), 500);
    assert_eq!(
        db.callees_limited("caller", 21)
            .expect("limited callees")
            .len(),
        21
    );
    assert_eq!(
        db.dependents_of_limited("shared::Thing", 21)
            .expect("limited dependents")
            .len(),
        21
    );

    group.bench_function("callees_500", |b| {
        b.iter(|| black_box(db.callees("caller").expect("callees")))
    });
    group.bench_function("callees_limited_20", |b| {
        b.iter(|| black_box(db.callees_limited("caller", 20).expect("limited callees")))
    });
    group.bench_function("callers_single", |b| {
        b.iter(|| black_box(db.callers("target_250").expect("callers")))
    });
    group.bench_function("dependents_limited_20", |b| {
        b.iter(|| {
            black_box(
                db.dependents_of_limited("shared::Thing", 20)
                    .expect("limited dependents"),
            )
        })
    });

    let write_db = Database::in_memory().expect("graph write db");
    let mut write_file_ids = Vec::with_capacity(200);
    let mut write_symbols = Vec::with_capacity(200);
    let mut write_calls = Vec::with_capacity(200);
    let mut write_imports = Vec::with_capacity(200);
    for i in 0..200 {
        let file_id = write_db
            .upsert_file(
                &format!("write/file_{i}.rs"),
                &format!("fn function_{i}() {{ shared_call(); }}"),
                i as u64,
            )
            .expect("write file");
        write_file_ids.push(file_id);
        write_symbols.push(vec![SymbolRow {
            name: format!("function_{i}"),
            kind: "fn".to_string(),
            start_line: 1,
            end_line: 1,
            start_byte: 0,
            end_byte: 40,
        }]);
        write_calls.push(vec![RawCall {
            name: "shared_call".to_string(),
            byte: 18,
        }]);
        write_imports.push(vec!["crate::shared::Thing".to_string()]);
    }
    let write_batch: Vec<_> = (0..write_file_ids.len())
        .map(|i| FileGraphBatchItem {
            file_id: write_file_ids[i],
            symbols: &write_symbols[i],
            calls: &write_calls[i],
            imports: &write_imports[i],
        })
        .collect();
    let write_conn = write_db.conn().expect("graph write conn");
    Database::replace_file_graphs_on(&write_conn, &write_batch).expect("graph write preflight");

    group.bench_function("replace_graphs_200_files", |b| {
        b.iter(|| {
            Database::replace_file_graphs_on(&write_conn, &write_batch)
                .expect("replace graph batch");
            black_box(())
        })
    });

    group.finish();
}

// ============================================================================
// Standalone Grep Benchmarks
// ============================================================================

/// Benchmarks grep search with pattern variety matching all QueryIntent types.
///
/// Tests literal, simple regex, complex regex, and no-match patterns to
/// reveal how grep performance varies by query complexity.
fn bench_grep_search(c: &mut Criterion) {
    let mut group = c.benchmark_group("grep_search");
    group.sample_size(50);

    let dir = TempDir::new().expect("temp dir");
    let db = Arc::new(Database::in_memory().expect("db"));
    let search = bench_search_service(dir.path(), Arc::clone(&db), 200);

    let patterns = [
        ("literal", "authenticate"),
        ("simple_regex", r"fn\s+\w+"),
        ("complex_regex", r"impl.*Handler"),
        ("no_match", "xyzzy_nonexistent"),
    ];

    for (name, pattern) in patterns {
        let preflight = search.search_grep(pattern, 20).expect("grep preflight");
        if name != "no_match" {
            assert!(!preflight.is_empty(), "grep preflight returned no results");
        }
        group.bench_with_input(BenchmarkId::new("pattern", name), &pattern, |b, p| {
            b.iter(|| black_box(search.search_grep(p, 20).expect("grep search")))
        });
    }

    group.finish();
}

/// Benchmarks grep file aggregation when one file has many matching lines.
///
/// Ranked search only keeps compact proof snippets, so this stresses the
/// avoidable allocation path where raw matches are collected before aggregation.
fn bench_grep_dense_matches(c: &mut Criterion) {
    let mut group = c.benchmark_group("grep_dense_matches");
    group.sample_size(50);

    let dir = TempDir::new().expect("temp dir");
    let dense_content: String = (0..5_000)
        .map(|i| format!("let needle_{i} = expensive_call();\n"))
        .collect();
    fs::write(dir.path().join("dense.rs"), dense_content).expect("dense file");
    fs::write(
        dir.path().join("sparse.rs"),
        "fn sparse() { let needle = 1; }\n",
    )
    .expect("sparse file");

    let grep = GrepService::new(dir.path().to_path_buf()).expect("grep");
    let preflight = grep
        .search_files_with_matches("needle", 200)
        .expect("dense grep preflight");
    assert!(!preflight.0.is_empty(), "dense grep returned no files");

    group.throughput(Throughput::Elements(1));
    group.bench_function("search_files_with_matches_limit_200", |b| {
        b.iter(|| {
            black_box(
                grep.search_files_with_matches("needle", 200)
                    .expect("dense grep search"),
            )
        })
    });

    group.finish();
}

/// Benchmarks the trigram prefilter execution strategy.
///
/// The old path still walks every directory entry and checks the filter. The
/// direct path searches the already-resolved candidate file list.
fn bench_grep_candidate_filter(c: &mut Criterion) {
    let mut group = c.benchmark_group("grep_candidate_filter");
    group.sample_size(50);

    let dir = TempDir::new().expect("temp dir");
    let target = dir.path().join("file_1999.rs");
    for i in 0..2_000 {
        let path = dir.path().join(format!("file_{i}.rs"));
        let content = if i == 1_999 {
            "fn target() { let needle_unique_candidate = true; }\n".to_string()
        } else {
            format!("fn file_{i}() {{ let common_value = {i}; }}\n")
        };
        fs::write(path, content).expect("candidate bench file");
    }

    let grep = GrepService::new(dir.path().to_path_buf()).expect("grep");
    let candidate = Arc::<Path>::from(target.as_path());
    let candidates = vec![Arc::clone(&candidate)];
    let filter = HashSet::from([candidate]);
    let direct_preflight = grep
        .search_files_with_matches_candidates("needle_unique_candidate", 20, &candidates)
        .expect("direct candidate preflight");
    assert_eq!(direct_preflight.0.len(), 1, "direct candidate result count");
    let filter_preflight = grep
        .search_files_with_matches_filtered("needle_unique_candidate", 20, Some(&filter))
        .expect("walker filter preflight");
    assert_eq!(filter_preflight.0.len(), 1, "walker filter result count");

    group.throughput(Throughput::Elements(1));
    group.bench_function("walker_filter_1_of_2000", |b| {
        b.iter(|| {
            black_box(
                grep.search_files_with_matches_filtered(
                    "needle_unique_candidate",
                    20,
                    Some(&filter),
                )
                .expect("walker filter search"),
            )
        })
    });

    group.bench_function("direct_candidate_1_of_2000", |b| {
        b.iter(|| {
            black_box(
                grep.search_files_with_matches_candidates(
                    "needle_unique_candidate",
                    20,
                    &candidates,
                )
                .expect("direct candidate search"),
            )
        })
    });

    group.finish();
}

/// Benchmarks direct-candidate search against walker filtering at different
/// candidate-set sizes. Each candidate contains the trigram literal, but only
/// the last candidate satisfies the full regex, which approximates the
/// expensive false-positive shape that decides the direct/walk cutoff.
fn bench_grep_candidate_threshold_matrix(c: &mut Criterion) {
    let mut group = c.benchmark_group("grep_candidate_threshold_matrix");
    group
        .sample_size(20)
        .warm_up_time(Duration::from_millis(500))
        .measurement_time(Duration::from_secs(2));

    let counts = [1usize, 4, 16, 64, 128, 192, 256, 257, 320, 512];
    let dir = TempDir::new().expect("temp dir");

    for i in 0..2_000usize {
        let mut content = format!("fn file_{i}() {{ let common_value = {i}; }}\n");
        for &count in &counts {
            if i < count {
                let value = if i + 1 == count { "true" } else { "false" };
                content.push_str(&format!("let threshold_token_{count} = {value};\n"));
            }
        }
        fs::write(dir.path().join(format!("file_{i}.rs")), content).expect("threshold bench file");
    }

    let grep = GrepService::new(dir.path().to_path_buf()).expect("grep");
    let all_paths: Vec<Arc<Path>> = (0..2_000usize)
        .map(|i| Arc::<Path>::from(dir.path().join(format!("file_{i}.rs")).as_path()))
        .collect();

    for &count in &counts {
        let pattern = format!(r"threshold_token_{count}\s*=\s*true");
        let candidates: Vec<_> = all_paths.iter().take(count).cloned().collect();
        let filter: HashSet<_> = candidates.iter().cloned().collect();

        let direct_preflight = grep
            .search_files_with_matches_candidates(&pattern, 20, &candidates)
            .expect("direct threshold preflight");
        assert_eq!(
            direct_preflight.0.len(),
            1,
            "direct candidate result count for {count}"
        );
        let filter_preflight = grep
            .search_files_with_matches_filtered(&pattern, 20, Some(&filter))
            .expect("walker threshold preflight");
        assert_eq!(
            filter_preflight.0.len(),
            1,
            "walker filter result count for {count}"
        );

        group.bench_with_input(BenchmarkId::new("direct", count), &count, |b, _| {
            b.iter(|| {
                black_box(
                    grep.search_files_with_matches_candidates(&pattern, 20, &candidates)
                        .expect("direct threshold search"),
                )
            })
        });

        group.bench_with_input(BenchmarkId::new("walker", count), &count, |b, _| {
            b.iter(|| {
                black_box(
                    grep.search_files_with_matches_filtered(&pattern, 20, Some(&filter))
                        .expect("walker threshold search"),
                )
            })
        });
    }

    group.finish();
}

// ============================================================================
// Structural Search Benchmarks
// ============================================================================

fn empty_search_service(root: &Path) -> SearchService {
    let db = Arc::new(Database::in_memory().expect("structural bench db"));
    let trigram = Arc::new(RwLock::new(TrigramIndex::new()));
    SearchService::with_trigram(db, root.to_path_buf(), trigram)
        .expect("structural bench search service")
}

fn structural_input(query: StructuralQuery, limit: usize) -> StructuralSearchInput {
    StructuralSearchInput {
        language: StructuralLanguage::Rust,
        query,
        path: ".".to_string(),
        globs: Vec::new(),
        limit,
        include_meta: false,
        strictness: None,
        timeout_ms: STRUCTURAL_DEFAULT_TIMEOUT_MS,
    }
}

fn setup_structural_files(dir: &Path, count: usize) {
    fs::create_dir_all(dir.join("src")).expect("structural bench src dir");
    for i in 0..count {
        let target = if i + 1 == count {
            format!("fn target_marker_{i}() {{ process_target(); }}\n")
        } else {
            String::new()
        };
        let content = format!(
            r#"
            pub struct Handler{i} {{
                value: usize,
            }}

            impl Handler{i} {{
                pub fn new() -> Self {{
                    Self {{ value: {i} }}
                }}

                pub fn handle(&self, request: Request) -> Result<Response, Error> {{
                    authenticate(&request)?;
                    Ok(Response::new(self.value))
                }}
            }}

            fn helper_{i}() {{
                let value = {i};
                process_value(value);
            }}

            {target}
            "#,
            i = i,
            target = target
        );
        fs::write(dir.join("src").join(format!("module_{i}.rs")), content)
            .expect("structural bench file");
    }
}

fn bench_structural_search(c: &mut Criterion) {
    let mut group = c.benchmark_group("structural_search");
    group.sample_size(20);

    for file_count in [200usize, 2_000] {
        let dir = TempDir::new().expect("structural temp dir");
        setup_structural_files(dir.path(), file_count);
        let search = empty_search_service(dir.path());

        let pattern_input = structural_input(
            StructuralQuery::Pattern {
                pattern: "fn $NAME($$$ARGS) { $$$BODY }".to_string(),
                selector: None,
            },
            20,
        );
        let kind_input = structural_input(
            StructuralQuery::Kind {
                kind: "function_item".to_string(),
            },
            20,
        );

        assert!(
            !execute_structural_search(&search, pattern_input.clone())
                .expect("structural pattern preflight")
                .results
                .is_empty(),
            "structural pattern preflight returned no results"
        );
        assert!(
            !execute_structural_search(&search, kind_input.clone())
                .expect("structural kind preflight")
                .results
                .is_empty(),
            "structural kind preflight returned no results"
        );

        group.bench_with_input(
            BenchmarkId::new("pattern_functions", file_count),
            &pattern_input,
            |b, input| {
                b.iter(|| {
                    black_box(
                        execute_structural_search(&search, input.clone())
                            .expect("structural pattern search"),
                    )
                })
            },
        );

        group.bench_with_input(
            BenchmarkId::new("kind_functions", file_count),
            &kind_input,
            |b, input| {
                b.iter(|| {
                    black_box(
                        execute_structural_search(&search, input.clone())
                            .expect("structural kind search"),
                    )
                })
            },
        );

        if file_count == 2_000 {
            let unique_input = structural_input(
                StructuralQuery::Pattern {
                    pattern: "fn target_marker_1999() { $$$BODY }".to_string(),
                    selector: None,
                },
                20,
            );
            assert_eq!(
                execute_structural_search(&search, unique_input.clone())
                    .expect("structural unique preflight")
                    .results
                    .len(),
                1,
                "structural unique preflight should find the tail target"
            );

            group.bench_function("pattern_unique_tail_2000_files", |b| {
                b.iter(|| {
                    black_box(
                        execute_structural_search(&search, unique_input.clone())
                            .expect("structural unique search"),
                    )
                })
            });

            group.bench_function("grep_regex_functions_2000_files", |b| {
                b.iter(|| black_box(search.search_grep(r"fn\s+\w+", 20).expect("grep search")))
            });
        }
    }

    group.finish();
}

// ============================================================================
// Real Repository Benchmarks
// ============================================================================

/// Benchmarks search against a real git repository.
///
/// By default, indexes and searches this repo (grepika).
/// Set `BENCH_REPO_PATH` to benchmark a different/larger repo.
fn bench_real_repo(c: &mut Criterion) {
    let mut group = c.benchmark_group("real_repo");
    group.sample_size(20);

    let root = std::env::var("BENCH_REPO_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(env!("CARGO_MANIFEST_DIR")));

    // File-backed DB in target/ — persists across runs for fast incremental reindex
    let db_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("bench_cache");
    fs::create_dir_all(&db_dir).expect("Failed to create bench cache dir");
    let db_path = db_dir.join("real_repo.db");
    let db = Arc::new(Database::open(&db_path).expect("Failed to open database"));

    // Index the real directory (incremental — fast on rerun)
    let trigram = Arc::new(RwLock::new(TrigramIndex::new()));
    let indexer = Indexer::new(Arc::clone(&db), Arc::clone(&trigram), root.clone());
    indexer.index(None, false).expect("Failed to index");

    let search = SearchService::with_trigram(Arc::clone(&db), root, Arc::clone(&trigram))
        .expect("Failed to create search service");

    group.throughput(Throughput::Elements(1));
    group.bench_function("combined_search", |b| {
        b.iter(|| black_box(search.search("fn ", 20)))
    });
    group.bench_function("grep_search", |b| {
        b.iter(|| black_box(search.search_grep("fn ", 20)))
    });
    let structural_kind_input = structural_input(
        StructuralQuery::Kind {
            kind: "function_item".to_string(),
        },
        20,
    );
    assert!(
        !execute_structural_search(&search, structural_kind_input.clone())
            .expect("real repo structural kind preflight")
            .results
            .is_empty(),
        "real repo structural kind search returned no results"
    );
    group.bench_function("structural_kind_functions", |b| {
        b.iter(|| {
            black_box(
                execute_structural_search(&search, structural_kind_input.clone())
                    .expect("real repo structural kind search"),
            )
        })
    });
    group.finish();
}

// ============================================================================
// Criterion Configuration
// ============================================================================

criterion_group!(
    trigram_benches,
    bench_trigram_search,
    bench_trigram_query_length,
    bench_trigram_add_file,
    bench_trigram_update_file,
);

criterion_group!(score_benches, bench_score_operations, bench_result_merging,);

criterion_group!(fts_benches, bench_fts_search, bench_fts_query_complexity,);

criterion_group!(
    search_benches,
    bench_combined_search,
    bench_combined_search_2k,
    bench_grep_search,
    bench_grep_dense_matches,
    bench_grep_candidate_filter,
    bench_grep_candidate_threshold_matrix,
    bench_structural_search,
);

criterion_group!(
    db_benches,
    bench_db_upsert,
    bench_db_read,
    bench_graph_queries,
);

criterion_group!(real_repo_benches, bench_real_repo,);

criterion_main!(
    trigram_benches,
    score_benches,
    fts_benches,
    search_benches,
    db_benches,
    real_repo_benches,
);
