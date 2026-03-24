"""Tests for chisel.next_steps — contextual next-step suggestions."""

from chisel.next_steps import compute_next_steps


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
        assert any("risk_map" in h for h in hints)
        assert any("test_gaps" in h for h in hints)

    def test_non_analyze_result(self):
        assert compute_next_steps("analyze", []) == []


class TestUpdateHints:
    def test_after_update_with_changes(self):
        result = {"files_updated": 3, "new_commits": 1}
        hints = compute_next_steps("update", result)
        assert len(hints) >= 1
        assert any("diff_impact" in h for h in hints)

    def test_after_update_no_changes(self):
        result = {"files_updated": 0, "new_commits": 0}
        hints = compute_next_steps("update", result)
        assert hints == []


class TestRiskMapHints:
    def test_with_results(self):
        result = [
            {"file_path": "a.py", "risk_score": 0.8, "breakdown": {"churn": 0.7, "coupling": 0.4}},
            {"file_path": "b.py", "risk_score": 0.5, "breakdown": {"churn": 0.2, "coupling": 0.1}},
        ]
        hints = compute_next_steps("risk_map", result)
        assert len(hints) >= 2
        assert any("test_gaps" in h for h in hints)
        assert any("coupling" in h for h in hints)
        assert any("churn" in h for h in hints)

    def test_empty_results(self):
        hints = compute_next_steps("risk_map", [])
        assert any("analyze" in h for h in hints)

    def test_low_coupling_no_coupling_hint(self):
        result = [
            {"file_path": "a.py", "risk_score": 0.3, "breakdown": {"churn": 0.1, "coupling": 0.1}},
        ]
        hints = compute_next_steps("risk_map", result)
        # Should not suggest coupling drill-down for low coupling
        coupling_hints = [h for h in hints if h.startswith("Run 'coupling")]
        assert coupling_hints == []


class TestDiffImpactHints:
    def test_with_impacted_tests(self):
        result = [{"test_id": "test_foo", "reason": "direct"}]
        hints = compute_next_steps("diff_impact", result)
        assert any("record_result" in h for h in hints)
        assert any("coupling" in h for h in hints)

    def test_no_impacted_tests(self):
        hints = compute_next_steps("diff_impact", [])
        assert any("test_gaps" in h for h in hints)


class TestTestGapsHints:
    def test_with_gaps(self):
        result = [{"file_path": "core.py", "name": "process", "unit_type": "function"}]
        hints = compute_next_steps("test_gaps", result)
        assert any("churn" in h for h in hints)
        assert any("ownership" in h for h in hints)

    def test_no_gaps(self):
        hints = compute_next_steps("test_gaps", [])
        assert any("coverage" in h.lower() for h in hints)


class TestStaleTestsHints:
    def test_with_stale(self):
        result = [{"test_id": "test_old", "edge_type": "import"}]
        hints = compute_next_steps("stale_tests", result)
        assert any("update" in h.lower() for h in hints)

    def test_no_stale(self):
        hints = compute_next_steps("stale_tests", [])
        assert any("current" in h.lower() for h in hints)


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
        assert any("suggest_tests" in h for h in hints)

    def test_no_gaps(self):
        result = {
            "top_risk_files": [{"file_path": "a.py", "risk_score": 0.3}],
            "test_gaps": [],
            "stale_tests": [],
            "summary": {"files_triaged": 1, "total_test_gaps": 0, "total_stale_tests": 0},
        }
        hints = compute_next_steps("triage", result)
        # Should not have "focus on files appearing in both sections"
        assert not any("both" in h.lower() for h in hints)
