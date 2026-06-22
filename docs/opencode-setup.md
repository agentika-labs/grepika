# OpenCode Setup

## MCP Server Setup

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

> **Note:** OpenCode uses the `"mcp"` key (not `"mcpServers"`), and the command is an array (not a string with separate `args`).

Verify the server is registered:

```bash
opencode mcp list
```

<details>
<summary>Optional fields</summary>

```json
{
  "mcp": {
    "grepika": {
      "type": "local",
      "command": ["npx", "-y", "@agentika/grepika", "--mcp"],
      "enabled": true,
      "timeout": 30,
      "environment": {
        "NODE_ENV": "production"
      }
    }
  }
}
```

</details>

For other editors: [Claude Code setup](claude-code-setup.md) · [Codex setup](codex-setup.md) · [Cursor setup](cursor-setup.md)

## AGENTS.md Snippet

OpenCode reads instructions from `AGENTS.md`. Add this to your project's `AGENTS.md` (or `~/.config/opencode/AGENTS.md` for global):

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

---

> **Fallback:** If no `AGENTS.md` exists, OpenCode also reads `CLAUDE.md`. You can also use the `"instructions"` array in `opencode.json` to point to instruction files by path.

### Minimal Version

```markdown
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

## Tool Permissions

OpenCode supports per-tool permissions in `opencode.json`. To auto-approve all grepika tools:

```json
{
  "permission": {
    "grepika_*": "allow"
  }
}
```

Values: `"allow"`, `"deny"`, `"ask"` (default).

<details>
<summary>Explicit tool list</summary>

```json
{
  "permission": {
    "grepika_search": "allow",
    "grepika_refs": "allow",
    "grepika_structural_search": "allow",
    "grepika_graph": "allow",
    "grepika_outline": "allow",
    "grepika_context": "allow",
    "grepika_get": "allow",
    "grepika_toc": "allow",
    "grepika_stats": "allow",
    "grepika_index": "allow",
    "grepika_diff": "allow",
    "grepika_add_workspace": "allow"
  }
}
```

</details>
