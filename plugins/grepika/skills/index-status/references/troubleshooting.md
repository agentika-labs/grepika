# Index Troubleshooting Guide

This reference helps diagnose and resolve common grepika index issues.

## Common Index Issues

### 1. Stale Index (Missing Recent Files)

**Symptoms**:
- Search doesn't find files you just created
- Results show old content that's been modified

**Diagnosis**:
```
1. Check stats() → note last indexed timestamp
2. Compare with recent file modifications
3. Look for files created after last index
```

**Solutions**:
- Run `/index` to trigger incremental update
- For persistent issues, force full rebuild

### 2. Missing File Types

**Symptoms**:
- Certain extensions don't appear in search results
- stats() shows unexpected file type distribution

**Diagnosis**:
```
1. Check stats() with detailed: true
2. Look at file type breakdown
3. Compare against expected file types in project
```

**Solutions**:
- Check if file type is in .gitignore
- Verify grepika configuration includes the file type
- Large binary files may be intentionally excluded

### 3. Empty Search Results

**Symptoms**:
- All searches return no results
- Index appears to have files but search fails

**Diagnosis**:
```
1. Check stats() → verify indexed file count
2. Try toc() → verify directory coverage
3. Try simple known-good search term
```

**Solutions**:
- Force full rebuild with `/index`
- Check for index corruption
- Verify workspace is properly configured

### 4. Slow Searches

**Symptoms**:
- Searches take several seconds
- Search mode affects performance differently

**Diagnosis**:
```
1. Check stats() → look at total file count
2. Check for unexpectedly large index
3. Look for binary files or large generated files
```

**Solutions**:
- Exclude binary files and generated code
- Use more specific search queries
- Consider adding patterns to .gitignore

### 5. Workspace Not Found

**Symptoms**:
- Tools return "No active workspace"
- Must repeatedly call add_workspace

**Diagnosis**:
- Server running in global mode without workspace set
- Workspace path incorrect

**Solutions**:
- Call add_workspace with correct project root
- Verify the path exists and contains source files

## Health Check Procedure

### Quick Check (30 seconds)

```
1. stats() → Verify file count is reasonable
2. toc() depth: 2 → Verify main directories present
3. search("function") → Verify basic search works
```

### Full Diagnostic (2 minutes)

```
1. stats(detailed: true) → Full file type breakdown
2. toc() → Complete directory tree
3. Compare file types against expected
4. Test each search mode: fts, grep, combined
5. Test refs() on known symbol
```

## Expected Healthy Values

| Metric | Healthy Range | Concern If |
|--------|---------------|------------|
| Indexed files | Project-dependent | <10 for non-trivial project |
| File types | 3-10 | 0 or >50 |
| Index freshness | <24 hours | >1 week |
| Search latency | <1 second | >5 seconds |

## Index Maintenance Best Practices

### When to Rebuild

- After major refactoring
- When switching branches significantly
- If search results seem wrong
- After pulling many changes

### Exclusion Patterns

These should typically be excluded:
- `node_modules/`
- `dist/`, `build/`, `out/`
- `.git/`
- `vendor/`, `third_party/`
- `*.min.js`, `*.bundle.js`
- Large binary files

### Monitoring Tips

- Run `/index-status` periodically
- Watch for growing index size
- Note any new file types appearing
- Check coverage after major changes

## Error Messages Reference

| Error | Cause | Fix |
|-------|-------|-----|
| "No active workspace" | Workspace not set | Call add_workspace first |
| "Index not found" | Never indexed | Run /index |
| "File not in index" | File excluded or new | Check exclusions, reindex |
| "Search timeout" | Query too broad | Use more specific query |
| "Invalid pattern" | Regex syntax error | Fix regex syntax |
