"""Tests for chisel.engine — full integration with temp git repo + test files."""

import os
import subprocess

import pytest

from chisel.engine import ChiselEngine


def _run_git(repo_dir, *args, env_extra=None):
    """Helper to run git commands in a temp repo."""
    env = os.environ.copy()
    env.update({
        "GIT_AUTHOR_DATE": "2026-01-15T10:00:00+00:00",
        "GIT_COMMITTER_DATE": "2026-01-15T10:00:00+00:00",
    })
    if env_extra:
        env.update(env_extra)
    subprocess.run(
        ["git"] + list(args),
        cwd=str(repo_dir),
        capture_output=True,
        text=True,
        check=True,
        env=env,
    )


@pytest.fixture
def git_project(tmp_path):
    """Create a temp git repo with source + test files."""
    project = tmp_path / "myproject"
    project.mkdir()

    # Init git repo
    _run_git(project, "init")
    _run_git(project, "config", "user.name", "TestUser")
    _run_git(project, "config", "user.email", "test@example.com")

    # Source file
    src = project / "app.py"
    src.write_text(
        "def process_data(data):\n"
        "    return [x * 2 for x in data]\n\n"
        "def validate_input(data):\n"
        "    if not isinstance(data, list):\n"
        "        raise TypeError('Expected list')\n"
        "    return True\n"
    )

    # Test file
    tests_dir = project / "tests"
    tests_dir.mkdir()
    test_file = tests_dir / "test_app.py"
    test_file.write_text(
        "from app import process_data, validate_input\n\n"
        "def test_process_data():\n"
        "    assert process_data([1, 2, 3]) == [2, 4, 6]\n\n"
        "def test_validate_input():\n"
        "    assert validate_input([1, 2]) is True\n"
    )

    # Commit
    _run_git(project, "add", "-A")
    _run_git(project, "commit", "-m", "Initial commit")

    # Second commit — modify app.py
    src.write_text(
        "def process_data(data):\n"
        "    return [x * 2 for x in data]\n\n"
        "def validate_input(data):\n"
        "    if not isinstance(data, list):\n"
        "        raise TypeError('Expected list')\n"
        "    return True\n\n"
        "def format_output(result):\n"
        "    return ', '.join(str(x) for x in result)\n"
    )
    _run_git(project, "add", "-A")
    _run_git(
        project, "commit", "-m", "Add format_output",
        env_extra={
            "GIT_AUTHOR_DATE": "2026-02-01T10:00:00+00:00",
            "GIT_COMMITTER_DATE": "2026-02-01T10:00:00+00:00",
        },
    )

    return project


@pytest.fixture
def engine(git_project, tmp_path):
    storage_dir = tmp_path / "chisel_storage"
    return ChiselEngine(str(git_project), storage_dir=storage_dir)


class TestAnalyze:
    def test_full_analyze(self, engine):
        stats = engine.analyze()
        assert stats["code_files_scanned"] > 0
        assert stats["code_units_found"] > 0
        assert stats["test_files_found"] > 0
        assert stats["test_units_found"] > 0
        assert stats["commits_parsed"] > 0

    def test_analyze_populates_code_units(self, engine):
        engine.analyze()
        units = engine.storage.get_code_units_by_file("app.py")
        names = [u["name"] for u in units]
        assert "process_data" in names
        assert "validate_input" in names
        assert "format_output" in names

    def test_analyze_populates_test_units(self, engine):
        engine.analyze()
        all_tests = engine.storage.get_all_test_units()
        names = [t["name"] for t in all_tests]
        assert "test_process_data" in names
        assert "test_validate_input" in names

    def test_analyze_populates_commits(self, engine):
        engine.analyze()
        latest = engine.storage.get_latest_commit_date()
        assert latest is not None

    def test_force_analyze(self, engine):
        engine.analyze()
        stats = engine.analyze(force=True)
        assert stats["code_units_found"] > 0

    def test_analyze_creates_churn_stats(self, engine):
        engine.analyze()
        stat = engine.storage.get_churn_stat("app.py")
        assert stat is not None
        assert stat["commit_count"] >= 1

    def test_analyze_creates_unit_level_churn(self, engine):
        engine.analyze()
        # process_data is a function in app.py, should have unit-level churn
        stat = engine.storage.get_churn_stat("app.py", "process_data")
        assert stat is not None
        assert stat["commit_count"] >= 1

    def test_analyze_creates_blame(self, engine):
        engine.analyze()
        file_hash = engine.storage.get_file_hash("app.py")
        blame = engine.storage.get_blame("app.py", file_hash)
        assert len(blame) > 0


class TestUpdate:
    def test_incremental_update(self, engine, git_project):
        engine.analyze()

        # Modify a file
        src = git_project / "app.py"
        content = src.read_text()
        src.write_text(content + "\ndef new_func():\n    pass\n")
        _run_git(
            git_project, "add", "-A",
        )
        _run_git(
            git_project, "commit", "-m", "Add new_func",
            env_extra={
                "GIT_AUTHOR_DATE": "2026-03-01T10:00:00+00:00",
                "GIT_COMMITTER_DATE": "2026-03-01T10:00:00+00:00",
            },
        )

        stats = engine.update()
        assert stats["files_updated"] >= 1
        assert stats["new_commits"] >= 1

        # Verify new code unit exists
        units = engine.storage.get_code_units_by_file("app.py")
        names = [u["name"] for u in units]
        assert "new_func" in names


class TestToolMethods:
    def test_tool_analyze(self, engine):
        result = engine.tool_analyze()
        assert isinstance(result, dict)

    def test_tool_impact(self, engine):
        engine.analyze()
        result = engine.tool_impact(["app.py"])
        assert isinstance(result, list)

    def test_tool_suggest_tests(self, engine):
        engine.analyze()
        result = engine.tool_suggest_tests("app.py")
        assert isinstance(result, list)

    def test_tool_churn(self, engine):
        engine.analyze()
        result = engine.tool_churn("app.py")
        assert result is not None

    def test_tool_ownership(self, engine):
        engine.analyze()
        result = engine.tool_ownership("app.py")
        assert isinstance(result, list)

    def test_tool_coupling(self, engine):
        engine.analyze()
        result = engine.tool_coupling("app.py")
        assert isinstance(result, list)

    def test_tool_risk_map(self, engine):
        engine.analyze()
        result = engine.tool_risk_map()
        assert isinstance(result, list)

    def test_tool_stale_tests(self, engine):
        engine.analyze()
        result = engine.tool_stale_tests()
        assert isinstance(result, list)

    def test_tool_history(self, engine):
        engine.analyze()
        result = engine.tool_history("app.py")
        assert isinstance(result, list)
        assert len(result) >= 1

    def test_tool_who_reviews(self, engine):
        engine.analyze()
        result = engine.tool_who_reviews("app.py")
        assert isinstance(result, list)
