# Live MCP Evaluation Harnesses

This repo now has two evaluation layers for grepika's MCP behavior. They test
different things and should stay separate.

## 1. Deterministic live MCP contract suite

Script:

```bash
python3 scripts/live_mcp_eval.py --binary target/release/grepika
```

This is a black-box protocol test. It starts the release binary in MCP stdio
mode, sends JSON-RPC messages directly, and verifies the live server contract.

It checks:

- `initialize` returns concise server instructions.
- `tools/list` exposes the expected tool names and descriptions.
- `add_workspace` works in global mode and reports the requested DB path.
- `index` builds a search index for the workspace.
- Stable tool payloads are returned for `toc`, `outline`, `refs`, `context`,
  `get`, and `search`.
- Search modes can be invoked directly, including `mode=grep` and `mode=fts`.
- The harness can record tool traces, output bytes, schema bytes, and failures.

This suite is deterministic because the script chooses the tool calls. It proves
that the MCP server is live and that the tool contracts make the desired actions
possible. It does not prove that a language model will choose those tools.

## 2. Codex LLM-agent behavior suite

Script:

```bash
python3 scripts/codex_llm_mcp_eval.py --trials 3
```

This suite runs real Codex CLI turns with grepika configured as an MCP server.
Codex receives a natural-language task and must decide which MCP tools to call.
The runner captures Codex JSONL output, reads the final answer file, and grades
both answer correctness and tool-choice behavior.

It is designed to measure whether the server instructions and tool descriptions
lead Codex to use grepika effectively:

- Calls `add_workspace` first in global mode.
- Runs `index` before indexed `search`.
- Uses `search(mode="grep")` for regex or exact-line search tasks.
- Uses `search(mode="fts")` or natural-language search for prose queries.
- Uses `refs` for exact symbol references.
- Uses `toc` and `outline` for structure questions instead of reading files.
- Uses `get` and `context` only when targeted evidence is needed.
- Avoids shell commands and direct filesystem reads.
- Avoids wasteful repeated calls and overly broad file reads.

The default case set mirrors the deterministic suite, but grading is different:
the LLM chooses the path and the runner scores that path.

## Why both are needed

The deterministic suite catches server regressions quickly. For example, it can
detect if `search(mode="grep")` stops returning proof snippets or if the schema
grows unexpectedly.

The Codex LLM suite catches instruction and discoverability regressions. For
example, it can detect if Codex starts using `get` for directory listings, uses
`search` instead of `refs` for exact symbols, or forgets to index before search.

Use the deterministic suite as a fast gate on every MCP change. Use the Codex
LLM suite when changing server instructions, tool descriptions, argument names,
or response shape.

## Recommended commands

Build the latest binary:

```bash
cargo build --release
```

Run the deterministic contract suite:

```bash
python3 scripts/live_mcp_eval.py --binary target/release/grepika
```

Run a dry run of the Codex LLM suite:

```bash
python3 scripts/codex_llm_mcp_eval.py --dry-run --cases search_grep_mode
```

Run one real Codex trial:

```bash
python3 scripts/codex_llm_mcp_eval.py \
  --trials 1 \
  --cases toc_src_tools_no_reads
```

Run a more reliable behavior check:

```bash
python3 scripts/codex_llm_mcp_eval.py --trials 3
```

By default, the runner uses `CODEX_BIN` when set, then the bundled Codex desktop
binary at `/Applications/Codex.app/Contents/Resources/codex` when present, then
falls back to `codex` on `PATH`. Pass `--codex-bin` to force a specific CLI and
`--model` only when you need to override the CLI's default model.

The runner auto-passes `--ephemeral` when the selected Codex CLI supports it.
The Codex CLI still writes its own session files, so restricted environments may
require running the command with the same permissions normally needed for
`codex exec`.

## Current limitation

grepika currently has regex search (`mode=grep`), FTS natural-language search
(`mode=fts`), and combined indexed search (`mode=combined`). It does not
currently implement vector embedding search. In the evaluation suite,
"semantic" behavior means natural-language FTS/combined search unless a vector
backend is added later.
