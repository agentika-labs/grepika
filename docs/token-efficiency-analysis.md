# grepika vs Built-in Search

## Search Quality

grep is a pattern matcher. grepika is a code search engine. The difference shows up
when the LLM needs to understand code, not just find strings.

### What grepika can do that Grep can't

| Task | Grep | grepika |
|------|------|---------|
| Natural language query | Needs regex | FTS5 routes "authentication flow" to concept search |
| Ranked results | Flat file list | BM25 + grep + sparse n-gram scoring, best matches first |
| Reference classification | Finds the string | `refs` tells you: definition, import, usage, type_usage |
| File structure | Read the whole file | `outline` extracts functions/classes/structs |
| Related code | Guess and grep again | `refs` classifies definitions, imports, usages |
| Syntax patterns | Regex over text | `structural_search` matches AST patterns and node kinds |
| Relationships | Manual file hopping | `graph` navigates indexed call/import edges |

### Where Grep is better

- Exact regex patterns — Grep is precise and has no indexing step
- Known file paths — Read is direct, no MCP wrapper needed
- Simple file discovery — Glob with patterns like `**/*.rs`

### Persistent index

Grep scans the filesystem on every call. grepika indexes once and persists the db
across sessions at `~/.cache/grepika/<hash>.db`.

- First session: `add_workspace` + `index` (full index, one-time)
- Subsequent sessions: `index` verifies xxHash digests, skips unchanged files (~50 tokens, milliseconds)
- Search hits SQLite FTS5 + in-memory sparse n-gram posting lists
- Incremental: only changed files get re-read and re-indexed

## Token Efficiency

### How the index reduces tokens

ripgrep scans the filesystem and returns all matches — it can't rank because
it has no term frequencies or document statistics. grepika's index stores
term statistics, sparse n-gram posting lists, graph data, and file metadata,
enabling it to return only the top-N most relevant results instead of everything.

The savings come from this mechanism: "return top-20 ranked results"
vs "return all matching lines."

### What we measured

We ran queries through ripgrep (Claude Code's Grep backend) and grepika `search`
on the grepika codebase. The benchmark suite
(`benchmarks/token_efficiency.rs`) covers 9 queries across all 4 `QueryIntent`
categories: exact symbols, short tokens, natural language, and regex.

#### Query: "SearchService" (exact symbol)
| Tool | Mode | Bytes | Tokens (~) | What you get |
|------|------|-------|------------|--------------|
| Grep | files_with_matches | ~500 B* | ~125 | Bare file paths |
| ripgrep | content (matching lines) | 10,053 B | ~2,513 | Unranked matching lines |
| grepika | search (20 results) | 3,227 B | ~807 | 20 ranked results + scores + snippets |

#### Query: "fn" (short token — many matches)
| Tool | Mode | Bytes | Tokens (~) | What you get |
|------|------|-------|------------|--------------|
| Grep | files_with_matches | ~500 B* | ~125 | Bare file paths |
| ripgrep | content (matching lines) | 46,125 B | ~11,531 | Unranked matching lines |
| grepika | search (20 results) | 2,880 B | ~720 | 20 ranked results + scores + snippets |

\* File-list mode bytes are approximate (not benchmarked). Grep file-list returns
fewer bytes than grepika but provides no context about what matched or why.

### The comparison depends on what you're comparing against

- **Grep file-list mode**: Returns fewer bytes (~500 B) than grepika (2,352 B avg).
  But it gives the LLM zero context about what matched or why.
- **ripgrep content mode**: Returns more bytes on average (17,336 B) than
  grepika (2,352 B) — and grepika's results are ranked. However, on low-match
  patterns where ripgrep finds few lines, grepika's structured JSON can be larger.
- **Full workflow**: Grep file-list mode needs 5-10 follow-up Read calls to get
  context. grepika's snippets often provide enough to act on directly, needing
  only 1-3 targeted `get` calls.

### Per-query comparison (Criterion benchmarks)

Compared against ripgrep content mode on the grepika codebase, 9 queries
covering all `QueryIntent` categories:

```
Query                │  grepika │  ripgrep (content) │ Savings
─────────────────────┼──────────┼────────────────────┼────────
SearchService        │  3,227 B │        10,053 B    │  67.9%
Score                │  2,433 B │         6,497 B    │  62.6%
Database             │  2,680 B │        12,379 B    │  78.4%
fn                   │  2,880 B │        46,125 B    │  93.8%
use                  │  2,676 B │        31,868 B    │  91.6%
search service       │    787 B │         1,556 B    │  49.4%
error handling       │    782 B │           985 B    │  20.6%
fn\s+\w+             │  2,896 B │        43,898 B    │  93.4%
impl.*for            │  2,806 B │         2,664 B    │  -5.3%
─────────────────────┼──────────┼────────────────────┼────────
Average              │  2,352 B │        17,336 B    │  61.4%*
```

Savings are largest on high-match queries (symbols, short tokens, regex) where
ripgrep returns many unranked lines. grepika can be larger on low-match
patterns such as `impl.*for`, where ripgrep's output is small while grepika's
structured JSON adds overhead.

\* The 61.4% average is the mean of per-query savings percentages. The
byte-weighted reduction across the benchmark set is 86.4%
(`(17,336 - 2,352) / 17,336`). The bigger win is qualitative: ranked results
with snippets vs unranked matching lines.

### Structural search and graph benchmarks

The search benchmark coverage is not limited to text search. `benchmarks/hot_paths.rs`
also includes structural-search groups for ast-grep pattern and node-kind
queries, a regex comparison over the same generated files, and a real-repo
`structural_kind_functions` case. The graph benchmark group covers call/import
fan-out reads and batched graph replacement writes. These benchmarks measure
the MCP server's syntax-aware and relationship-aware paths separately from the
token-efficiency comparison above.

### MCP schema overhead

grepika's 12 tools total ~2,869 tokens (11,475 bytes) if loaded all at once.
In practice, Claude Code lazy-loads MCP tools on demand — only the tools
actually invoked add schema tokens to the context. Loaded schemas are
prompt-cached after the first API call (~90% discount on subsequent turns).
Tool call results in conversation history are also cached.

With the current average response sizes, the full schema would break even after
one average search compared with ripgrep content output. In real sessions, the
lazy-loaded and cached schema cost is lower.

### Methodology

- Token approximation: 1 token ~ 4 bytes
- All numbers from Criterion benchmarks (`benchmarks/token_efficiency.rs`) run on
  the grepika codebase against ripgrep content-mode output
- 9 queries covering all 4 `QueryIntent` categories: exact symbols (SearchService,
  Score, Database), short tokens (fn, use), natural language (search service,
  error handling), and regex (fn\s+\w+, impl.*for)
- MCP schema size measured by extracting live tool schemas from `GrepikaServer`
- File-list mode bytes are approximate (not benchmarked)
