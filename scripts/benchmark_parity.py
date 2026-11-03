#!/usr/bin/env python3
"""Compare Chisel Python 0.15.1 with the Rust binary.

Five runs per command. Prints median milliseconds and the min-max range,
plus peak RSS in kilobytes when /usr/bin/time can measure it.

Usage:
  python3 scripts/benchmark_parity.py \
    --rust target/release/chisel \
    --python /path/to/venv/bin/chisel \
    --small /tmp/chisel-bench-small \
    --large /tmp/django
"""

from __future__ import annotations

import argparse
import json
import os
import platform
import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path


RUNS = 5


def machine_spec() -> dict[str, str]:
    spec = {
        "system": platform.platform(),
        "python": platform.python_version(),
        "cpu": platform.processor() or "unknown",
        "machine": platform.machine(),
    }
    for key, cmd in (
        ("uname", ["uname", "-a"]),
        ("cpu_model", ["bash", "-lc", "lscpu | awk -F: '/Model name/ {print $2; exit}'"]),
        ("nproc", ["nproc"]),
        ("mem_gb", ["bash", "-lc", "awk '/MemTotal/ {printf \"%.1f\", $2/1024/1024}' /proc/meminfo"]),
        ("rustc", ["rustc", "--version"]),
    ):
        try:
            spec[key] = subprocess.check_output(cmd, text=True, stderr=subprocess.DEVNULL).strip()
        except (OSError, subprocess.CalledProcessError):
            spec[key] = "unavailable"
    return spec


def peak_rss_kb(pid: int) -> int | None:
    try:
        text = Path(f"/proc/{pid}/status").read_text()
    except OSError:
        return None
    for line in text.splitlines():
        if line.startswith("VmHWM:"):
            parts = line.split()
            if len(parts) >= 2 and parts[1].isdigit():
                return int(parts[1])
    return None


def measure(cmd: list[str], cwd: Path, env: dict[str, str]) -> dict:
    import threading
    proc = subprocess.Popen(cmd, cwd=cwd, env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    rss = {"n": 0}
    stop = threading.Event()

    def sample() -> None:
        while not stop.is_set():
            value = peak_rss_kb(proc.pid)
            if value:
                rss["n"] = max(rss["n"], value)
            stop.wait(0.01)

    thread = threading.Thread(target=sample, daemon=True)
    thread.start()
    start = time.perf_counter()
    stdout, stderr = proc.communicate()
    elapsed_ms = (time.perf_counter() - start) * 1000.0
    stop.set()
    thread.join(timeout=0.2)
    return {
        "ms": elapsed_ms,
        "rss_kb": rss["n"] or None,
        "code": proc.returncode,
        "stderr": (stderr or "")[-500:],
    }


def summarise(samples: list[dict]) -> dict:
    ok = [s for s in samples if s["code"] == 0]
    if not ok:
        return {"error": samples[-1]["stderr"] if samples else "no runs", "n": len(samples)}
    values = sorted(s["ms"] for s in ok)
    mid = values[len(values) // 2]
    rss = [s["rss_kb"] for s in ok if s["rss_kb"] is not None]
    return {
        "median_ms": round(mid, 1),
        "min_ms": round(values[0], 1),
        "max_ms": round(values[-1], 1),
        "n": len(ok),
        "peak_rss_kb": max(rss) if rss else None,
    }


def fmt(summary: dict) -> str:
    if "error" in summary:
        return "not measured"
    return f"{summary['median_ms']:.1f} ({summary['min_ms']:.1f}–{summary['max_ms']:.1f})"


def prepare_small(path: Path) -> None:
    if (path / ".git").exists():
        return
    path.mkdir(parents=True, exist_ok=True)
    (path / "src").mkdir(exist_ok=True)
    (path / "tests").mkdir(exist_ok=True)
    (path / "src" / "calc.py").write_text("def add(a, b):\n    return a + b\n\ndef sub(a, b):\n    return a - b\n")
    (path / "tests" / "test_calc.py").write_text(
        "from src.calc import add\n\ndef test_add():\n    assert add(1, 1) == 2\n"
    )
    env = os.environ.copy()
    subprocess.check_call(["git", "init", "-b", "main"], cwd=path)
    subprocess.check_call(["git", "config", "user.email", "bench@example.com"], cwd=path)
    subprocess.check_call(["git", "config", "user.name", "Bench"], cwd=path)
    subprocess.check_call(["git", "add", "-A"], cwd=path)
    subprocess.check_call(["git", "commit", "-m", "init"], cwd=path, env={**env, "GIT_AUTHOR_DATE": "2024-01-01T00:00:00Z", "GIT_COMMITTER_DATE": "2024-01-01T00:00:00Z"})


def run_suite(label: str, binary: str, project: Path, storage: Path, impact_file: str) -> dict[str, dict]:
    env = os.environ.copy()
    tail = ["--json", "--project-dir", str(project), "--storage-dir", str(storage)]
    commands = {
        "startup": [binary, "--help"],
        "cold_analyze": [binary, "analyze", ".", *tail],
        "warm_analyze": [binary, "analyze", ".", *tail],
        "warm_update": [binary, "update", *tail],
        "impact": [binary, "impact", impact_file, *tail],
        "suggest_tests": [binary, "suggest-tests", impact_file, *tail],
        "risk_map": [binary, "risk-map", *tail],
        "test_gaps": [binary, "test-gaps", *tail],
        "triage": [binary, "triage", *tail],
        "churn": [binary, "churn", impact_file, *tail],
        "coupling": [binary, "coupling", impact_file, *tail],
        "stats": [binary, "stats", *tail],
    }
    out: dict[str, dict] = {}
    for name, cmd in commands.items():
        samples = []
        for i in range(RUNS):
            if name == "cold_analyze":
                shutil.rmtree(storage, ignore_errors=True)
            elif name != "startup" and not storage.exists() and name != "cold_analyze":
                # Queries need a snapshot. Build one if a previous cold run was skipped.
                subprocess.run([binary, "analyze", ".", *tail], cwd=project, env=env, capture_output=True, text=True)
            samples.append(measure(cmd, project if name != "startup" else Path.cwd(), env))
            if samples[-1]["code"] != 0 and name != "startup":
                break
        out[name] = summarise(samples)
        out[name]["label"] = label
        print(f"{label} {name}: {fmt(out[name])}", flush=True)
    return out


def snapshot_counts(storage: Path) -> str:
    snap = storage / "store"
    if snap.is_file():
        text = snap.read_text(errors="replace")
        return f"rust snapshot bytes {len(text)}, test_units mentions {text.count('\"framework\"')}"
    db = storage / "chisel.db"
    if db.is_file():
        import sqlite3
        conn = sqlite3.connect(db)
        try:
            units = conn.execute("select count(*) from test_units").fetchone()[0]
            edges = conn.execute("select count(*) from test_edges").fetchone()[0]
        except sqlite3.Error as exc:
            return f"python db unreadable: {exc}"
        finally:
            conn.close()
        return f"python test_units {units}, test_edges {edges}"
    return "no snapshot"


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--rust", required=True)
    parser.add_argument("--python", required=True)
    parser.add_argument("--small", required=True)
    parser.add_argument("--large", required=True)
    parser.add_argument("--small-file", default="src/calc.py")
    parser.add_argument("--large-file", default="django/http/request.py")
    parser.add_argument("--out", default="scripts/benchmark_results.md")
    args = parser.parse_args()
    small = Path(args.small)
    large = Path(args.large)
    prepare_small(small)
    if not large.is_dir():
        print(f"large repo missing: {large}", file=sys.stderr)
        return 1
    spec = machine_spec()
    rust_bin = str(Path(args.rust).resolve())
    py_bin = str(Path(args.python).resolve())
    results = {
        "machine": spec,
        "rust_bin": rust_bin,
        "python_bin": py_bin,
        "runs": RUNS,
        "small": {
            "rust": run_suite("rust-small", rust_bin, small, Path("/tmp/chisel-bench-small-rust"), args.small_file),
            "python": run_suite("py-small", py_bin, small, Path("/tmp/chisel-bench-small-py"), args.small_file),
        },
        "large": {
            "rust": run_suite("rust-large", rust_bin, large, Path("/tmp/chisel-bench-large-rust"), args.large_file),
            "python": run_suite("py-large", py_bin, large, Path("/tmp/chisel-bench-large-py"), args.large_file),
        },
    }
    lines = ["# Chisel benchmark results", "", "Generated by `scripts/benchmark_parity.py`. Every figure is from a real run.", "", "## Machine", ""]
    for key, value in spec.items():
        lines.append(f"- {key}: {value}")
    lines += ["", f"Rust binary: `{rust_bin}`", f"Python binary: `{py_bin}`", f"Runs per command: {RUNS}", ""]
    lines.append("Times are median milliseconds, with the minimum and maximum in parentheses. Peak RSS is the highest VmHWM sampled from /proc during each run, in KiB.")
    lines.append("")
    header = "| Command | Python small | Rust small | Python large | Rust large |"
    lines += [header, "|---|---:|---:|---:|---:|"]
    order = ["startup", "cold_analyze", "warm_analyze", "warm_update", "impact", "suggest_tests", "risk_map", "test_gaps", "triage", "churn", "coupling", "stats"]
    for name in order:
        cells = []
        for side in ("small", "large"):
            for impl in ("python", "rust"):
                cells.append(fmt(results[side][impl].get(name, {"error": "missing"})))
        lines.append(f"| {name} | " + " | ".join(cells) + " |")
    lines += ["", "## Peak RSS (KiB)", "", "| Command | Python small | Rust small | Python large | Rust large |", "|---|---:|---:|---:|---:|"]
    for name in order:
        cells = []
        for side in ("small", "large"):
            for impl in ("python", "rust"):
                summary = results[side][impl].get(name, {})
                rss = summary.get("peak_rss_kb")
                cells.append(str(rss) if rss else "n/a")
        lines.append(f"| {name} | " + " | ".join(cells) + " |")
    lines += ["", "## Failures", ""]
    any_fail = False
    for side in ("small", "large"):
        for impl in ("python", "rust"):
            for name, summary in results[side][impl].items():
                if "error" in summary:
                    any_fail = True
                    lines.append(f"- {impl} {side} {name}: {summary['error'].strip()}")
    if not any_fail:
        lines.append("None.")
    lines += ["", "## Snapshot counts after the timed runs", ""]
    lines.append(f"- small rust: {snapshot_counts(Path('/tmp/chisel-bench-small-rust'))}")
    lines.append(f"- small python: {snapshot_counts(Path('/tmp/chisel-bench-small-py'))}")
    lines.append(f"- large rust: {snapshot_counts(Path('/tmp/chisel-bench-large-rust'))}")
    lines.append(f"- large python: {snapshot_counts(Path('/tmp/chisel-bench-large-py'))}")
    lines.append("")
    lines.append("The small repo is a two-file Python sample created by this script. The large repo is the django package from django/django (about 1000 code files) plus 80 of its test modules. History is 38 commits of at most 100 paths each: a shallow clone's boundary commit lists every file, and pairing those files dominates co-change time, so the benchmark uses this bounded history of the same tree. Counts are taken from the snapshot left by the last timed command.")
    text = "\n".join(lines) + "\n"
    Path(args.out).write_text(text)
    print(text)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
