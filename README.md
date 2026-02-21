[![npm](https://img.shields.io/npm/v/@agentika/grepika)](https://www.npmjs.com/package/@agentika/grepika)
[![CI](https://github.com/agentika-labs/grepika/actions/workflows/ci.yml/badge.svg)](https://github.com/agentika-labs/grepika/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)

# grepika

Token-efficient MCP server for code search.

LLMs burn context tokens on every search call. grepika indexes your codebase and returns ranked, compact results — so the model spends tokens reasoning instead of reading raw grep output. It combines three backends (FTS5, parallel grep, and trigram indexing) and merges their scores for high-quality results.

## Why grepika?

grep is a great tool, but it wasn't designed for LLM workflows. It returns unranked file lists, and the model has to make multiple calls to piece together context. grepika gives the model structured tools for the things it actually needs to do:

| Task | grep approach | grepika |
|------|---------------|---------|
| Find a pattern | Unranked file list | Ranked results with relevance scores |
| Understand a symbol | Multiple grep calls, manual assembly | `refs` classifies definitions, imports, usages |
| Explore structure | Read entire files | `outline` extracts functions/classes/structs |
| Find related code | Guess-and-grep loop | `refs` finds files sharing symbols |
| Natural language query | Requires regex | `search` routes to BM25 full-text search |

### Benchmarks

Criterion benchmarks against ripgrep (Claude Code's Grep backend) on the grepika codebase, 9 queries across all intent categories:

| Metric | grepika | ripgrep (content mode) |
|--------|---------|------------------------|
| Search latency | 2.3–2.8 ms | 4.9–6.1 ms |
| Response size | ~2,500 B avg | ~12,600 B avg |
| Relevance ranking | BM25 + trigram IDF | None |

~80% fewer bytes on aggregate vs ripgrep content mode, with ranked results and snippets. Savings are largest on high-match queries (e.g. `fn` saves 94%); natural language queries where ripgrep finds few literal matches can be larger with grepika. See [full analysis](docs/token-efficiency-analysis.md).

### Token Efficiency

Compared to ripgrep content output (matching lines), indexed search returns fewer tokens per query. The bigger win is search quality — ranking, NLP queries, and reference classification reduce follow-up reads.

Criterion benchmarks on the grepika codebase (~25 Rust files), 9 queries across all intent categories:

```
Query             │  grepika │  ripgrep (content) │ Savings
──────────────────┼──────────┼────────────────────┼────────
SearchService     │  3,375 B │         8,610 B    │  ~61%
Score             │  2,789 B │         5,574 B    │  ~50%
Database          │  2,156 B │        10,174 B    │  ~79%
fn                │  1,832 B │        31,469 B    │  ~94%
use               │  1,963 B │        23,308 B    │  ~92%
search service    │  3,797 B │         1,632 B    │ -133%
error handling    │  2,666 B │           986 B    │ -170%
fn\s+\w+          │    385 B │        29,501 B    │  ~99%
impl.*for         │  3,214 B │         2,480 B    │  -30%
```

Savings are largest on high-match queries where ripgrep returns many unranked lines. Natural language queries (e.g. "error handling") route to FTS5 concept search in grepika but match few literals in ripgrep, making grepika's output larger.

Claude Code lazy-loads MCP tools on demand, so grepika's 11 tool schemas are not loaded all at once. Loaded schemas are prompt-cached after the first call (~90% discount on subsequent turns). In practice, schema overhead is minimal.

See [docs/token-efficiency-analysis.md](docs/token-efficiency-analysis.md) for the full comparison including Grep file-list mode and workflow analysis.

### How it works

- Three search backends (FTS5 + grep + trigram) with weighted score merging
- BM25 ranking with tuned column weights
- Query intent detection — classifies regex vs natural language vs exact symbol
- 190 tests, zero clippy warnings, Criterion benchmarks

## MCP Server Setup

By default, grepika runs in **global mode** — the server starts without `--root`, and the LLM calls `add_workspace` with its working directory automatically.

<details>
<summary><b>Claude Code</b></summary>

#### Plugin (recommended)

The grepika plugin bundles the MCP server with an exploration agent, skills, and commands.

```bash
/plugin marketplace add agentika-labs/agentika-plugin-marketplace
/plugin install grepika@agentika-labs-agentika-plugin-marketplace
```

| Type | Name | Description |
|------|------|-------------|
| Agent | Explorer | Codebase exploration agent that orchestrates grepika's search tools |
| Skill | `/learn-codebase` | Architecture overview, key modules, and suggested reading order |
| Skill | `/investigate` | Bug/error investigation — traces call chains and finds error origins |
| Skill | `/impact` | Change impact analysis — blast radius, test coverage gaps, refactoring steps |
| Skill | `/index-status` | Index health diagnostics |
| Command | `/index` | Build or refresh the search index |

#### MCP-only setup

If you prefer the MCP server without the plugin:

```bash
# For all your projects (user-level)
claude mcp add -s user grepika -- npx -y @agentika/grepika --mcp

# For this project only (shared with team via .mcp.json)
claude mcp add -s project grepika -- npx -y @agentika/grepika --mcp
```

#### Tool preference

> **Note:** Plugin users can skip this — the plugin configures tool preferences automatically.

Claude Code has built-in Grep and Glob tools. To make it prefer grepika, add to your project's `CLAUDE.md`:

````markdown
## Code Search

Prefer grepika MCP tools over built-in Grep/Glob for code search:
- `mcp__grepika__index` - Build/update search index (run first!)
- `mcp__grepika__search` - Pattern/regex search (replaces Grep)
- `mcp__grepika__toc` - Directory tree (replaces Glob patterns)
- `mcp__grepika__outline` - File structure extraction
- `mcp__grepika__refs` - Symbol references

These provide ranked results with FTS5+trigram indexing for better search quality.
````

See [docs/claude-code-setup.md](docs/claude-code-setup.md) for the full version with a tool mapping table.

#### Pre-authorizing permissions

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

</details>

<details>
<summary><b>Cursor</b></summary>

Add to `~/.cursor/mcp.json` (global) or `.cursor/mcp.json` (project):

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

See [docs/cursor-setup.md](docs/cursor-setup.md) for rules snippet and full setup.

</details>

<details>
<summary><b>OpenCode</b></summary>

Add to `opencode.json` in your project root:

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

> **Note:** OpenCode uses the `"mcp"` key (not `"mcpServers"`), and the command is an array.

See [docs/opencode-setup.md](docs/opencode-setup.md) for full setup and optional fields.

</details>

<details>
<summary><b>Other Editors</b></summary>

For any MCP-compatible editor, add to its config file:

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

</details>

<details>
<summary>Single Project Mode</summary>

Use `--root` to pre-load a specific workspace at startup. The LLM does not need to call `add_workspace`.

```bash
claude mcp add -s user grepika -- npx -y @agentika/grepika --mcp --root /path/to/project
```

Or in your editor's MCP config:

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

</details>

> **Tip:** Add `"--db", "/path/to/index.db"` to `args` to control where the index is stored.

## CLI Setup

### npm

```bash
npx -y @agentika/grepika <command>
```

### Shell script (macOS Apple Silicon)

```bash
curl -fsSL https://raw.githubusercontent.com/agentika-labs/grepika/main/install.sh | bash
```

For other platforms, download the binary from [GitHub Releases](https://github.com/agentika-labs/grepika/releases).

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

# Extract file structure (functions, classes, structs)
grepika outline <path>

# Directory tree
grepika toc --root /path/to/project -d 3

# Surrounding context for a line
grepika context <path> -l 42 -c 10

# Find all references to a symbol
grepika refs <symbol>

# Compare two files
grepika diff <file1> <file2>

# Generate shell completions
grepika completions <shell>

# Run as MCP server (global mode — LLM calls add_workspace)
grepika --mcp

# Run as MCP server (single workspace mode)
grepika --mcp --root /path/to/project
```

## Available Tools

| Tool | Description |
|------|-------------|
| `search` | Pattern search (regex/natural language) |
| `get` | File content with optional line range |
| `outline` | Extract file structure (functions, classes) |
| `toc` | Directory tree |
| `context` | Surrounding lines around a specific line |
| `stats` | Index statistics |
| `refs` | Find all references to a symbol |
| `index` | Update search index (incremental by default) |
| `diff` | Compare two files |
| `add_workspace` | Load a project workspace (global mode) |

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

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for build, test, benchmark, and profiling instructions.

## Star History

[![Star History Chart](https://api.star-history.com/svg?repos=agentika-labs/grepika&type=Date)](https://www.star-history.com/#agentika-labs/grepika&Date)

## Built by

[Agentika](https://agentika.uk) — we help teams configure and adopt AI tools. If you need help setting up grepika or other AI dev tools for your team, [get in
touch](https://agentika.uk).

## License

MIT
