#!/usr/bin/env python3
"""Smoke-test hybrid static suggest_tests without pytest (stdlib + Chisel only).

Run from the Chisel repo root::

    python scripts/verify_static_hybrid.py

This exercises the same path as tests/test_engine.py::TestStaticSuggestWhenEdgesCleared
without pulling in the test harness — useful when validating installs or CI sandboxes
that traditionally rely on separate tooling.
"""

from __future__ import annotations

import subprocess
import sys
from pathlib import Path


def _run_git(cwd: Path, *args: str) -> None:
    r = subprocess.run(
        ["git", *args],
        cwd=str(cwd),
        capture_output=True,
        text=True,
    )
    if r.returncode != 0:
        raise RuntimeError(r.stderr or r.stdout or "git failed")


def main() -> int:
    repo_root = Path(__file__).resolve().parent.parent
    sys.path.insert(0, str(repo_root))

    import tempfile

    from chisel.engine import ChiselEngine

    with tempfile.TemporaryDirectory() as td:
        root = Path(td) / "proj"
        root.mkdir()
        (root / "src").mkdir()
        (root / "tests").mkdir()
        (root / "src" / "widget.js").write_text("export const x = 1;\n", encoding="utf-8")
        (root / "tests" / "widget.test.js").write_text(
            "const w = require('../src/widget');\n"
            "describe('w', () => { it('a', () => {}); });\n",
            encoding="utf-8",
        )
        _run_git(root, "init")
        _run_git(root, "config", "user.email", "verify@local")
        _run_git(root, "config", "user.name", "verify")
        _run_git(root, "add", "-A")
        _run_git(root, "commit", "-m", "init")

        storage_dir = Path(td) / "chisel_db"
        with ChiselEngine(str(root), storage_dir=storage_dir) as eng:
            eng.analyze(force=True)
            eng.storage._execute("DELETE FROM test_edges")
            sug = eng.tool_suggest_tests("src/widget.js")

        if not sug:
            print("FAIL: expected non-empty suggest_tests after static scan", file=sys.stderr)
            return 1
        if not any(x.get("source") == "static_require" for x in sug):
            print("FAIL: expected at least one static_require source", file=sys.stderr)
            return 1

    print("verify_static_hybrid: OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
