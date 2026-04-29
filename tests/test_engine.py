"""Tests for chisel.engine — integration + unit tests for private methods."""

import os
import time
from unittest.mock import MagicMock, patch

import pytest

from chisel.engine import ChiselEngine, _NO_DATA_RESPONSE, _coupling_threshold, _test_to_source_stem


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

    def test_incremental_import_edges_preserve_unchanged_files(self, engine, git_project):
        """Update with no changes should not wipe existing import edges."""
        helper = git_project / "helper.py"
        helper.write_text("def helper_func():\n    return 1\n")
        consumer = git_project / "consumer.py"
        consumer.write_text("from helper import helper_func\n\ndef use():\n    return helper_func()\n")

        engine.analyze()
        before = engine.storage.get_imported_files("consumer.py")
        assert "helper.py" in before

        stats = engine.update()
        assert stats["files_updated"] == 0

        after = engine.storage.get_imported_files("consumer.py")
        assert "helper.py" in after

    def test_incremental_import_edges_update_on_change(self, engine, git_project):
        """Changing a source file updates its import edges without wiping others."""
        helper = git_project / "helper.py"
        helper.write_text("def helper_func():\n    return 1\n")
        other = git_project / "other.py"
        other.write_text("def other_func():\n    return 2\n")
        consumer = git_project / "consumer.py"
        consumer.write_text("from helper import helper_func\n\ndef use():\n    return helper_func()\n")

        engine.analyze()
        assert "helper.py" in engine.storage.get_imported_files("consumer.py")

        # Now change consumer to import other instead
        consumer.write_text("from other import other_func\n\ndef use():\n    return other_func()\n")
        engine.update()

        edges = engine.storage.get_imported_files("consumer.py")
        assert "other.py" in edges
        assert "helper.py" not in edges


class TestToolMethods:
    def test_tool_analyze(self, engine):
        result = engine.tool_analyze()
        assert isinstance(result, dict)

    def test_tool_analyze_auto_fallback_large_repo(self, engine, git_project):
        """force=True on a repo with >300 code files auto-queues a background job."""
        engine.analyze()
        # Create enough .py files to cross the threshold
        for i in range(305):
            (git_project / f"dummy_{i}.py").write_text(f"def dummy_{i}(): pass\n")
        orig_threshold = engine._AUTO_BG_JOB_THRESHOLD
        try:
            engine._AUTO_BG_JOB_THRESHOLD = 300
            result = engine.tool_analyze(force=True)
            assert result.get("status") == "auto_queued"
            assert "job_id" in result
            assert result.get("kind") == "analyze"
        finally:
            engine._AUTO_BG_JOB_THRESHOLD = orig_threshold

    def test_tool_impact(self, engine):
        engine.analyze()
        result = engine.tool_impact(["app.py"])
        assert isinstance(result, list)

    def test_tool_suggest_tests(self, engine):
        engine.analyze()
        result = engine.tool_suggest_tests("app.py")
        assert isinstance(result, list)

    def test_tool_suggest_tests_directory_mode(self, engine, git_project):
        engine.analyze()
        # Add a second source file with a test
        src_dir = git_project / "src"
        src_dir.mkdir()
        (src_dir / "mod.py").write_text("def mod_func():\n    return 1\n")
        (git_project / "tests" / "test_mod.py").write_text(
            "from src.mod import mod_func\n\ndef test_mod_func():\n    assert mod_func() == 1\n"
        )
        engine.update()
        result = engine.tool_suggest_tests(directory="src")
        assert isinstance(result, dict)
        assert "src/mod.py" in result
        assert any("test_mod_func" in s["name"] for s in result["src/mod.py"])

    def test_tool_suggest_tests_includes_hybrid_when_static_and_db_agree(self, engine):
        engine.analyze()
        result = engine.tool_suggest_tests("app.py")
        assert any(r.get("source") == "hybrid" for r in result)

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
        assert isinstance(result, dict)
        assert "co_change_partners" in result
        assert "import_partners" in result
        assert "import_coupling" in result
        assert "effective_coupling" in result
        assert "import_breadth" in result

    def test_tool_risk_map(self, engine):
        engine.analyze()
        result = engine.tool_risk_map()
        assert isinstance(result, dict)
        assert "files" in result
        assert "_meta" in result
        assert isinstance(result["files"], list)
        assert len(result["files"]) > 0

    def test_tool_risk_map_excludes_test_files(self, engine):
        engine.analyze()
        result = engine.tool_risk_map()
        files = [r["file_path"] for r in result["files"]]
        # test_app.py should be excluded by default
        assert not any("test_" in f for f in files)
        # Source file should be present
        assert "app.py" in files

    def test_tool_risk_map_include_tests(self, engine):
        engine.analyze()
        result = engine.tool_risk_map(exclude_tests=False)
        files = [r["file_path"] for r in result["files"]]
        assert any("test_" in f for f in files)

    def test_tool_risk_map_coverage_gap_with_edges(self, engine):
        """coverage_gap should be < 1.0 for files with test edges."""
        engine.analyze()
        result = engine.tool_risk_map()
        app = next(r for r in result["files"] if r["file_path"] == "app.py")
        # app.py has 3 functions, 2 are tested (process_data, validate_input)
        # In default line mode: ~tested_lines / ~total_lines ≈ 0.75, gap = 0.25
        assert app["breakdown"]["coverage_gap"] < 1.0
        assert app["breakdown"]["coverage_gap"] == 0.25

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
        # After analyze with clean working tree, returns diagnostic dict
        result = engine.tool_diff_impact()
        assert isinstance(result, dict)
        assert result["status"] == "no_changes"
        assert "ref" in result
        assert "message" in result

    def test_tool_start_job_update_completes(self, engine):
        engine.analyze()
        start = engine.tool_start_job("update")
        assert start.get("job_id")
        assert start.get("status") == "running"
        job_id = start["job_id"]
        for _ in range(200):
            st = engine.tool_job_status(job_id)
            assert st.get("job_id") == job_id
            if st.get("status") == "completed":
                assert "result" in st
                assert "files_updated" in st["result"]
                return
            if st.get("status") == "failed":
                raise AssertionError(st.get("error"))
            time.sleep(0.02)
        raise AssertionError("background job did not complete")

    def test_tool_diff_impact_git_error_when_not_git_repo(self, tmp_path):
        proj = tmp_path / "nogit"
        proj.mkdir()
        (proj / "app.py").write_text("def x():\n    pass\n")
        storage_dir = tmp_path / "chisel_st"
        with ChiselEngine(str(proj), storage_dir=storage_dir) as engine:
            engine.analyze(force=True)
            result = engine.tool_diff_impact()
        assert isinstance(result, dict)
        assert result["status"] == "git_error"
        assert result.get("error") == "not_a_git_repo"
        assert result.get("cwd") == str(proj)
        assert "project_dir" in result
        assert "git" in result["message"].lower() or "failed" in result["message"].lower()

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

    def test_tool_diff_impact_includes_untracked_code(self, engine, git_project):
        engine.analyze()
        (git_project / "addon.py").write_text("def addon():\n    return 42\n")
        (git_project / "tests" / "test_addon.py").write_text(
            "from addon import addon\n\n"
            "def test_addon():\n"
            "    assert addon() == 42\n"
        )
        engine.analyze()
        result = engine.tool_diff_impact()
        assert isinstance(result, list)
        assert len(result) >= 1
        test_ids = {item["test_id"] for item in result}
        assert any("test_addon" in tid for tid in test_ids)

    def test_tool_diff_impact_auto_update_clears_stale_db(self, engine, git_project, run_git):
        engine.analyze()
        # Add a new tracked file without re-analyzing
        (git_project / "new_module.py").write_text("def new_func(): pass\n")
        # Stage it so git sees it as a changed file
        run_git(git_project, "add", "new_module.py")
        # Without auto_update, this would be stale_db because new_module.py is missing from DB
        result = engine.tool_diff_impact(auto_update=True)
        # After auto-update, it should return a normal result (list or no_changes)
        assert isinstance(result, (list, dict))
        if isinstance(result, dict):
            assert result.get("status") != "stale_db"

    def test_tool_suggest_tests_auto_update_clears_stale_db(self, engine, git_project, run_git):
        engine.analyze()
        # Add a new file without re-analyzing
        (git_project / "new_module.py").write_text("def new_func(): pass\n")
        run_git(git_project, "add", "new_module.py")
        # Without auto_update this returns stale_db
        stale = engine.tool_suggest_tests("new_module.py")
        assert isinstance(stale, dict) and stale.get("status") == "stale_db"
        # With auto_update it should refresh and return a list (possibly empty)
        result = engine.tool_suggest_tests("new_module.py", auto_update=True)
        assert isinstance(result, list)

    def test_tool_risk_map_auto_update_meta(self, engine):
        engine.analyze()
        result = engine.tool_risk_map(auto_update=True)
        assert "_meta" in result
        assert "auto_update_performed" in result["_meta"]
        # In a clean repo with no changes, auto_update should be skipped with reason "no_changes"
        assert result["_meta"]["auto_update_performed"] is False
        assert result["_meta"]["auto_update_skip_reason"] == "no_changes"

    def test_tool_triage_auto_update_summary(self, engine):
        engine.analyze()
        result = engine.tool_triage(auto_update=True)
        assert "summary" in result
        assert "auto_update_performed" in result["summary"]
        assert result["summary"]["auto_update_performed"] is False
        assert result["summary"]["auto_update_skip_reason"] == "no_changes"

    def test_tool_auto_update_skipped_when_bg_job_running(self, engine):
        engine.analyze()
        # Fake a background job in progress
        with engine._bg_jobs_lock:
            engine._bg_job_in_progress = True
        try:
            result = engine.tool_risk_map(auto_update=True)
            assert result["_meta"]["auto_update_performed"] is False
            assert result["_meta"]["auto_update_skip_reason"] == "background_job_running"
        finally:
            with engine._bg_jobs_lock:
                engine._bg_job_in_progress = False

    def test_tool_auto_update_skipped_when_too_many_files(self, engine, git_project, run_git):
        engine.analyze()
        # Create many changed files to exceed the cap
        for i in range(60):
            (git_project / f"mod_{i}.py").write_text(f"def mod_{i}(): pass\n")
            run_git(git_project, "add", f"mod_{i}.py")
        result = engine.tool_risk_map(auto_update=True)
        assert result["_meta"]["auto_update_performed"] is False
        assert result["_meta"]["auto_update_skip_reason"] == "too_many_changed_files"

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

    def test_tool_triage(self, engine):
        engine.analyze()
        result = engine.tool_triage()
        assert isinstance(result, dict)
        assert "top_risk_files" in result
        assert "test_gaps" in result
        assert "stale_tests" in result
        assert "summary" in result
        assert result["summary"]["files_triaged"] > 0

    def test_tool_triage_with_top_n(self, engine):
        engine.analyze()
        result = engine.tool_triage(top_n=1)
        assert len(result["top_risk_files"]) <= 1

    def test_tool_triage_summary_has_data_quality(self, engine):
        engine.analyze()
        result = engine.tool_triage()
        summary = result["summary"]
        assert "test_edge_count" in summary
        assert "test_result_count" in summary
        assert "coupling_threshold" in summary

    def test_tool_risk_map_exclude_new_file_boost(self, engine):
        engine.analyze()
        result = engine.tool_risk_map(exclude_new_file_boost=True)
        assert isinstance(result, dict)
        assert "files" in result
        # No file should have a new_file_boost when excluded
        for f in result["files"]:
            assert f["breakdown"]["new_file_boost"] == 0.0

    def test_tool_triage_exclude_new_file_boost(self, engine):
        engine.analyze()
        result = engine.tool_triage(exclude_new_file_boost=True)
        assert isinstance(result, dict)
        for f in result["top_risk_files"]:
            assert f["breakdown"]["new_file_boost"] == 0.0

    def test_tool_risk_map_meta_structure(self, engine):
        engine.analyze()
        result = engine.tool_risk_map()
        meta = result["_meta"]
        assert "total_files" in meta
        assert "effective_components" in meta
        assert "uniform_components" in meta
        assert "coverage_gap_mode" in meta
        assert meta["coverage_gap_mode"] == "proximity_adjusted"
        assert isinstance(meta["effective_components"], list)
        assert isinstance(meta["uniform_components"], dict)
        # With the test fixture, some components should be effective
        assert meta["total_files"] > 0

    def test_tool_stats(self, engine):
        engine.analyze()
        result = engine.tool_stats()
        assert isinstance(result, dict)
        base_keys = {
            "code_units", "test_units", "test_edges", "commits",
            "commit_files", "blame_cache", "co_changes", "branch_co_changes",
            "import_edges", "churn_stats", "file_hashes", "test_results",
        }
        assert base_keys.issubset(set(result.keys()))
        for key in base_keys:
            assert isinstance(result[key], int)
            assert result[key] >= 0
        assert result["code_units"] > 0
        # coupling_threshold present when commits > 0
        if result["commits"] > 0:
            assert "coupling_threshold" in result
            import math
            expected = max(2, int(math.log2(result["commits"]) / 2) + 1)
            assert result["coupling_threshold"] == expected


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
            ("tool_triage", {}),
        ]
        for method_name, kwargs in tools_with_args:
            result = getattr(engine, method_name)(**kwargs)
            assert result["status"] == "no_data", f"{method_name} did not return no_data"
            assert "hint" in result, f"{method_name} missing hint"

    def test_no_data_response_after_analyze(self, engine):
        """After analyze, query tools should NOT return the no-data dict."""
        engine.analyze()
        result = engine.tool_risk_map()
        assert isinstance(result, dict)
        assert "files" in result  # dict envelope, not no-data

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


# ------------------------------------------------------------------ #
# Coupling threshold unit tests
# ------------------------------------------------------------------ #

class TestCouplingThreshold:
    def test_minimum_floor(self):
        assert _coupling_threshold(0) == 2
        assert _coupling_threshold(1) == 2
        assert _coupling_threshold(4) == 2

    def test_half_log_scaling(self):
        # Half-log: int(log2(N)/2) + 1, floor 2
        assert _coupling_threshold(10) == 2   # log2(10)/2=1.66 → 1+1=2
        assert _coupling_threshold(50) == 3   # log2(50)/2=2.82 → 2+1=3
        assert _coupling_threshold(200) == 4  # log2(200)/2=3.82 → 3+1=4
        assert _coupling_threshold(1000) == 5  # log2(1000)/2=4.98 → 4+1=5

    def test_large_repos_reasonable(self):
        # At 10k commits, threshold should be ~7, not 2500
        threshold = _coupling_threshold(10000)
        assert threshold <= 8
        assert threshold >= 5


# ------------------------------------------------------------------ #
# Stale-tests diagnostic when zero edges
# ------------------------------------------------------------------ #


class TestStaleTestsDiagnostic:
    def test_returns_diagnostic_when_no_edges(self, engine):
        """stale_tests returns status=no_edges when DB has 0 test edges."""
        engine.analyze()
        # Force-clear all test edges
        engine.storage._execute("DELETE FROM test_edges")
        result = engine.tool_stale_tests()
        assert isinstance(result, dict)
        assert result["status"] == "no_edges"
        assert "stale_tests" in result
        assert result["stale_tests"] == []

    def test_returns_list_when_edges_exist(self, engine):
        """stale_tests returns a normal list when edges are present."""
        engine.analyze()
        stats = engine.tool_stats()
        if stats.get("test_edges", 0) > 0:
            result = engine.tool_stale_tests()
            assert isinstance(result, list)


# ------------------------------------------------------------------ #
# record_result heuristic edge creation
# ------------------------------------------------------------------ #


class TestRecordResultHeuristicEdges:
    def test_creates_edges_from_filename_match(self, engine, git_project, run_git):
        """record_result creates heuristic edges when no edges exist."""
        engine.analyze()
        # Clear edges to simulate missing edge builder
        engine.storage._execute("DELETE FROM test_edges")

        # Record result for a test file
        test_units = engine.storage.get_test_units_by_file("tests/test_app.py")
        if not test_units:
            pytest.skip("no test units found")

        result = engine.tool_record_result(
            "tests/test_app.py", passed=True, duration_ms=100,
        )
        assert result["recorded"] is True
        # "app" stem should match "app.py" code units
        if result.get("heuristic_edges_created", 0) > 0:
            edges = engine.storage.get_edges_for_test(test_units[0]["id"])
            assert any(e["edge_type"] == "heuristic" for e in edges)

    def test_skips_when_edges_already_exist(self, engine):
        """record_result does NOT create heuristic edges if analyze built them."""
        engine.analyze()
        test_units = engine.storage.get_all_test_units()
        if not test_units:
            pytest.skip("no test units found")

        # Edges already exist from analyze
        edges_before = engine.storage.get_edges_for_test(test_units[0]["id"])
        result = engine.tool_record_result(
            test_units[0]["id"], passed=True, duration_ms=50,
        )
        assert result.get("heuristic_edges_created") is None
        # No extra edges added
        edges_after = engine.storage.get_edges_for_test(test_units[0]["id"])
        assert len(edges_after) == len(edges_before)


# ------------------------------------------------------------------ #
# Static require/import fallback (no DB test edges)
# ------------------------------------------------------------------ #


class TestStaticSuggestWhenEdgesCleared:
    """When test_edges are empty, suggest_tests / test_gaps use static imports."""

    def test_suggest_tests_and_gaps_use_static_requires(self, tmp_path, run_git):
        proj = tmp_path / "jsp"
        proj.mkdir()
        src = proj / "src"
        src.mkdir()
        tdir = proj / "tests"
        tdir.mkdir()
        (src / "widget.js").write_text("export const x = 1;\n")
        (tdir / "widget.test.js").write_text(
            "const w = require('../src/widget');\n"
            "describe('w', () => { it('a', () => {}); });\n"
        )
        run_git(proj, "init")
        run_git(proj, "config", "user.name", "T")
        run_git(proj, "config", "user.email", "t@e.org")
        run_git(proj, "add", "-A")
        run_git(proj, "commit", "-m", "init")
        storage_dir = tmp_path / "chisel_st"
        with ChiselEngine(str(proj), storage_dir=storage_dir) as engine:
            engine.analyze(force=True)
            engine.storage._execute("DELETE FROM test_edges")
            sug = engine.tool_suggest_tests("src/widget.js")
            gaps = engine.tool_test_gaps(file_path="src/widget.js")
        assert len(sug) >= 1
        assert any(r.get("source") == "static_require" for r in sug)
        assert gaps == []


# ------------------------------------------------------------------ #
# Working-tree improvements
# ------------------------------------------------------------------ #

class TestWorkingTreeRiskMap:
    def test_risk_map_includes_untracked_files_when_working_tree(self, engine, git_project):
        engine.analyze()
        # Create an untracked source file
        (git_project / "new_module.py").write_text("def new_func():\n    pass\n")
        result = engine.tool_risk_map(working_tree=True)
        files = [r["file_path"] for r in result["files"]]
        assert "new_module.py" in files
        meta = result["_meta"]
        assert meta.get("working_tree_files_included") >= 1

    def test_risk_map_warns_about_untracked_files_excluded(self, engine, git_project):
        engine.analyze()
        (git_project / "untracked.py").write_text("def x(): pass\n")
        result = engine.tool_risk_map(working_tree=False)
        warnings = result["_meta"].get("warnings", [])
        assert any("untracked" in w for w in warnings)


class TestWorkingTreeTestGaps:
    def test_test_gaps_puts_working_tree_gaps_first(self, engine, git_project):
        engine.analyze()
        # untracked file should appear before low-churn DB gaps
        (git_project / "fresh.py").write_text("def alpha(): pass\ndef beta(): pass\n")
        gaps = engine.tool_test_gaps(working_tree=True)
        # First gap should be from the working tree
        assert gaps[0]["file_path"] == "fresh.py"
        assert gaps[0].get("_working_tree") is True


class TestWorkingTreeDiffImpact:
    def test_diff_impact_stem_matches_new_tracked_files_with_working_tree(self, engine, git_project):
        engine.analyze()
        # Create a new source file and a matching test file, then stage them
        (git_project / "services").mkdir()
        (git_project / "services" / "nutrition.py").write_text("def calc(): return 1\n")
        (git_project / "tests" / "test_nutrition.py").write_text(
            "def test_calc():\n    assert True\n"
        )
        # Stage the new source file so it appears in diff_files, not untracked
        import subprocess
        subprocess.run(["git", "add", "services/nutrition.py"], cwd=str(git_project), check=True)
        # Must update so the new file is in the DB; otherwise diff_impact returns stale_db
        engine.update()
        result = engine.tool_diff_impact(working_tree=True)
        test_ids = {item["test_id"] for item in result}
        assert any("test_nutrition" in tid for tid in test_ids)


class TestWorkingTreeSuggest:
    def test_working_tree_suggest_prefers_same_directory(self, engine, git_project):
        engine.analyze()
        (git_project / "services").mkdir()
        (git_project / "tests" / "services").mkdir()
        (git_project / "tests" / "services" / "widget.test.js").write_text(
            "describe('w', () => { it('a', () => {}); });\n"
        )
        (git_project / "tests" / "cli").mkdir()
        (git_project / "tests" / "cli" / "widget.test.js").write_text(
            "describe('c', () => { it('b', () => {}); });\n"
        )
        # Re-analyze so new test files are in DB
        engine.analyze(force=True)
        suggestions = engine._working_tree_suggest("services/widget.js")
        by_path = {s["file_path"]: s["relevance"] for s in suggestions}
        assert by_path["tests/services/widget.test.js"] > by_path["tests/cli/widget.test.js"]


class TestTriageWorkingTree:
    def test_triage_forwards_working_tree(self, engine, git_project):
        engine.analyze()
        (git_project / "fresh.py").write_text("def alpha(): pass\n")
        result = engine.tool_triage(working_tree=True)
        files = [r["file_path"] for r in result["top_risk_files"]]
        assert "fresh.py" in files


class TestStaleDbDetection:
    def test_diff_impact_returns_stale_db_for_new_file(self, engine, git_project):
        engine.analyze()
        (git_project / "new_module.py").write_text("def x(): pass\n")
        # Do NOT update — file is missing from DB
        result = engine.tool_diff_impact()
        assert isinstance(result, dict)
        assert result["status"] == "stale_db"
        assert "new_module.py" in result["missing_from_db"]

    def test_suggest_tests_returns_stale_db_for_new_file(self, engine, git_project):
        engine.analyze()
        (git_project / "new_module.py").write_text("def x(): pass\n")
        result = engine.tool_suggest_tests("new_module.py")
        assert isinstance(result, dict)
        assert result["status"] == "stale_db"

    def test_diff_impact_ignores_non_code_diffs(self, engine, git_project, run_git):
        """Regression: tracked changes to non-code files (.json data, .md
        docs, .db binaries) must NOT trigger stale_db. Only indexed code
        extensions count toward missing_from_db.
        """
        engine.analyze()
        # Edit a tracked non-code file. This shows up in `git diff` but is
        # not something Chisel indexes — it shouldn't trip stale_db.
        readme = git_project / "README.md"
        readme.write_text(
            "# project\n\nSome existing readme content for the test fixture.\n"
        )
        run_git(git_project, "add", "README.md")
        run_git(git_project, "commit", "-m", "add readme")
        readme.write_text(
            "# project\n\nUpdated readme content with more detail.\n"
        )
        result = engine.tool_diff_impact()
        # No stale_db because no CODE files changed.
        if isinstance(result, dict) and result.get("status") == "stale_db":
            raise AssertionError(
                "diff_impact tripped stale_db on a non-code file change: "
                f"{result.get('missing_from_db')}"
            )

    def test_suggest_tests_auto_fallback_for_known_file(self, engine):
        engine.analyze()
        # app.py is known but may have no DB edges in some fixtures
        result = engine.tool_suggest_tests("app.py")
        # Should return a list (either DB hits or auto-fallback)
        assert isinstance(result, list)


class TestBackgroundJobProgress:
    def test_job_status_includes_progress(self, engine):
        engine.analyze()
        start = engine.tool_start_job("update")
        assert start["status"] == "running"
        job_id = start["job_id"]
        status = engine.tool_job_status(job_id)
        assert "progress_pct" in status
        # Wait for completion
        for _ in range(200):
            st = engine.tool_job_status(job_id)
            if st["status"] == "completed":
                assert st.get("progress_pct") == 100
                return
            if st["status"] == "failed":
                raise AssertionError(st.get("error"))
            import time
            time.sleep(0.02)
        raise AssertionError("background job did not complete")

    def test_job_status_includes_events(self, engine):
        engine.analyze()
        start = engine.tool_start_job("update")
        job_id = start["job_id"]
        for _ in range(200):
            st = engine.tool_job_status(job_id)
            if st["status"] in ("completed", "failed"):
                assert "events" in st
                assert any(e["event_type"] == "phase_end" for e in st["events"])
                return
            import time
            time.sleep(0.02)
        raise AssertionError("background job did not complete")

    def test_cancel_job_sets_cancel_requested(self, engine):
        engine.analyze()
        start = engine.tool_start_job("update")
        job_id = start["job_id"]
        result = engine.tool_cancel_job(job_id)
        assert result["status"] == "ok"
        st = engine.tool_job_status(job_id)
        assert st["cancel_requested"] is True

    def test_cancel_nonexistent_job_returns_not_found(self, engine):
        result = engine.tool_cancel_job("no-such-job")
        assert result["status"] == "not_found"


class TestHeuristicEdgesDuringAnalyze:
    def test_analyze_creates_heuristic_edges_for_missing_scans(self, engine, git_project):
        engine.analyze()
        # Create a test file that mirrors an existing source file by stem (app.py)
        (git_project / "tests" / "test_app.py").write_text(
            "def test_process_data():\n    assert True\n"
        )
        # Clear existing edges so heuristic can apply
        engine.storage._execute("DELETE FROM test_edges")
        engine.analyze(force=True)
        # Heuristic edges should have been backfilled
        test_units = engine.storage.get_test_units_by_file("tests/test_app.py")
        assert len(test_units) > 0
        edges = engine.storage.get_edges_for_test(test_units[0]["id"])
        assert any(e["edge_type"] == "heuristic" for e in edges)


class TestProjectFingerprint:
    def test_stats_warns_on_fingerprint_mismatch(self, engine):
        engine.analyze()
        engine.storage.set_meta("project_fingerprint", "/some/other/path")
        stats = engine.tool_stats()
        assert "warning" in stats
        assert "mismatch" in stats["warning"]


# ------------------------------------------------------------------ #
# _test_to_source_stem helper
# ------------------------------------------------------------------ #


class TestTestToSourceStem:
    def test_jest_test_js(self):
        assert _test_to_source_stem("tests/services/nutritionService.test.js") == "nutritionService"

    def test_jest_spec_ts(self):
        assert _test_to_source_stem("tests/app.spec.ts") == "app"

    def test_pytest_prefix(self):
        assert _test_to_source_stem("tests/test_utils.py") == "utils"

    def test_pytest_suffix(self):
        assert _test_to_source_stem("tests/utils_test.py") == "utils"

    def test_csharp_test_suffix(self):
        assert _test_to_source_stem("Tests/CalculatorTest.cs") == "Calculator"

    def test_no_test_markers(self):
        # If there's no recognizable test affix, returns the raw stem
        assert _test_to_source_stem("src/foo.js") == "foo"

    def test_empty_stem_returns_none(self):
        # Pathological: file named ".test.js" → stem "" → None
        assert _test_to_source_stem(".test.js") is None


class TestIncrementalImportEdgePerformance:
    def test_update_zero_changes_fast(self, tmp_path, run_git):
        """Acceptance: update with no changed files finishes in < 50 ms."""
        project = tmp_path / "fastproject"
        project.mkdir()
        src_dir = project / "src"
        src_dir.mkdir()
        tests_dir = project / "tests"
        tests_dir.mkdir()

        run_git(project, "init")
        run_git(project, "config", "user.name", "Test")
        run_git(project, "config", "user.email", "test@test.com")

        for i in range(100):
            (src_dir / f"mod_{i:03d}.py").write_text(f"def func_{i}():\n    return {i}\n")

        (tests_dir / "test_sample.py").write_text("def test_sample():\n    pass\n")

        run_git(project, "add", "-A")
        run_git(project, "commit", "-m", "initial")

        engine = ChiselEngine(str(project), storage_dir=str(tmp_path / "db"))
        engine.analyze()

        start = time.perf_counter()
        stats = engine.update()
        elapsed_ms = (time.perf_counter() - start) * 1000
        assert elapsed_ms < 50, f"update took {elapsed_ms:.1f} ms"
        assert stats["files_updated"] == 0

    def test_incremental_update_with_1k_files_is_fast(self, tmp_path, run_git):
        """Regression: scaling incremental import edges to 1k+ files."""
        project = tmp_path / "bigproject"
        project.mkdir()
        src_dir = project / "src"
        src_dir.mkdir()
        tests_dir = project / "tests"
        tests_dir.mkdir()

        run_git(project, "init")
        run_git(project, "config", "user.name", "Test")
        run_git(project, "config", "user.email", "test@test.com")

        for i in range(1000):
            (src_dir / f"mod_{i:04d}.py").write_text(f"def func_{i}():\n    return {i}\n")

        test_content = "\n".join([f"from src.mod_{i:04d} import func_{i}" for i in range(10)])
        test_content += "\n\ndef test_sample():\n    pass\n"
        (tests_dir / "test_sample.py").write_text(test_content)

        run_git(project, "add", "-A")
        run_git(project, "commit", "-m", "initial")

        storage_dir = tmp_path / "db"
        engine = ChiselEngine(str(project), storage_dir=str(storage_dir))
        engine.analyze()

        # Modify a single file without committing
        (src_dir / "mod_0500.py").write_text("def func_500():\n    return 5000\n")

        start = time.perf_counter()
        stats = engine.update()
        elapsed_ms = (time.perf_counter() - start) * 1000
        assert stats["files_updated"] == 1
        # Must complete in < 3 seconds even with 1k files
        assert elapsed_ms < 3000, f"incremental update took {elapsed_ms:.1f} ms"
