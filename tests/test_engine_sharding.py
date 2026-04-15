"""Tests for monorepo SQLite sharding in ChiselEngine."""

import os
from unittest.mock import patch

import pytest

from chisel.engine import ChiselEngine


@pytest.fixture
def sharded_project(tmp_path, run_git):
    """Create a temp git repo with two shardable subdirectories."""
    project = tmp_path / "sharded_project"
    project.mkdir()
    run_git(project, "init")
    run_git(project, "config", "user.name", "Test")
    run_git(project, "config", "user.email", "test@test.com")

    # frontend shard
    frontend = project / "frontend"
    frontend.mkdir()
    (frontend / "app.js").write_text(
        "function frontendMain() { return 1; }\n"
    )
    (frontend / "app.test.js").write_text(
        "test('frontend', () => { expect(frontendMain()).toBe(1); });\n"
    )

    # backend shard
    backend = project / "backend"
    backend.mkdir()
    (backend / "api.py").write_text(
        "def backend_main():\n    return 2\n"
    )
    (backend / "test_api.py").write_text(
        "from api import backend_main\n\n"
        "def test_backend():\n"
        "    assert backend_main() == 2\n"
    )

    run_git(project, "add", "-A")
    run_git(project, "commit", "-m", "initial")
    return project


@pytest.fixture
def sharded_engine(sharded_project, tmp_path):
    """Engine with CHISEL_SHARDS env var pointing to frontend and backend."""
    storage_dir = tmp_path / "chisel_storage"
    with patch.dict(os.environ, {"CHISEL_SHARDS": "frontend,backend"}):
        eng = ChiselEngine(str(sharded_project), storage_dir=str(storage_dir))
        yield eng
        eng.close()


class TestShardingAnalyze:
    def test_sharded_analyze_populates_shards(self, sharded_engine):
        sharded_engine.tool_analyze(directory="frontend", shard="frontend")
        sharded_engine.tool_analyze(directory="backend", shard="backend")

        # frontend shard has frontend data
        with sharded_engine._with_shard("frontend"):
            assert sharded_engine.storage.get_file_hash("frontend/app.js") is not None
            assert sharded_engine.storage.get_file_hash("backend/api.py") is None

        # backend shard has backend data
        with sharded_engine._with_shard("backend"):
            assert sharded_engine.storage.get_file_hash("backend/api.py") is not None
            assert sharded_engine.storage.get_file_hash("frontend/app.js") is None

    def test_sharded_tool_stats_aggregates(self, sharded_engine):
        sharded_engine.tool_analyze(directory="frontend", shard="frontend")
        sharded_engine.tool_analyze(directory="backend", shard="backend")

        stats = sharded_engine.tool_stats()
        # file_hashes are scoped per shard, so 2+2 = 4
        assert stats["file_hashes"] == 4
        # test discovery is project-wide, so each shard stores all test units
        assert stats["test_units"] == 4
        # commits are duplicated across shards as well
        assert stats["commits"] == 2


class TestShardingQueries:
    def test_sharded_tool_impact_aggregates(self, sharded_engine, sharded_project):
        sharded_engine.tool_analyze(directory="frontend", shard="frontend")
        sharded_engine.tool_analyze(directory="backend", shard="backend")

        result = sharded_engine.tool_impact(
            files=["frontend/app.js", "backend/api.py"]
        )
        test_ids = {r["test_id"] for r in result}
        # frontend test
        assert any("frontend" in tid for tid in test_ids)
        # backend test
        assert any("backend" in tid for tid in test_ids)

    def test_sharded_tool_risk_map_aggregates(self, sharded_engine):
        sharded_engine.tool_analyze(directory="frontend", shard="frontend")
        sharded_engine.tool_analyze(directory="backend", shard="backend")

        result = sharded_engine.tool_risk_map()
        files = {f["file_path"] for f in result["files"]}
        assert "frontend/app.js" in files
        assert "backend/api.py" in files
        assert "_meta" in result
        assert result["_meta"]["cycles"] == []

    def test_sharded_tool_stale_tests_aggregates(self, sharded_engine):
        sharded_engine.tool_analyze(directory="frontend", shard="frontend")
        sharded_engine.tool_analyze(directory="backend", shard="backend")

        # Immediately after analysis there are no stale tests
        stale = sharded_engine.tool_stale_tests()
        assert stale == []

    def test_sharded_tool_test_gaps_aggregates(self, sharded_engine, sharded_project):
        # Add an untested source file in each shard
        (sharded_project / "frontend" / "untested.js").write_text(
            "function untestedFrontend() { return 0; }\n"
        )
        (sharded_project / "backend" / "untested.py").write_text(
            "def untested_backend():\n    return 0\n"
        )
        sharded_engine.tool_analyze(directory="frontend", shard="frontend")
        sharded_engine.tool_analyze(directory="backend", shard="backend")

        gaps = sharded_engine.tool_test_gaps()
        files = {g["file_path"] for g in gaps}
        assert "frontend/untested.js" in files
        assert "backend/untested.py" in files

    def test_sharded_tool_triage_aggregates(self, sharded_engine):
        sharded_engine.tool_analyze(directory="frontend", shard="frontend")
        sharded_engine.tool_analyze(directory="backend", shard="backend")

        result = sharded_engine.tool_triage()
        risk_files = {r["file_path"] for r in result["top_risk_files"]}
        assert "frontend/app.js" in risk_files or "backend/api.py" in risk_files
        assert "summary" in result
        assert result["summary"]["test_edge_count"] >= 0

    def test_sharded_tool_diff_impact_aggregates(self, sharded_engine, sharded_project):
        sharded_engine.tool_analyze(directory="frontend", shard="frontend")
        sharded_engine.tool_analyze(directory="backend", shard="backend")

        # Modify files in both shards
        (sharded_project / "frontend" / "app.js").write_text(
            "function frontendMain() { return 10; }\n"
        )
        (sharded_project / "backend" / "api.py").write_text(
            "def backend_main():\n    return 20\n"
        )

        result = sharded_engine.tool_diff_impact()
        test_ids = {r["test_id"] for r in result}
        assert any("frontend" in tid for tid in test_ids)
        assert any("backend" in tid for tid in test_ids)

    def test_sharded_tool_suggest_tests_routes_to_shard(self, sharded_engine):
        sharded_engine.tool_analyze(directory="frontend", shard="frontend")
        sharded_engine.tool_analyze(directory="backend", shard="backend")

        # file_path mode routes to the correct shard
        result = sharded_engine.tool_suggest_tests(file_path="frontend/app.js")
        assert isinstance(result, list)
        assert any("frontend" in s["test_id"] for s in result)

        result = sharded_engine.tool_suggest_tests(file_path="backend/api.py")
        assert isinstance(result, list)
        assert any("backend" in s["test_id"] for s in result)

    def test_sharded_tool_record_result_routes_to_shard(self, sharded_engine):
        sharded_engine.tool_analyze(directory="frontend", shard="frontend")
        sharded_engine.tool_analyze(directory="backend", shard="backend")

        # Record a result for a backend test
        test_id = "backend/test_api.py::test_backend"
        result = sharded_engine.tool_record_result(test_id, passed=True)
        assert result["recorded"] is True

        # Verify it landed in the backend shard
        with sharded_engine._with_shard("backend"):
            rows = sharded_engine.storage._fetchall(
                "SELECT * FROM test_results WHERE test_id = ?", (test_id,)
            )
            assert len(rows) == 1
            assert rows[0]["passed"] == 1

        # Verify it did NOT land in the frontend shard
        with sharded_engine._with_shard("frontend"):
            rows = sharded_engine.storage._fetchall(
                "SELECT * FROM test_results WHERE test_id = ?", (test_id,)
            )
            assert len(rows) == 0
