"""Tests for chisel.project — multi-agent safety utilities."""

import os
import subprocess

import pytest

from chisel.project import (
    ProcessLock,
    detect_project_root,
    normalize_path,
    resolve_storage_dir,
)


# ------------------------------------------------------------------ #
# detect_project_root
# ------------------------------------------------------------------ #

class TestDetectProjectRoot:
    def test_finds_current_repo(self):
        """Running inside the Chisel repo should find its root."""
        root = detect_project_root()
        assert os.path.isdir(root)
        assert os.path.isfile(os.path.join(root, "pyproject.toml"))

    def test_finds_root_from_subdirectory(self, tmp_path):
        """Should walk up from a subdir to find the git root."""
        repo = tmp_path / "repo"
        repo.mkdir()
        subprocess.run(["git", "init"], cwd=str(repo), capture_output=True)
        subdir = repo / "a" / "b" / "c"
        subdir.mkdir(parents=True)
        root = detect_project_root(str(subdir))
        assert os.path.samefile(root, str(repo))

    def test_non_git_dir_returns_start(self, tmp_path):
        """A dir with no .git should return itself."""
        plain = tmp_path / "plain"
        plain.mkdir()
        root = detect_project_root(str(plain))
        assert os.path.samefile(root, str(plain))

    def test_defaults_to_cwd(self):
        """With no args, should use cwd."""
        root = detect_project_root()
        assert os.path.isabs(root)

    def test_returns_absolute_path(self, tmp_path):
        repo = tmp_path / "repo"
        repo.mkdir()
        subprocess.run(["git", "init"], cwd=str(repo), capture_output=True)
        root = detect_project_root(str(repo))
        assert os.path.isabs(root)


# ------------------------------------------------------------------ #
# normalize_path
# ------------------------------------------------------------------ #

class TestNormalizePath:
    def test_absolute_path_becomes_relative(self):
        root = "/home/user/project"
        result = normalize_path("/home/user/project/src/main.py", root)
        assert result == "src/main.py"

    def test_relative_path_unchanged(self):
        result = normalize_path("src/main.py", "/any/root")
        assert result == "src/main.py"

    def test_dot_slash_stripped(self):
        result = normalize_path("./src/main.py", "/any/root")
        assert result == "src/main.py"

    def test_parent_refs_normalized(self):
        result = normalize_path("src/../lib/utils.py", "/any/root")
        assert result == "lib/utils.py"

    def test_already_clean_path_unchanged(self):
        result = normalize_path("chisel/engine.py", "/any/root")
        assert result == "chisel/engine.py"

    def test_plain_filename(self):
        result = normalize_path("README.md", "/any/root")
        assert result == "README.md"


# ------------------------------------------------------------------ #
# resolve_storage_dir
# ------------------------------------------------------------------ #

class TestResolveStorageDir:
    def test_explicit_dir_wins(self, tmp_path):
        explicit = str(tmp_path / "custom")
        result = resolve_storage_dir(project_dir="/any", explicit_dir=explicit)
        assert result == explicit

    def test_env_var_second_priority(self, tmp_path, monkeypatch):
        env_dir = str(tmp_path / "env_storage")
        monkeypatch.setenv("CHISEL_STORAGE_DIR", env_dir)
        result = resolve_storage_dir(project_dir="/any")
        assert result == env_dir

    def test_project_local_third_priority(self, tmp_path, monkeypatch):
        monkeypatch.delenv("CHISEL_STORAGE_DIR", raising=False)
        repo = tmp_path / "repo"
        repo.mkdir()
        subprocess.run(["git", "init"], cwd=str(repo), capture_output=True)
        result = resolve_storage_dir(project_dir=str(repo))
        assert result.endswith(".chisel")
        # Should be under the repo root
        assert str(repo) in result

    def test_fallback_to_home(self, monkeypatch):
        monkeypatch.delenv("CHISEL_STORAGE_DIR", raising=False)
        result = resolve_storage_dir(project_dir=None)
        assert result == os.path.join(os.path.expanduser("~"), ".chisel")

    def test_explicit_overrides_env(self, tmp_path, monkeypatch):
        monkeypatch.setenv("CHISEL_STORAGE_DIR", "/should/be/ignored")
        explicit = str(tmp_path / "winner")
        result = resolve_storage_dir(project_dir="/any", explicit_dir=explicit)
        assert result == explicit


# ------------------------------------------------------------------ #
# ProcessLock
# ------------------------------------------------------------------ #

class TestProcessLock:
    def test_exclusive_lock_context(self, tmp_path):
        lock = ProcessLock(str(tmp_path))
        with lock.exclusive():
            # Lock file should exist during the context
            assert os.path.isfile(os.path.join(str(tmp_path), "chisel.lock"))

    def test_shared_lock_context(self, tmp_path):
        lock = ProcessLock(str(tmp_path))
        with lock.shared():
            assert os.path.isfile(os.path.join(str(tmp_path), "chisel.lock"))

    def test_multiple_shared_locks(self, tmp_path):
        """Multiple shared locks should not block each other."""
        lock = ProcessLock(str(tmp_path))
        with lock.shared():
            with lock.shared():
                pass  # Should not deadlock

    def test_lock_creates_directory(self, tmp_path):
        lock_dir = str(tmp_path / "nested" / "lock" / "dir")
        lock = ProcessLock(lock_dir)
        assert os.path.isdir(lock_dir)
        with lock.exclusive():
            pass

    def test_lock_releases_on_exception(self, tmp_path):
        lock = ProcessLock(str(tmp_path))
        with pytest.raises(ValueError):
            with lock.exclusive():
                raise ValueError("test")
        # Should be able to acquire again
        with lock.exclusive():
            pass
