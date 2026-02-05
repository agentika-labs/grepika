# Claude Code CLAUDE.md Snippet

Copy this section into your project's `CLAUDE.md` file to instruct Claude Code to prefer agentika-grep for code search operations.

---

## Code Search

Use the agentika-grep MCP server for all code search operations instead of built-in Grep/Glob tools:

| Task | Use This Tool | Instead Of |
|------|---------------|------------|
| **Index codebase** | `mcp__agentika-grep__index` | N/A (run first!) |
| Pattern search | `mcp__agentika-grep__search` | `Grep` |
| Find relevant files | `mcp__agentika-grep__relevant` | `Glob`, `Grep` |
| Get file content | `mcp__agentika-grep__get` | `Read` (for search results) |
| File structure | `mcp__agentika-grep__outline` | Manual parsing |
| Directory tree | `mcp__agentika-grep__toc` | `Glob` with patterns |
| Context around line | `mcp__agentika-grep__context` | `Read` with offset |
| Find references | `mcp__agentika-grep__refs` | `Grep` for symbol |
| Related files | `mcp__agentika-grep__related` | Multiple `Grep` calls |
| Index statistics | `mcp__agentika-grep__stats` | N/A |

**First time setup:** Run `mcp__agentika-grep__index` to build the search index before using other tools. The index updates incrementally on subsequent runs.

**Why prefer agentika-grep:**
- Combines FTS5 + ripgrep + trigram indexing for superior search quality
- Returns ranked results with relevance scores
- More token-efficient than multiple Grep/Glob calls
- Maintains an incremental index for faster subsequent searches

**When to still use Claude Code's built-in tools:**
- `Read` for viewing specific files you already know the path to
- `Bash` for git operations, builds, and running commands
- `Edit`/`Write` for modifying files (agentika-grep is read-only)

---

## Minimal Version

If you prefer a shorter snippet:

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
