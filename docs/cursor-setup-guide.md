# Cursor + grepika Setup Guide

Set up grepika as an MCP server in Cursor for token-efficient code search with FTS5 + ripgrep + trigram indexing.

## Prerequisites

- macOS (Apple Silicon)
- [Cursor](https://cursor.com) installed
- The shared `grepika-macos-arm64.tar.gz` binary (profiling-enabled build)

## 1. Install the Binary

```bash
mkdir -p ~/.local/bin
tar -xzf grepika-macos-arm64.tar.gz
xattr -d com.apple.quarantine grepika
chmod +x grepika
mv grepika ~/.local/bin/
grepika --help
```

> **Why `xattr -d`?** macOS quarantines files downloaded from the internet. Without removing this attribute, the binary will be blocked by Gatekeeper.

## 2. Add `~/.local/bin` to PATH

Add the following to your `~/.zshrc`:

```bash
export PATH="$HOME/.local/bin:$PATH"
```

Then reload your shell:

```bash
source ~/.zshrc
```

Verify the binary is accessible:

```bash
which grepika
# Should output: /Users/<you>/.local/bin/grepika
```

## 3. Cursor MCP Config

Create or edit `~/.cursor/mcp.json` (global config):

```json
{
  "mcpServers": {
    "grepika": {
      "command": "grepika",
      "args": ["--mcp", "--log-file", "/tmp/grepika.log"]
    }
  }
}
```

**Key details:**
- **Global mode** (no `--root`): The server starts without a workspace. The AI will call `add_workspace` with the project root automatically based on its system prompt context.
- **`--log-file`**: The binary is built with the `profiling` feature, which emits timing and memory metrics to stderr. Since MCP mode uses stderr for logging, `--log-file` captures this data to a file you can inspect:

```bash
tail -f /tmp/grepika.log
```

## 4. Explorer Agent

Copy the explorer agent definition into Cursor's global agents directory:

```bash
mkdir -p ~/.cursor/agents
```

Create `~/.cursor/agents/explorer.md` with the following content (or copy from the repo):

```bash
# If you have the repo cloned:
cp .claude/plugins/grepika/agents/explorer.md ~/.cursor/agents/explorer.md
```

The explorer agent gives the AI a structured approach to codebase exploration using grepika tools — it teaches the AI to start broad with `relevant` and `toc`, then narrow down with `refs`, `outline`, and `context`.

## 5. Skills

Copy the four skill definitions into Cursor's global skills directory:

```bash
mkdir -p ~/.cursor/skills/{index-status,impact,investigate,learn-codebase}

# If you have the repo cloned:
cp .claude/skills/index-status/SKILL.md ~/.cursor/skills/index-status/SKILL.md
cp .claude/skills/impact/SKILL.md ~/.cursor/skills/impact/SKILL.md
cp .claude/skills/investigate/SKILL.md ~/.cursor/skills/investigate/SKILL.md
cp .claude/skills/learn-codebase/SKILL.md ~/.cursor/skills/learn-codebase/SKILL.md
```

### Skill Reference

| Skill | Purpose |
|-------|---------|
| `/index-status` | Check search index health, file coverage, and trigger reindexing |
| `/impact` | Analyze blast radius of changes to a symbol, function, or file |
| `/investigate` | Trace errors and bugs through the codebase to find origins and call chains |
| `/learn-codebase` | Get an architectural overview of the codebase or a specific subsystem |

## 6. Cursor User Rules

Add the grepika tool mapping to Cursor's global user rules so the AI prefers grepika over built-in search tools.

1. Open **Cursor Settings** (Cmd+,) → **Rules** → **User Rules**
2. Paste the following:

```
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

**When to still use built-in tools:**
- `Read` for viewing specific files you already know the path to
- `Bash` for git operations, builds, and running commands
- `Edit`/`Write` for modifying files (grepika is read-only)
```

> **Source**: This is the "Code Search" section from [`docs/claude-code-snippet.md`](claude-code-snippet.md) in the repo.

## 7. Verify Setup

1. **Restart Cursor** completely (Cmd+Q, reopen)

2. **Check MCP connection**: Open a new chat and look for grepika in the MCP server list. The server should show as connected.

3. **Test with `/index-status`**: Type `/index-status` in the chat to trigger the index health check skill. This will call `add_workspace` automatically, then report on the search index.

4. **Check profiling output**:

```bash
tail -f /tmp/grepika.log
```

You should see timing metrics for indexing and search operations.

## Troubleshooting

| Problem | Solution |
|---------|----------|
| "command not found" | Verify `~/.local/bin` is in your PATH (`echo $PATH`) |
| Gatekeeper blocks binary | Run `xattr -d com.apple.quarantine ~/.local/bin/grepika` |
| MCP server not connecting | Check `~/.cursor/mcp.json` is valid JSON, restart Cursor |
| "No active workspace" errors | The AI needs to call `add_workspace` first — check that user rules are set |
| Empty search results | Run `/index-status reindex` to force a full index rebuild |
| No profiling output in log | Verify the binary was built with `--features profiling` |
