# Claude Code Setup

## Plugin (recommended)

The grepika plugin bundles the MCP server with an exploration agent, skills, and commands — no manual configuration needed.

**Add the marketplace:**

```bash
/plugin marketplace add agentika-labs/agentika-plugin-marketplace
```

**Install the plugin:**

```bash
/plugin install grepika@agentika-labs-agentika-plugin-marketplace
```

### What it adds

| Type | Name | Description |
|------|------|-------------|
| Agent | Explorer | Codebase exploration agent that orchestrates grepika's search tools |
| Skill | `/learn-codebase` | Architecture overview, key modules, and suggested reading order |
| Skill | `/investigate` | Bug/error investigation — traces call chains and finds error origins |
| Skill | `/impact` | Change impact analysis — blast radius, test coverage gaps, refactoring steps |
| Skill | `/index-status` | Index health diagnostics |
| Command | `/index` | Build or refresh the search index |

The plugin is optional. The MCP tools work fine on their own.

## MCP-only setup

If you'd rather use just the MCP server without the plugin:

```bash
# For all your projects (user-level — recommended)
claude mcp add -s user grepika -- npx -y @agentika/grepika --mcp

# For this project only (shared with team via .mcp.json)
claude mcp add -s project grepika -- npx -y @agentika/grepika --mcp
```

> **Scope reference:** `-s user` writes to `~/.claude.json` (all projects), `-s project` writes to `.mcp.json` (committed, shared with team), `-s local` (default) writes to `.claude/settings.local.json` (gitignored, personal).

For other editors: [Cursor setup](cursor-setup.md) · [OpenCode setup](opencode-setup.md)

## Tool preference

> **Note:** Plugin users can skip this — the plugin configures tool preferences automatically.

Add this to your project's `CLAUDE.md` so Claude Code reaches for grepika instead of its built-in search tools.

## Code Search

Prefer grepika MCP tools over built-in Grep/Glob for code search:

| Task | Use This Tool | Instead Of |
|------|---------------|------------|
| **Index codebase** | `mcp__grepika__index` | N/A (run first!) |
| Pattern search | `mcp__grepika__search` | `Grep` |
| Get file content | `mcp__grepika__get` | `Read` (for search results) |
| File structure | `mcp__grepika__outline` | Manual parsing |
| Directory tree | `mcp__grepika__toc` | `Glob` with patterns |
| Context around line | `mcp__grepika__context` | `Read` with offset |
| Find references | `mcp__grepika__refs` | `Grep` for symbol |
| Index statistics | `mcp__grepika__stats` | N/A |
| **Set workspace** | `mcp__grepika__add_workspace` | N/A (global mode only) |

**First time setup:** Run `mcp__grepika__index` to build the search index before using other tools. The index updates incrementally on subsequent runs.

**Global Mode (recommended):** When the server is started with `--mcp` (without `--root`), it runs in global mode. The LLM must call `mcp__grepika__add_workspace` with the project root path before using any other tools. The server's `get_info()` response will guide this. The LLM reads its working directory from its system prompt and calls `add_workspace` automatically.

**Why prefer grepika:**
- Combines FTS5 + ripgrep + trigram indexing for ranked, relevance-scored results
- Returns compact responses — about 6x smaller than raw grep output on average
- Maintains an incremental index for faster subsequent searches

**When to still use Claude Code's built-in tools:**
- `Read` for viewing specific files you already know the path to
- `Bash` for git operations, builds, and running commands
- `Edit`/`Write` for modifying files (grepika is read-only)

<details>
<summary>Minimal version</summary>

If you prefer a shorter snippet:

```markdown
## Code Search

Prefer grepika MCP tools over built-in Grep/Glob for code search:
- `mcp__grepika__index` - Build/update search index (run first!)
- `mcp__grepika__search` - Pattern/regex search (replaces Grep)
- `mcp__grepika__toc` - Directory tree (replaces Glob patterns)
- `mcp__grepika__outline` - File structure extraction
- `mcp__grepika__refs` - Symbol references

These provide ranked results with FTS5+trigram indexing for better search quality.
```

</details>

## Pre-authorizing permissions

To skip the permission prompt on every tool call, allowlist grepika in your settings:

**Project-level (recommended)** — add to `.claude/settings.local.json`:

```json
{
  "permissions": {
    "allow": [
      "mcp__grepika__*"
    ]
  }
}
```

**Global (all projects)** — add to `~/.claude/settings.json`:

```json
{
  "permissions": {
    "allow": [
      "mcp__grepika__*"
    ]
  }
}
```

<details>
<summary>Explicit Tool List</summary>

If you prefer explicit permissions instead of the wildcard:

```json
{
  "permissions": {
    "allow": [
      "mcp__grepika__search",
      "mcp__grepika__refs",
      "mcp__grepika__outline",
      "mcp__grepika__context",
      "mcp__grepika__get",
      "mcp__grepika__toc",
      "mcp__grepika__stats",
      "mcp__grepika__index",
      "mcp__grepika__diff",
      "mcp__grepika__add_workspace"
    ]
  }
}
```

</details>

**Verify** — run `/permissions` in Claude Code to see active permissions, or `/doctor` to check for issues.
