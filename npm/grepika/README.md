# @agentika/grepika

Token-efficient MCP server for code search. Combines FTS5, parallel grep, and trigram indexing for ranked results with minimal token usage.

## Setup

**Claude Code:**

```bash
claude mcp add grepika -- npx -y @agentika/grepika --mcp
```

**Cursor / other MCP clients:**

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

## Tools

| Tool | Description |
|------|-------------|
| `search` | Pattern search (regex/natural language) |
| `relevant` | Find files most relevant to a topic |
| `get` | File content with optional line range |
| `outline` | Extract file structure (functions, classes) |
| `toc` | Directory tree |
| `refs` | Find all references to a symbol |
| `index` | Update search index |

## Platforms

The correct binary is installed automatically via `optionalDependencies`:

- `@agentika/grepika-darwin-arm64` — macOS Apple Silicon
- `@agentika/grepika-linux-x64` — Linux x64
- `@agentika/grepika-linux-arm64` — Linux ARM64
- `@agentika/grepika-win32-x64` — Windows x64

## Links

- [GitHub](https://github.com/agentika-labs/grepika)
- [Full documentation](https://github.com/agentika-labs/grepika#readme)

## License

MIT
