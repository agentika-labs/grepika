# Grepika Code Search

ALWAYS use grepika MCP tools instead of built-in Grep/Glob for code search. Grepika combines FTS5, ripgrep-backed regex search, and sparse n-gram prefiltering for ranked results (~6x smaller responses), plus AST structural search and indexed code-graph navigation.

DO NOT use built-in Grep for code search when grepika is available. DO NOT use Glob for file discovery — use `toc` instead.

## Quick Start

1. Run `/index` to build the search index (cached across sessions)
2. Use `search` to find code, `refs` to trace symbols

## No Index Required

These tools work immediately without running `/index`:
- `refs` — find all references to a symbol
- `toc` — directory tree structure
- `outline` — file structure (functions, classes, exports)
- `context` — surrounding code for a line number
- `get` — read file content with line ranges
- `diff` — compare two files
- `structural_search` — AST pattern/kind search

`search` and `graph` use the index. Run `/index` first for best ranked search and code-graph navigation.

## Decision Tree

| Need to... | Use | Not |
|------------|-----|-----|
| Find code by pattern or concept | `mcp__grepika__search` | Grep |
| Find where a symbol is used | `mcp__grepika__refs` | Grep for identifier |
| See directory structure | `mcp__grepika__toc` | Glob patterns |
| Understand a file's shape | `mcp__grepika__outline` | Reading the whole file |
| Get code around a line | `mcp__grepika__context` | Read with offset |
| Read a file from search results | `mcp__grepika__get` | Read (relative paths) |
| Find syntax patterns | `mcp__grepika__structural_search` | Regex for AST shapes |
| Trace call/import relationships | `mcp__grepika__graph` | Manual file hopping |
| Compare two files | `mcp__grepika__diff` | Manual comparison |

Still use **Read** for known file paths, **Bash** for git/builds, **Edit/Write** for modifications.

## Search Modes

- **`combined`** (default): FTS5 + grep merged, best overall quality
- **`fts`**: Natural language queries ("error handling in auth")
- **`grep`**: Exact regex patterns (`fn\s+main`)

## Available Skills

| Skill | Purpose |
|-------|---------|
| `/find-usages` | Trace symbol references and build call hierarchies |
| `/investigate` | Trace errors and bugs through the codebase |
| `/architecture` | Map module boundaries, dependencies, and coupling |
| `/impact` | Analyze blast radius of a change or refactoring |
| `/index-status` | Diagnose search index health and recommend fixes |
| `/learn-codebase` | Onboard to a codebase — explain architecture and key patterns |
| `/diff-files` | Compare two files and explain differences |
| `/study` | Extract reusable architectural patterns into blueprints |
| `/apply-pattern` | Apply a previously extracted blueprint pattern |

## Blueprints

- **Learn patterns from a codebase** → `/study [focus]` — extracts reusable architectural patterns to `~/.claude/blueprints/`
- **Apply a learned pattern** → `/apply-pattern <source>:<pattern>` — adapts a pattern to the current project
- **List available blueprints** → `/apply-pattern --list`
