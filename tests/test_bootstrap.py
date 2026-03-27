"""Tests for CHISEL_BOOTSTRAP optional module load."""

import sys
from pathlib import Path

from chisel.engine import ChiselEngine

_TESTS_DIR = Path(__file__).resolve().parent


def test_chisel_bootstrap_env_imports_module(monkeypatch, git_project, tmp_path):
    sys.path.insert(0, str(_TESTS_DIR))
    try:
        monkeypatch.setenv("CHISEL_BOOTSTRAP", "bootstrap_stub")
        storage = tmp_path / "chisel_data"
        with ChiselEngine(str(git_project), storage_dir=storage):
            pass
        assert "bootstrap_stub" in sys.modules
        assert sys.modules["bootstrap_stub"].CHISEL_BOOTSTRAP_TEST_LOADED is True
    finally:
        sys.path.remove(str(_TESTS_DIR))


def test_chisel_bootstrap_unset_does_not_require_stub(monkeypatch, git_project, tmp_path):
    monkeypatch.delenv("CHISEL_BOOTSTRAP", raising=False)
    storage = tmp_path / "chisel_data2"
    with ChiselEngine(str(git_project), storage_dir=storage) as eng:
        eng.analyze()
