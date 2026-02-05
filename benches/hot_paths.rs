//! Performance benchmarks for agentika-grep hot paths.
//!
//! Measures the core operations that dominate runtime:
//! - Trigram index search and addition
//! - Score merging from multiple sources
//! - FTS5 search performance
//! - Grep parallel search
//!
//! Run with: `cargo bench`
//! View reports: `open target/criterion/report/index.html`

use agentika_grep::db::Database;
use agentika_grep::services::{SearchService, TrigramIndex};
use agentika_grep::types::{FileId, Score};
use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use std::fs;
use std::sync::Arc;
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
        group.bench_with_input(BenchmarkId::new("query", name), &(query, &index), |b, (q, idx)| {
            b.iter(|| black_box(idx.search(q)))
        });
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
    group.bench_function("new", |b| {
        b.iter(|| Score::new(black_box(0.75)))
    });

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
        let trigram_ids: Vec<u32> = (result_count / 4..result_count)
            .map(|i| i as u32)
            .collect();

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
        group.bench_with_input(
            BenchmarkId::from_parameter(file_count),
            &db,
            |b, db| {
                b.iter(|| {
                    // Search for common pattern
                    black_box(db.fts_search("authenticate*", 20))
                })
            },
        );
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
        group.bench_with_input(BenchmarkId::new("query", name), &(query, &db), |b, (q, db)| {
            b.iter(|| black_box(db.fts_search(q, 20)))
        });
    }

    group.finish();
}

// ============================================================================
// Combined Search Service Benchmarks
// ============================================================================

/// Benchmarks the full combined search pipeline.
///
/// This is the most realistic benchmark - it measures end-to-end
/// search performance including FTS, grep, and result merging.
fn bench_combined_search(c: &mut Criterion) {
    let mut group = c.benchmark_group("combined_search");
    group.sample_size(50); // Reduce sample size for slower benchmarks

    // Setup: Create temporary directory with files
    let dir = TempDir::new().expect("Failed to create temp dir");
    let db = Arc::new(Database::in_memory().expect("Failed to create database"));

    // Create actual files on disk (needed for grep)
    for i in 0..200 {
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
        fs::write(dir.path().join(&filename), &content).expect("Failed to write file");
        db.upsert_file(
            dir.path().join(&filename).to_string_lossy().as_ref(),
            &content,
            i as u64,
        )
        .expect("Failed to insert file");
    }

    let search = SearchService::new(Arc::clone(&db), dir.path().to_path_buf())
        .expect("Failed to create search service");

    group.throughput(Throughput::Elements(1));
    group.bench_function("combined_200_files", |b| {
        b.iter(|| {
            // Search for common pattern
            black_box(search.search("authenticate", 20))
        })
    });

    // Also benchmark individual modes
    group.bench_function("fts_only_200_files", |b| {
        b.iter(|| black_box(search.search_fts("authenticate", 20)))
    });

    group.bench_function("grep_only_200_files", |b| {
        b.iter(|| black_box(search.search_grep("authenticate", 20)))
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
        b.iter(|| {
            black_box(db.upsert_file("test.rs", &updated_content, 0x2))
        })
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

    group.bench_function("file_count", |b| {
        b.iter(|| black_box(db.file_count()))
    });

    group.bench_function("get_indexed_files", |b| {
        b.iter(|| black_box(db.get_indexed_files()))
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
);

criterion_group!(
    score_benches,
    bench_score_operations,
    bench_result_merging,
);

criterion_group!(
    fts_benches,
    bench_fts_search,
    bench_fts_query_complexity,
);

criterion_group!(
    search_benches,
    bench_combined_search,
);

criterion_group!(
    db_benches,
    bench_db_upsert,
    bench_db_read,
);

criterion_main!(
    trigram_benches,
    score_benches,
    fts_benches,
    search_benches,
    db_benches,
);
