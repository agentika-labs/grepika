Index the grepika codebase for code search.

## Arguments

`$ARGUMENTS`

## Behavior

- If arguments are empty or not "incremental": **force rebuild** the index (`force: true`)
- If arguments contain "incremental": **incremental update** (`force: false`, skips unchanged files)

## Steps

1. Call `mcp__agentika-grep__add_workspace` with path set to the project root directory (from your working directory context)
2. Call `mcp__agentika-grep__index` with `force: true` (default) or `force: false` (if incremental)
3. Call `mcp__agentika-grep__stats` with `detailed: true`
4. Report results in this exact format:

```
## Index Complete

| Metric | Value |
|--------|-------|
| Total files | N |
| Indexed files | N |

### File Types
[file type breakdown from stats detailed output]
```

## Important

- Do NOT use a sub-agent. Call the MCP tools directly in the main conversation.
- Be concise. No extra explanation needed beyond the table.
