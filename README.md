# agentika-grep

Token-efficient MCP server for code search. Combines three search backends for high-quality results:

- **FTS5** - SQLite full-text search with BM25 ranking
- **Grep** - Parallel regex search using ripgrep internals
- **Trigram** - Fast substring search via 3-byte sequence indexing

## Installation

### npm (recommended for MCP users)

```bash
npx -y agentika-grep --mcp --root .
```

Or install globally:

```bash
npm install -g agentika-grep
```

### cargo-binstall (pre-built binary)

```bash
cargo binstall agentika-grep
```

### cargo install (compile from source)

Requires Rust 1.75+:

```bash
cargo install agentika-grep
```

### GitHub Releases (manual download)

Download the binary for your platform from
[GitHub Releases](https://github.com/agentika/agentika-grep/releases),
extract, and place in your `PATH`.

<details>
<summary>macOS quarantine removal</summary>

macOS may quarantine unsigned binaries. After extracting:

```bash
xattr -d com.apple.quarantine agentika-grep
chmod +x agentika-grep
sudo mv agentika-grep /usr/local/bin/
```
</details>

## Quick Start

```bash
# Index a codebase
agentika-grep index --root /path/to/project

# Search
agentika-grep search "authentication" --root /path/to/project

# Run as MCP server
agentika-grep --mcp --root /path/to/project
```

## MCP Server Setup

### Claude Code

Add to your project's `.mcp.json` or global `~/.claude.json`:

```json
{
  "mcpServers": {
    "agentika-grep": {
      "command": "npx",
      "args": ["-y", "agentika-grep", "--mcp", "--root", "/path/to/project"]
    }
  }
}
```

Or with a locally installed binary:

```json
{
  "mcpServers": {
    "agentika-grep": {
      "command": "agentika-grep",
      "args": ["--mcp", "--root", "/path/to/project"]
    }
  }
}
```

Or use the CLI:

```bash
claude mcp add agentika-grep -- npx -y agentika-grep --mcp --root /path/to/project
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

**Verify** - Run `/permissions` in Claude Code to see active permissions, or `/doctor` to check for issues.

### Cursor

Add to `~/.cursor/mcp.json` or project `.cursor/mcp.json`:

```json
{
  "mcpServers": {
    "agentika-grep": {
      "command": "npx",
      "args": ["-y", "agentika-grep", "--mcp", "--root", "/path/to/project"]
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
      "command": ["npx", "-y", "agentika-grep", "--mcp", "--root", "/path/to/project"]
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

## CLI Usage

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

## Index Location

By default, the index is stored in a **global cache directory**, not in the project:

| Platform | Default Location |
|----------|------------------|
| macOS    | `~/Library/Caches/agentika-grep/<hash>.db` |
| Linux    | `~/.cache/agentika-grep/<hash>.db` |
| Windows  | `%LOCALAPPDATA%\agentika-grep\<hash>.db` |

The `<hash>` is derived from the absolute path to `--root`, ensuring each project gets its own index without polluting the project directory.

Use `--db` to specify a custom location:

```bash
agentika-grep --mcp --root /path/to/project --db /custom/path/index.db
```

## Build from Source

```bash
# Debug build
cargo build

# Release build
cargo build --release

# Release with profiling
cargo build --release --features profiling

# Run tests
cargo test

# Run benchmarks
cargo bench
```

## Profiling

Build with the `profiling` feature to enable timing and memory logging:

```bash
cargo build --release --features profiling
```

When enabled, each tool invocation logs performance metrics to stderr:

```
[search] 42ms | mem: 128.5MB (+2.1MB)
[index] 1.2s | mem: 256.0MB (+127.5MB)
```

**MCP mode** - use `--log-file` to capture logs:

```json
{
  "mcpServers": {
    "agentika-grep": {
      "command": "agentika-grep",
      "args": ["--mcp", "--root", "/path/to/project", "--log-file", "/tmp/agentika-grep.log"]
    }
  }
}
```

Then: `tail -f /tmp/agentika-grep.log`

## Configuration

- **Max file size**: 1MB (files larger than this are skipped during indexing)
- **Gitignore**: Patterns in `.gitignore` are respected during indexing
- **Logging**: All logs go to stderr (stdout is reserved for JSON-RPC in MCP mode)

## License

MIT
