# Cursor Setup

## MCP Server Setup

Add to your MCP config file:

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

| Scope | Config file |
|-------|-------------|
| Global (all projects) | `~/.cursor/mcp.json` |
| Project (shared with team) | `.cursor/mcp.json` |

For other editors: [Claude Code setup](claude-code-setup.md) · [OpenCode setup](opencode-setup.md)

## Rules Snippet

Create `.cursor/rules/grepika.mdc` to instruct the model to prefer grepika for code search:

```markdown
---
description: Prefer grepika MCP tools for code search over built-in grep/glob
alwaysApply: true
---

## Code Search

Prefer grepika MCP tools over built-in search tools:

| Task | Use This Tool | Instead Of |
|------|---------------|------------|
| **Index codebase** | `mcp__grepika__index` | N/A (run first!) |
| Pattern search | `mcp__grepika__search` | `Grep` |
| Get file content | `mcp__grepika__get` | `Read` (for search results) |
| File structure | `mcp__grepika__outline` | Manual parsing |
| Directory tree | `mcp__grepika__toc` | `Glob` with patterns |
| Context around line | `mcp__grepika__context` | `Read` with offset |
| Find references | `mcp__grepika__refs` | `Grep` for symbol |
| Structural search | `mcp__grepika__structural_search` | Fragile regex for AST shapes |
| Code graph | `mcp__grepika__graph` | Manual call/import tracing |
| Compare files | `mcp__grepika__diff` | Manual side-by-side reads |
| Index statistics | `mcp__grepika__stats` | N/A |
| **Set workspace** | `mcp__grepika__add_workspace` | N/A (global mode only) |

**First time setup:** Run `mcp__grepika__index` before indexed search or graph navigation. The index updates incrementally on subsequent runs.

**Why prefer grepika:**
- Combines FTS5 + ripgrep + sparse n-gram prefiltering for ranked, relevance-scored results
- Adds AST structural search and indexed call/import graph navigation
- Returns compact responses: 2,352 B average vs 17,336 B for ripgrep content mode in the current benchmark
- Maintains an incremental index for faster subsequent searches

**When to still use built-in tools:**
- `Read` for viewing specific files you already know the path to
- Terminal for git operations, builds, and running commands
- File editing tools for modifying files (grepika is read-only)
```

### Minimal Version

If you prefer a shorter snippet, use the same `.mdc` wrapper with this body:

```markdown
---
description: Prefer grepika MCP tools for code search over built-in grep/glob
alwaysApply: true
---

## Code Search

Prefer grepika MCP tools over built-in search tools:
- `mcp__grepika__index` - Build/update search index (run first!)
- `mcp__grepika__search` - Pattern/regex search (replaces Grep)
- `mcp__grepika__toc` - Directory tree (replaces Glob patterns)
- `mcp__grepika__outline` - File structure extraction
- `mcp__grepika__refs` - Symbol references
- `mcp__grepika__structural_search` - AST pattern/kind search
- `mcp__grepika__graph` - Indexed call/import graph navigation

These provide ranked results with FTS5, grep, sparse n-gram prefiltering, AST search, and code-graph navigation.
```

<details>
<summary>Legacy .cursorrules alternative</summary>

If your project uses the older `.cursorrules` file instead of `.cursor/rules/`, you can add the same content there without the frontmatter block. The `.mdc` format in `.cursor/rules/` is preferred — it supports metadata like `alwaysApply` and `description`.

</details>

## Tool Approval

Cursor prompts for approval on each MCP tool call by default. There is no per-tool allow-list.

To auto-approve all tool calls (including grepika), enable **Yolo mode** in Cursor settings. This applies globally — it is not scoped to specific tools or servers.
