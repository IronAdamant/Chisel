"""Tests for .gitignore-aware file scanning (git_visible_paths and friends).

Regression for Logged_issues/2026-06-11_directory-scoped-analyze-walks-all-tests.md:
gitignored trees (vendored deps, build output, bulk fixture dirs) were
scanned by both the engine code scan and TestMapper test discovery, making
analyze/update minutes-long on repos with large ignored directories.
"""

import os

import pytest

from chisel.engine import ChiselEngine
from chisel.project import git_visible_paths, is_git_visible_file, prune_walk_dirs
from chisel.test_mapper import TestMapper


@pytest.fixture
def ignored_bigfixtures_project(tmp_path, run_git):
    """Git repo with a tracked src tree and a gitignored bigfixtures tree."""
    project = tmp_path / "proj"
    project.mkdir()
    run_git(project, "init")
    run_git(project, "config", "user.name", "TestUser")
    run_git(project, "config", "user.email", "test@example.com")

    (project / ".gitignore").write_text("bigfixtures/\n")

    (project / "app.py").write_text("def real_func():\n    return 1\n")
    tests_dir = project / "tests"
    tests_dir.mkdir()
    (tests_dir / "test_app.py").write_text(
        "from app import real_func\n\n"
        "def test_real_func():\n    assert real_func() == 1\n"
    )

    bigfixtures = project / "bigfixtures" / "lib"
    bigfixtures.mkdir(parents=True)
    (bigfixtures / "dep.py").write_text("def fixture_dep():\n    return 2\n")
    (bigfixtures / "test_dep.py").write_text(
        "def test_fixture_dep():\n    assert True\n"
    )

    run_git(project, "add", "-A")
    run_git(project, "commit", "-m", "initial")
    return project


class TestGitVisiblePaths:
    def test_tracked_and_untracked_visible_ignored_dropped(
            self, ignored_bigfixtures_project):
        (ignored_bigfixtures_project / "new_untracked.py").write_text("x = 1\n")
        files, dirs = git_visible_paths(str(ignored_bigfixtures_project))
        assert "app.py" in files
        assert "tests/test_app.py" in files
        assert "new_untracked.py" in files
        assert "bigfixtures/lib/dep.py" not in files
        assert "tests" in dirs
        assert "bigfixtures" not in dirs
        assert "bigfixtures/lib" not in dirs

    def test_non_git_dir_returns_none(self, tmp_path):
        files, dirs = git_visible_paths(str(tmp_path))
        assert files is None
        assert dirs is None

    def test_env_optout_returns_none(self, ignored_bigfixtures_project, monkeypatch):
        monkeypatch.setenv("CHISEL_INCLUDE_IGNORED", "1")
        files, dirs = git_visible_paths(str(ignored_bigfixtures_project))
        assert files is None
        assert dirs is None

    def test_helpers_pass_everything_when_filter_disabled(self, tmp_path):
        assert is_git_visible_file(str(tmp_path / "x.py"), str(tmp_path), None)
        kept = prune_walk_dirs(
            str(tmp_path), ["a", "node_modules"], {"node_modules"},
            str(tmp_path), None)
        assert kept == ["a"]


class TestEngineScanRespectsGitignore:
    def test_analyze_skips_ignored_tree(self, ignored_bigfixtures_project, tmp_path):
        engine = ChiselEngine(
            str(ignored_bigfixtures_project), storage_dir=str(tmp_path / "store"))
        try:
            stats = engine.tool_analyze()
            assert stats["code_files_scanned"] == 2  # app.py + tests/test_app.py
            assert engine.storage.get_code_units_by_file("app.py")
            assert not engine.storage.get_code_units_by_file("bigfixtures/lib/dep.py")
        finally:
            engine.close()

    def test_untracked_non_ignored_still_scanned(
            self, ignored_bigfixtures_project, tmp_path):
        (ignored_bigfixtures_project / "wip.py").write_text("def wip():\n    pass\n")
        engine = ChiselEngine(
            str(ignored_bigfixtures_project), storage_dir=str(tmp_path / "store"))
        try:
            scanned = engine._scan_code_files()
            rels = {os.path.relpath(p, str(ignored_bigfixtures_project)) for p in scanned}
            assert "wip.py" in rels
        finally:
            engine.close()


class TestDiscoverTestFilesRespectsGitignore:
    def test_ignored_test_tree_skipped(self, ignored_bigfixtures_project):
        mapper = TestMapper(str(ignored_bigfixtures_project))
        found = {os.path.relpath(p, str(ignored_bigfixtures_project))
                 for p in mapper.discover_test_files()}
        assert os.path.join("tests", "test_app.py") in found
        assert not any(p.startswith("bigfixtures") for p in found)

    def test_optout_includes_ignored_tests(
            self, ignored_bigfixtures_project, monkeypatch):
        monkeypatch.setenv("CHISEL_INCLUDE_IGNORED", "1")
        mapper = TestMapper(str(ignored_bigfixtures_project))
        found = {os.path.relpath(p, str(ignored_bigfixtures_project))
                 for p in mapper.discover_test_files()}
        assert os.path.join("bigfixtures", "lib", "test_dep.py") in found

    def test_non_git_project_unfiltered(self, tmp_path):
        (tmp_path / "test_loose.py").write_text(
            "def test_loose():\n    assert True\n")
        mapper = TestMapper(str(tmp_path))
        found = [os.path.basename(p) for p in mapper.discover_test_files()]
        assert "test_loose.py" in found
