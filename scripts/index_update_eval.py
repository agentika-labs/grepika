#!/usr/bin/env python3
"""Black-box eval for index update/delete behavior and timing.

The Rust unit tests can inspect in-memory n-gram bitmaps directly. This eval
checks the CLI contract and persisted SQLite state that a restarted grepika
process will observe.
"""

from __future__ import annotations

import argparse
import json
import os
import sqlite3
import statistics
import subprocess
import tempfile
import time
from pathlib import Path
from typing import Any


Json = dict[str, Any]


class EvalFailure(AssertionError):
    pass


def run_json(binary: Path, root: Path, db: Path, args: list[str], ok: set[int] | None = None) -> tuple[Json, float]:
    ok = ok or {0}
    cmd = [str(binary), "--root", str(root), "--db", str(db), "--json", *args]
    start = time.perf_counter()
    proc = subprocess.run(cmd, text=True, capture_output=True, check=False)
    elapsed_ms = (time.perf_counter() - start) * 1000.0
    if proc.returncode not in ok:
        raise EvalFailure(
            f"{' '.join(cmd)} exited {proc.returncode}\nstdout={proc.stdout}\nstderr={proc.stderr}"
        )
    try:
        payload = json.loads(proc.stdout)
    except json.JSONDecodeError as exc:
        raise EvalFailure(f"invalid JSON from {' '.join(cmd)}: {proc.stdout!r}") from exc
    return payload, elapsed_ms


def run_json_with_stderr(
    binary: Path,
    root: Path,
    db: Path,
    args: list[str],
    env: dict[str, str] | None = None,
    ok: set[int] | None = None,
) -> tuple[Json, float, str]:
    ok = ok or {0}
    cmd = [str(binary), "--root", str(root), "--db", str(db), "--json", *args]
    merged_env = os.environ.copy()
    if env:
        merged_env.update(env)
    start = time.perf_counter()
    proc = subprocess.run(cmd, text=True, capture_output=True, check=False, env=merged_env)
    elapsed_ms = (time.perf_counter() - start) * 1000.0
    if proc.returncode not in ok:
        raise EvalFailure(
            f"{' '.join(cmd)} exited {proc.returncode}\nstdout={proc.stdout}\nstderr={proc.stderr}"
        )
    try:
        payload = json.loads(proc.stdout)
    except json.JSONDecodeError as exc:
        raise EvalFailure(f"invalid JSON from {' '.join(cmd)}: {proc.stdout!r}") from exc
    return payload, elapsed_ms, proc.stderr


def run_cmd(args: list[str], cwd: Path) -> None:
    proc = subprocess.run(args, cwd=cwd, text=True, capture_output=True, check=False)
    if proc.returncode != 0:
        raise EvalFailure(
            f"{' '.join(args)} exited {proc.returncode}\nstdout={proc.stdout}\nstderr={proc.stderr}"
        )


def assert_true(condition: bool, message: str) -> None:
    if not condition:
        raise EvalFailure(message)


def paths_in_db(db: Path) -> list[str]:
    with sqlite3.connect(db) as conn:
        return [row[0] for row in conn.execute("SELECT path FROM files ORDER BY path")]


def db_counts(db: Path) -> Json:
    with sqlite3.connect(db) as conn:
        files = conn.execute("SELECT COUNT(*) FROM files").fetchone()[0]
        trigrams = conn.execute("SELECT COUNT(*) FROM trigrams").fetchone()[0]
        fts_old = conn.execute(
            "SELECT COUNT(*) FROM files_fts WHERE files_fts MATCH ?",
            ("unique_old_token_alpha",),
        ).fetchone()[0]
        fts_removed = conn.execute(
            "SELECT COUNT(*) FROM files_fts WHERE files_fts MATCH ?",
            ("unique_force_removed_token",),
        ).fetchone()[0]
    return {
        "files": files,
        "trigrams": trigrams,
        "fts_old_token_hits": fts_old,
        "fts_removed_token_hits": fts_removed,
    }


def search_paths(payload: Json) -> list[str]:
    return [item["p"] for item in payload.get("r", [])]


def init_git_repo(root: Path) -> None:
    run_cmd(["git", "init"], root)
    run_cmd(["git", "config", "user.email", "eval@example.com"], root)
    run_cmd(["git", "config", "user.name", "grepika eval"], root)


def git_commit(root: Path, message: str) -> None:
    run_cmd(["git", "add", "."], root)
    run_cmd(["git", "commit", "-m", message], root)


def run_git_fast_path_eval(binary: Path, runs: int) -> Json:
    with tempfile.TemporaryDirectory(prefix="grepika-index-git-eval-") as tmp:
        root = Path(tmp) / "repo"
        root.mkdir()
        db = Path(tmp) / "grepika-git.db"
        init_git_repo(root)

        update_file = root / "update.rs"
        keep_file = root / "keep.rs"
        remove_file = root / "remove.rs"

        update_file.write_text(
            "fn marker() { let git_old_token_alpha = 1; }\n",
            encoding="utf-8",
        )
        keep_file.write_text("fn git_keep_token() {}\n", encoding="utf-8")
        remove_file.write_text("fn git_removed_token() {}\n", encoding="utf-8")
        git_commit(root, "initial")

        initial, initial_ms = run_json(binary, root, db, ["index"])
        assert_true(initial["indexed"] == 3, f"git initial indexed={initial}")

        update_file.write_text(
            "fn marker() { let git_new_token_beta = 2; }\n",
            encoding="utf-8",
        )
        remove_file.unlink()
        update, update_ms, update_stderr = run_json_with_stderr(
            binary,
            root,
            db,
            ["index"],
            env={"RUST_LOG": "grepika=debug"},
        )
        assert_true(
            "Git fast path: detected changes" in update_stderr,
            f"git fast path proof log missing:\n{update_stderr}",
        )
        assert_true(update["indexed"] == 1, f"git update indexed={update}")
        assert_true(update["deleted"] == 1, f"git update deleted={update}")

        new_search, new_search_ms = run_json(
            binary, root, db, ["search", "git_new_token_beta", "--limit", "5"]
        )
        assert_true(
            "update.rs" in search_paths(new_search),
            f"git new token search paths={search_paths(new_search)}",
        )

        old_search, old_search_ms = run_json(
            binary,
            root,
            db,
            ["search", "git_old_token_alpha", "--limit", "5"],
            ok={0, 1},
        )
        assert_true(
            search_paths(old_search) == [],
            f"git old token should not return results: {old_search}",
        )

        removed_search, removed_search_ms = run_json(
            binary,
            root,
            db,
            ["search", "git_removed_token", "--limit", "5"],
            ok={0, 1},
        )
        assert_true(
            search_paths(removed_search) == [],
            f"git removed token should not return results: {removed_search}",
        )

        noop_times: list[float] = []
        for _ in range(runs):
            noop, elapsed_ms = run_json(binary, root, db, ["index"])
            assert_true(noop["indexed"] == 0, f"git noop indexed={noop}")
            assert_true(noop["deleted"] == 0, f"git noop deleted={noop}")
            noop_times.append(elapsed_ms)

        with sqlite3.connect(db) as conn:
            files = conn.execute("SELECT COUNT(*) FROM files").fetchone()[0]
            old_hits = conn.execute(
                "SELECT COUNT(*) FROM files_fts WHERE files_fts MATCH ?",
                ("git_old_token_alpha",),
            ).fetchone()[0]
            removed_hits = conn.execute(
                "SELECT COUNT(*) FROM files_fts WHERE files_fts MATCH ?",
                ("git_removed_token",),
            ).fetchone()[0]
            last_commit = conn.execute(
                "SELECT value FROM schema_info WHERE key = 'last_indexed_commit'"
            ).fetchone()

        assert_true(files == 2, f"git files count={files}")
        assert_true(old_hits == 0, f"git old FTS token remains: {old_hits}")
        assert_true(removed_hits == 0, f"git removed FTS token remains: {removed_hits}")
        assert_true(last_commit is not None, "git fast path did not persist last indexed commit")

    return {
        "status": "pass",
        "runs": runs,
        "timings_ms": {
            "initial_index": round(initial_ms, 3),
            "git_update_delete": round(update_ms, 3),
            "search_new_token": round(new_search_ms, 3),
            "search_old_token": round(old_search_ms, 3),
            "search_removed_token": round(removed_search_ms, 3),
            "git_noop_p50": round(statistics.median(noop_times), 3),
            "git_noop_mean": round(statistics.fmean(noop_times), 3),
        },
        "db_counts_after_git_update": {
            "files": files,
            "fts_old_token_hits": old_hits,
            "fts_removed_token_hits": removed_hits,
        },
    }


def run_graph_batch_eval(binary: Path, runs: int) -> Json:
    graph_file_count = 64

    with tempfile.TemporaryDirectory(prefix="grepika-index-graph-eval-") as tmp:
        root = Path(tmp) / "repo"
        root.mkdir()
        db = Path(tmp) / "grepika-graph.db"
        init_git_repo(root)

        (root / "target.rs").write_text(
            "pub fn shared_graph_target() {}\npub fn shared_graph_target_v2() {}\n",
            encoding="utf-8",
        )
        for i in range(graph_file_count):
            (root / f"graph_{i}.rs").write_text(
                f"pub fn graph_eval_{i}() {{ shared_graph_target(); }}\n",
                encoding="utf-8",
            )
        git_commit(root, "initial graph")

        initial, initial_ms = run_json(binary, root, db, ["index"])
        assert_true(
            initial["indexed"] == graph_file_count + 1,
            f"graph initial indexed={initial}",
        )

        for i in range(graph_file_count):
            (root / f"graph_{i}.rs").write_text(
                f"pub fn graph_eval_{i}() {{ shared_graph_target_v2(); }}\n",
                encoding="utf-8",
            )

        update, update_ms, update_stderr = run_json_with_stderr(
            binary,
            root,
            db,
            ["index"],
            env={"RUST_LOG": "grepika=debug"},
        )
        assert_true(
            "Git fast path: detected changes" in update_stderr,
            f"graph git fast path proof log missing:\n{update_stderr}",
        )
        assert_true(update["indexed"] == graph_file_count, f"graph update indexed={update}")
        assert_true(update["deleted"] == 0, f"graph update deleted={update}")

        noop_times: list[float] = []
        for _ in range(runs):
            noop, elapsed_ms = run_json(binary, root, db, ["index"])
            assert_true(noop["indexed"] == 0, f"graph noop indexed={noop}")
            assert_true(noop["deleted"] == 0, f"graph noop deleted={noop}")
            noop_times.append(elapsed_ms)

        with sqlite3.connect(db) as conn:
            graph_symbols = conn.execute(
                "SELECT COUNT(*) FROM symbols WHERE name LIKE 'graph_eval_%'"
            ).fetchone()[0]
            old_edges = conn.execute(
                "SELECT COUNT(*) FROM edges WHERE kind = 'CALLS' AND dst_name = ?",
                ("shared_graph_target",),
            ).fetchone()[0]
            new_edges = conn.execute(
                "SELECT COUNT(*) FROM edges WHERE kind = 'CALLS' AND dst_name = ?",
                ("shared_graph_target_v2",),
            ).fetchone()[0]

        assert_true(
            graph_symbols == graph_file_count,
            f"graph symbol count={graph_symbols}",
        )
        assert_true(old_edges == 0, f"stale graph edges remain: {old_edges}")
        assert_true(
            new_edges == graph_file_count,
            f"new graph edge count={new_edges}",
        )

    return {
        "status": "pass",
        "runs": runs,
        "graph_file_count": graph_file_count,
        "timings_ms": {
            "initial_index": round(initial_ms, 3),
            "graph_batch_update": round(update_ms, 3),
            "graph_noop_p50": round(statistics.median(noop_times), 3),
            "graph_noop_mean": round(statistics.fmean(noop_times), 3),
        },
        "db_counts_after_graph_update": {
            "graph_symbols": graph_symbols,
            "old_edges": old_edges,
            "new_edges": new_edges,
        },
    }


def run_eval(binary: Path, runs: int) -> Json:
    with tempfile.TemporaryDirectory(prefix="grepika-index-eval-") as tmp:
        root = Path(tmp) / "repo"
        root.mkdir()
        db = Path(tmp) / "grepika.db"

        update_file = root / "update.rs"
        keep_file = root / "keep.rs"
        remove_file = root / "remove.rs"

        update_file.write_text(
            "fn marker() { let unique_old_token_alpha = 1; }\n",
            encoding="utf-8",
        )
        keep_file.write_text("fn unique_keep_token() {}\n", encoding="utf-8")
        remove_file.write_text("fn unique_force_removed_token() {}\n", encoding="utf-8")

        initial, initial_ms = run_json(binary, root, db, ["index"])
        assert_true(initial["indexed"] == 3, f"initial indexed={initial}")

        update_file.write_text(
            "fn marker() { let unique_new_token_beta = 2; }\n",
            encoding="utf-8",
        )
        update, update_ms = run_json(binary, root, db, ["index"])
        assert_true(update["indexed"] == 1, f"incremental update={update}")

        new_search, new_search_ms = run_json(
            binary, root, db, ["search", "unique_new_token_beta", "--limit", "5"]
        )
        assert_true(
            "update.rs" in search_paths(new_search),
            f"new token search paths={search_paths(new_search)}",
        )

        old_search, old_search_ms = run_json(
            binary,
            root,
            db,
            ["search", "unique_old_token_alpha", "--limit", "5"],
            ok={0, 1},
        )
        assert_true(
            search_paths(old_search) == [],
            f"old token should not return results: {old_search}",
        )

        noop_times: list[float] = []
        for _ in range(runs):
            noop, elapsed_ms = run_json(binary, root, db, ["index"])
            assert_true(noop["indexed"] == 0, f"noop indexed={noop}")
            assert_true(noop["deleted"] == 0, f"noop deleted={noop}")
            noop_times.append(elapsed_ms)

        remove_file.unlink()
        force, force_ms = run_json(binary, root, db, ["index", "--force"])
        assert_true(force["indexed"] == 2, f"force indexed={force}")
        assert_true(force["deleted"] == 1, f"force deleted={force}")

        counts = db_counts(db)
        assert_true(counts["files"] == 2, f"db counts after force={counts}")
        assert_true(counts["trigrams"] > 0, f"db counts after force={counts}")
        assert_true(counts["fts_old_token_hits"] == 0, f"old FTS token remains: {counts}")
        assert_true(counts["fts_removed_token_hits"] == 0, f"removed FTS token remains: {counts}")
        assert_true(
            all("remove.rs" not in path for path in paths_in_db(db)),
            f"removed path still in DB: {paths_in_db(db)}",
        )

        removed_search, removed_search_ms = run_json(
            binary,
            root,
            db,
            ["search", "unique_force_removed_token", "--limit", "5"],
            ok={0, 1},
        )
        assert_true(
            search_paths(removed_search) == [],
            f"removed token should not return results: {removed_search}",
        )

    return {
        "status": "pass",
        "runs": runs,
        "timings_ms": {
            "initial_index": round(initial_ms, 3),
            "incremental_update": round(update_ms, 3),
            "search_new_token": round(new_search_ms, 3),
            "search_old_token": round(old_search_ms, 3),
            "noop_index_p50": round(statistics.median(noop_times), 3),
            "noop_index_mean": round(statistics.fmean(noop_times), 3),
            "force_reindex_after_delete": round(force_ms, 3),
            "search_removed_token": round(removed_search_ms, 3),
        },
        "db_counts_after_force": counts,
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=Path("target/release/grepika"))
    parser.add_argument("--runs", type=int, default=5)
    parser.add_argument("--out", type=Path)
    parser.add_argument(
        "--git-fast-path",
        action="store_true",
        help="also exercise git diff based incremental indexing",
    )
    parser.add_argument(
        "--graph-batch",
        action="store_true",
        help="also exercise batch graph replacement during git fast-path indexing",
    )
    args = parser.parse_args()

    if args.runs < 1:
        raise EvalFailure("--runs must be >= 1")

    binary = args.binary.resolve()
    if not binary.exists():
        raise EvalFailure(f"binary not found: {binary}")

    report = run_eval(binary, args.runs)
    if args.git_fast_path:
        report["git_fast_path"] = run_git_fast_path_eval(binary, args.runs)
    if args.graph_batch:
        report["graph_batch"] = run_graph_batch_eval(binary, args.runs)
    text = json.dumps(report, indent=2, sort_keys=True)
    if args.out:
        args.out.parent.mkdir(parents=True, exist_ok=True)
        args.out.write_text(text + "\n", encoding="utf-8")
    print(text)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
