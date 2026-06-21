#!/usr/bin/env python3
"""Run real Codex CLI turns against grepika's live MCP server.

This is intentionally separate from live_mcp_eval.py. The deterministic harness
chooses exact MCP calls itself; this harness gives Codex a natural-language task
and grades whether Codex chooses grepika tools well.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import time
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any


Json = dict[str, Any]

KNOWN_GREPICA_TOOLS = {
    "add_workspace",
    "search",
    "get",
    "outline",
    "toc",
    "context",
    "stats",
    "refs",
    "index",
    "diff",
    "graph",
}

SHELL_MARKERS = {
    "exec",
    "exec_command",
    "shell",
    "bash",
    "zsh",
    "terminal",
    "run_command",
}


@dataclass(frozen=True)
class EvalCase:
    case_id: str
    prompt: str
    expected_answer_terms: tuple[str, ...]
    required_tools: tuple[str, ...]
    forbidden_tools: tuple[str, ...] = ()
    required_modes: tuple[str, ...] = ()
    max_tool_calls: int = 4
    expected_behavior: str = ""


@dataclass
class TrialResult:
    case_id: str
    trial: int
    ok: bool
    score: int
    answer_score: int
    sequence_score: int
    specificity_score: int
    waste_score: int
    footprint_score: int
    final_answer: str
    observed_tools: list[str]
    observed_modes: list[str]
    non_grepika_tools: list[str]
    shell_markers: list[str]
    transcript_path: str
    final_path: str
    db_path: str
    elapsed_ms: int
    stdout_bytes: int
    final_bytes: int
    returncode: int
    errors: list[str] = field(default_factory=list)


REPO_ROOT = Path(__file__).resolve().parents[1]


def line_containing(path: str, needle: str) -> str:
    for number, line in enumerate((REPO_ROOT / path).read_text(encoding="utf-8").splitlines(), 1):
        if needle in line:
            return str(number)
    raise RuntimeError(f"{needle!r} not found in {path}")


CASES: tuple[EvalCase, ...] = (
    EvalCase(
        case_id="toc_src_tools_no_reads",
        prompt=(
            "List the Rust source filenames directly under src/tools. "
            "Do not read file contents. Include the file count and directory count."
        ),
        expected_answer_terms=(
            "analysis.rs",
            "content.rs",
            "graph.rs",
            "index.rs",
            "mod.rs",
            "search.rs",
            "6",
            "0",
        ),
        required_tools=("add_workspace", "toc"),
        forbidden_tools=("index", "search", "get", "context", "refs", "outline"),
        max_tool_calls=2,
        expected_behavior="add_workspace, then toc(path=src/tools, depth=1).",
    ),
    EvalCase(
        case_id="toc_src_services_footprint",
        prompt=(
            "Using the smallest grepika output, list Rust source filenames directly "
            "under src/services. Do not search or read file contents. Include the "
            "file count and directory count."
        ),
        expected_answer_terms=(
            "fts.rs",
            "git_diff.rs",
            "grep.rs",
            "indexer.rs",
            "mod.rs",
            "ngram.rs",
            "regex_literals.rs",
            "search.rs",
            "semantic.rs",
            "trigram.rs",
            "11",
            "0",
        ),
        required_tools=("add_workspace", "toc"),
        forbidden_tools=("index", "search", "get", "context", "refs", "outline"),
        max_tool_calls=2,
        expected_behavior="add_workspace, then toc(path=src/services, depth=1).",
    ),
    EvalCase(
        case_id="outline_searchmode",
        prompt=(
            "What symbol kind and start/end lines does the file outline report for "
            "SearchMode in src/tools/search.rs? Do not read file contents."
        ),
        expected_answer_terms=("SearchMode", "enum", "33", "41"),
        required_tools=("add_workspace", "outline"),
        forbidden_tools=("index", "search", "get", "context", "refs", "toc"),
        max_tool_calls=2,
        expected_behavior="add_workspace, then outline(path=src/tools/search.rs).",
    ),
    EvalCase(
        case_id="outline_toolrouter_order",
        prompt=(
            "Using only the file outline, list the GrepikaServer MCP tool handler "
            "names in the #[tool_router] impl, in order. Do not use get or context."
        ),
        expected_answer_terms=(
            "add_workspace",
            "search",
            "get",
            "outline",
            "toc",
            "context",
            "stats",
            "refs",
            "index",
            "diff",
            "graph",
        ),
        required_tools=("add_workspace", "outline"),
        forbidden_tools=("index", "search", "get", "context", "refs", "toc"),
        max_tool_calls=2,
        expected_behavior="add_workspace, then outline(path=src/server.rs).",
    ),
    EvalCase(
        case_id="refs_exact_symbol",
        prompt=(
            "Using the refs output only, find exact Rust source references to "
            "search_grep_with_matches and classify them. Do not call context. "
            "Return only the definition and the Rust source usage with file paths "
            "and lines."
        ),
        expected_answer_terms=(
            "src/services/search.rs",
            "definition",
            "src/tools/analysis.rs",
            "usage",
        ),
        required_tools=("add_workspace", "refs"),
        forbidden_tools=("index", "search", "get", "context", "toc", "outline"),
        max_tool_calls=2,
        expected_behavior="add_workspace, then refs(symbol=search_grep_with_matches).",
    ),
    EvalCase(
        case_id="context_center_marker",
        prompt=(
            "At src/tools/content.rs line 309, what line does context mark as the "
            "center when using 3 surrounding context lines? Include the returned "
            "start/end range and the exact center line text."
        ),
        expected_answer_terms=("306", "312", "309", "Format with line numbers"),
        required_tools=("add_workspace", "context"),
        forbidden_tools=("index", "search", "get", "refs", "toc", "outline"),
        max_tool_calls=2,
        expected_behavior=(
            "add_workspace, then context(path=src/tools/content.rs, line=309, "
            "context_lines=3)."
        ),
    ),
    EvalCase(
        case_id="get_global_mode_branch",
        prompt=(
            "Read src/main.rs lines 176-187. Which MCP branch handles global mode "
            "and what function call is made?"
        ),
        expected_answer_terms=(
            "None",
            "global mode",
            "run_mcp_server_global",
        ),
        required_tools=("add_workspace", "get"),
        forbidden_tools=("index", "search", "context", "refs", "toc", "outline"),
        max_tool_calls=2,
        expected_behavior="add_workspace, then get(path=src/main.rs, 176-187).",
    ),
    EvalCase(
        case_id="search_grep_mode",
        prompt=(
            "In src/server.rs, find the exact ServerHandler::get_info instruction "
            "line containing `Use mode=grep for regex and mode=fts for natural "
            "language`. Return path:line and the complete line text."
        ),
        expected_answer_terms=(
            "src/server.rs",
            line_containing("src/server.rs", "Use mode=grep for regex"),
            "Use mode=grep",
            "mode=fts",
            "untrusted data",
        ),
        required_tools=("add_workspace", "index", "search"),
        required_modes=("grep",),
        max_tool_calls=5,
        expected_behavior=(
            "add_workspace, index, search(mode=grep), then context or targeted get "
            "for proof."
        ),
    ),
    EvalCase(
        case_id="search_fts_natural_language",
        prompt=(
            "Using natural-language indexed search, find the README sentence "
            "explaining default global mode and add_workspace. Return path:line "
            "and the sentence."
        ),
        expected_answer_terms=(
            "README.md",
            "70",
            "global mode",
            "without `--root`",
            "add_workspace",
        ),
        required_tools=("add_workspace", "index", "search"),
        required_modes=("fts",),
        max_tool_calls=5,
        expected_behavior=(
            "add_workspace, index, search(mode=fts), then targeted get/context proof."
        ),
    ),
    EvalCase(
        case_id="get_searchmode_variants",
        prompt=(
            "Read src/tools/search.rs lines 31-42, the SearchMode enum declaration, "
            "and list variants with documented purpose; include the default variant."
        ),
        expected_answer_terms=(
            "Combined",
            "default",
            "weighted score",
            "Fts",
            "natural language",
            "Grep",
            "regex",
        ),
        required_tools=("add_workspace", "get"),
        forbidden_tools=("index", "search", "context", "refs", "toc", "outline"),
        max_tool_calls=2,
        expected_behavior="add_workspace, then get(path=src/tools/search.rs, 31-42).",
    ),
)


def case_by_id() -> dict[str, EvalCase]:
    return {case.case_id: case for case in CASES}


def default_codex_bin() -> str:
    env = os.environ.get("CODEX_BIN")
    if env:
        return env

    app_bundle = Path("/Applications/Codex.app/Contents/Resources/codex")
    if app_bundle.exists() and os.access(app_bundle, os.X_OK):
        return str(app_bundle)

    return "codex"


def q(s: str) -> str:
    return json.dumps(s)


def build_prompt(case: EvalCase, root: Path) -> str:
    return f"""You are evaluating the grepika MCP server.

Rules:
- Use only the MCP server named grepika.
- Do not use shell commands, direct filesystem reads, or non-grepika MCP servers.
- If the grepika server starts in global mode, call add_workspace with this exact path first: {root}
- If a task needs search, call index once before the first search call.
- Do not call search before index in a fresh workspace.
- Use refs for exact symbol references.
- Use toc or outline for structure questions instead of reading full files.
- Use search mode=grep for regex/exact-line search tasks.
- Use search mode=fts for natural-language search tasks.
- Keep the final answer concise and include only the requested answer.

Task:
{case.prompt}
"""


def codex_command(
    *,
    codex_bin: str,
    model: str | None,
    reasoning_effort: str,
    root: Path,
    grepika_binary: Path,
    db_path: Path,
    final_path: Path,
    prompt: str,
    ephemeral: bool,
) -> list[str]:
    cmd = [
        codex_bin,
        "exec",
        "--json",
        "--sandbox",
        "read-only",
        "-C",
        str(root),
        "-c",
        f"model_reasoning_effort={q(reasoning_effort)}",
        "-c",
        f"mcp_servers.grepika.command={q(str(grepika_binary))}",
        "-c",
        "mcp_servers.grepika.args="
        + json.dumps(["--mcp", "--db", str(db_path)], separators=(",", ":")),
        "-c",
        "mcp_servers.grepika.startup_timeout_ms=10000",
        "--output-last-message",
        str(final_path),
    ]
    if ephemeral:
        cmd.append("--ephemeral")
    if model:
        cmd.extend(["-m", model])
    cmd.append(prompt)
    return cmd


def codex_exec_help(codex_bin: str) -> str:
    try:
        proc = subprocess.run(
            [codex_bin, "exec", "--help"],
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            timeout=10,
            check=False,
        )
    except (OSError, subprocess.TimeoutExpired):
        return ""
    return proc.stdout


def supports_ephemeral(codex_bin: str) -> bool:
    return "--ephemeral" in codex_exec_help(codex_bin)


def parse_jsonl(path: Path) -> list[Json]:
    events: list[Json] = []
    if not path.exists():
        return events
    with path.open("r", encoding="utf-8", errors="replace") as f:
        for line in f:
            line = line.strip()
            if not line:
                continue
            try:
                events.append(json.loads(line))
            except json.JSONDecodeError:
                events.append({"raw": line})
    return events


def iter_key_values(obj: Any) -> Any:
    if isinstance(obj, dict):
        for key, value in obj.items():
            yield key, value
            yield from iter_key_values(value)
    elif isinstance(obj, list):
        for value in obj:
            yield from iter_key_values(value)


def normalize_tool_name(value: str) -> str | None:
    low = value.lower()
    for tool in KNOWN_GREPICA_TOOLS:
        patterns = (
            tool,
            f"grepika.{tool}",
            f"grepika/{tool}",
            f"mcp__grepika__{tool}",
            f"mcp.grepika.{tool}",
        )
        if low in patterns or any(pattern in low for pattern in patterns[1:]):
            return tool
    return None


def extract_observations(events: list[Json], transcript_text: str) -> tuple[list[str], list[str], list[str], list[str], list[str]]:
    tools: list[str] = []
    modes: list[str] = []
    non_grepika: list[str] = []
    shell: list[str] = []
    errors: list[str] = []

    for event in events:
        if "prompt" in event:
            continue
        msg = event.get("msg", event)
        msg_type = str(msg.get("type", "")).lower() if isinstance(msg, dict) else ""
        item = msg.get("item") if isinstance(msg, dict) and isinstance(msg.get("item"), dict) else None
        item_type = str(item.get("type", "")).lower() if item else ""

        if "error" in msg_type:
            message = msg.get("message") if isinstance(msg, dict) else None
            errors.append(str(message or msg))

        if msg_type.startswith("exec_command") or item_type.startswith("exec_command"):
            shell.append(msg_type)

        if any(marker in msg_type for marker in SHELL_MARKERS):
            shell.append(msg_type)

        if item and item_type == "mcp_tool_call":
            server = str(item.get("server", "")).lower()
            if server and server != "grepika":
                non_grepika.append(server)
            if msg_type in {"item.started", "mcp_tool_call_begin"}:
                tool = normalize_tool_name(str(item.get("tool", "") or item.get("name", "")))
                if tool:
                    tools.append(tool)
                arguments = item.get("arguments")
                if isinstance(arguments, dict):
                    mode = arguments.get("mode")
                    if isinstance(mode, str) and mode.lower() in {"grep", "fts", "combined"}:
                        modes.append(mode.lower())
            continue

        if msg_type.startswith("mcp_tool_call"):
            server = str(msg.get("server", "")).lower() if isinstance(msg, dict) else ""
            if server and server != "grepika":
                non_grepika.append(server)
            if msg_type.endswith("begin") or msg_type.endswith("started"):
                tool = normalize_tool_name(str(msg.get("tool", "") or msg.get("name", "")))
                if tool:
                    tools.append(tool)
                arguments = msg.get("arguments") if isinstance(msg, dict) else None
                if isinstance(arguments, dict):
                    mode = arguments.get("mode")
                    if isinstance(mode, str) and mode.lower() in {"grep", "fts", "combined"}:
                        modes.append(mode.lower())
            continue

        for key, value in iter_key_values(msg):
            key_s = str(key).lower()
            if isinstance(value, str):
                value_s = value.strip()
                tool = normalize_tool_name(value_s)
                if tool and key_s in {
                    "name",
                    "tool",
                    "tool_name",
                    "function",
                    "command",
                    "recipient",
                    "server_tool",
                }:
                    tools.append(tool)
                if value_s.startswith("mcp__") and "grepika" not in value_s:
                    non_grepika.append(value_s)
                if key_s == "mode" and value_s.lower() in {"grep", "fts", "combined"}:
                    modes.append(value_s.lower())
                if key_s in SHELL_MARKERS or value_s.lower() in SHELL_MARKERS:
                    shell.append(value_s)
            elif key_s == "mode" and str(value).lower() in {"grep", "fts", "combined"}:
                modes.append(str(value).lower())

    # Fallback regexes for Codex JSONL formats that embed tool calls as text.
    if not tools:
        for match in re.finditer(r"mcp__grepika__(add_workspace|search|get|outline|toc|context|stats|refs|index|diff|graph)", transcript_text):
            tools.append(match.group(1))
        for match in re.finditer(r'"(?:tool|tool_name|name)"\s*:\s*"(add_workspace|search|get|outline|toc|context|stats|refs|index|diff|graph)"', transcript_text):
            tools.append(match.group(1))
    if not modes:
        for match in re.finditer(r'"mode"\s*:\s*"(grep|fts|combined)"', transcript_text):
            modes.append(match.group(1))
    for marker in SHELL_MARKERS:
        if f'"type":"{marker}' in transcript_text or f'"tool_name":"{marker}"' in transcript_text:
            shell.append(marker)

    return tools, modes, dedupe(non_grepika), dedupe(shell), errors


def dedupe(items: list[str]) -> list[str]:
    seen: set[str] = set()
    out: list[str] = []
    for item in items:
        if item not in seen:
            seen.add(item)
            out.append(item)
    return out


def contains_all(answer: str, terms: tuple[str, ...]) -> int:
    if not terms:
        return 40
    low = answer.lower()
    hits = sum(1 for term in terms if term.lower() in low)
    return round(40 * hits / len(terms))


def ordered_subset(required: tuple[str, ...], observed: list[str]) -> bool:
    if not required:
        return True
    pos = 0
    for tool in observed:
        if tool == required[pos]:
            pos += 1
            if pos == len(required):
                return True
    return False


def grade_trial(
    case: EvalCase,
    trial: int,
    final_answer: str,
    events: list[Json],
    transcript_text: str,
    transcript_path: Path,
    final_path: Path,
    db_path: Path,
    elapsed_ms: int,
    returncode: int,
) -> TrialResult:
    observed_tools, modes, non_grepika, shell, event_errors = extract_observations(
        events, transcript_text
    )

    answer_score = contains_all(final_answer, case.expected_answer_terms)

    sequence_score = 0
    if case.required_tools:
        observed_set = set(observed_tools)
        required_present = sum(1 for tool in case.required_tools if tool in observed_set)
        sequence_score += round(15 * required_present / len(case.required_tools))
        if ordered_subset(case.required_tools, observed_tools):
            sequence_score += 10
    else:
        sequence_score = 25

    specificity_score = 15
    missing_modes = [mode for mode in case.required_modes if mode not in modes]
    if missing_modes:
        specificity_score -= round(15 * len(missing_modes) / len(case.required_modes))
    if "refs" in case.required_tools and "search" in observed_tools:
        specificity_score = max(0, specificity_score - 8)
    if "toc" in case.required_tools and ("get" in observed_tools or "search" in observed_tools):
        specificity_score = max(0, specificity_score - 8)
    if "outline" in case.required_tools and "get" in observed_tools:
        specificity_score = max(0, specificity_score - 8)

    waste_score = 10
    forbidden_seen = [tool for tool in case.forbidden_tools if tool in observed_tools]
    if forbidden_seen:
        waste_score -= min(8, len(forbidden_seen) * 3)
    if shell:
        waste_score = 0
    if non_grepika:
        waste_score = 0
    waste_score = max(0, waste_score)

    footprint_score = 10
    if len(observed_tools) > case.max_tool_calls:
        footprint_score -= min(8, (len(observed_tools) - case.max_tool_calls) * 2)
    if len(final_answer.encode("utf-8")) > 1500:
        footprint_score -= 2
    footprint_score = max(0, footprint_score)

    errors = list(event_errors)
    if returncode != 0:
        errors.append(f"codex exited with status {returncode}")
    if "index" in case.required_tools and "search" in case.required_tools:
        if "search" in observed_tools and "index" in observed_tools:
            if observed_tools.index("search") < observed_tools.index("index"):
                sequence_score = max(0, sequence_score - 10)
                errors.append("search ran before index")
    for mode in missing_modes:
        errors.append(f"missing required search mode: {mode}")
    for tool in case.required_tools:
        if tool not in observed_tools:
            errors.append(f"missing required tool: {tool}")
    for tool in forbidden_seen:
        errors.append(f"forbidden tool observed: {tool}")
    if shell:
        errors.append(f"shell/direct execution observed: {', '.join(shell)}")
    if non_grepika:
        errors.append(f"non-grepika tools observed: {', '.join(non_grepika)}")

    score = answer_score + sequence_score + specificity_score + waste_score + footprint_score
    ok = (
        score >= 85
        and answer_score >= 34
        and returncode == 0
        and not errors
        and not shell
        and not non_grepika
    )

    return TrialResult(
        case_id=case.case_id,
        trial=trial,
        ok=ok,
        score=score,
        answer_score=answer_score,
        sequence_score=sequence_score,
        specificity_score=specificity_score,
        waste_score=waste_score,
        footprint_score=footprint_score,
        final_answer=final_answer,
        observed_tools=observed_tools,
        observed_modes=modes,
        non_grepika_tools=non_grepika,
        shell_markers=shell,
        transcript_path=str(transcript_path),
        final_path=str(final_path),
        db_path=str(db_path),
        elapsed_ms=elapsed_ms,
        stdout_bytes=len(transcript_text.encode("utf-8")),
        final_bytes=len(final_answer.encode("utf-8")),
        returncode=returncode,
        errors=errors,
    )


def run_trial(
    *,
    args: argparse.Namespace,
    case: EvalCase,
    trial: int,
    output_dir: Path,
) -> TrialResult:
    case_dir = output_dir / case.case_id / f"trial-{trial}"
    case_dir.mkdir(parents=True, exist_ok=True)
    db_path = case_dir / "grepika.db"
    final_path = case_dir / "final.txt"
    transcript_path = case_dir / "codex.jsonl"
    prompt_path = case_dir / "prompt.txt"
    prompt = build_prompt(case, args.root)
    prompt_path.write_text(prompt, encoding="utf-8")

    cmd = codex_command(
        codex_bin=args.codex_bin,
        model=args.model,
        reasoning_effort=args.reasoning_effort,
        root=args.root,
        grepika_binary=args.binary,
        db_path=db_path,
        final_path=final_path,
        prompt=prompt,
        ephemeral=args.ephemeral,
    )

    if args.dry_run:
        transcript_path.write_text(
            json.dumps({"dry_run_command": cmd, "prompt": prompt}, indent=2),
            encoding="utf-8",
        )
        return TrialResult(
            case_id=case.case_id,
            trial=trial,
            ok=True,
            score=0,
            answer_score=0,
            sequence_score=0,
            specificity_score=0,
            waste_score=0,
            footprint_score=0,
            final_answer="",
            observed_tools=[],
            observed_modes=[],
            non_grepika_tools=[],
            shell_markers=[],
            transcript_path=str(transcript_path),
            final_path=str(final_path),
            db_path=str(db_path),
            elapsed_ms=0,
            stdout_bytes=0,
            final_bytes=0,
            returncode=0,
            errors=[],
        )

    started = time.perf_counter()
    timed_out = False
    with transcript_path.open("w", encoding="utf-8", errors="replace") as transcript:
        proc = subprocess.Popen(
            cmd,
            cwd=str(args.root),
            text=True,
            stdout=transcript,
            stderr=subprocess.STDOUT,
        )
        try:
            returncode = proc.wait(timeout=args.timeout)
        except subprocess.TimeoutExpired:
            timed_out = True
            proc.kill()
            returncode = proc.wait(timeout=10)
            transcript.write(f"\n{{\"type\":\"eval_timeout\",\"seconds\":{args.timeout}}}\n")
    elapsed_ms = round((time.perf_counter() - started) * 1000)
    final_answer = (
        final_path.read_text(encoding="utf-8", errors="replace") if final_path.exists() else ""
    )
    transcript_text = transcript_path.read_text(encoding="utf-8", errors="replace")
    events = parse_jsonl(transcript_path)

    result = grade_trial(
        case,
        trial,
        final_answer,
        events,
        transcript_text,
        transcript_path,
        final_path,
        db_path,
        elapsed_ms,
        returncode,
    )
    if timed_out:
        result.errors.append(f"timeout after {args.timeout}s")
        result.ok = False
        result.returncode = 124
    return result


def summarize(results: list[TrialResult]) -> Json:
    total = len(results)
    passed = sum(1 for result in results if result.ok)
    by_case: dict[str, Json] = {}
    for case_id in sorted({result.case_id for result in results}):
        case_results = [result for result in results if result.case_id == case_id]
        by_case[case_id] = {
            "trials": len(case_results),
            "passed": sum(1 for result in case_results if result.ok),
            "avg_score": round(sum(result.score for result in case_results) / len(case_results), 1),
            "required_trace_passed": sum(
                1 for result in case_results if not any(e.startswith("missing required") for e in result.errors)
            ),
        }
    return {
        "total_trials": total,
        "passed": passed,
        "pass_rate": round(passed / total, 3) if total else 0.0,
        "avg_score": round(sum(result.score for result in results) / total, 1) if total else 0.0,
        "by_case": by_case,
    }


def write_reports(output_dir: Path, results: list[TrialResult], metadata: Json) -> None:
    report = {
        "metadata": metadata,
        "summary": summarize(results),
        "trials": [result.__dict__ for result in results],
    }
    (output_dir / "report.json").write_text(
        json.dumps(report, indent=2, sort_keys=True),
        encoding="utf-8",
    )

    lines = [
        "# Codex LLM MCP Eval Report",
        "",
        f"Total trials: {report['summary']['total_trials']}",
        f"Passed: {report['summary']['passed']}",
        f"Pass rate: {report['summary']['pass_rate']}",
        f"Average score: {report['summary']['avg_score']}",
        "",
        "## Cases",
        "",
    ]
    for case_id, case_summary in report["summary"]["by_case"].items():
        lines.append(
            f"- `{case_id}`: {case_summary['passed']}/{case_summary['trials']} "
            f"passed, avg score {case_summary['avg_score']}"
        )
    failures = [result for result in results if not result.ok]
    if failures:
        lines.extend(["", "## Failures", ""])
        for result in failures:
            lines.append(
                f"- `{result.case_id}` trial {result.trial}: score {result.score}; "
                f"errors: {', '.join(result.errors) or 'none'}"
            )
            lines.append(f"  transcript: `{result.transcript_path}`")
    (output_dir / "report.md").write_text("\n".join(lines) + "\n", encoding="utf-8")


def selected_cases(names: list[str] | None) -> list[EvalCase]:
    if not names:
        return list(CASES)
    lookup = case_by_id()
    missing = [name for name in names if name not in lookup]
    if missing:
        raise SystemExit(f"unknown case(s): {', '.join(missing)}")
    return [lookup[name] for name in names]


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--codex-bin", default=default_codex_bin())
    parser.add_argument("--model", default=os.environ.get("CODEX_EVAL_MODEL"))
    parser.add_argument("--reasoning-effort", default="high")
    parser.add_argument("--binary", type=Path, default=Path("target/release/grepika"))
    parser.add_argument("--root", type=Path, default=Path.cwd())
    parser.add_argument("--output-dir", type=Path, default=Path("target/codex-llm-mcp-eval"))
    parser.add_argument("--trials", type=int, default=1)
    parser.add_argument("--timeout", type=int, default=180)
    parser.add_argument("--cases", nargs="*")
    parser.add_argument("--build", action="store_true", help="Run cargo build --release first.")
    parser.add_argument("--dry-run", action="store_true", help="Write prompts/commands without running Codex.")
    parser.add_argument(
        "--no-ephemeral",
        action="store_true",
        help="Do not pass --ephemeral to Codex, even if the selected Codex binary supports it.",
    )
    args = parser.parse_args()

    args.root = args.root.resolve()
    if not args.binary.is_absolute():
        args.binary = (args.root / args.binary).resolve()
    args.output_dir = args.output_dir.resolve()
    args.output_dir.mkdir(parents=True, exist_ok=True)

    if args.trials < 1:
        raise SystemExit("--trials must be >= 1")
    if not shutil.which(args.codex_bin):
        raise SystemExit(f"codex binary not found: {args.codex_bin}")
    args.ephemeral = supports_ephemeral(args.codex_bin) and not args.no_ephemeral

    if args.build:
        subprocess.run(["cargo", "build", "--release"], cwd=str(args.root), check=True)

    if not args.binary.exists():
        raise SystemExit(f"grepika binary not found: {args.binary}")

    cases = selected_cases(args.cases)
    metadata = {
        "root": str(args.root),
        "grepika_binary": str(args.binary),
        "codex_bin": args.codex_bin,
        "model": args.model,
        "reasoning_effort": args.reasoning_effort,
        "trials_per_case": args.trials,
        "dry_run": args.dry_run,
        "ephemeral": args.ephemeral,
        "case_count": len(cases),
    }

    results: list[TrialResult] = []
    for case in cases:
        for trial in range(1, args.trials + 1):
            print(f"running {case.case_id} trial {trial}/{args.trials}", file=sys.stderr)
            results.append(run_trial(args=args, case=case, trial=trial, output_dir=args.output_dir))

    write_reports(args.output_dir, results, metadata)
    summary = summarize(results)
    print(json.dumps({"summary": summary, "report_dir": str(args.output_dir)}, indent=2))
    return 0 if summary["passed"] == summary["total_trials"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
