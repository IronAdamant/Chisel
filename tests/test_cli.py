"""Tests for chisel.cli — parser, handlers, and main dispatch."""

import json
from unittest.mock import MagicMock, patch

from chisel.cli import _is_no_data, _limit
from chisel.cli import cmd_analyze, cmd_churn, cmd_coupling, cmd_diff_impact
from chisel.cli import cmd_history, cmd_impact, cmd_ownership, cmd_record_result
from chisel.cli import cmd_risk_map, cmd_serve, cmd_serve_mcp, cmd_stale_tests
from chisel.cli import cmd_stats, cmd_suggest_tests, cmd_test_gaps
from chisel.cli import cmd_triage, cmd_update, cmd_who_reviews
from chisel.cli import create_parser, main


# ------------------------------------------------------------------ #
# Parser tests
# ------------------------------------------------------------------ #

class TestCreateParser:
    """Tests for create_parser() and argument parsing."""

    def test_parser_exists(self):
        parser = create_parser()
        assert parser is not None

    def test_analyze_defaults(self):
        parser = create_parser()
        args = parser.parse_args(["analyze"])
        assert args.command == "analyze"
        assert args.directory == "."
        assert args.force is False

    def test_analyze_with_directory_and_force(self):
        parser = create_parser()
        args = parser.parse_args(["analyze", "src/", "--force"])
        assert args.command == "analyze"
        assert args.directory == "src/"
        assert args.force is True

    def test_impact_files(self):
        parser = create_parser()
        args = parser.parse_args(["impact", "a.py", "b.py"])
        assert args.command == "impact"
        assert args.files == ["a.py", "b.py"]

    def test_suggest_tests(self):
        parser = create_parser()
        args = parser.parse_args(["suggest-tests", "app.py"])
        assert args.command == "suggest-tests"
        assert args.file == "app.py"

    def test_churn_defaults(self):
        parser = create_parser()
        args = parser.parse_args(["churn", "app.py"])
        assert args.command == "churn"
        assert args.file == "app.py"
        assert args.unit is None

    def test_churn_with_unit(self):
        parser = create_parser()
        args = parser.parse_args(["churn", "app.py", "--unit", "process_data"])
        assert args.unit == "process_data"

    def test_ownership(self):
        parser = create_parser()
        args = parser.parse_args(["ownership", "app.py"])
        assert args.command == "ownership"
        assert args.file == "app.py"

    def test_coupling_defaults(self):
        parser = create_parser()
        args = parser.parse_args(["coupling", "app.py"])
        assert args.command == "coupling"
        assert args.file == "app.py"
        assert args.min_count == 3

    def test_coupling_with_min_count(self):
        parser = create_parser()
        args = parser.parse_args(["coupling", "app.py", "--min-count", "5"])
        assert args.min_count == 5

    def test_risk_map_defaults(self):
        parser = create_parser()
        args = parser.parse_args(["risk-map"])
        assert args.command == "risk-map"
        assert args.directory is None

    def test_risk_map_with_directory(self):
        parser = create_parser()
        args = parser.parse_args(["risk-map", "src/"])
        assert args.directory == "src/"

    def test_stale_tests(self):
        parser = create_parser()
        args = parser.parse_args(["stale-tests"])
        assert args.command == "stale-tests"

    def test_history(self):
        parser = create_parser()
        args = parser.parse_args(["history", "app.py"])
        assert args.command == "history"
        assert args.file == "app.py"

    def test_who_reviews(self):
        parser = create_parser()
        args = parser.parse_args(["who-reviews", "app.py"])
        assert args.command == "who-reviews"
        assert args.file == "app.py"

    def test_triage_defaults(self):
        parser = create_parser()
        args = parser.parse_args(["triage"])
        assert args.command == "triage"
        assert args.directory is None
        assert args.top_n == 10

    def test_triage_with_args(self):
        parser = create_parser()
        args = parser.parse_args(["triage", "src/", "--top-n", "5"])
        assert args.directory == "src/"
        assert args.top_n == 5

    def test_serve_defaults(self):
        parser = create_parser()
        args = parser.parse_args(["serve"])
        assert args.command == "serve"
        assert args.port == 8377
        assert args.host == "127.0.0.1"

    def test_serve_custom(self):
        parser = create_parser()
        args = parser.parse_args(["serve", "--port", "9000", "--host", "0.0.0.0"])
        assert args.port == 9000
        assert args.host == "0.0.0.0"

    def test_serve_mcp(self):
        parser = create_parser()
        args = parser.parse_args(["serve-mcp"])
        assert args.command == "serve-mcp"

    def test_global_json_flag(self):
        parser = create_parser()
        args = parser.parse_args(["analyze", "--json"])
        assert args.json_output is True

    def test_global_project_dir(self):
        parser = create_parser()
        args = parser.parse_args(["analyze", "--project-dir", "/tmp/proj"])
        assert args.project_dir == "/tmp/proj"

    def test_global_storage_dir(self):
        parser = create_parser()
        args = parser.parse_args(["analyze", "--storage-dir", "/tmp/store"])
        assert args.storage_dir == "/tmp/store"

    def test_no_subcommand(self):
        parser = create_parser()
        args = parser.parse_args([])
        assert args.command is None


# ------------------------------------------------------------------ #
# Helper to build fake args
# ------------------------------------------------------------------ #

def _make_args(**kwargs):
    """Create a mock args namespace with sensible defaults."""
    defaults = {
        "project_dir": "/tmp/fake_project",
        "storage_dir": None,
        "json_output": False,
        "limit": None,
    }
    defaults.update(kwargs)
    args = MagicMock()
    for key, val in defaults.items():
        setattr(args, key, val)
    return args


def _make_engine_mock():
    """Create a MagicMock engine that supports context manager protocol."""
    engine = MagicMock()
    engine.__enter__ = MagicMock(return_value=engine)
    engine.__exit__ = MagicMock(return_value=False)
    return engine


# ------------------------------------------------------------------ #
# Handler tests (mocked engine)
# ------------------------------------------------------------------ #

class TestHandlerOutputFormats:
    """Test that handlers produce correct human-readable and JSON output."""

    @patch("chisel.cli.ChiselEngine")
    def test_cmd_analyze_human(self, mock_cls, capsys):
        engine = _make_engine_mock()
        engine.tool_analyze.return_value = {
            "code_files_scanned": 10,
            "code_units_found": 25,
        }
        mock_cls.return_value = engine

        args = _make_args(directory=".", force=False)
        result = cmd_analyze(args)

        assert result == {"code_files_scanned": 10, "code_units_found": 25}
        output = capsys.readouterr().out
        assert "Analysis complete" in output
        assert "10" in output

    @patch("chisel.cli.ChiselEngine")
    def test_cmd_analyze_json(self, mock_cls, capsys):
        engine = _make_engine_mock()
        engine.tool_analyze.return_value = {"code_files_scanned": 5}
        mock_cls.return_value = engine

        args = _make_args(directory=".", force=False, json_output=True)
        cmd_analyze(args)

        output = capsys.readouterr().out
        parsed = json.loads(output)
        assert parsed == {"code_files_scanned": 5}

    @patch("chisel.cli.ChiselEngine")
    def test_cmd_impact_human(self, mock_cls, capsys):
        engine = _make_engine_mock()
        engine.tool_impact.return_value = [
            {"test_id": "test_foo", "reason": "import"},
        ]
        mock_cls.return_value = engine

        args = _make_args(files=["a.py"])
        result = cmd_impact(args)

        assert len(result) == 1
        output = capsys.readouterr().out
        assert "test_foo" in output

    @patch("chisel.cli.ChiselEngine")
    def test_cmd_impact_empty(self, mock_cls, capsys):
        engine = _make_engine_mock()
        engine.tool_impact.return_value = []
        mock_cls.return_value = engine

        args = _make_args(files=["a.py"])
        cmd_impact(args)

        output = capsys.readouterr().out
        assert "No impacted tests" in output

    @patch("chisel.cli.ChiselEngine")
    def test_cmd_impact_json(self, mock_cls, capsys):
        engine = _make_engine_mock()
        engine.tool_impact.return_value = [{"test_id": "test_x"}]
        mock_cls.return_value = engine

        args = _make_args(files=["a.py"], json_output=True)
        cmd_impact(args)

        output = capsys.readouterr().out
        parsed = json.loads(output)
        assert parsed == [{"test_id": "test_x"}]

    @patch("chisel.cli.ChiselEngine")
    def test_cmd_suggest_tests_human(self, mock_cls, capsys):
        engine = _make_engine_mock()
        engine.tool_suggest_tests.return_value = [
            {"name": "test_bar", "relevance": 0.9},
        ]
        mock_cls.return_value = engine

        args = _make_args(file="app.py")
        result = cmd_suggest_tests(args)

        assert len(result) == 1
        output = capsys.readouterr().out
        assert "test_bar" in output

    @patch("chisel.cli.ChiselEngine")
    def test_cmd_suggest_tests_empty(self, mock_cls, capsys):
        engine = _make_engine_mock()
        engine.tool_suggest_tests.return_value = []
        mock_cls.return_value = engine

        args = _make_args(file="app.py")
        cmd_suggest_tests(args)

        output = capsys.readouterr().out
        assert "No test suggestions" in output

    @patch("chisel.cli.ChiselEngine")
    def test_cmd_churn_list(self, mock_cls, capsys):
        engine = _make_engine_mock()
        engine.tool_churn.return_value = [
            {"commit_count": 12, "churn_score": 3.5},
        ]
        mock_cls.return_value = engine

        args = _make_args(file="app.py", unit=None)
        result = cmd_churn(args)

        assert result[0]["commit_count"] == 12
        output = capsys.readouterr().out
        assert "12" in output

    @patch("chisel.cli.ChiselEngine")
    def test_cmd_churn_json(self, mock_cls, capsys):
        engine = _make_engine_mock()
        engine.tool_churn.return_value = [{"commit_count": 7}]
        mock_cls.return_value = engine

        args = _make_args(file="app.py", unit=None, json_output=True)
        cmd_churn(args)

        output = capsys.readouterr().out
        parsed = json.loads(output)
        assert parsed == [{"commit_count": 7}]

    @patch("chisel.cli.ChiselEngine")
    def test_cmd_ownership_human(self, mock_cls, capsys):
        engine = _make_engine_mock()
        engine.tool_ownership.return_value = [
            {"author": "Alice", "percentage": 70},
        ]
        mock_cls.return_value = engine

        args = _make_args(file="app.py")
        result = cmd_ownership(args)

        assert len(result) == 1
        output = capsys.readouterr().out
        assert "Alice" in output

    @patch("chisel.cli.ChiselEngine")
    def test_cmd_coupling_human(self, mock_cls, capsys):
        engine = _make_engine_mock()
        engine.tool_coupling.return_value = [
            {"file_b": "utils.py", "co_commit_count": 5},
        ]
        mock_cls.return_value = engine

        args = _make_args(file="app.py", min_count=3)
        result = cmd_coupling(args)

        assert len(result) == 1
        output = capsys.readouterr().out
        assert "utils.py" in output
        assert "5 co-commits" in output

    @patch("chisel.cli.ChiselEngine")
    def test_cmd_coupling_empty(self, mock_cls, capsys):
        engine = _make_engine_mock()
        engine.tool_coupling.return_value = []
        mock_cls.return_value = engine

        args = _make_args(file="app.py", min_count=3)
        cmd_coupling(args)

        output = capsys.readouterr().out
        assert "No coupling data" in output

    @patch("chisel.cli.ChiselEngine")
    def test_cmd_risk_map_human(self, mock_cls, capsys):
        engine = _make_engine_mock()
        engine.tool_risk_map.return_value = {
            "files": [{"file_path": "core.py", "risk_score": 8.2}],
            "_meta": {"total_files": 1, "effective_components": ["churn"],
                      "uniform_components": {}},
        }
        mock_cls.return_value = engine

        args = _make_args(directory=None)
        result = cmd_risk_map(args)

        assert "files" in result
        output = capsys.readouterr().out
        assert "core.py" in output
        assert "8.2" in output

    @patch("chisel.cli.ChiselEngine")
    def test_cmd_stale_tests_human(self, mock_cls, capsys):
        engine = _make_engine_mock()
        engine.tool_stale_tests.return_value = [
            {"test_id": "test_old", "edge_type": "import"},
        ]
        mock_cls.return_value = engine

        args = _make_args()
        result = cmd_stale_tests(args)

        assert len(result) == 1
        output = capsys.readouterr().out
        assert "test_old" in output

    @patch("chisel.cli.ChiselEngine")
    def test_cmd_stale_tests_empty(self, mock_cls, capsys):
        engine = _make_engine_mock()
        engine.tool_stale_tests.return_value = []
        mock_cls.return_value = engine

        args = _make_args()
        cmd_stale_tests(args)

        output = capsys.readouterr().out
        assert "No stale tests" in output

    @patch("chisel.cli.ChiselEngine")
    def test_cmd_history_human(self, mock_cls, capsys):
        engine = _make_engine_mock()
        engine.tool_history.return_value = [
            {
                "hash": "abc12345deadbeef",
                "author": "Bob",
                "date": "2026-01-15",
                "message": "Fix bug",
            },
        ]
        mock_cls.return_value = engine

        args = _make_args(file="app.py")
        result = cmd_history(args)

        assert len(result) == 1
        output = capsys.readouterr().out
        assert "abc12345" in output
        assert "Bob" in output
        assert "Fix bug" in output

    @patch("chisel.cli.ChiselEngine")
    def test_cmd_history_json(self, mock_cls, capsys):
        engine = _make_engine_mock()
        engine.tool_history.return_value = [{"hash": "aaa", "author": "X"}]
        mock_cls.return_value = engine

        args = _make_args(file="app.py", json_output=True)
        cmd_history(args)

        output = capsys.readouterr().out
        parsed = json.loads(output)
        assert parsed == [{"hash": "aaa", "author": "X"}]

    @patch("chisel.cli.ChiselEngine")
    def test_cmd_who_reviews_human(self, mock_cls, capsys):
        engine = _make_engine_mock()
        engine.tool_who_reviews.return_value = [
            {"author": "Carol", "percentage": 55, "recent_commits": 10,
             "days_since_last_commit": 3},
        ]
        mock_cls.return_value = engine

        args = _make_args(file="app.py")
        result = cmd_who_reviews(args)

        assert len(result) == 1
        output = capsys.readouterr().out
        assert "Carol" in output

    @patch("chisel.cli.ChiselEngine")
    def test_cmd_diff_impact_human(self, mock_cls, capsys):
        engine = _make_engine_mock()
        engine.tool_diff_impact.return_value = [
            {"test_id": "test_changed", "reason": "diff"},
        ]
        mock_cls.return_value = engine

        args = _make_args(ref=None)
        result = cmd_diff_impact(args)

        assert len(result) == 1
        output = capsys.readouterr().out
        assert "test_changed" in output

    @patch("chisel.cli.ChiselEngine")
    def test_cmd_diff_impact_empty(self, mock_cls, capsys):
        engine = _make_engine_mock()
        engine.tool_diff_impact.return_value = []
        mock_cls.return_value = engine

        args = _make_args(ref=None)
        cmd_diff_impact(args)

        output = capsys.readouterr().out
        assert "No impacted tests" in output

    @patch("chisel.cli.ChiselEngine")
    def test_cmd_update_human(self, mock_cls, capsys):
        engine = _make_engine_mock()
        engine.tool_update.return_value = {
            "files_updated": 3,
            "new_commits": 7,
        }
        mock_cls.return_value = engine

        args = _make_args()
        result = cmd_update(args)

        assert result == {"files_updated": 3, "new_commits": 7}
        output = capsys.readouterr().out
        assert "Incremental update complete" in output
        assert "3" in output

    @patch("chisel.cli.ChiselEngine")
    def test_cmd_stats_human(self, mock_cls, capsys):
        engine = _make_engine_mock()
        engine.tool_stats.return_value = {"code_units": 5}
        mock_cls.return_value = engine

        args = _make_args()
        result = cmd_stats(args)

        assert result == {"code_units": 5}
        output = capsys.readouterr().out
        assert "Chisel database stats" in output
        assert "5" in output

    @patch("chisel.cli.ChiselEngine")
    def test_cmd_test_gaps_human(self, mock_cls, capsys):
        engine = _make_engine_mock()
        engine.tool_test_gaps.return_value = [
            {"file_path": "core.py", "name": "process", "unit_type": "function",
             "line_start": 10, "line_end": 25, "churn_score": 1.5},
        ]
        mock_cls.return_value = engine

        args = _make_args(file=None, directory=None, no_exclude_tests=False)
        result = cmd_test_gaps(args)

        assert len(result) == 1
        output = capsys.readouterr().out
        assert "core.py" in output

    @patch("chisel.cli.ChiselEngine")
    def test_cmd_test_gaps_empty(self, mock_cls, capsys):
        engine = _make_engine_mock()
        engine.tool_test_gaps.return_value = []
        mock_cls.return_value = engine

        args = _make_args(file=None, directory=None, no_exclude_tests=False)
        cmd_test_gaps(args)

        output = capsys.readouterr().out
        assert "No untested" in output

    @patch("chisel.cli.ChiselEngine")
    def test_cmd_record_result_passed(self, mock_cls, capsys):
        engine = _make_engine_mock()
        engine.tool_record_result.return_value = {"status": "recorded"}
        mock_cls.return_value = engine

        args = _make_args(test_id="test_foo", failed=False, duration=None)
        cmd_record_result(args)

        output = capsys.readouterr().out
        assert "PASSED" in output
        assert "test_foo" in output

    @patch("chisel.cli.ChiselEngine")
    def test_cmd_record_result_failed(self, mock_cls, capsys):
        engine = _make_engine_mock()
        engine.tool_record_result.return_value = {"status": "recorded"}
        mock_cls.return_value = engine

        args = _make_args(test_id="test_bar", failed=True, duration=100)
        cmd_record_result(args)

        output = capsys.readouterr().out
        assert "FAILED" in output
        assert "test_bar" in output

    @patch("chisel.cli.ChiselEngine")
    def test_cmd_triage_human(self, mock_cls, capsys):
        engine = _make_engine_mock()
        engine.tool_triage.return_value = {
            "top_risk_files": [
                {"file_path": "core.py", "risk_score": 0.8,
                 "coupling_partners": [{"file": "utils.py", "co_commits": 5}]},
            ],
            "test_gaps": [
                {"file_path": "core.py", "name": "process", "unit_type": "function"},
            ],
            "stale_tests": [],
            "summary": {"files_triaged": 1, "total_test_gaps": 1, "total_stale_tests": 0},
        }
        mock_cls.return_value = engine

        args = _make_args(directory=None, top_n=10)
        result = cmd_triage(args)

        assert result["summary"]["files_triaged"] == 1
        output = capsys.readouterr().out
        assert "core.py" in output
        assert "utils.py" in output
        assert "No stale tests" in output

    @patch("chisel.cli.ChiselEngine")
    def test_cmd_triage_json(self, mock_cls, capsys):
        engine = _make_engine_mock()
        engine.tool_triage.return_value = {
            "top_risk_files": [],
            "test_gaps": [],
            "stale_tests": [],
            "summary": {"files_triaged": 0, "total_test_gaps": 0, "total_stale_tests": 0},
        }
        mock_cls.return_value = engine

        args = _make_args(directory=None, top_n=10, json_output=True)
        cmd_triage(args)

        output = capsys.readouterr().out
        parsed = json.loads(output)
        assert "summary" in parsed

    @patch("chisel.mcp_server.ChiselMCPServer")
    def test_cmd_serve_human(self, mock_server_cls, capsys):
        server = MagicMock()
        server.get_url.return_value = "http://127.0.0.1:8377"
        mock_server_cls.return_value = server
        args = _make_args(host="127.0.0.1", port=8377)
        cmd_serve(args)
        output = capsys.readouterr().out
        assert "127.0.0.1" in output
        assert "8377" in output

    @patch("chisel.cli.mcp_main", create=True)
    def test_cmd_serve_mcp_human(self, mock_mcp_main, capsys):
        mock_mcp_main.return_value = None
        with patch("chisel.mcp_stdio.main", mock_mcp_main):
            args = _make_args()
            cmd_serve_mcp(args)


# ------------------------------------------------------------------ #
# main() dispatch tests
# ------------------------------------------------------------------ #

class TestMain:
    """Tests for the main() entry point."""

    @patch("chisel.cli.ChiselEngine")
    def test_main_analyze(self, mock_cls):
        engine = _make_engine_mock()
        engine.tool_analyze.return_value = {"code_files_scanned": 1}
        mock_cls.return_value = engine

        main(["analyze", "--project-dir", "/tmp/p"])

        mock_cls.assert_called_once_with("/tmp/p", storage_dir=None)
        engine.tool_analyze.assert_called_once_with(directory=".", force=False)

    @patch("chisel.cli.ChiselEngine")
    def test_main_impact(self, mock_cls):
        engine = _make_engine_mock()
        engine.tool_impact.return_value = []
        mock_cls.return_value = engine

        main(["impact", "--project-dir", "/tmp/p", "x.py", "y.py"])

        engine.tool_impact.assert_called_once_with(files=["x.py", "y.py"])

    @patch("chisel.cli.ChiselEngine")
    def test_main_churn_with_unit(self, mock_cls):
        engine = _make_engine_mock()
        engine.tool_churn.return_value = [{"commit_count": 3}]
        mock_cls.return_value = engine

        main(["churn", "--project-dir", "/tmp/p", "app.py", "--unit", "my_func"])

        engine.tool_churn.assert_called_once_with(file_path="app.py", unit_name="my_func")

    @patch("chisel.cli.ChiselEngine")
    def test_main_coupling_with_min_count(self, mock_cls):
        engine = _make_engine_mock()
        engine.tool_coupling.return_value = []
        mock_cls.return_value = engine

        main(["coupling", "--project-dir", "/tmp/p", "f.py", "--min-count", "7"])

        engine.tool_coupling.assert_called_once_with(file_path="f.py", min_count=7)

    @patch("chisel.cli.ChiselEngine")
    def test_main_json_flag(self, mock_cls, capsys):
        engine = _make_engine_mock()
        engine.tool_analyze.return_value = {"files": 2}
        mock_cls.return_value = engine

        main(["analyze", "--json", "--project-dir", "/tmp/p"])

        output = capsys.readouterr().out
        parsed = json.loads(output)
        assert parsed == {"files": 2}

    def test_main_no_command(self, capsys):
        result = main([])
        assert result is None

    @patch("chisel.cli.ChiselEngine")
    def test_main_suggest_tests(self, mock_cls):
        engine = _make_engine_mock()
        engine.tool_suggest_tests.return_value = [{"name": "test_x", "relevance": 0.8}]
        mock_cls.return_value = engine

        main(["suggest-tests", "--project-dir", "/tmp/p", "app.py"])

        engine.tool_suggest_tests.assert_called_once_with(file_path="app.py")

    @patch("chisel.cli.ChiselEngine")
    def test_main_ownership(self, mock_cls):
        engine = _make_engine_mock()
        engine.tool_ownership.return_value = [{"author": "A", "percentage": 100.0}]
        mock_cls.return_value = engine

        main(["ownership", "--project-dir", "/tmp/p", "app.py"])

        engine.tool_ownership.assert_called_once_with(file_path="app.py")

    @patch("chisel.cli.ChiselEngine")
    def test_main_risk_map(self, mock_cls):
        engine = _make_engine_mock()
        engine.tool_risk_map.return_value = {
            "files": [], "_meta": {"total_files": 0},
        }
        mock_cls.return_value = engine

        main(["risk-map", "--project-dir", "/tmp/p"])

        engine.tool_risk_map.assert_called_once_with(directory=None)

    @patch("chisel.cli.ChiselEngine")
    def test_main_stale_tests(self, mock_cls):
        engine = _make_engine_mock()
        engine.tool_stale_tests.return_value = []
        mock_cls.return_value = engine

        main(["stale-tests", "--project-dir", "/tmp/p"])

        engine.tool_stale_tests.assert_called_once()

    @patch("chisel.cli.ChiselEngine")
    def test_main_history(self, mock_cls):
        engine = _make_engine_mock()
        engine.tool_history.return_value = [{"hash": "aaa", "date": "2026-01-01", "author": "X", "message": "init"}]
        mock_cls.return_value = engine

        main(["history", "--project-dir", "/tmp/p", "app.py"])

        engine.tool_history.assert_called_once_with(file_path="app.py")

    @patch("chisel.cli.ChiselEngine")
    def test_main_who_reviews(self, mock_cls):
        engine = _make_engine_mock()
        engine.tool_who_reviews.return_value = []
        mock_cls.return_value = engine

        main(["who-reviews", "--project-dir", "/tmp/p", "app.py"])

        engine.tool_who_reviews.assert_called_once_with(file_path="app.py")

    @patch("chisel.cli.ChiselEngine")
    def test_main_diff_impact(self, mock_cls):
        engine = _make_engine_mock()
        engine.tool_diff_impact.return_value = []
        mock_cls.return_value = engine

        main(["diff-impact", "--project-dir", "/tmp/p"])

        engine.tool_diff_impact.assert_called_once_with(ref=None)

    @patch("chisel.cli.ChiselEngine")
    def test_main_update(self, mock_cls):
        engine = _make_engine_mock()
        engine.tool_update.return_value = {"files_updated": 0, "new_commits": 0}
        mock_cls.return_value = engine

        main(["update", "--project-dir", "/tmp/p"])

        engine.tool_update.assert_called_once()

    @patch("chisel.cli.ChiselEngine")
    def test_main_test_gaps(self, mock_cls):
        engine = _make_engine_mock()
        engine.tool_test_gaps.return_value = []
        mock_cls.return_value = engine

        main(["test-gaps", "--project-dir", "/tmp/p"])

        engine.tool_test_gaps.assert_called_once_with(file_path=None, directory=None, exclude_tests=True)

    @patch("chisel.cli.ChiselEngine")
    def test_main_test_gaps_with_file(self, mock_cls):
        engine = _make_engine_mock()
        engine.tool_test_gaps.return_value = [
            {"file_path": "app.py", "name": "foo", "unit_type": "function",
             "line_start": 1, "line_end": 5, "churn_score": 2.0},
        ]
        mock_cls.return_value = engine

        main(["test-gaps", "--project-dir", "/tmp/p", "app.py"])

        engine.tool_test_gaps.assert_called_once_with(file_path="app.py", directory=None, exclude_tests=True)

    @patch("chisel.cli.ChiselEngine")
    def test_main_analyze_force(self, mock_cls):
        engine = _make_engine_mock()
        engine.tool_analyze.return_value = {}
        mock_cls.return_value = engine

        main(["analyze", "--project-dir", "/tmp/p", "--force"])

        engine.tool_analyze.assert_called_once_with(directory=".", force=True)

    @patch("chisel.cli.ChiselEngine")
    def test_main_analyze_with_directory(self, mock_cls):
        engine = _make_engine_mock()
        engine.tool_analyze.return_value = {}
        mock_cls.return_value = engine

        main(["analyze", "--project-dir", "/tmp/p", "src/"])

        engine.tool_analyze.assert_called_once_with(directory="src/", force=False)

    @patch("chisel.cli.ChiselEngine")
    def test_main_triage(self, mock_cls):
        engine = _make_engine_mock()
        engine.tool_triage.return_value = {
            "top_risk_files": [], "test_gaps": [], "stale_tests": [],
            "summary": {"files_triaged": 0, "total_test_gaps": 0, "total_stale_tests": 0},
        }
        mock_cls.return_value = engine

        main(["triage", "--project-dir", "/tmp/p"])

        engine.tool_triage.assert_called_once_with(directory=None, top_n=10)

    @patch("chisel.cli.ChiselEngine")
    def test_main_triage_with_args(self, mock_cls):
        engine = _make_engine_mock()
        engine.tool_triage.return_value = {
            "top_risk_files": [], "test_gaps": [], "stale_tests": [],
            "summary": {"files_triaged": 0, "total_test_gaps": 0, "total_stale_tests": 0},
        }
        mock_cls.return_value = engine

        main(["triage", "--project-dir", "/tmp/p", "src/", "--top-n", "5"])

        engine.tool_triage.assert_called_once_with(directory="src/", top_n=5)

    @patch("chisel.cli.ChiselEngine")
    def test_main_stats(self, mock_cls):
        engine = _make_engine_mock()
        engine.tool_stats.return_value = {"code_units": 5, "test_units": 3}
        mock_cls.return_value = engine

        main(["stats", "--project-dir", "/tmp/p"])

        engine.tool_stats.assert_called_once()


# ------------------------------------------------------------------ #
# Limit parameter tests
# ------------------------------------------------------------------ #

class TestLimitParameter:
    """Tests for the _limit() helper and limit pass-through in handlers."""

    def test_limit_truncates_list(self):
        args = _make_args(limit=2)
        result = _limit([1, 2, 3, 4, 5], args)
        assert result == [1, 2]

    def test_limit_none_returns_full_list(self):
        args = _make_args(limit=None)
        result = _limit([1, 2, 3, 4, 5], args)
        assert result == [1, 2, 3, 4, 5]

    def test_limit_non_list_returns_unchanged(self):
        args = _make_args(limit=5)
        data = {"key": "value", "count": 42}
        result = _limit(data, args)
        assert result == {"key": "value", "count": 42}

    def test_limit_no_data_dict_unchanged(self):
        """The no-data dict should pass through _limit unchanged."""
        args = _make_args(limit=5)
        no_data = {"status": "no_data", "message": "msg", "hint": "hint"}
        result = _limit(no_data, args)
        assert result == no_data

    def test_limit_dict_with_files_key(self):
        """Dicts with a 'files' list (risk_map envelope) should be truncated."""
        args = _make_args(limit=2)
        data = {"files": [1, 2, 3, 4], "_meta": {"total_files": 4}}
        result = _limit(data, args)
        assert result["files"] == [1, 2]
        assert result["_meta"]["total_files"] == 4  # meta unchanged

    @patch("chisel.cli.ChiselEngine")
    def test_cmd_with_limit(self, mock_cls, capsys):
        engine = _make_engine_mock()
        engine.tool_impact.return_value = [
            {"test_id": f"test_{i}", "reason": "import"} for i in range(10)
        ]
        mock_cls.return_value = engine

        args = _make_args(files=["a.py"], limit=3)
        result = cmd_impact(args)

        assert len(result) == 3
        output = capsys.readouterr().out
        assert "test_0" in output
        assert "test_2" in output


# ------------------------------------------------------------------ #
# No-data response tests
# ------------------------------------------------------------------ #

_NO_DATA = {"status": "no_data", "message": "No analysis data. Run 'chisel analyze' on this project first.", "hint": "chisel analyze"}


class TestIsNoData:
    def test_detects_no_data_dict(self):
        assert _is_no_data(_NO_DATA) is True

    def test_rejects_empty_list(self):
        assert _is_no_data([]) is False

    def test_rejects_normal_dict(self):
        assert _is_no_data({"code_units": 5}) is False


class TestNoDataCliOutput:
    """CLI should print the message (not crash) when engine returns no-data."""

    @patch("chisel.cli.ChiselEngine")
    def test_no_data_human_output(self, mock_cls, capsys):
        engine = _make_engine_mock()
        engine.tool_risk_map.return_value = dict(_NO_DATA)
        mock_cls.return_value = engine

        args = _make_args(directory=None)
        cmd_risk_map(args)

        output = capsys.readouterr().out
        assert "No analysis data" in output
        assert "chisel analyze" in output

    @patch("chisel.cli.ChiselEngine")
    def test_no_data_json_output(self, mock_cls, capsys):
        engine = _make_engine_mock()
        engine.tool_impact.return_value = dict(_NO_DATA)
        mock_cls.return_value = engine

        args = _make_args(files=["a.py"], json_output=True)
        cmd_impact(args)

        output = capsys.readouterr().out
        parsed = json.loads(output)
        assert parsed["status"] == "no_data"
