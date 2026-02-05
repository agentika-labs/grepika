# agentika-grep

Token-efficient MCP server for code search. Combines three search backends for high-quality results:

- **FTS5** - SQLite full-text search with BM25 ranking
- **Grep** - Parallel regex search using ripgrep internals
- **Trigram** - Fast substring search via 3-byte sequence indexing

## Installation

### Pre-built Binary (macOS Apple Silicon)

Download and install the pre-built binary:

```bash
# Extract
tar -xzf agentika-grep-macos-arm64.tar.gz

# Remove macOS quarantine flag (required for unsigned binaries)
xattr -d com.apple.quarantine agentika-grep-macos-arm64

# Make executable and install
chmod +x agentika-grep-macos-arm64
sudo mv agentika-grep-macos-arm64 /usr/local/bin/agentika-grep

# Verify installation
agentika-grep --help
```

### Build from Source (macOS Apple Silicon)

Requires Rust 1.75+:

```bash
# Build for Apple Silicon
cargo build --release --target aarch64-apple-darwin

# Binary location
ls -la target/aarch64-apple-darwin/release/agentika-grep

# Create distributable archive
tar -czvf agentika-grep-macos-arm64.tar.gz \
  -C target/aarch64-apple-darwin/release agentika-grep
```

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

### Token Efficiency Benchmarks

Measure token savings compared to raw ripgrep output:

```bash
# Run token efficiency benchmarks
cargo bench token_efficiency

# Capture MCP schema for overhead analysis
./scripts/capture_schema.sh
```

## Token Efficiency

agentika-grep's indexed search returns **83.8% fewer tokens** on average compared to Claude's built-in Grep tool (which uses ripgrep). This dramatically reduces context consumption when exploring codebases.

### Results (on agentika-grep codebase)

```
Query      │ agentika │  ripgrep │ Savings
───────────┼──────────┼──────────┼────────
auth       │    326 B │   2109 B │  84.5%
config     │    502 B │   3749 B │  86.6%
error      │    478 B │   5322 B │  91.0%
handler    │    248 B │   1239 B │  80.0%
database   │    242 B │   1048 B │  76.9%
───────────┼──────────┼──────────┼────────
AVERAGE    │    359 B │   2693 B │  83.8%
```

### Break-Even Analysis

The MCP schema adds ~825 tokens of one-time overhead. With ~584 tokens saved per query:

- **Break-even point: 2 queries**
- After 5 queries: ~2,000 tokens saved
- After 10 queries: ~5,000 tokens saved

For typical coding sessions involving dozens of searches, agentika-grep provides substantial context savings.

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

#### Pre-authorizing Permissions

To avoid permission prompts for agentika-grep tools:

**Project-Level (Recommended)** - Add to `.claude/settings.local.json`:

```json
{
  "permissions": {
    "allow": [
      "mcp__agentika-grep__*"
    ]
  }
}
```

**Global (All Projects)** - Add to `~/.claude/settings.json`:

```json
{
  "permissions": {
    "allow": [
      "mcp__agentika-grep__*"
    ]
  }
}
```

**Explicit Tool List** - If you prefer explicit permissions:

```json
{
  "permissions": {
    "allow": [
      "mcp__agentika-grep__search",
      "mcp__agentika-grep__relevant",
      "mcp__agentika-grep__refs",
      "mcp__agentika-grep__related",
      "mcp__agentika-grep__outline",
      "mcp__agentika-grep__context",
      "mcp__agentika-grep__get",
      "mcp__agentika-grep__toc",
      "mcp__agentika-grep__stats",
      "mcp__agentika-grep__index",
      "mcp__agentika-grep__diff"
    ]
  }
}
```

**Verify** - Run `/permissions` in Claude Code to see active permissions, or `/doctor` to check for issues.

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
