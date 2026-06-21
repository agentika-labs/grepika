---
name: search
description: |
  Search the codebase using grepika's ranked, token-efficient search.
  Combines FTS5, ripgrep-backed regex search, and sparse n-gram prefiltering
  for relevance-ranked results.
disable-model-invocation: true
context: fork
allowed-tools:
  - mcp__grepika__add_workspace
  - mcp__grepika__search
---

Search the codebase using grepika.

## Arguments

`$ARGUMENTS`

## Behavior

- If arguments contain "fts:" prefix: use `mode: "fts"` with the rest as query
- If arguments contain "grep:" prefix: use `mode: "grep"` with the rest as query
- Otherwise: use `mode: "combined"` (default) with full arguments as query

## Steps

1. Call `mcp__grepika__add_workspace` with path set to the project root directory
2. Call `mcp__grepika__search` with the query from `$ARGUMENTS` and `limit: 20`
3. Report results in this exact format:

    ## Search Results: [query]

    | # | File | Score | Snippet |
    |---|------|-------|---------|
    | 1 | `path` | 0.95 | matching line preview |

    **[N] results** | mode: [mode] | [has_more: true/false]

## Important

- Do NOT use a sub-agent. Call the MCP tools directly in the main conversation.
- Be concise. No extra explanation needed beyond the table.
- If search returns empty results, suggest running `/index` first.
