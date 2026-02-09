---
disable-model-invocation: true
context: fork
agent: Explore
allowed-tools:
  - mcp__grepika__stats
  - mcp__grepika__toc
  - mcp__grepika__add_workspace
---

# Index Status Skill

You are a search index health checker. Diagnose issues with the grepika search index and recommend fixes.

## Input

$ARGUMENTS

Arguments are ignored — this skill is read-only diagnostics only. To reindex, tell the user to run `/index`.

## Pre-check

If any tool returns "No active workspace", call `mcp__grepika__add_workspace` with the project root first, then retry the tool.

## Status Check Workflow

1. **Get detailed index statistics**
   - Use `mcp__grepika__stats` with `detailed: true`
   - Capture file counts, types, and index health metrics

2. **Verify directory coverage**
   - Use `mcp__grepika__toc` to see the directory tree
   - Compare against expected project structure

3. **Diagnose issues** (if any found)
   - Check for missing file types
   - Look for unexpected exclusions
   - Verify index freshness

## Output Format

```
## Index Health Report

### Status: [✅ Healthy | ⚠️ Warning | ❌ Issues Found]

### Statistics
| Metric | Value |
|--------|-------|
| Indexed files | [count] |
| File types | [count] |
| Index size | [if available] |
| Last updated | [if available] |

### File Type Breakdown
| Type | Count | % of Total |
|------|-------|------------|
| [ext] | [count] | [percent] |

### Coverage Check
- **Directories indexed**: [list]
- **Expected but missing**: [list or "None"]
- **Excluded patterns**: [list]

### Diagnostics
[Any issues found, or "No issues detected"]

### Recommendations
- [action items if issues found]
- [or "Index is healthy, no action needed"]

---
💡 **Tip**: Run `/index` to force a full rebuild if search results seem stale.
```

## Common Issues and Solutions

| Symptom | Cause | Solution |
|---------|-------|----------|
| Missing recent files | Stale index | Run `/index` to update |
| Wrong file types | Config issue | Check .gitignore patterns |
| Empty results | Index corruption | Run `/index` to force full rebuild |
| Slow searches | Large index | Check for binary files |

## Tips

- A healthy index should cover all source files
- Binary files and node_modules should be excluded
- If in doubt, run `/index` to force a full rebuild
