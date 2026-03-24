"""Tests for chisel.next_steps — contextual next-step suggestions."""

from chisel.next_steps import compute_next_steps


def _has_tool(hints, tool_name):
    """Check if any hint suggests the given tool."""
    return any(h.get("tool") == tool_name for h in hints)


def _has_reason(hints, text):
    """Check if any hint's reason contains the given text (case-insensitive)."""
    return any(text.lower() in h.get("reason", "").lower() for h in hints)


def _has_action(hints, action_name):
    """Check if any hint has the given action label."""
    return any(h.get("action") == action_name for h in hints)


class TestComputeNextSteps:
    def test_unknown_tool_returns_empty(self):
        assert compute_next_steps("nonexistent", {}) == []

    def test_none_result_returns_empty(self):
        assert compute_next_steps("risk_map", None) == []


class TestAnalyzeHints:
    def test_after_successful_analyze(self):
        result = {"code_files_scanned": 10, "code_units_found": 25}
        hints = compute_next_steps("analyze", result)
        assert len(hints) >= 2
        assert _has_tool(hints, "risk_map")
        assert _has_tool(hints, "test_gaps")

    def test_non_analyze_result(self):
        assert compute_next_steps("analyze", []) == []


class TestUpdateHints:
    def test_after_update_with_changes(self):
        result = {"files_updated": 3, "new_commits": 1}
        hints = compute_next_steps("update", result)
        assert len(hints) >= 1
        assert _has_tool(hints, "diff_impact")

    def test_after_update_no_changes(self):
        result = {"files_updated": 0, "new_commits": 0}
        hints = compute_next_steps("update", result)
        assert hints == []


class TestRiskMapHints:
    def test_with_dict_envelope(self):
        """risk_map now returns {files, _meta} — hints should handle both."""
        result = {
            "files": [
                {"file_path": "a.py", "risk_score": 0.8, "breakdown": {"churn": 0.7, "coupling": 0.4}},
                {"file_path": "b.py", "risk_score": 0.5, "breakdown": {"churn": 0.2, "coupling": 0.1}},
            ],
            "_meta": {"effective_components": ["churn", "coupling"]},
        }
        hints = compute_next_steps("risk_map", result)
        assert len(hints) >= 2
        assert _has_tool(hints, "test_gaps")
        assert _has_tool(hints, "coupling")
        assert _has_tool(hints, "churn")

    def test_with_legacy_list(self):
        """Should still handle plain list for backward compat."""
        result = [
            {"file_path": "a.py", "risk_score": 0.8, "breakdown": {"churn": 0.7, "coupling": 0.4}},
        ]
        hints = compute_next_steps("risk_map", result)
        assert _has_tool(hints, "test_gaps")

    def test_empty_results(self):
        hints = compute_next_steps("risk_map", {"files": [], "_meta": {}})
        assert _has_tool(hints, "analyze")

    def test_low_coupling_no_coupling_hint(self):
        result = {
            "files": [
                {"file_path": "a.py", "risk_score": 0.3, "breakdown": {"churn": 0.1, "coupling": 0.1}},
            ],
            "_meta": {},
        }
        hints = compute_next_steps("risk_map", result)
        coupling_hints = [h for h in hints if h.get("tool") == "coupling"]
        assert coupling_hints == []

    def test_hint_args_include_file_path(self):
        result = {
            "files": [
                {"file_path": "core.py", "risk_score": 0.9, "breakdown": {"churn": 0.8, "coupling": 0.5}},
            ],
            "_meta": {},
        }
        hints = compute_next_steps("risk_map", result)
        coupling_hint = next(h for h in hints if h.get("tool") == "coupling")
        assert coupling_hint["args"]["file_path"] == "core.py"
        suggest_hint = next(h for h in hints if h.get("tool") == "suggest_tests")
        assert suggest_hint["args"]["file_path"] == "core.py"

    def test_uniform_coupling_adds_stats_hint(self):
        """When _meta shows coupling=0.0 uniform, suggest stats check."""
        result = {
            "files": [
                {"file_path": "a.py", "risk_score": 0.5, "breakdown": {"churn": 0.3, "coupling": 0.0}},
            ],
            "_meta": {
                "uniform_components": {
                    "coupling": {"value": 0.0, "reason": "no co-changes above threshold"},
                },
            },
        }
        hints = compute_next_steps("risk_map", result)
        assert _has_tool(hints, "stats")
        assert _has_reason(hints, "coupling=0.0")


class TestDiffImpactHints:
    def test_with_impacted_tests(self):
        result = [{"test_id": "test_foo", "reason": "direct"}]
        hints = compute_next_steps("diff_impact", result)
        assert _has_tool(hints, "record_result")
        assert _has_tool(hints, "coupling")

    def test_no_impacted_tests(self):
        hints = compute_next_steps("diff_impact", [])
        assert _has_tool(hints, "test_gaps")

    def test_no_changes_diagnostic(self):
        result = {"status": "no_changes", "ref": "HEAD", "branch": "main", "message": "No files differ"}
        hints = compute_next_steps("diff_impact", result)
        assert _has_tool(hints, "diff_impact")
        diff_hint = next(h for h in hints if h.get("tool") == "diff_impact")
        assert diff_hint["args"]["ref"] == "HEAD~1"


class TestTestGapsHints:
    def test_with_gaps(self):
        result = [{"file_path": "core.py", "name": "process", "unit_type": "function"}]
        hints = compute_next_steps("test_gaps", result)
        assert _has_tool(hints, "churn")
        assert _has_tool(hints, "ownership")

    def test_no_gaps(self):
        hints = compute_next_steps("test_gaps", [])
        assert _has_reason(hints, "coverage")


class TestStaleTestsHints:
    def test_with_stale(self):
        result = [{"test_id": "test_old", "edge_type": "import"}]
        hints = compute_next_steps("stale_tests", result)
        assert _has_tool(hints, "update")

    def test_no_stale(self):
        hints = compute_next_steps("stale_tests", [])
        assert _has_reason(hints, "current")


class TestTriageHints:
    def test_with_results(self):
        result = {
            "top_risk_files": [{"file_path": "a.py", "risk_score": 0.8}],
            "test_gaps": [{"file_path": "a.py", "name": "foo"}],
            "stale_tests": [],
            "summary": {"files_triaged": 1, "total_test_gaps": 1, "total_stale_tests": 0},
        }
        hints = compute_next_steps("triage", result)
        assert len(hints) >= 1
        assert _has_tool(hints, "suggest_tests")

    def test_no_gaps(self):
        result = {
            "top_risk_files": [{"file_path": "a.py", "risk_score": 0.3}],
            "test_gaps": [],
            "stale_tests": [],
            "summary": {"files_triaged": 1, "total_test_gaps": 0, "total_stale_tests": 0},
        }
        hints = compute_next_steps("triage", result)
        # Should not have "focus on files appearing in both sections"
        assert not _has_action(hints, "prioritize")


# ------------------------------------------------------------------ #
# New hint functions (v0.7)
# ------------------------------------------------------------------ #

class TestChurnHints:
    def test_with_results(self):
        result = [{"file_path": "app.py", "churn_score": 3.0}]
        hints = compute_next_steps("churn", result)
        assert _has_tool(hints, "risk_map")
        assert _has_tool(hints, "ownership")

    def test_empty_results(self):
        assert compute_next_steps("churn", []) == []


class TestOwnershipHints:
    def test_with_results(self):
        result = [{"author": "Alice", "percentage": 80}]
        hints = compute_next_steps("ownership", result)
        assert _has_tool(hints, "who_reviews")

    def test_empty_results(self):
        assert compute_next_steps("ownership", []) == []


class TestCouplingHints:
    def test_with_results(self):
        result = [{"file_a": "app.py", "file_b": "utils.py", "co_commit_count": 5}]
        hints = compute_next_steps("coupling", result)
        assert _has_tool(hints, "risk_map")
        assert _has_tool(hints, "impact")
        impact_hint = next(h for h in hints if h.get("tool") == "impact")
        assert "utils.py" in impact_hint["args"]["files"]

    def test_empty_results(self):
        assert compute_next_steps("coupling", []) == []


class TestWhoReviewsHints:
    def test_with_results(self):
        result = [{"author": "Bob", "role": "suggested_reviewer"}]
        hints = compute_next_steps("who_reviews", result)
        assert _has_tool(hints, "ownership")

    def test_empty_results(self):
        assert compute_next_steps("who_reviews", []) == []


class TestHistoryHints:
    def test_with_results(self):
        result = [{"hash": "abc", "date": "2026-03-01", "author": "Alice", "message": "fix"}]
        hints = compute_next_steps("history", result)
        assert _has_tool(hints, "churn")

    def test_empty_results(self):
        assert compute_next_steps("history", []) == []


class TestStatsHints:
    def test_empty_database(self):
        result = {"code_units": 0, "test_units": 0, "commits": 0, "hint": "Run analyze"}
        hints = compute_next_steps("stats", result)
        assert _has_tool(hints, "analyze")

    def test_populated_database(self):
        result = {"code_units": 50, "test_units": 20, "commits": 100, "co_changes": 5}
        hints = compute_next_steps("stats", result)
        assert _has_tool(hints, "triage")

    def test_no_co_changes_warns(self):
        result = {"code_units": 50, "commits": 200, "co_changes": 0, "coupling_threshold": 8}
        hints = compute_next_steps("stats", result)
        assert _has_action(hints, "review_threshold")
        assert _has_reason(hints, "threshold")


class TestRecordResultHints:
    def test_after_recording(self):
        result = {"test_id": "t1", "passed": True, "recorded": True}
        hints = compute_next_steps("record_result", result)
        assert _has_tool(hints, "suggest_tests")

    def test_failed_record(self):
        assert compute_next_steps("record_result", {}) == []


class TestStructuredFormat:
    """Verify all hints follow the structured dict format."""

    def test_all_hints_are_dicts(self):
        """Every hint must be a dict with either 'tool' or 'action' + 'reason'."""
        cases = [
            ("analyze", {"code_files_scanned": 10}),
            ("update", {"files_updated": 3, "new_commits": 1}),
            ("risk_map", {"files": [{"file_path": "a.py", "risk_score": 0.5, "breakdown": {"churn": 0.3, "coupling": 0.1}}], "_meta": {}}),
            ("diff_impact", [{"test_id": "t", "reason": "direct"}]),
            ("diff_impact", {"status": "no_changes", "ref": "HEAD", "branch": "main", "message": "..."}),
            ("test_gaps", [{"file_path": "a.py", "name": "f", "unit_type": "function"}]),
            ("stale_tests", [{"test_id": "t", "edge_type": "import"}]),
            ("impact", [{"test_id": "t", "reason": "direct"}]),
            ("suggest_tests", [{"test_id": "t"}]),
            ("triage", {"top_risk_files": [{"file_path": "a.py", "risk_score": 0.5}], "test_gaps": [], "stale_tests": [], "summary": {"total_test_gaps": 0}}),
            ("churn", [{"file_path": "a.py", "churn_score": 1.0}]),
            ("ownership", [{"author": "Alice", "percentage": 100}]),
            ("coupling", [{"file_a": "a.py", "file_b": "b.py", "co_commit_count": 5}]),
            ("who_reviews", [{"author": "Bob", "role": "suggested_reviewer"}]),
            ("history", [{"hash": "abc", "date": "2026-03-01", "author": "A", "message": "x"}]),
            ("stats", {"code_units": 50, "commits": 100, "co_changes": 5}),
            ("record_result", {"test_id": "t1", "passed": True, "recorded": True}),
        ]
        for tool_name, result in cases:
            hints = compute_next_steps(tool_name, result)
            for hint in hints:
                assert isinstance(hint, dict), f"{tool_name}: hint is not a dict: {hint}"
                assert "reason" in hint, f"{tool_name}: hint missing 'reason': {hint}"
                assert "tool" in hint or "action" in hint, f"{tool_name}: hint needs 'tool' or 'action': {hint}"
                if "tool" in hint:
                    assert isinstance(hint["args"], dict), f"{tool_name}: hint 'args' must be dict: {hint}"
