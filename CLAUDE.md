# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

**grepika** is a token-efficient MCP (Model Context Protocol) server for code search. It combines three search backends for high-quality results:
- **FTS5** - SQLite full-text search with BM25 ranking
- **Grep** - Parallel regex search using ripgrep internals
- **Trigram** - Fast substring search via 3-byte sequence indexing

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
Tool Router (server.rs)    ← search, relevant, get, outline, toc, context, stats, refs, index, diff, add_workspace
       │
Workspace                  ← Holds root + SearchService + Indexer (None in global mode)
       │
SearchService              ← spawn_blocking for async bridge
       │
┌──────┼──────┐
FTS5   Grep   Trigram      ← Three search backends with weighted score merging
       │
SQLite + r2d2 pool         ← .grepika/index.db
```

### Key Design Decisions

- **spawn_blocking bridge**: MCP handlers are async (rmcp), but search operations are CPU-bound. The `run_tool` helper in `server.rs` uses `tokio::task::spawn_blocking` to bridge this gap.

- **Score merging**: Results from multiple backends are merged with configurable weights (default: FTS 0.4, grep 0.4, trigram 0.2) plus a multi-source bonus.

- **Type-safe newtypes**: `FileId`, `Score`, and `Trigram` in `types.rs` provide compile-time safety. `Score` uses saturating arithmetic (clamped to 0.0-1.0).

- **r2d2 connection pool**: rusqlite::Connection is not Sync, so r2d2 manages thread-safe SQLite access.

## Module Structure

| Module | Purpose |
|--------|---------|
| `server.rs` | MCP server with `#[tool]` macro handlers |
| `services/search.rs` | Combined search with score merging |
| `services/fts.rs` | FTS5 BM25 search |
| `services/grep.rs` | Parallel grep with ripgrep crates |
| `services/trigram.rs` | In-memory trigram index (RoaringBitmap) |
| `services/indexer.rs` | Incremental indexing with xxHash change detection |
| `db/mod.rs` | Database with r2d2 pool and FTS5 queries |
| `db/schema.rs` | SQLite schema (files, files_fts, trigrams tables) |
| `tools/*.rs` | MCP tool input/output types and executors |
| `types.rs` | FileId, Score, Trigram newtypes |
| `error.rs` | Hierarchical error types with machine-readable codes |

## MCP Tools Exposed

| Tool | Description |
|------|-------------|
| `search` | Pattern search (regex/natural language) |
| `relevant` | Find files most relevant to a topic |
| `get` | File content with optional line range |
| `outline` | Extract file structure (functions, classes) |
| `toc` | Directory tree |
| `context` | Surrounding lines around a specific line |
| `stats` | Index statistics |
| `related` | Files related by shared symbols |
| `refs` | Find all references to a symbol |
| `index` | Update search index (incremental by default) |
| `diff` | Compare two files |
| `add_workspace` | Load a project workspace (global mode) |

## Run Modes

- **Single workspace**: `--mcp --root <path>` — workspace pre-loaded at startup, no `add_workspace` needed
- **Global mode**: `--mcp` (no `--root`) — starts with no workspace; the LLM calls `add_workspace` with the project root. Recommended for IDE integration where the LLM reads its working directory from its system prompt.

## Verification

- **Always use `--all-targets` for final clippy**: `cargo test` does not compile benchmark files (`benches/`). When refactoring function signatures in library code, bench files are easy to overlook. Use `cargo clippy --all-targets` or `cargo bench` to catch mismatches.

## Critical Notes

- **Logging must go to stderr**: stdout is reserved for JSON-RPC in MCP mode
- Index stored at `~/.cache/grepika/<hash>.db` by default (not in the project directory)
- The `--db` flag can override the default index location
- Gitignore patterns are respected during indexing
- Max file size for indexing: 1MB (configurable in `IndexConfig`)
- Trigram index is persisted to database and loaded on startup
