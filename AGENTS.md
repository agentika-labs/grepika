# AGENTS.md

This file provides guidance to Codex (Codex.ai/code) when working with code in this repository.

## Project Overview

**grepika** is a token-efficient MCP (Model Context Protocol) server for code search. It combines three search backends for high-quality results:
- **FTS5** - SQLite full-text search with BM25 ranking
- **Grep** - Parallel regex search using ripgrep internals
- **Sparse n-gram prefilter** - Fast substring candidate filtering, exposed through the legacy trigram naming in the code

The server communicates via JSON-RPC over stdin/stdout when running as an MCP server.

## Build & Run Commands

```bash
# Build
cargo build --release

# Run MCP server — global mode (recommended, LLM calls add_workspace)
./target/release/grepika --mcp

# Run MCP server — single workspace mode
./target/release/grepika --mcp --root <path>

# CLI commands
grepika index                           # Index the codebase
grepika search <query> -l 20 -m combined  # Search (modes: combined, fts, grep)
grepika get <path> -s 1 -e 100          # Get file content with line range

# Run tests
cargo test

# Run single test
cargo test test_name

# Enable profiling logs
grepika --mcp --log-file /tmp/grepika.log
```

## Architecture

```
MCP Server (rmcp)          ← JSON-RPC stdin/stdout
       │
Tool Router (server.rs)    ← search, structural_search, get, outline, toc, context, stats, refs, graph, index, diff, add_workspace
       │
Workspace                  ← Holds root + SearchService + Indexer (None in global mode)
       │
SearchService              ← spawn_blocking for async bridge
       │
┌──────┼──────┐
FTS5   Grep   Sparse n-gram ← Three lexical backends with weighted score merging
       │
SQLite + r2d2 pool         ← ~/.cache/grepika/<hash>.db by default
```

### Key Design Decisions

- **spawn_blocking bridge**: MCP handlers are async (rmcp), but search operations are CPU-bound. The `run_tool` helper in `server.rs` uses `tokio::task::spawn_blocking` to bridge this gap.

- **Score merging**: Results from multiple backends are merged with configurable weights (default: FTS 0.4, grep 0.4, trigram/n-gram 0.2) plus a multi-source bonus.

- **Type-safe newtypes**: `FileId`, `Score`, and `NgramKey` in `types.rs` provide compile-time safety. `Trigram` remains a compatibility alias for `NgramKey`. `Score` uses saturating arithmetic (clamped to 0.0-1.0).

- **r2d2 connection pool**: rusqlite::Connection is not Sync, so r2d2 manages thread-safe SQLite access.

## Module Structure

| Module | Purpose |
|--------|---------|
| `server.rs` | MCP server with `#[tool]` macro handlers |
| `services/ast.rs` | Tree-sitter symbol and edge extraction |
| `services/semantic.rs` | Optional embeddings (`semantic` feature; cosine + fastembed) |
| `services/search.rs` | Combined search with score merging |
| `services/fts.rs` | FTS5 BM25 search |
| `services/grep.rs` | Parallel grep with ripgrep crates |
| `services/ngram.rs` | Sparse frequency-weighted n-gram extraction |
| `services/trigram.rs` | In-memory sparse n-gram index (RoaringBitmap; legacy trigram API name) |
| `services/indexer.rs` | Incremental indexing with xxHash change detection |
| `db/graph.rs` | Code-graph persistence and queries |
| `db/mod.rs` | Database with r2d2 pool and FTS5 queries |
| `db/schema.rs` | SQLite schema (files, files_fts, trigrams, symbols, edges tables) |
| `tools/*.rs` | MCP tool input/output types and executors |
| `types.rs` | FileId, Score, NgramKey newtypes |
| `error.rs` | Hierarchical error types with machine-readable codes |

## MCP Tools Exposed

| Tool | Description |
|------|-------------|
| `search` | Pattern search (regex/natural language) |
| `get` | File content with optional line range |
| `outline` | Extract file structure (functions, classes) |
| `toc` | Directory tree |
| `context` | Surrounding lines around a specific line |
| `stats` | Index statistics |
| `refs` | Find all references to a symbol |
| `graph` | Navigate the code graph: callers, callees, call_chain, imports, dependents |
| `structural_search` | Syntax-aware AST pattern/kind search via ast-grep (no index required) |
| `index` | Update search index (incremental by default) |
| `diff` | Compare two files |
| `add_workspace` | Load a project workspace (global mode) |

## Run Modes

- **Single workspace**: `--mcp --root <path>` — workspace pre-loaded at startup, no `add_workspace` needed
- **Global mode**: `--mcp` (no `--root`) — starts with no workspace; the LLM calls `add_workspace` with the project root. Recommended for IDE integration where the LLM reads its working directory from its system prompt.

## Verification

- **Always use `--all-targets` for final clippy**: `cargo test` does not compile benchmark files (`benchmarks/`). When refactoring function signatures in library code, bench files are easy to overlook. Use `cargo clippy --all-targets` or `cargo bench` to catch mismatches.

## Critical Notes

- **Logging must go to stderr**: stdout is reserved for JSON-RPC in MCP mode
- Index stored at `~/.cache/grepika/<hash>.db` by default (not in the project directory)
- The `--db` flag can override the default index location
- Gitignore patterns are respected during indexing
- Max file size for indexing: 1MB (configurable in `IndexConfig`)
- Sparse n-gram index is persisted in the `trigrams` table and loaded on startup
- Code graph (symbols/edges) is populated during indexing from tree-sitter; the `graph` tool queries it
- Semantic search is **off by default**: build with `cargo build --features semantic` to enable local embeddings. Without the feature, search runs lexical-only
