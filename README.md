[![npm](https://img.shields.io/npm/v/@agentika/grepika)](https://www.npmjs.com/package/@agentika/grepika)
[![CI](https://github.com/agentika-labs/grepika/actions/workflows/ci.yml/badge.svg)](https://github.com/agentika-labs/grepika/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)

# grepika

Token-efficient MCP server for code search. Combines three search backends for high-quality results:

- **FTS5** - SQLite full-text search with BM25 ranking
- **Grep** - Parallel regex search using ripgrep internals
- **Trigram** - Fast substring search via 3-byte sequence indexing

## Why grepika?

Every built-in grep call burns **6x more context** than it needs to. grepika breaks even after just **2 searches** — then every query after that is pure savings.

**Not faster grep — a codebase understanding engine.**

| You want to... | Built-in Grep | grepika |
|----------------|---------------|---------------|
| Find a pattern | Unranked file list | **Ranked results** with relevance scores |
| Understand a symbol | Multiple grep calls, manual assembly | `refs` classifies definitions, imports, usages |
| Explore structure | Read entire files | `outline` extracts functions/classes/structs |
| Find related code | Guess-and-grep loop | `related` finds files sharing symbols |
| Natural language query | Doesn't work (needs regex) | `search` with intent detection routes to BM25 |

### Performance

Benchmarked on the same codebase, same queries ([criterion](https://github.com/bheisler/criterion.rs)):

| Metric | grepika | Built-in Grep (ripgrep) |
|--------|--------------|------------------------|
| Search latency | **2.5 ms** | 5.3 ms |
| Response size | **364 bytes avg** | 2,693 bytes avg |
| Relevance ranking | BM25 + trigram IDF | None (unranked) |
| Break-even | **2 queries** | N/A |

**2x faster search. 6x smaller responses. Ranked results.**

**Technical architecture:**
- **3 search backends** (FTS5 + grep + trigram) with weighted score merging
- **BM25 ranking** with tuned column weights — the same algorithm powering Elasticsearch
- **Query intent detection** — automatically classifies regex vs natural language vs exact symbol
- **190 tests**, zero clippy warnings, Criterion benchmarks for every claim

**Add to your MCP config. Index once. Search smarter.**

## Installation

### npm (recommended for MCP users)

```bash
npx -y @agentika/grepika --mcp
```

### Shell script (macOS Apple Silicon)

```bash
curl -fsSL https://raw.githubusercontent.com/agentika-labs/grepika/main/install.sh | bash
```

For other platforms, download the binary from [GitHub Releases](https://github.com/agentika-labs/grepika/releases).

## MCP Server Setup

### Global Mode (Recommended)

In global mode, the server starts without `--root`. The LLM reads its working directory from its system prompt and calls `add_workspace` automatically.

**Claude Code:**

```bash
claude mcp add grepika -- npx -y @agentika/grepika --mcp
```

Or add to your project's `.mcp.json` (or global `~/.claude.json`):

```json
{
  "mcpServers": {
    "grepika": {
      "command": "npx",
      "args": ["-y", "@agentika/grepika", "--mcp"]
    }
  }
}
```

**Cursor:**

Add to `~/.cursor/mcp.json` or project `.cursor/mcp.json`:

```json
{
  "mcpServers": {
    "grepika": {
      "command": "npx",
      "args": ["-y", "@agentika/grepika", "--mcp"]
    }
  }
}
```

**OpenCode:**

Add to `opencode.config.json`:

```json
{
  "mcp": {
    "grepika": {
      "type": "local",
      "command": ["npx", "-y", "@agentika/grepika", "--mcp"]
    }
  }
}
```

### Single Project Mode (Alternative)

Use `--root` to pre-load a specific workspace at startup. The LLM does not need to call `add_workspace`.

**Claude Code:**

```bash
claude mcp add grepika -- npx -y @agentika/grepika --mcp --root /path/to/project
```

```json
{
  "mcpServers": {
    "grepika": {
      "command": "npx",
      "args": ["-y", "@agentika/grepika", "--mcp", "--root", "/path/to/project"]
    }
  }
}
```

**Cursor:**

```json
{
  "mcpServers": {
    "grepika": {
      "command": "npx",
      "args": ["-y", "@agentika/grepika", "--mcp", "--root", "/path/to/project"]
    }
  }
}
```

**OpenCode:**

```json
{
  "mcp": {
    "grepika": {
      "type": "local",
      "command": ["npx", "-y", "@agentika/grepika", "--mcp", "--root", "/path/to/project"]
    }
  }
}
```

> **Tip:** Add `"--db", "/path/to/index.db"` to `args` to control where the index is stored.

## Claude Code Integration

### Tool Preference

Claude Code has built-in Grep and Glob tools. To make Claude prefer grepika's superior search capabilities:

**Option A: Advisory Instructions (CLAUDE.md)**

Add to your project's `CLAUDE.md`:

```markdown
## Code Search

Prefer grepika MCP tools over built-in Grep/Glob for code search:
- `mcp__grepika__index` - Build/update search index (run first!)
- `mcp__grepika__search` - Pattern/regex search (replaces Grep)
- `mcp__grepika__relevant` - Find files by topic (replaces Glob exploration)
- `mcp__grepika__toc` - Directory tree (replaces Glob patterns)
- `mcp__grepika__outline` - File structure extraction
- `mcp__grepika__refs` - Symbol references

These provide ranked results with FTS5+trigram indexing for better search quality.
```

See [docs/claude-code-setup.md](docs/claude-code-setup.md) for a more detailed version.

### Pre-authorizing Permissions

To avoid permission prompts, add to `.claude/settings.local.json` (project) or `~/.claude/settings.json` (global):

```json
{
  "permissions": {
    "allow": [
      "mcp__grepika__*"
    ]
  }
}
```

## Usage

```bash
# Index a codebase
grepika index --root /path/to/project

# Search (modes: combined, fts, grep)
grepika search "authentication" --root /path/to/project -l 20 -m combined

# Get file content with line range
grepika get <path> -s 1 -e 100

# View index statistics
grepika stats

# Run as MCP server (global mode — LLM calls add_workspace)
grepika --mcp

# Run as MCP server (single workspace mode)
grepika --mcp --root /path/to/project
```

## Available Tools

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

## Token Efficiency

grepika's indexed search returns **83.8% fewer tokens** on average compared to Claude's built-in Grep tool (which uses ripgrep). This dramatically reduces context consumption when exploring codebases.

```
Query      │  grepika │  ripgrep │ Savings
───────────┼──────────┼──────────┼────────
auth       │    326 B │   2109 B │  84.5%
config     │    502 B │   3749 B │  86.6%
error      │    478 B │   5322 B │  91.0%
handler    │    248 B │   1239 B │  80.0%
database   │    242 B │   1048 B │  76.9%
───────────┼──────────┼──────────┼────────
AVERAGE    │    359 B │   2693 B │  83.8%
```

The MCP schema adds ~825 tokens of one-time overhead, which pays for itself after just 2 queries (~584 tokens saved per query).

## Configuration

### Index Location

By default, the index is stored in a **global cache directory**, not in the project:

| Platform | Default Location |
|----------|------------------|
| macOS    | `~/Library/Caches/grepika/<hash>.db` |
| Linux    | `~/.cache/grepika/<hash>.db` |
| Windows  | `%LOCALAPPDATA%\grepika\<hash>.db` |

The `<hash>` is derived from the absolute path to `--root`, ensuring each project gets its own index without polluting the project directory.

Use `--db` to specify a custom location:

```bash
grepika --mcp --root /path/to/project --db /custom/path/index.db
```

### Other Settings

- **Max file size**: 1MB (files larger than this are skipped during indexing)
- **Gitignore**: Patterns in `.gitignore` are respected during indexing
- **Logging**: All logs go to stderr (stdout is reserved for JSON-RPC in MCP mode)

## Development

```bash
# Debug build
cargo build

# Release build
cargo build --release

# Release with profiling
cargo build --release --features profiling

# Run tests
cargo test

# Run all benchmarks
cargo bench

# Run real-repo benchmarks (indexes and searches this repo)
cargo bench --bench hot_paths -- real_repo

# Run real-repo benchmarks against a different repository
BENCH_REPO_PATH=/path/to/repo cargo bench --bench hot_paths -- real_repo
```

### Profiling

Build with the `profiling` feature to enable timing and memory logging:

```bash
cargo build --release --features profiling
```

When enabled, each tool invocation logs performance metrics to stderr:

```
[search] 42ms | mem: 128.5MB (+2.1MB)
[index] 1.2s | mem: 256.0MB (+127.5MB)
```

**MCP mode** — use `--log-file` to capture logs:

```json
{
  "mcpServers": {
    "grepika": {
      "command": "grepika",
      "args": ["--mcp", "--root", "/path/to/project", "--log-file", "/tmp/grepika.log"]
    }
  }
}
```

Then: `tail -f /tmp/grepika.log`

## License

MIT
