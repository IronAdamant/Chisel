#!/usr/bin/env python3
"""Verify chisel.__version__ matches pyproject.toml [project].version (stdlib only).

Run from repo root: python scripts/check_version.py
Used in CI to prevent release drift between package metadata and chisel/__init__.py.
"""
from __future__ import annotations

import sys
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent


def main() -> int:
    pyproject = tomllib.loads((ROOT / "pyproject.toml").read_text(encoding="utf-8"))
    expected = pyproject["project"]["version"]
    sys.path.insert(0, str(ROOT))
    import chisel  # noqa: PLC0415 — after sys.path

    actual = chisel.__version__
    if actual != expected:
        print(
            f"Version mismatch: pyproject.toml has {expected!r}, "
            f"chisel.__version__ is {actual!r}",
            file=sys.stderr,
        )
        return 1
    print(f"OK: {actual}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
