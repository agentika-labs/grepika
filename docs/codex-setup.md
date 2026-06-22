# Codex Setup

## MCP Server Setup

For all your Codex projects, add grepika as a global MCP server:

```bash
codex mcp add grepika -- npx -y @agentika/grepika --mcp
```

Verify the server is registered:

```bash
codex mcp list
codex mcp get grepika
```

In the Codex TUI, run `/mcp` to see the active MCP servers and tools for the current session.

By default this starts grepika in **global mode**. The server starts without a preloaded root, then Codex calls `add_workspace` with the repository it is working in before using indexed tools.

For other editors: [Claude Code setup](claude-code-setup.md) | [Cursor setup](cursor-setup.md) | [OpenCode setup](opencode-setup.md)

## Project-Scoped Setup

Codex can also read MCP configuration from a trusted project's `.codex/config.toml`. Use this when you want the repository to carry its own grepika setup:

```toml
[mcp_servers.grepika]
command = "npx"
args = ["-y", "@agentika/grepika", "--mcp"]
```

For single-workspace mode, add `--root` so grepika starts with a fixed project root and Codex does not need to call `add_workspace`:

```toml
[mcp_servers.grepika]
command = "npx"
args = ["-y", "@agentika/grepika", "--mcp", "--root", "/path/to/project"]
```

## AGENTS.md Snippet

Codex reads `AGENTS.md` files before it starts work. Add this to your project's `AGENTS.md` so Codex reaches for grepika for code search:

```markdown
## Code Search

Prefer grepika MCP tools over built-in search tools:

| Task | Use This Tool | Instead Of |
|------|---------------|------------|
| **Index codebase** | `grepika.index` | N/A (run first!) |
| Pattern search | `grepika.search` | shell `grep`/`rg` for broad exploration |
| Get file content | `grepika.get` | ad hoc file reads from search results |
| File structure | `grepika.outline` | Manual parsing |
| Directory tree | `grepika.toc` | Glob patterns |
| Context around line | `grepika.context` | Manual line slicing |
| Find references | `grepika.refs` | Text-only symbol grep |
| Structural search | `grepika.structural_search` | Fragile regex for AST shapes |
| Code graph | `grepika.graph` | Manual call/import tracing |
| Compare files | `grepika.diff` | Manual side-by-side reads |
| Index statistics | `grepika.stats` | N/A |
| **Set workspace** | `grepika.add_workspace` | N/A (global mode only) |

**First time setup:** Run `grepika.index` before indexed search or graph navigation. The index updates incrementally on subsequent runs.

**Global mode:** If grepika is started with `--mcp` and no `--root`, call `grepika.add_workspace` with the project root before using workspace tools.

**Why prefer grepika:**
- Combines FTS5 + ripgrep + sparse n-gram prefiltering for ranked, relevance-scored results
- Adds AST structural search and indexed call/import graph navigation
- Returns compact responses for broad searches
- Maintains an incremental index for faster subsequent searches

**When to still use built-in tools:**
- Direct file reads for files you already know you need
- Terminal commands for git operations, builds, and tests
- Edit tools for modifying files; grepika is read-only
```

### Minimal Version

```markdown
## Code Search

Prefer grepika MCP tools over built-in search tools:
- `grepika.index` - Build/update search index (run first!)
- `grepika.search` - Pattern/regex search
- `grepika.toc` - Directory tree
- `grepika.outline` - File structure extraction
- `grepika.refs` - Symbol references
- `grepika.structural_search` - AST pattern/kind search
- `grepika.graph` - Indexed call/import graph navigation

These provide ranked results with FTS5, grep, sparse n-gram prefiltering, AST search, and code-graph navigation.
```

## Tool Approval

Codex MCP tool approval can be configured in `config.toml`. To approve grepika tools by default:

```toml
[mcp_servers.grepika]
command = "npx"
args = ["-y", "@agentika/grepika", "--mcp"]
default_tools_approval_mode = "approve"
```

You can also approve only specific tools:

```toml
[mcp_servers.grepika]
command = "npx"
args = ["-y", "@agentika/grepika", "--mcp"]
default_tools_approval_mode = "prompt"

[mcp_servers.grepika.tools.search]
approval_mode = "approve"

[mcp_servers.grepika.tools.get]
approval_mode = "approve"

[mcp_servers.grepika.tools.toc]
approval_mode = "approve"

[mcp_servers.grepika.tools.outline]
approval_mode = "approve"
```

Use `/permissions` in the Codex TUI to adjust the current session's approval posture, and `/mcp` to confirm the grepika tools are available.
