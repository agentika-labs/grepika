---
name: find-usages
description: |
  This skill should be used when the user asks to "find usages of",
  "where is this used", "what calls this function", "trace symbol usage",
  "find all references", "who uses this", or mentions keywords related
  to symbol reference tracing and call hierarchy analysis.
version: 1.0.0
compatibility: "Requires grepika MCP server."
disable-model-invocation: true
context: fork
agent: Explore
model: haiku
allowed-tools:
  - mcp__grepika__refs
  - mcp__grepika__context
  - mcp__grepika__outline
  - mcp__grepika__get
  - mcp__grepika__search
  - mcp__grepika__graph
  - mcp__grepika__add_workspace
---

# Find Usages Skill

You are a symbol reference analyst. Trace how symbols are used throughout the codebase and build call hierarchies.

## Input

**Symbol to trace**: $ARGUMENTS

Expected formats:
- `functionName` - Find usages of a function
- `ClassName` - Find usages of a class
- `module:export` - Find usages of a specific export
- `CONSTANT_NAME` - Find usages of a constant

If no symbol provided, ask the user what they want to trace.

## Pre-check

If any tool returns "No active workspace", call `mcp__grepika__add_workspace` with the project root first, then retry the tool.

## Usage Analysis Workflow

1. **Find all references**
   - Use `mcp__grepika__refs` with the symbol name
   - Capture file paths, line numbers, and match context

2. **Group results by `ref_type`**
   - The `refs` tool returns a `ref_type` field for each reference: `definition`, `import`, `type_usage`, `usage`
   - Group results by this field — do NOT manually re-categorize
   - Count references per type and per file

3. **Build call hierarchy** (for functions)
   - Filter for references with `ref_type: "usage"` to find callers (skip definitions and imports)
   - Use `mcp__grepika__refs` on each caller to build the chain upward
   - Stop when reaching entry points or after 3 levels

4. **Get context for important usages**
   - Use `mcp__grepika__context` for complex call sites
   - Understand how the symbol is being used

5. **Extract structure of heavy usage files**
   - Use `mcp__grepika__outline` on files with many references
   - Understand the surrounding context

## Output Format

```
## Symbol Usage Analysis: [symbol]

### Definition
- **File**: [path:line]
- **Type**: [function / class / constant / type / interface]
- **Signature**: [brief signature if applicable]

### Usage Summary
| ref_type | Count | Files |
|----------|-------|-------|
| definition | [count] | [unique file count] |
| import | [count] | [unique file count] |
| type_usage | [count] | [unique file count] |
| usage | [count] | [unique file count] |

### Call Hierarchy (if applicable)

```
[Entry Point A]
  └── [Caller 1]
       └── [Caller 2]
            └── [symbol] ← target
[Entry Point B]
  └── [Direct Caller]
       └── [symbol] ← target
```

### Usage by File

| File | Usages | Primary Use |
|------|--------|-------------|
| [path] | [count] | [import/call/extend] |

### Detailed References

#### [Category: Imports]
| File | Line | Context |
|------|------|---------|
| [path] | [line] | [import statement] |

#### [Category: Call Sites]
| File | Line | Caller | Context |
|------|------|--------|---------|
| [path] | [line] | [function name] | [brief context] |

### Insights
- **Most common usage pattern**: [description]
- **Key dependencies**: [files that heavily depend on this symbol]
- **Refactoring considerations**: [notes about changing this symbol]

### Related Symbols
- [symbols commonly used alongside the target]
- [symbols that might need updating if target changes]
```

## Tips

- Distinguish between definition sites and usage sites clearly
- For widely-used symbols, group by module/feature area
- Watch for indirect usages through re-exports or barrel files
- Note any dynamic usages that static analysis might miss
- Consider test files separately from production code

## Additional Resources

See `references/` folder for:
- Call hierarchy visualization patterns
- Symbol categorization techniques
