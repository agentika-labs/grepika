---
name: refs
description: |
  Find all references to a symbol using grepika's word-boundary matching
  and automatic reference classification (definition, import, type_usage, usage).
disable-model-invocation: true
context: fork
allowed-tools:
  - mcp__grepika__add_workspace
  - mcp__grepika__refs
  - mcp__grepika__context
---

Find all references to a symbol.

## Arguments

`$ARGUMENTS`

## Steps

1. Call `mcp__grepika__add_workspace` with path set to the project root directory
2. Call `mcp__grepika__refs` with the symbol from `$ARGUMENTS` and `limit: 50`
3. Group results by `ref_type` and report in this exact format:

    ## References: [symbol]

    ### Definition
    | File | Line | Context |
    |------|------|---------|
    | `path` | N | code snippet |

    ### Imports
    | File | Line | Context |
    |------|------|---------|
    | `path` | N | import statement |

    ### Type Usage
    | File | Line | Context |
    |------|------|---------|
    | `path` | N | type annotation |

    ### Usage (Call Sites)
    | File | Line | Context |
    |------|------|---------|
    | `path` | N | call/reference |

    **[N] total references** across [M] files

## Important

- Do NOT use a sub-agent. Call the MCP tools directly in the main conversation.
- Be concise. No extra explanation needed beyond the tables.
- Omit empty sections (e.g., if no imports found, skip that table).
