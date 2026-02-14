# grepika vs Built-in Search

## Search Quality

grep is a pattern matcher. grepika is a code search engine. The difference shows up
when the LLM needs to understand code, not just find strings.

### What grepika can do that Grep can't

| Task | Grep | grepika |
|------|------|---------|
| Natural language query | Needs regex | FTS5 routes "authentication flow" to concept search |
| Ranked results | Flat file list | BM25 + trigram scoring, best matches first |
| Reference classification | Finds the string | `refs` tells you: definition, import, usage, type_usage |
| File structure | Read the whole file | `outline` extracts functions/classes/structs |
| Related code | Guess and grep again | `refs` classifies definitions, imports, usages |

### Where Grep is better

- Exact regex patterns — Grep is precise and has no indexing step
- Known file paths — Read is direct, no MCP wrapper needed
- Simple file discovery — Glob with patterns like `**/*.rs`

### Persistent index

Grep scans the filesystem on every call. grepika indexes once and persists the db
across sessions at `~/.cache/grepika/<hash>.db`.

- First session: `add_workspace` + `index` (full index, one-time)
- Subsequent sessions: `index` verifies xxHash digests, skips unchanged files (~50 tokens, milliseconds)
- Search hits SQLite FTS5 + in-memory trigrams — sub-5ms even on large codebases
- Incremental: only changed files get re-read and re-indexed

## Token Efficiency

### How the index reduces tokens

ripgrep scans the filesystem and returns all matches — it can't rank because
it has no term frequencies or document statistics. grepika's index stores
pre-computed BM25 scores, trigram counts, and file metadata, enabling it to
return only the top-N most relevant results instead of everything.

The savings come from this mechanism: "return top-20 ranked results"
vs "return all matching lines."

### What we measured

We ran queries through ripgrep (Claude Code's Grep backend) and grepika `search`
on the grepika codebase (~25 Rust files). The benchmark suite
(`benches/token_efficiency.rs`) covers 9 queries across all 4 `QueryIntent`
categories: exact symbols, short tokens, natural language, and regex.

#### Query: "SearchService" (exact symbol)
| Tool | Mode | Bytes | Tokens (~) | What you get |
|------|------|-------|------------|--------------|
| Grep | files_with_matches | ~500 B* | ~125 | Bare file paths |
| ripgrep | content (matching lines) | ~8,610 B | ~2,153 | Unranked matching lines |
| grepika | search (20 results) | ~3,375 B | ~844 | 20 ranked results + scores + snippets |

#### Query: "fn" (short token — many matches)
| Tool | Mode | Bytes | Tokens (~) | What you get |
|------|------|-------|------------|--------------|
| Grep | files_with_matches | ~500 B* | ~125 | Bare file paths |
| ripgrep | content (matching lines) | ~31,469 B | ~7,867 | Unranked matching lines |
| grepika | search (20 results) | ~1,832 B | ~458 | 20 ranked results + scores + snippets |

\* File-list mode bytes are approximate (not benchmarked). Grep file-list returns
fewer bytes than grepika but provides no context about what matched or why.

### The comparison depends on what you're comparing against

- **Grep file-list mode**: Returns fewer bytes (~500 B) than grepika (~2,500 B avg).
  But it gives the LLM zero context about what matched or why.
- **ripgrep content mode**: Returns more bytes on average (~12,600 B) than
  grepika (~2,500 B) — and grepika's results are ranked. However, on natural
  language queries where ripgrep finds few literal matches, grepika can be larger.
- **Full workflow**: Grep file-list mode needs 5-10 follow-up Read calls to get
  context. grepika's snippets often provide enough to act on directly, needing
  only 1-3 targeted `get` calls.

### Per-query comparison (Criterion benchmarks)

Compared against ripgrep content mode on the grepika codebase, 9 queries
covering all `QueryIntent` categories:

```
Query                │  grepika │  ripgrep (content) │ Savings
─────────────────────┼──────────┼────────────────────┼────────
SearchService        │  3,375 B │         8,610 B    │  ~61%
Score                │  2,789 B │         5,574 B    │  ~50%
Database             │  2,156 B │        10,174 B    │  ~79%
fn                   │  1,832 B │        31,469 B    │  ~94%
use                  │  1,963 B │        23,308 B    │  ~92%
search service       │  3,797 B │         1,632 B    │ -133%
error handling       │  2,666 B │           986 B    │ -170%
fn\s+\w+             │    385 B │        29,501 B    │  ~99%
impl.*for            │  3,214 B │         2,480 B    │  -30%
─────────────────────┼──────────┼────────────────────┼────────
Average              │  2,464 B │        12,637 B    │
```

Savings are largest on high-match queries (symbols, short tokens, regex) where
ripgrep returns many unranked lines. grepika can be larger on natural language
queries — "error handling" routes to FTS5 concept search in grepika but ripgrep
treats it as a literal string, finding few matches (986 B). Similarly,
`impl.*for` matches relatively few lines, so ripgrep's output is small while
grepika's structured JSON adds overhead.

On aggregate, grepika returns ~80% fewer bytes
(`(12,637 - 2,464) / 12,637`). The bigger win is qualitative:
ranked results with snippets vs unranked matching lines.

### MCP schema overhead

grepika's 11 tools add ~1,915 tokens (7,661 bytes) to the tool definitions.
Claude Code uses prompt caching — tool definitions are cached after
the first API call. Cached cost: ~192 tokens/turn (90% discount).
Tool call results in conversation history are also cached on subsequent turns.

Break-even: ~1 search query per session. Average per-query savings (~10,173 bytes)
exceed the full schema overhead on the first query.

### Methodology

- Token approximation: 1 token ~ 4 bytes
- All numbers from Criterion benchmarks (`benches/token_efficiency.rs`) run on
  the grepika codebase (~25 Rust files) against ripgrep content-mode output
- 9 queries covering all 4 `QueryIntent` categories: exact symbols (SearchService,
  Score, Database), short tokens (fn, use), natural language (search service,
  error handling), and regex (fn\s+\w+, impl.*for)
- MCP schema size measured by extracting live tool schemas from `GrepikaServer`
- File-list mode bytes are approximate (not benchmarked)
