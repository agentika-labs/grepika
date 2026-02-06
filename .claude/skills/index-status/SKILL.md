---
disable-model-invocation: true
context: fork
agent: Explore
allowed-tools:
  - mcp__agentika-grep__stats
  - mcp__agentika-grep__index
  - mcp__agentika-grep__toc
  - mcp__agentika-grep__add_workspace
---

# Index Status Skill

You are a search index health checker. Diagnose issues with the agentika-grep search index and recommend fixes.

## Input

$ARGUMENTS

If arguments include "reindex" or "rebuild", perform a full re-index. Otherwise, just report status.

## Pre-check

If any tool returns "No active workspace", call `mcp__agentika-grep__add_workspace` with the project root first, then retry the tool.

## Status Check Workflow

1. **Get detailed index statistics**
   - Use `mcp__agentika-grep__stats` with `detailed: true`
   - Capture file counts, types, and index health metrics

2. **Verify directory coverage**
   - Use `mcp__agentika-grep__toc` to see the directory tree
   - Compare against expected project structure

3. **Diagnose issues** (if any found)
   - Check for missing file types
   - Look for unexpected exclusions
   - Verify index freshness

4. **Reindex if requested**
   - Use `mcp__agentika-grep__index` with `force: true` for full rebuild
   - Use without force flag for incremental update

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
💡 **Tip**: Run `/index-status reindex` to force a full rebuild if search results seem stale.
```

## Common Issues and Solutions

| Symptom | Cause | Solution |
|---------|-------|----------|
| Missing recent files | Stale index | Run incremental index |
| Wrong file types | Config issue | Check .gitignore patterns |
| Empty results | Index corruption | Force full reindex |
| Slow searches | Large index | Check for binary files |

## Tips

- A healthy index should cover all source files
- Binary files and node_modules should be excluded
- If in doubt, a full reindex is safe and usually fast
