# Claude Code Setup

## MCP Server Setup

```bash
# For all your projects (user-level — recommended)
claude mcp add -s user grepika -- npx -y @agentika/grepika --mcp

# For this project only (shared with team via .mcp.json)
claude mcp add -s project grepika -- npx -y @agentika/grepika --mcp
```

> **Scope reference:** `-s user` writes to `~/.claude.json` (all projects), `-s project` writes to `.mcp.json` (committed, shared with team), `-s local` (default) writes to `.claude/settings.local.json` (gitignored, personal).

For Cursor and OpenCode setup, see the [README](../README.md#mcp-server-setup).

## CLAUDE.md Snippet

Copy this section into your project's `CLAUDE.md` file to instruct Claude Code to prefer grepika for code search operations.

---

## Code Search

Use the grepika MCP server for all code search operations instead of built-in Grep/Glob tools:

| Task | Use This Tool | Instead Of |
|------|---------------|------------|
| **Index codebase** | `mcp__grepika__index` | N/A (run first!) |
| Pattern search | `mcp__grepika__search` | `Grep` |
| Find relevant files | `mcp__grepika__relevant` | `Glob`, `Grep` |
| Get file content | `mcp__grepika__get` | `Read` (for search results) |
| File structure | `mcp__grepika__outline` | Manual parsing |
| Directory tree | `mcp__grepika__toc` | `Glob` with patterns |
| Context around line | `mcp__grepika__context` | `Read` with offset |
| Find references | `mcp__grepika__refs` | `Grep` for symbol |
| Related files | `mcp__grepika__related` | Multiple `Grep` calls |
| Index statistics | `mcp__grepika__stats` | N/A |
| **Set workspace** | `mcp__grepika__add_workspace` | N/A (global mode only) |

**First time setup:** Run `mcp__grepika__index` to build the search index before using other tools. The index updates incrementally on subsequent runs.

**Global Mode:** When the server is started with `--mcp` (without `--root`), it runs in global mode. The LLM must call `mcp__grepika__add_workspace` with the project root path before using any other tools. The server's `get_info()` response will guide this. This is the recommended setup — the LLM reads its working directory from its system prompt and calls `add_workspace` automatically.

**Why prefer grepika:**
- Combines FTS5 + ripgrep + trigram indexing for superior search quality
- Returns ranked results with relevance scores
- More token-efficient than multiple Grep/Glob calls
- Maintains an incremental index for faster subsequent searches

**When to still use Claude Code's built-in tools:**
- `Read` for viewing specific files you already know the path to
- `Bash` for git operations, builds, and running commands
- `Edit`/`Write` for modifying files (grepika is read-only)

---

## Minimal Version

If you prefer a shorter snippet:

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

---

## Pre-authorizing Permissions

To avoid permission prompts for grepika tools:

**Project-Level (Recommended)** - Add to `.claude/settings.local.json`:

```json
{
  "permissions": {
    "allow": [
      "mcp__grepika__*"
    ]
  }
}
```

**Global (All Projects)** - Add to `~/.claude/settings.json`:

```json
{
  "permissions": {
    "allow": [
      "mcp__grepika__*"
    ]
  }
}
```

**Explicit Tool List** - If you prefer explicit permissions:

```json
{
  "permissions": {
    "allow": [
      "mcp__grepika__search",
      "mcp__grepika__relevant",
      "mcp__grepika__refs",
      "mcp__grepika__related",
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

**Verify** - Run `/permissions` in Claude Code to see active permissions, or `/doctor` to check for issues.
