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
| Find related code | Guess-and-grep loop | `related` finds files sharing symbols |
| Natural language query | Requires regex | `search` routes to BM25 full-text search |

### Benchmarks

Measured on the same codebase and queries using [Criterion](https://github.com/bheisler/criterion.rs):

| Metric | grepika | Built-in Grep (ripgrep) |
|--------|---------|-------------------------|
| Search latency | 2.5 ms | 5.3 ms |
| Response size | 364 bytes avg | 2,693 bytes avg |
| Relevance ranking | BM25 + trigram IDF | None |

Responses are ~6x smaller on average, which means the MCP schema overhead (~825 tokens) pays for itself after about 2 queries.

### How it works

- Three search backends (FTS5 + grep + trigram) with weighted score merging
- BM25 ranking with tuned column weights
- Query intent detection — classifies regex vs natural language vs exact symbol
- 190 tests, zero clippy warnings, Criterion benchmarks

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

By default, grepika runs in **global mode** — the server starts without `--root`, and the LLM calls `add_workspace` with its working directory automatically.

<details>
<summary><b>Claude Code</b></summary>

```bash
# For all your projects (user-level)
claude mcp add -s user grepika -- npx -y @agentika/grepika --mcp

# For this project only (shared with team via .mcp.json)
claude mcp add -s project grepika -- npx -y @agentika/grepika --mcp
```

See [docs/claude-code-setup.md](docs/claude-code-setup.md) for tool preference config and permissions.

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

## Claude Code Integration

### Tool Preference

Claude Code has built-in Grep and Glob tools. To make it prefer grepika, add to your project's `CLAUDE.md`:

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

See [docs/claude-code-setup.md](docs/claude-code-setup.md) for the full version with a tool mapping table.

Editor-specific guides: [Claude Code](docs/claude-code-setup.md) · [Cursor](docs/cursor-setup.md) · [OpenCode](docs/opencode-setup.md)

### Claude Code Plugin

The optional [grepika plugin](https://github.com/agentika-labs/agentika-plugin-marketplace/tree/main/plugins/grepika) adds skills, an exploration agent, and a slash command on top of the base MCP tools.

Add the marketplace and install:

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

The plugin is optional — all MCP tools work standalone without it.

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

Indexed search returns significantly fewer tokens than raw grep output. Measured against Claude Code's built-in Grep tool (ripgrep):

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

The MCP schema adds ~825 tokens of one-time overhead, which pays for itself after about 2 queries (~584 tokens saved per query).

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

## License

MIT
