"""Shared fixtures for Chisel test suite."""

import os
import subprocess

import pytest

from chisel.storage import Storage


def _run_git_cmd(repo_dir, *args, env_extra=None):
    """Run a git command inside repo_dir with optional env overrides.

    Sets deterministic default dates for reproducible commits.
    Returns stdout. Raises RuntimeError on failure.
    """
    env = os.environ.copy()
    env.update({
        "GIT_AUTHOR_DATE": "2026-01-15T10:00:00+00:00",
        "GIT_COMMITTER_DATE": "2026-01-15T10:00:00+00:00",
    })
    if env_extra:
        env.update(env_extra)
    result = subprocess.run(
        ["git"] + list(args),
        cwd=str(repo_dir),
        capture_output=True,
        text=True,
        env=env,
    )
    if result.returncode != 0:
        raise RuntimeError(f"git {' '.join(args)} failed: {result.stderr}")
    return result.stdout


@pytest.fixture
def run_git():
    """Provide the shared git command runner as a callable fixture."""
    return _run_git_cmd


@pytest.fixture
def git_project(tmp_path):
    """Create a temp git repo with source + test files and two commits.

    Used by test_engine and test_mcp_server integration tests.

    Layout after fixture:
        myproject/
            app.py          — process_data, validate_input, format_output
            tests/
                test_app.py — test_process_data, test_validate_input

    Commit 1 (2026-01-15): Initial — app.py with two functions, test file
    Commit 2 (2026-02-01): Add format_output to app.py
    """
    project = tmp_path / "myproject"
    project.mkdir()

    # Init git repo
    _run_git_cmd(project, "init")
    _run_git_cmd(project, "config", "user.name", "TestUser")
    _run_git_cmd(project, "config", "user.email", "test@example.com")

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
    _run_git_cmd(project, "add", "-A")
    _run_git_cmd(project, "commit", "-m", "Initial commit")

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
    _run_git_cmd(project, "add", "-A")
    _run_git_cmd(
        project, "commit", "-m", "Add format_output",
        env_extra={
            "GIT_AUTHOR_DATE": "2026-02-01T10:00:00+00:00",
            "GIT_COMMITTER_DATE": "2026-02-01T10:00:00+00:00",
        },
    )

    return project


@pytest.fixture
def storage(tmp_path):
    """Provide a fresh Storage instance backed by a temporary directory."""
    s = Storage(base_dir=tmp_path / "chisel_data")
    yield s
    s.close()
