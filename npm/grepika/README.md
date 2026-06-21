# @agentika/grepika

Token-efficient MCP server for code search. Combines FTS5, parallel grep, sparse n-gram prefiltering, AST structural search, and code-graph navigation for ranked results with minimal token usage.

## Setup

### Claude Code — Plugin (recommended)

The plugin bundles the MCP server with an exploration agent, skills, and commands:

```bash
/plugin marketplace add agentika-labs/agentika-plugin-marketplace
/plugin install grepika@agentika-marketplace
```

Adds `/learn-codebase`, `/investigate`, `/impact`, `/index-status` skills and an Explorer agent. See the [full documentation](https://github.com/agentika-labs/grepika#claude-code-plugin) for details.

### Claude Code — MCP-only

If you'd rather use just the MCP server without the plugin:

```bash
claude mcp add -s user grepika -- npx -y @agentika/grepika --mcp
```

### Cursor / other MCP clients

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
| `search` | Indexed pattern, regex, and natural-language search |
| `get` | File content with optional line range |
| `outline` | Extract file structure (functions, classes) |
| `toc` | Directory tree |
| `context` | Surrounding lines around a specific line |
| `stats` | Index statistics |
| `refs` | Find all references to a symbol |
| `structural_search` | Syntax-aware AST pattern/kind search via ast-grep |
| `graph` | Navigate indexed call/import graph relationships |
| `index` | Update search index |
| `diff` | Compare two files |
| `add_workspace` | Load a project workspace in global mode |

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
