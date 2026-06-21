# Error Pattern Recognition Guide

This reference helps identify common error signatures and trace their origins effectively.

## Error Message Signatures

### JavaScript/TypeScript Errors

| Pattern | Meaning | Trace Strategy |
|---------|---------|----------------|
| `TypeError: Cannot read property 'x' of undefined` | Null access | Search for property access patterns, trace data flow |
| `ReferenceError: x is not defined` | Missing import/declaration | Search for import statements, check scoping |
| `SyntaxError: Unexpected token` | Parse failure | Usually file-level, check recent changes |
| `RangeError: Maximum call stack size exceeded` | Infinite recursion | Find recursive functions, trace call cycles |
| `Error: ENOENT: no such file or directory` | File not found | Search for file operations, path construction |

### Promise/Async Errors

| Pattern | Meaning | Trace Strategy |
|---------|---------|----------------|
| `UnhandledPromiseRejection` | Missing catch | Find async functions, trace promise chains |
| `Error: Timeout exceeded` | Slow operation | Search for setTimeout, API calls, database queries |
| `AbortError: The operation was aborted` | Cancelled request | Find AbortController usage, cancellation logic |

### HTTP/API Errors

| Pattern | Meaning | Trace Strategy |
|---------|---------|----------------|
| `Error: Network request failed` | Connection issue | Find fetch/axios calls, check URL construction |
| `Error: 401 Unauthorized` | Auth failure | Trace token/session handling, header setting |
| `Error: 404 Not Found` | Wrong endpoint | Search for route definitions, URL building |
| `Error: 500 Internal Server Error` | Server-side bug | Check server logs, trace request handlers |

## Call Chain Analysis Techniques

### 1. Bottom-Up Tracing

Start from the error location and trace callers upward:

```
1. Find error throw site with search()
2. Get function name from context
3. Use refs() to find all callers
4. Repeat until reaching entry point
```

### 2. Top-Down Tracing

Start from user action and trace downward:

```
1. Identify entry point (route handler, event listener)
2. Use outline() to see function structure
3. Follow function calls with refs()
4. Stop when reaching error site
```

### 3. Data Flow Tracing

Follow the data that causes the error:

```
1. Identify the variable/value that's wrong
2. Search for assignments to that variable
3. Trace parameter passing through calls
4. Find where bad data originates
```

## Error Handling Patterns to Watch

### Swallowed Errors

```javascript
// Silent catch - errors disappear
try { ... } catch (e) { }

// Logged but not re-thrown
try { ... } catch (e) { console.log(e) }
```

Search for: `catch` followed by empty blocks or single log statements.

### Error Transformation

```javascript
// Original error lost
throw new Error("Something went wrong")

// Proper wrapping preserves cause
throw new Error("Failed to process", { cause: originalError })
```

Search for: `throw new Error` without `cause` option.

### Error Boundaries

In React apps, search for `componentDidCatch` or `ErrorBoundary` components that might catch and transform errors before they reach logs.

## Investigation Checklist

- [ ] Found exact error throw site
- [ ] Identified complete call chain
- [ ] Checked for error transformation/wrapping
- [ ] Located relevant error handling
- [ ] Identified data source causing error
- [ ] Found any related logging
- [ ] Checked test coverage for error path

## Common Pitfalls

1. **Minified stack traces** - Search for source maps or original function names
2. **Async gaps in traces** - Promise chains break stack traces; search for await/then
3. **Re-thrown errors** - The throw site may not be the origin; look for catch blocks
4. **Dynamic errors** - Template literals in error messages make searching harder
5. **Third-party errors** - Check node_modules for error origins if not found in source
