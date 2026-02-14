# @agentika/grepika

Token-efficient MCP server for code search. Combines FTS5, parallel grep, and trigram indexing for ranked results with minimal token usage.

## Setup

**Claude Code:**

```bash
claude mcp add -s user grepika -- npx -y @agentika/grepika --mcp
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
| `get` | File content with optional line range |
| `outline` | Extract file structure (functions, classes) |
| `toc` | Directory tree |
| `refs` | Find all references to a symbol |
| `index` | Update search index |

## Claude Code Plugin

Install the optional plugin for codebase exploration skills:

```bash
/plugin marketplace add agentika-labs/agentika-plugin-marketplace
/plugin install grepika@agentika-labs-agentika-plugin-marketplace
```

Adds `/learn-codebase`, `/investigate`, `/impact`, `/index-status` skills and an Explorer agent. See the [full documentation](https://github.com/agentika-labs/grepika#claude-code-plugin) for details.

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
