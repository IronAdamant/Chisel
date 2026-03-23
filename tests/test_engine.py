"""Tests for chisel.engine — integration + unit tests for private methods."""

import os
from unittest.mock import MagicMock, patch

import pytest

from chisel.engine import ChiselEngine, _NO_DATA_RESPONSE


@pytest.fixture
def engine(git_project, tmp_path):
    storage_dir = tmp_path / "chisel_storage"
    eng = ChiselEngine(str(git_project), storage_dir=storage_dir)
    yield eng
    eng.close()


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
        stats = engine.storage.get_stats()
        assert stats["commits"] > 0

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
    def test_incremental_update(self, engine, git_project, run_git):
        engine.analyze()

        # Modify a file
        src = git_project / "app.py"
        content = src.read_text()
        src.write_text(content + "\ndef new_func():\n    pass\n")
        run_git(
            git_project, "add", "-A",
        )
        run_git(
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

    def test_tool_update(self, engine):
        engine.analyze()
        result = engine.tool_update()
        assert isinstance(result, dict)
        assert "files_updated" in result
        assert "new_commits" in result

    def test_tool_test_gaps(self, engine):
        engine.analyze()
        result = engine.tool_test_gaps()
        assert isinstance(result, list)
        # format_output has no test coverage in the fixture
        names = [item["name"] for item in result]
        assert "format_output" in names

    def test_tool_test_gaps_scoped_by_file(self, engine):
        engine.analyze()
        result = engine.tool_test_gaps(file_path="app.py")
        assert isinstance(result, list)
        # All results should be from app.py
        for item in result:
            assert item["file_path"] == "app.py"

    def test_tool_diff_impact_no_changes(self, engine):
        engine.analyze()
        # After analyze with clean working tree, no diff changes
        result = engine.tool_diff_impact()
        assert isinstance(result, list)

    def test_tool_diff_impact_with_changes(self, engine, git_project):
        engine.analyze()
        # Create unstaged changes
        src = git_project / "app.py"
        content = src.read_text()
        src.write_text(content + "\ndef brand_new(): pass\n")
        result = engine.tool_diff_impact()
        assert isinstance(result, list)
        # Should find tests impacted by app.py changes
        if result:
            assert all("test_id" in item for item in result)

    def test_tool_record_result(self, engine):
        engine.analyze()
        all_tests = engine.storage.get_all_test_units()
        assert len(all_tests) > 0
        test_id = all_tests[0]["id"]
        result = engine.tool_record_result(test_id, passed=True, duration_ms=150)
        assert isinstance(result, dict)
        assert result["recorded"] is True
        assert result["passed"] is True
        assert result["test_id"] == test_id

    def test_tool_stats(self, engine):
        engine.analyze()
        result = engine.tool_stats()
        assert isinstance(result, dict)
        expected_keys = {
            "code_units", "test_units", "test_edges", "commits",
            "commit_files", "blame_cache", "co_changes", "churn_stats",
            "file_hashes", "test_results",
        }
        assert set(result.keys()) == expected_keys
        for key in expected_keys:
            assert isinstance(result[key], int)
            assert result[key] >= 0
        assert result["code_units"] > 0


# ------------------------------------------------------------------ #
# Empty-state detection (no analyze run)
# ------------------------------------------------------------------ #


class TestEmptyStateDetection:
    """Query tools should return a structured warning when DB has no analysis data."""

    def test_query_tools_return_no_data_on_empty_db(self, engine):
        """All read-only query tools return the no-data dict before analyze."""
        tools_with_args = [
            ("tool_impact", {"files": ["app.py"]}),
            ("tool_suggest_tests", {"file_path": "app.py"}),
            ("tool_churn", {"file_path": "app.py"}),
            ("tool_ownership", {"file_path": "app.py"}),
            ("tool_coupling", {"file_path": "app.py"}),
            ("tool_risk_map", {}),
            ("tool_stale_tests", {}),
            ("tool_history", {"file_path": "app.py"}),
            ("tool_who_reviews", {"file_path": "app.py"}),
            ("tool_test_gaps", {}),
            ("tool_diff_impact", {}),
        ]
        for method_name, kwargs in tools_with_args:
            result = getattr(engine, method_name)(**kwargs)
            assert result["status"] == "no_data", f"{method_name} did not return no_data"
            assert "hint" in result, f"{method_name} missing hint"

    def test_no_data_response_after_analyze(self, engine):
        """After analyze, query tools should NOT return the no-data dict."""
        engine.analyze()
        result = engine.tool_risk_map()
        assert isinstance(result, list)

    def test_stats_hint_on_empty_db(self, engine):
        """tool_stats should include a hint when all counts are zero."""
        result = engine.tool_stats()
        assert "hint" in result
        assert "analyze" in result["hint"]

    def test_stats_no_hint_after_analyze(self, engine):
        """tool_stats should NOT include a hint after analyze populates data."""
        engine.analyze()
        result = engine.tool_stats()
        assert "hint" not in result

    def test_write_tools_unaffected(self, engine):
        """Write tools (analyze, update, record_result) are not gated."""
        result = engine.tool_analyze()
        assert isinstance(result, dict)
        assert "code_files_scanned" in result

    def test_no_data_response_shape(self, engine):
        """The no-data response has the expected keys."""
        result = engine.tool_risk_map()
        assert set(result.keys()) == {"status", "message", "hint"}
        assert result == _NO_DATA_RESPONSE


# ------------------------------------------------------------------ #
# Unit tests for private methods
# ------------------------------------------------------------------ #


class TestScanCodeFiles:
    def test_finds_py_files(self, engine, git_project):
        files = engine._scan_code_files()
        basenames = [os.path.basename(f) for f in files]
        assert "app.py" in basenames

    def test_returns_sorted(self, engine):
        files = engine._scan_code_files()
        assert files == sorted(files)

    def test_skips_pycache(self, engine, git_project):
        cache_dir = git_project / "__pycache__"
        cache_dir.mkdir()
        (cache_dir / "cached.py").write_text("x = 1\n")
        files = engine._scan_code_files()
        assert not any("__pycache__" in f for f in files)

    def test_skips_non_code_extensions(self, engine, git_project):
        (git_project / "notes.md").write_text("# notes")
        (git_project / "data.json").write_text("{}")
        files = engine._scan_code_files()
        exts = {os.path.splitext(f)[1] for f in files}
        assert ".md" not in exts
        assert ".json" not in exts

    def test_directory_scopes_scan(self, engine, git_project):
        sub = git_project / "subpkg"
        sub.mkdir()
        (sub / "mod.py").write_text("def f(): pass\n")
        all_files = engine._scan_code_files()
        scoped = engine._scan_code_files(directory="subpkg")
        assert len(scoped) < len(all_files)
        assert any("mod.py" in f for f in scoped)
        assert not any("app.py" in f for f in scoped)

    def test_nonexistent_directory_falls_back(self, engine):
        """Non-existent directory falls back to project root."""
        fallback = engine._scan_code_files(directory="no_such_dir")
        default = engine._scan_code_files()
        assert fallback == default


class TestFindChangedFiles:
    def test_all_files_on_first_run(self, engine):
        code_files = engine._scan_code_files()
        changed = engine._find_changed_files(code_files)
        # First run: no hashes stored, everything is "changed"
        assert len(changed) == len(code_files)
        # Each entry is (abs_path, rel_path, hash)
        _, rel, h = changed[0]
        assert isinstance(rel, str)
        assert len(h) == 64  # SHA-256 hex

    def test_nothing_changed_after_store(self, engine):
        code_files = engine._scan_code_files()
        changed = engine._find_changed_files(code_files)
        # Store the hashes
        for _, rel, new_hash in changed:
            engine.storage.set_file_hash(rel, new_hash)
        # Second run: nothing changed
        changed2 = engine._find_changed_files(code_files)
        assert changed2 == []

    def test_detects_modification(self, engine, git_project):
        code_files = engine._scan_code_files()
        changed = engine._find_changed_files(code_files)
        for _, rel, h in changed:
            engine.storage.set_file_hash(rel, h)

        # Modify one file
        (git_project / "app.py").write_text("def changed(): pass\n")
        changed2 = engine._find_changed_files(code_files)
        rels = [rel for _, rel, _ in changed2]
        assert "app.py" in rels
        assert len(changed2) == 1

    def test_force_returns_all(self, engine):
        code_files = engine._scan_code_files()
        # Store hashes first
        changed = engine._find_changed_files(code_files)
        for _, rel, h in changed:
            engine.storage.set_file_hash(rel, h)
        # Force: returns all even though nothing changed
        forced = engine._find_changed_files(code_files, force=True)
        assert len(forced) == len(code_files)


class TestParseAndStoreCodeUnits:
    def test_stores_units_and_returns_count(self, engine):
        code_files = engine._scan_code_files()
        changed = engine._find_changed_files(code_files)
        count = engine._parse_and_store_code_units(changed)
        assert count > 0
        # Verify units exist in DB
        units = engine.storage.get_code_units_by_file("app.py")
        assert len(units) > 0

    def test_updates_file_hashes(self, engine):
        code_files = engine._scan_code_files()
        changed = engine._find_changed_files(code_files)
        engine._parse_and_store_code_units(changed)
        h = engine.storage.get_file_hash("app.py")
        assert h is not None and len(h) == 64

    def test_replaces_old_units_on_reparse(self, engine, git_project):
        code_files = engine._scan_code_files()
        changed = engine._find_changed_files(code_files)
        engine._parse_and_store_code_units(changed)
        old_units = engine.storage.get_code_units_by_file("app.py")

        # Modify and reparse
        (git_project / "app.py").write_text("def only_one(): pass\n")
        changed2 = engine._find_changed_files(code_files)
        engine._parse_and_store_code_units(changed2)
        new_units = engine.storage.get_code_units_by_file("app.py")
        names = [u["name"] for u in new_units]
        assert names == ["only_one"]
        assert len(new_units) < len(old_units)

    def test_empty_changed_files(self, engine):
        count = engine._parse_and_store_code_units([])
        assert count == 0


class TestStoreCommits:
    def test_stores_commits_and_files(self, engine):
        commits = [{
            "hash": "abc123", "author": "A", "author_email": "a@x.com",
            "date": "2026-01-15", "message": "init",
            "files": [{"path": "app.py", "insertions": 10, "deletions": 0}],
        }]
        engine._store_commits(commits)
        stored = engine.storage.get_commit("abc123")
        assert stored is not None
        assert stored["author"] == "A"
        # Commit file entry
        file_commits = engine.storage.get_commits_for_file("app.py")
        assert any(c["hash"] == "abc123" for c in file_commits)

    def test_handles_no_files(self, engine):
        commits = [{
            "hash": "def456", "author": "B", "author_email": "b@x.com",
            "date": "2026-02-01", "message": "empty",
        }]
        engine._store_commits(commits)
        assert engine.storage.get_commit("def456") is not None

    def test_idempotent_upsert(self, engine):
        commit = {
            "hash": "aaa111", "author": "C", "author_email": "c@x.com",
            "date": "2026-03-01", "message": "first", "files": [],
        }
        engine._store_commits([commit])
        commit["message"] = "updated"
        engine._store_commits([commit])
        stored = engine.storage.get_commit("aaa111")
        assert stored["message"] == "updated"


class TestStoreBlame:
    def test_stores_blame_blocks(self, engine, git_project, run_git):
        engine.analyze()
        h = engine.storage.get_file_hash("app.py")
        blame = engine.storage.get_blame("app.py", h)
        assert len(blame) > 0
        assert blame[0]["author"] == "TestUser"

    def test_invalidates_old_blame(self, engine, git_project, run_git):
        engine.analyze()
        h1 = engine.storage.get_file_hash("app.py")
        engine.storage.get_blame("app.py", h1)  # verify blame exists before invalidation

        # Modify, recommit, re-run blame
        src = git_project / "app.py"
        src.write_text("def new_only(): pass\n")
        run_git(git_project, "add", "-A")
        run_git(git_project, "commit", "-m", "replace")
        code_files = engine._scan_code_files()
        changed = engine._find_changed_files(code_files)
        engine._parse_and_store_code_units(changed)
        engine._store_blame(changed)
        h2 = engine.storage.get_file_hash("app.py")
        # Old hash blame should be gone
        assert engine.storage.get_blame("app.py", h1) == []
        # New hash blame should exist
        assert len(engine.storage.get_blame("app.py", h2)) > 0

    def test_handles_git_error_gracefully(self, engine, tmp_path):
        """Blame for a file not in git doesn't crash."""
        fake = tmp_path / "nocommit.py"
        fake.write_text("x = 1\n")
        from chisel.ast_utils import compute_file_hash
        h = compute_file_hash(str(fake))
        # This should not raise — RuntimeError is caught internally
        engine._store_blame([(str(fake), "nocommit.py", h)])


class TestComputeChurnAndCoupling:
    def test_stores_file_level_churn(self, engine, git_project):
        engine.analyze()
        stat = engine.storage.get_churn_stat("app.py")
        assert stat is not None
        assert stat["commit_count"] >= 1
        assert stat["churn_score"] > 0

    def test_stores_unit_level_churn(self, engine, git_project):
        engine.analyze()
        stat = engine.storage.get_churn_stat("app.py", "process_data")
        assert stat is not None
        assert stat["commit_count"] >= 1

    def test_skips_classes_for_unit_churn(self, engine, git_project):
        """Unit-level churn only runs for functions, not classes."""
        src = git_project / "app.py"
        content = src.read_text()
        src.write_text(content + "\nclass MyClass:\n    def method(self): pass\n")
        engine.analyze(force=True)
        # Class should not get its own churn stat
        assert engine.storage.get_churn_stat("app.py", "MyClass") is None


class TestDiscoverAndBuildEdges:
    def test_returns_correct_tuple(self, engine, git_project):
        engine.analyze()
        code_files = engine._scan_code_files()
        # Run again to test in isolation
        units, tf_count, edge_count = engine._discover_and_build_edges(code_files)
        assert isinstance(units, list)
        assert tf_count >= 1
        assert edge_count >= 1

    def test_stores_test_units(self, engine, git_project):
        engine.analyze()
        tests = engine.storage.get_all_test_units()
        names = [t["name"] for t in tests]
        assert "test_process_data" in names

    def test_stores_edges(self, engine, git_project):
        engine.analyze()
        tests = engine.storage.get_all_test_units()
        test_with_edges = [
            t for t in tests if t["name"] == "test_process_data"
        ]
        assert len(test_with_edges) == 1
        edges = engine.storage.get_edges_for_test(test_with_edges[0]["id"])
        assert len(edges) > 0

    def test_replaces_units_on_rediscovery(self, engine, git_project, run_git):
        """Modifying a test file replaces its old test units."""
        engine.analyze()
        old_names = [t["name"] for t in engine.storage.get_all_test_units()]
        assert "test_process_data" in old_names

        # Rewrite the test file with different functions
        (git_project / "tests" / "test_app.py").write_text(
            "def test_replacement(): pass\n"
        )
        run_git(git_project, "add", "-A")
        run_git(git_project, "commit", "-m", "rewrite tests")

        code_files = engine._scan_code_files()
        engine._discover_and_build_edges(code_files)
        new_names = [t["name"] for t in engine.storage.get_all_test_units()]
        assert "test_replacement" in new_names
        # Old test units for test_app.py were deleted and replaced
        assert "test_process_data" not in new_names


# ------------------------------------------------------------------ #
# Process lock wiring on tool methods
# ------------------------------------------------------------------ #

class TestProcessLockUsage:
    def test_read_tool_acquires_shared_lock(self, engine):
        """Read-only tool methods should acquire the shared process lock."""
        engine.analyze()
        mock_ctx = MagicMock()
        mock_ctx.__enter__ = MagicMock(return_value=None)
        mock_ctx.__exit__ = MagicMock(return_value=False)
        with patch.object(engine._process_lock, "shared", return_value=mock_ctx) as mock_shared:
            engine.tool_stats()
            mock_shared.assert_called_once()
            mock_ctx.__enter__.assert_called_once()

    def test_record_result_acquires_exclusive_lock(self, engine):
        """tool_record_result should acquire the exclusive process lock."""
        engine.analyze()
        # Insert a test unit so record_result has a valid target
        all_tests = engine.storage.get_all_test_units()
        assert len(all_tests) > 0
        test_id = all_tests[0]["id"]

        mock_ctx = MagicMock()
        mock_ctx.__enter__ = MagicMock(return_value=None)
        mock_ctx.__exit__ = MagicMock(return_value=False)
        with patch.object(engine._process_lock, "exclusive", return_value=mock_ctx) as mock_excl:
            engine.tool_record_result(test_id, passed=True, duration_ms=100)
            mock_excl.assert_called_once()
            mock_ctx.__enter__.assert_called_once()
