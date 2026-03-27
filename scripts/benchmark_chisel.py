#!/usr/bin/env python3
"""Lightweight timing harness for Chisel (stdlib only).

Creates a minimal git repo in a temp dir, runs analyze + risk_map, prints timings.
Optional env caps (seconds), exit 1 if exceeded — for CI regression guard.

  BENCH_MAX_ANALYZE_SEC   default 120
  BENCH_MAX_RISKMAP_SEC   default 60

Run from repo root: python scripts/benchmark_chisel.py
"""
from __future__ import annotations

import os
import subprocess
import sys
import tempfile
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT))


def _init_minimal_git(project: Path) -> None:
    subprocess.run(
        ["git", "init"],
        cwd=project, check=True, capture_output=True, text=True,
    )
    subprocess.run(
        ["git", "config", "user.email", "bench@example.com"],
        cwd=project, check=True, capture_output=True,
    )
    subprocess.run(
        ["git", "config", "user.name", "bench"],
        cwd=project, check=True, capture_output=True,
    )
    (project / "mod.py").write_text("def hello():\n    return 1\n", encoding="utf-8")
    subprocess.run(
        ["git", "add", "-A"],
        cwd=project, check=True, capture_output=True,
    )
    subprocess.run(
        ["git", "commit", "-m", "init"],
        cwd=project, check=True, capture_output=True,
    )


def main() -> int:
    t_all = time.perf_counter()
    with tempfile.TemporaryDirectory() as td:
        tmp = Path(td)
        proj = tmp / "benchproj"
        proj.mkdir()
        _init_minimal_git(proj)
        storage = tmp / "chisel_data"

        from chisel.engine import ChiselEngine  # noqa: PLC0415

        with ChiselEngine(str(proj), storage_dir=storage) as eng:
            t0 = time.perf_counter()
            eng.analyze()
            t_analyze = time.perf_counter() - t0

            t1 = time.perf_counter()
            with eng._process_lock.shared():
                with eng.lock.read_lock():
                    eng.impact.get_risk_map(
                        None, True, False, "unit",
                    )
            t_risk = time.perf_counter() - t1

    total = time.perf_counter() - t_all
    print(f"analyze:   {t_analyze:.3f}s")
    print(f"risk_map:  {t_risk:.3f}s")
    print(f"total:     {total:.3f}s")

    max_a = float(os.environ.get("BENCH_MAX_ANALYZE_SEC", "120"))
    max_r = float(os.environ.get("BENCH_MAX_RISKMAP_SEC", "60"))
    if t_analyze > max_a:
        print(f"FAIL: analyze exceeded {max_a}s", file=sys.stderr)
        return 1
    if t_risk > max_r:
        print(f"FAIL: risk_map exceeded {max_r}s", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
