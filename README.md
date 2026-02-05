# agentika-grep

Token-efficient MCP server for code search. Combines three search backends for high-quality results:

- **FTS5** - SQLite full-text search with BM25 ranking
- **Grep** - Parallel regex search using ripgrep internals
- **Trigram** - Fast substring search via 3-byte sequence indexing

## Quick Start

```bash
# Build
cargo build --release

# Index a codebase
./target/release/agentika-grep index --root /path/to/project

# Search
./target/release/agentika-grep search "authentication" --root /path/to/project
```

## Usage Modes

### MCP Server Mode

Run as an MCP server for IDE/editor integration:

```bash
./target/release/agentika-grep --mcp --root /path/to/project
```

The server communicates via JSON-RPC over stdin/stdout.

### CLI Mode

```bash
# Index the codebase
agentika-grep index

# Search (modes: combined, fts, grep)
agentika-grep search <query> -l 20 -m combined

# Get file content with line range
agentika-grep get <path> -s 1 -e 100

# View index statistics
agentika-grep stats
```

## Build Options

```bash
# Debug build
cargo build

# Release build (recommended)
cargo build --release

# Release with profiling enabled
cargo build --release --features profiling

# Run tests
cargo test

# Run benchmarks
cargo bench
```

## Benchmarks

Performance benchmarks using [Criterion](https://docs.rs/criterion):

| Benchmark | Why It Matters |
|-----------|----------------|
| `combined_search` | End-to-end search pipeline (FTS + grep + merge) |
| `fts_search` | Database search performance at different scales |
| `trigram_search` | Core index lookup speed (100 to 100K files) |
| `result_merging` | Score combination overhead when merging results |

```bash
# Run all benchmarks
cargo bench

# Run specific benchmark
cargo bench -- "combined_search"

# Run trigram search at specific scale (100, 1000, 10000, 100000 files)
cargo bench -- "trigram_search/100000"

# Quick performance check
cargo bench -- "combined_search|fts_search/500|trigram_search/1000"

# Compare against baseline
cargo bench -- --save-baseline main
cargo bench -- --baseline main

# View HTML reports
open target/criterion/report/index.html
```

## Profiling

Build with the `profiling` feature to enable timing and memory logging:

```bash
cargo build --release --features profiling
```

When enabled, each tool invocation logs performance metrics:

```
[search] 42ms | mem: 128.5MB (+2.1MB)
[index] 1.2s | mem: 256.0MB (+127.5MB)
```

### Viewing Profiler Logs

**CLI mode** - logs appear directly in terminal (stderr):
```bash
./target/release/agentika-grep --root . search "query"
```

**MCP mode (Claude Code)** - use `--log-file` to capture logs to a file.
Add it to your MCP config:
```json
{
  "mcpServers": {
    "agentika-grep": {
      "command": "/path/to/agentika-grep",
      "args": ["--mcp", "--root", "/path/to/project", "--log-file", "/tmp/agentika-grep.log"]
    }
  }
}
```

Then view logs in a separate terminal:
```bash
tail -f /tmp/agentika-grep.log
```

### Log File Location

Recommended locations:
- **macOS/Linux**: `/tmp/agentika-grep.log` or `~/.local/share/agentika-grep/profile.log`
- **Project-specific**: `.agentika-grep/profile.log` (next to the index)

Use profiling for:
- Performance tuning during development
- Identifying slow queries or operations
- Tracking memory usage across tool calls
- Debugging performance regressions

## MCP Server Setup

### Claude Code

Add to your project's `.mcp.json` or global `~/.claude.json`:

```json
{
  "mcpServers": {
    "agentika-grep": {
      "command": "/path/to/agentika-grep",
      "args": ["--mcp", "--root", "/path/to/project"]
    }
  }
}
```

Or use the CLI:

```bash
claude mcp add agentika-grep /path/to/agentika-grep -- --mcp --root /path/to/project
```

#### Configuring Tool Preference

Claude Code has built-in Grep and Glob tools. To make Claude prefer agentika-grep's superior search capabilities, you have two options:

**Option A: Advisory Instructions (CLAUDE.md)**

Add to your project's `CLAUDE.md`:

```markdown
## Code Search

Prefer agentika-grep MCP tools over built-in Grep/Glob for code search:
- `mcp__agentika-grep__index` - Build/update search index (run first!)
- `mcp__agentika-grep__search` - Pattern/regex search (replaces Grep)
- `mcp__agentika-grep__relevant` - Find files by topic (replaces Glob exploration)
- `mcp__agentika-grep__toc` - Directory tree (replaces Glob patterns)
- `mcp__agentika-grep__outline` - File structure extraction
- `mcp__agentika-grep__refs` - Symbol references

These provide ranked results with FTS5+trigram indexing for better search quality.
```

See [docs/claude-code-snippet.md](docs/claude-code-snippet.md) for a more detailed version.

**Option B: Enforcement via Hooks**

For deterministic enforcement, add PreToolUse hooks to `.claude/settings.json`:

```json
{
  "hooks": {
    "PreToolUse": [
      {
        "matcher": "Grep",
        "hooks": [{ "type": "command", "command": "echo '⚠️  Consider mcp__agentika-grep__search'" }]
      },
      {
        "matcher": "Glob",
        "hooks": [{ "type": "command", "command": "echo '⚠️  Consider mcp__agentika-grep__toc or relevant'" }]
      }
    ]
  }
}
```

See [docs/hooks-example.json](docs/hooks-example.json) for the full example.

**Advisory vs Enforcement:**
- CLAUDE.md instructions are *advisory* — Claude reads and follows them, but may still use built-in tools in some cases
- Hooks are *deterministic* — they execute before every matching tool call, providing consistent reminders or blocks

### Cursor

Add to `~/.cursor/mcp.json` or project `.cursor/mcp.json`:

```json
{
  "mcpServers": {
    "agentika-grep": {
      "command": "/path/to/agentika-grep",
      "args": ["--mcp", "--root", "/path/to/project"]
    }
  }
}
```

### OpenCode

Add to `opencode.config.json`:

```json
{
  "mcp": {
    "agentika-grep": {
      "type": "local",
      "command": ["/path/to/agentika-grep", "--mcp", "--root", "/path/to/project"]
    }
  }
}
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

## Index Location

By default, the index is stored at:

```
<root>/.agentika-grep/index.db
```

Use the `--db` flag to specify a custom location:

```bash
agentika-grep --mcp --root /path/to/project --db /custom/path/index.db
```

## Configuration

- **Max file size**: 1MB (files larger than this are skipped during indexing)
- **Gitignore**: Patterns in `.gitignore` are respected during indexing
- **Logging**: All logs go to stderr (stdout is reserved for JSON-RPC in MCP mode)

## License

MIT
