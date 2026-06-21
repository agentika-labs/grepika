#!/usr/bin/env python3
"""Deterministic natural-language search quality and latency eval.

This fixture intentionally lives outside the grepika repo so prompt/docs text
cannot contaminate ranking. It checks whether natural-language searches return
the expected implementation file near the top while keeping combined-search
latency close to FTS.
"""

from __future__ import annotations

import argparse
import json
import math
import sqlite3
import statistics
import subprocess
import tempfile
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Optional


Json = dict[str, Any]


class EvalFailure(AssertionError):
    pass


@dataclass(frozen=True)
class Case:
    case_id: str
    query: str
    expected_path: str
    must_improve_fts: bool = False


CASES = [
    Case("auth_reset", "issue password reset workflow", "src/auth.rs"),
    Case(
        "auth_reset_adversarial",
        "password reset token expiry email verification",
        "src/auth.rs",
        True,
    ),
    Case("billing_ledger", "invoice ledger reconciliation payment status", "src/billing.rs"),
    Case("graph_symbols", "syntax tree symbol graph call edges", "src/graph.rs"),
    Case("workspace_global", "global mode add workspace startup root", "docs/workspace.md"),
    Case("search_ranking", "ranked code search fts trigram grep backend", "src/search.rs"),
    Case("queue_retry", "rate limiter backoff retry queue worker", "src/queue.rs"),
]


def run_json(binary: Path, root: Path, db: Path, args: list[str]) -> tuple[Json, float, int]:
    cmd = [str(binary), "--root", str(root), "--db", str(db), "--json", *args]
    start = time.perf_counter()
    proc = subprocess.run(cmd, text=True, capture_output=True, check=False)
    elapsed_ms = (time.perf_counter() - start) * 1000.0
    if proc.returncode != 0:
        raise EvalFailure(
            f"{' '.join(cmd)} exited {proc.returncode}\nstdout={proc.stdout}\nstderr={proc.stderr}"
        )
    try:
        payload = json.loads(proc.stdout)
    except json.JSONDecodeError as exc:
        raise EvalFailure(f"invalid JSON from {' '.join(cmd)}: {proc.stdout!r}") from exc
    return payload, elapsed_ms, len(proc.stdout.encode("utf-8"))


def assert_true(condition: bool, message: str) -> None:
    if not condition:
        raise EvalFailure(message)


def write_fixture(root: Path) -> None:
    files = {
        "src/auth.rs": """
            /// Password reset token expiry and email verification workflow.
            pub fn issue_password_reset_token() {
                let expiry_window = "password reset token expiry";
                let full_flow = "password reset token expiry email verification";
                send_email_verification(expiry_window);
                audit(full_flow);
            }
        """,
        "src/billing.rs": """
            /// Invoice ledger reconciliation and payment status handling.
            pub fn reconcile_invoice_ledger() {
                let payment_status = "invoice ledger reconciliation payment status";
                persist_status(payment_status);
            }
        """,
        "src/graph.rs": """
            /// Syntax tree symbol graph with call edges.
            pub fn extract_symbol_graph() {
                let call_edges = "syntax tree symbol graph call edges";
                record_edges(call_edges);
            }
        """,
        "src/search.rs": """
            /// Ranked code search combines fts trigram grep backend signals.
            pub fn ranked_code_search() {
                let backend = "ranked code search fts trigram grep backend";
                merge_backend_scores(backend);
            }
        """,
        "src/queue.rs": """
            /// Rate limiter backoff retry queue worker loop.
            pub fn retry_queue_worker() {
                let retry_policy = "rate limiter backoff retry queue worker";
                apply_backoff(retry_policy);
            }
        """,
        "docs/workspace.md": """
            # Workspace startup

            Global mode starts without a root. The caller uses add_workspace
            during startup to attach the project root.
        """,
        "src/noise.rs": """
            pub fn unrelated() {
                let words = "token payment graph search queue workspace";
                println!("{}", words);
            }
        """,
        "src/password_reset_token_expiry_email_verification.rs": """
            pub fn scattered_password_terms() {
                let terms = [
                    "password password password password",
                    "reset reset reset reset",
                    "token token token token",
                    "expiry expiry expiry",
                    "email email email",
                    "verification verification verification",
                ];
                println!("{:?}", terms);
            }
        """,
    }
    for rel, content in files.items():
        path = root / rel
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(content.strip() + "\n", encoding="utf-8")


def write_prefilter_fixture(root: Path) -> None:
    src = root / "src"
    src.mkdir(parents=True, exist_ok=True)
    for i in range(700):
        if i == 17:
            content = "pub fn direct_hit() { let unique_direct_prefilter_token = true; }\n"
        elif i < 317:
            content = f"pub fn medium_hit_{i}() {{ let medium_selective_prefilter_token = {i}; }}\n"
        else:
            content = f"pub fn distractor_{i}() {{ let common_prefilter_noise = {i}; }}\n"
        (src / f"file_{i}.rs").write_text(content, encoding="utf-8")


def write_regex_prefilter_fixture(root: Path) -> None:
    src = root / "src"
    src.mkdir(parents=True, exist_ok=True)
    special_files = {
        17: "pub fn password_only() { let password = true; }\n",
        23: "pub fn password_reset() { let passwordreset = true; }\n",
        31: "pub fn zero_repeat() { let without_zero_marker = true; }\n",
        43: 'pub fn alt_foo() { let value = "prefix_foo_suffix"; }\n',
        47: 'pub fn alt_bar() { let value = "prefix_bar_suffix"; }\n',
    }
    for i in range(700):
        content = special_files.get(
            i,
            f"pub fn regex_distractor_{i}() {{ let common_regex_noise = {i}; }}\n",
        )
        (src / f"file_{i}.rs").write_text(content, encoding="utf-8")


def ranked_paths(payload: Json) -> list[str]:
    return [item["p"] for item in payload.get("r", [])]


def rank_of(paths: list[str], expected: str) -> Optional[int]:
    try:
        return paths.index(expected) + 1
    except ValueError:
        return None


def dcg(rank: Optional[int]) -> float:
    if rank is None:
        return 0.0
    return 1.0 / math.log2(rank + 1)


def percentile(values: list[float], pct: float) -> float:
    if not values:
        return 0.0
    sorted_values = sorted(values)
    idx = min(len(sorted_values) - 1, math.ceil((pct / 100.0) * len(sorted_values)) - 1)
    return sorted_values[idx]


def run_eval(binary: Path, runs: int) -> Json:
    with tempfile.TemporaryDirectory(prefix="grepika-natural-eval-") as tmp:
        root = Path(tmp) / "repo"
        root.mkdir()
        write_fixture(root)
        db = Path(tmp) / "grepika.db"

        index_payload, index_ms, _ = run_json(binary, root, db, ["index"])
        assert_true(index_payload["indexed"] == 8, f"unexpected index output: {index_payload}")

        ranks: list[Optional[int]] = []
        fts_ranks: list[Optional[int]] = []
        combined_latencies: list[float] = []
        fts_latencies: list[float] = []
        output_bytes: list[int] = []
        grep_source_count = 0
        result_count = 0
        diagnostics: list[Json] = []

        for _ in range(runs):
            for case in CASES:
                combined, combined_ms, combined_bytes = run_json(
                    binary,
                    root,
                    db,
                    ["search", case.query, "--limit", "5", "--mode", "combined"],
                )
                fts, fts_ms, _ = run_json(
                    binary,
                    root,
                    db,
                    ["search", case.query, "--limit", "5", "--mode", "fts"],
                )

                paths = ranked_paths(combined)
                fts_paths = ranked_paths(fts)
                rank = rank_of(paths, case.expected_path)
                fts_rank = rank_of(fts_paths, case.expected_path)
                ranks.append(rank)
                fts_ranks.append(fts_rank)
                combined_latencies.append(combined_ms)
                fts_latencies.append(fts_ms)
                output_bytes.append(combined_bytes)
                result_count += len(combined.get("r", []))
                grep_source_count += sum(
                    1 for item in combined.get("r", []) if "g" in item.get("src", "")
                )
                diagnostics.append(
                    {
                        "case": case.case_id,
                        "expected": case.expected_path,
                        "combined_top": paths[:3],
                        "combined_rank": rank,
                        "fts_top": fts_paths[:3],
                        "fts_rank": fts_rank,
                    }
                )

        case_runs = len(ranks)
        top1 = sum(1 for rank in ranks if rank == 1) / case_runs
        recall5 = sum(1 for rank in ranks if rank is not None and rank <= 5) / case_runs
        mrr5 = sum(0.0 if rank is None or rank > 5 else 1.0 / rank for rank in ranks) / case_runs
        ndcg5 = sum(dcg(rank if rank is not None and rank <= 5 else None) for rank in ranks) / case_runs
        combined_p50 = statistics.median(combined_latencies)
        fts_p50 = statistics.median(fts_latencies)
        combined_p95 = percentile(combined_latencies, 95)
        fts_p95 = percentile(fts_latencies, 95)
        latency_ratio = combined_p50 / max(fts_p50, 0.001)
        grep_source_rate = grep_source_count / max(result_count, 1)
        rank_regressions = [
            diag
            for diag in diagnostics
            if diag["combined_rank"] is None
            or (
                diag["fts_rank"] is not None
                and diag["combined_rank"] > diag["fts_rank"]
            )
        ]
        strict_improvements = sum(
            1
            for diag in diagnostics
            if diag["combined_rank"] is not None
            and diag["fts_rank"] is not None
            and diag["combined_rank"] < diag["fts_rank"]
        )
        adversarial_failures = [
            diag
            for diag in diagnostics
            if diag["case"] == "auth_reset_adversarial"
            and not (
                diag["combined_rank"] is not None
                and diag["fts_rank"] is not None
                and diag["combined_rank"] < diag["fts_rank"]
            )
        ]

        report = {
            "status": "pass",
            "runs": runs,
            "index_ms": round(index_ms, 3),
            "quality": {
                "case_count": len(CASES),
                "case_runs": case_runs,
                "top1_accuracy": round(top1, 4),
                "recall_at_5": round(recall5, 4),
                "mrr_at_5": round(mrr5, 4),
                "ndcg_at_5": round(ndcg5, 4),
                "combined_rank_regressions": len(rank_regressions),
                "combined_strict_improvements": strict_improvements,
                "adversarial_failures": len(adversarial_failures),
            },
            "latency_ms": {
                "combined_p50": round(combined_p50, 3),
                "combined_p95": round(combined_p95, 3),
                "fts_p50": round(fts_p50, 3),
                "fts_p95": round(fts_p95, 3),
                "combined_vs_fts_p50_ratio": round(latency_ratio, 3),
            },
            "diagnostics": {
                "combined_grep_source_rate": round(grep_source_rate, 4),
                "avg_output_bytes": round(statistics.fmean(output_bytes), 1),
                "cases": diagnostics[: len(CASES)],
            },
        }

        assert_true(top1 == 1.0, f"top1 failed: {diagnostics}")
        assert_true(recall5 == 1.0, f"recall@5 failed: {diagnostics}")
        assert_true(mrr5 == 1.0, f"mrr@5 failed: {diagnostics}")
        assert_true(ndcg5 >= 0.95, f"ndcg@5 failed: {diagnostics}")
        assert_true(not rank_regressions, f"combined rank regressed vs FTS: {rank_regressions}")
        assert_true(
            strict_improvements >= runs,
            f"combined did not improve adversarial ranking often enough: {diagnostics}",
        )
        assert_true(
            not adversarial_failures,
            f"adversarial case did not improve every run: {adversarial_failures}",
        )
        assert_true(
            combined_p95 <= max(40.0, fts_p95 * 3.0),
            f"combined p95 too slow: combined={combined_p95:.3f} fts={fts_p95:.3f}",
        )
        assert_true(
            latency_ratio <= 2.0,
            f"combined/fts p50 ratio too high: {latency_ratio:.3f}",
        )

        return report


def run_prefilter_eval(binary: Path) -> Json:
    with tempfile.TemporaryDirectory(prefix="grepika-prefilter-eval-") as tmp:
        root = Path(tmp) / "repo"
        root.mkdir()
        write_prefilter_fixture(root)
        db = Path(tmp) / "grepika.db"

        index_payload, index_ms, _ = run_json(binary, root, db, ["index"])
        assert_true(index_payload["indexed"] == 700, f"unexpected prefilter index: {index_payload}")

        direct, direct_ms, _ = run_json(
            binary,
            root,
            db,
            ["search", "unique_direct_prefilter_token", "--limit", "5", "--mode", "combined"],
        )
        direct_paths = ranked_paths(direct)
        assert_true(
            direct_paths[:1] == ["src/file_17.rs"],
            f"direct prefilter paths={direct_paths}",
        )
        assert_true(
            "t" in direct["r"][0].get("src", "") and "g" in direct["r"][0].get("src", ""),
            f"direct prefilter sources={direct['r'][0].get('src')}",
        )

        walk, walk_ms, _ = run_json(
            binary,
            root,
            db,
            ["search", "medium_selective_prefilter_token", "--limit", "20", "--mode", "combined"],
        )
        walk_paths = ranked_paths(walk)
        assert_true(len(walk_paths) == 20, f"walk prefilter count={len(walk_paths)}")
        assert_true(
            all(path.startswith("src/file_") for path in walk_paths),
            f"walk prefilter paths={walk_paths}",
        )
        assert_true(
            all(int(path.removeprefix("src/file_").removesuffix(".rs")) < 317 for path in walk_paths),
            f"walk prefilter returned distractors: {walk_paths}",
        )
        assert_true(
            any("t" in item.get("src", "") and "g" in item.get("src", "") for item in walk["r"]),
            f"walk prefilter sources={[item.get('src') for item in walk['r']]}",
        )

    return {
        "status": "pass",
        "indexed": 700,
        "timings_ms": {
            "index": round(index_ms, 3),
            "direct_candidate_query": round(direct_ms, 3),
            "walk_filter_query": round(walk_ms, 3),
        },
        "direct_top": direct_paths[:3],
        "walk_top": walk_paths[:3],
    }


def run_regex_prefilter_eval(binary: Path) -> Json:
    with tempfile.TemporaryDirectory(prefix="grepika-regex-prefilter-eval-") as tmp:
        root = Path(tmp) / "repo"
        root.mkdir()
        write_regex_prefilter_fixture(root)
        db = Path(tmp) / "grepika.db"

        index_payload, index_ms, _ = run_json(binary, root, db, ["index"])
        assert_true(index_payload["indexed"] == 700, f"unexpected regex prefilter index: {index_payload}")

        cases = [
            ("optional_suffix", r"password(reset)?", ["src/file_17.rs", "src/file_23.rs"]),
            ("zero_or_more_prefix", r"(token)*without_zero_marker", ["src/file_31.rs"]),
            ("alternation", r"prefix_(foo|bar)_suffix", ["src/file_43.rs", "src/file_47.rs"]),
        ]
        diagnostics: list[Json] = []
        timings: dict[str, float] = {"index": round(index_ms, 3)}

        for case_id, query, expected_paths in cases:
            payload, elapsed_ms, _ = run_json(
                binary,
                root,
                db,
                ["search", query, "--limit", "10", "--mode", "combined"],
            )
            paths = ranked_paths(payload)
            timings[case_id] = round(elapsed_ms, 3)
            missing = [path for path in expected_paths if path not in paths]
            assert_true(
                not missing,
                f"regex prefilter {case_id} missed {missing}; paths={paths}",
            )
            assert_true(
                any("g" in item.get("src", "") for item in payload.get("r", [])),
                f"regex prefilter {case_id} did not use grep sources: {payload.get('r', [])}",
            )
            diagnostics.append(
                {
                    "case": case_id,
                    "query": query,
                    "expected": expected_paths,
                    "top": paths[:5],
                }
            )

    return {
        "status": "pass",
        "indexed": 700,
        "timings_ms": timings,
        "cases": diagnostics,
    }


def run_empty_trigram_eval(binary: Path) -> Json:
    with tempfile.TemporaryDirectory(prefix="grepika-empty-trigram-eval-") as tmp:
        root = Path(tmp) / "repo"
        root.mkdir()
        src = root / "src"
        src.mkdir(parents=True)
        db = Path(tmp) / "grepika.db"
        target_path = "src/file_17.rs"

        for i in range(700):
            content = (
                "pub fn target() { let needle_empty_trigram_17 = true; }\n"
                if i == 17
                else f"pub fn distractor_{i}() {{ let common_value = {i}; }}\n"
            )
            (src / f"file_{i}.rs").write_text(content, encoding="utf-8")

        index_payload, index_ms, _ = run_json(binary, root, db, ["index"])
        assert_true(index_payload["indexed"] == 700, f"unexpected empty-trigram index: {index_payload}")

        with sqlite3.connect(db) as conn:
            conn.execute("DELETE FROM trigrams")
            conn.commit()
            trigram_rows = conn.execute("SELECT COUNT(*) FROM trigrams").fetchone()[0]
            file_rows = conn.execute("SELECT COUNT(*) FROM files").fetchone()[0]

        assert_true(trigram_rows == 0, f"trigrams not cleared: {trigram_rows}")
        assert_true(file_rows == 700, f"file rows changed unexpectedly: {file_rows}")

        search, search_ms, _ = run_json(
            binary,
            root,
            db,
            ["search", r"needle_empty_trigram_17.*true", "--limit", "5", "--mode", "combined"],
        )
        paths = ranked_paths(search)
        assert_true(target_path in paths, f"empty-trigram regex fallback paths={paths}")

    return {
        "status": "pass",
        "indexed": 700,
        "timings_ms": {
            "index": round(index_ms, 3),
            "regex_search_after_trigram_delete": round(search_ms, 3),
        },
        "top": paths[:3],
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=Path("target/release/grepika"))
    parser.add_argument("--runs", type=int, default=10)
    parser.add_argument("--out", type=Path)
    args = parser.parse_args()

    if args.runs < 1:
        raise EvalFailure("--runs must be >= 1")
    binary = args.binary.resolve()
    if not binary.exists():
        raise EvalFailure(f"binary not found: {binary}")

    report = run_eval(binary, args.runs)
    report["prefilter"] = run_prefilter_eval(binary)
    report["regex_prefilter"] = run_regex_prefilter_eval(binary)
    report["empty_trigram_fallback"] = run_empty_trigram_eval(binary)
    text = json.dumps(report, indent=2, sort_keys=True)
    if args.out:
        args.out.parent.mkdir(parents=True, exist_ok=True)
        args.out.write_text(text + "\n", encoding="utf-8")
    print(text)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
