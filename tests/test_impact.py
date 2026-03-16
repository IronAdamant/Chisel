"""Tests for chisel.impact — impacted tests, risk scoring, stale detection."""

import pytest

from chisel.impact import ImpactAnalyzer, _author_concentration
from chisel.storage import Storage


@pytest.fixture
def storage(tmp_path):
    s = Storage(base_dir=tmp_path / "chisel_data")
    yield s
    s.close()


@pytest.fixture
def analyzer(storage):
    return ImpactAnalyzer(storage)


def _seed_basic_data(storage):
    """Populate storage with a small, known graph for testing."""
    # Code units
    storage.upsert_code_unit("app.py:foo:func", "app.py", "foo", "func", 1, 10)
    storage.upsert_code_unit("app.py:bar:func", "app.py", "bar", "func", 12, 20)
    storage.upsert_code_unit("lib.py:helper:func", "lib.py", "helper", "func", 1, 5)

    # Test units
    storage.upsert_test_unit("test_app.py:test_foo", "test_app.py", "test_foo", "pytest")
    storage.upsert_test_unit("test_app.py:test_bar", "test_app.py", "test_bar", "pytest")
    storage.upsert_test_unit("test_lib.py:test_helper", "test_lib.py", "test_helper", "pytest")

    # Test edges
    storage.upsert_test_edge("test_app.py:test_foo", "app.py:foo:func", "import", 2.0)
    storage.upsert_test_edge("test_app.py:test_bar", "app.py:bar:func", "call", 1.0)
    storage.upsert_test_edge("test_lib.py:test_helper", "lib.py:helper:func", "import", 1.0)

    # Co-change coupling: app.py <-> lib.py
    storage.upsert_co_change("app.py", "lib.py", 5, "abc123")

    # Churn stats
    storage.upsert_churn_stat("app.py", "", 10, 3, 50, 20, "2026-03-01", 2.5)
    storage.upsert_churn_stat("lib.py", "", 2, 1, 5, 2, "2026-01-01", 0.3)

    # File hashes (for blame lookups)
    storage.set_file_hash("app.py", "hash_app")

    # Blame data
    storage.store_blame("app.py", 1, 15, "abc", "Alice", "a@b.com", "2026-03-01", "hash_app")
    storage.store_blame("app.py", 16, 20, "def", "Bob", "b@b.com", "2026-02-01", "hash_app")


class TestGetImpactedTests:
    def test_direct_impact(self, storage, analyzer):
        _seed_basic_data(storage)
        result = analyzer.get_impacted_tests(["app.py"])
        test_ids = [r["test_id"] for r in result]
        assert "test_app.py:test_foo" in test_ids
        assert "test_app.py:test_bar" in test_ids

    def test_transitive_via_co_change(self, storage, analyzer):
        _seed_basic_data(storage)
        result = analyzer.get_impacted_tests(["app.py"])
        test_ids = [r["test_id"] for r in result]
        # lib.py is co-changed with app.py, so test_helper should show up
        assert "test_lib.py:test_helper" in test_ids

    def test_function_filter(self, storage, analyzer):
        _seed_basic_data(storage)
        result = analyzer.get_impacted_tests(["app.py"], changed_functions=["foo"])
        test_ids = [r["test_id"] for r in result]
        assert "test_app.py:test_foo" in test_ids
        # bar not in changed_functions, but may show via co-change
        # Direct edge to bar should NOT be in results
        direct_bar = [r for r in result if "direct" in r["reason"] and "bar" in r["reason"]]
        assert len(direct_bar) == 0

    def test_empty_changed_files(self, storage, analyzer):
        _seed_basic_data(storage)
        assert analyzer.get_impacted_tests([]) == []

    def test_unknown_file(self, storage, analyzer):
        _seed_basic_data(storage)
        result = analyzer.get_impacted_tests(["nonexistent.py"])
        assert result == []

    def test_sorted_by_score(self, storage, analyzer):
        _seed_basic_data(storage)
        result = analyzer.get_impacted_tests(["app.py"])
        if len(result) >= 2:
            assert result[0]["score"] >= result[1]["score"]


class TestRiskScore:
    def test_basic_risk(self, storage, analyzer):
        _seed_basic_data(storage)
        risk = analyzer.compute_risk_score("app.py")
        assert 0 <= risk["risk_score"] <= 1
        assert "breakdown" in risk

    def test_breakdown_components(self, storage, analyzer):
        _seed_basic_data(storage)
        risk = analyzer.compute_risk_score("app.py")
        bd = risk["breakdown"]
        assert "churn" in bd
        assert "coupling" in bd
        assert "coverage_gap" in bd
        assert "author_concentration" in bd

    def test_low_risk_file(self, storage, analyzer):
        _seed_basic_data(storage)
        risk_lib = analyzer.compute_risk_score("lib.py")
        risk_app = analyzer.compute_risk_score("app.py")
        # lib.py has lower churn, should generally have lower risk
        assert risk_lib["risk_score"] <= risk_app["risk_score"]

    def test_unknown_file_zero_risk(self, storage, analyzer):
        risk = analyzer.compute_risk_score("nonexistent.py")
        # Should not crash, returns some default
        assert risk["risk_score"] >= 0


class TestSuggestTests:
    def test_suggest_returns_results(self, storage, analyzer):
        _seed_basic_data(storage)
        suggestions = analyzer.suggest_tests("app.py")
        assert len(suggestions) > 0

    def test_suggest_has_fields(self, storage, analyzer):
        _seed_basic_data(storage)
        suggestions = analyzer.suggest_tests("app.py")
        for s in suggestions:
            assert "test_id" in s
            assert "relevance" in s
            assert "reason" in s


class TestStaleTests:
    def test_no_stale_when_all_exist(self, storage, analyzer):
        _seed_basic_data(storage)
        stale = analyzer.detect_stale_tests()
        assert stale == []

    def test_detects_stale_edge(self, storage, analyzer):
        _seed_basic_data(storage)
        # Add an edge to a nonexistent code unit
        storage.upsert_test_edge("test_app.py:test_foo", "removed.py:old_func:func", "import")
        stale = analyzer.detect_stale_tests()
        assert len(stale) == 1
        assert stale[0]["missing_code_id"] == "removed.py:old_func:func"


class TestRiskMap:
    def test_risk_map_returns_all_files(self, storage, analyzer):
        _seed_basic_data(storage)
        risk_map = analyzer.get_risk_map()
        files = [r["file_path"] for r in risk_map]
        assert "app.py" in files
        assert "lib.py" in files

    def test_risk_map_sorted_desc(self, storage, analyzer):
        _seed_basic_data(storage)
        risk_map = analyzer.get_risk_map()
        if len(risk_map) >= 2:
            assert risk_map[0]["risk_score"] >= risk_map[1]["risk_score"]

    def test_risk_map_with_directory(self, storage, analyzer):
        _seed_basic_data(storage)
        # Only app.py starts with "app"
        risk_map = analyzer.get_risk_map(directory="app")
        files = [r["file_path"] for r in risk_map]
        assert "app.py" in files
        assert "lib.py" not in files


class TestGetOwnership:
    def test_returns_authors(self, storage, analyzer):
        _seed_basic_data(storage)
        owners = analyzer.get_ownership("app.py")
        assert len(owners) > 0
        authors = [r["author"] for r in owners]
        assert "Alice" in authors

    def test_percentages_sum_to_100(self, storage, analyzer):
        _seed_basic_data(storage)
        owners = analyzer.get_ownership("app.py")
        total = sum(r["percentage"] for r in owners)
        assert abs(total - 100.0) < 0.1

    def test_role_is_original_author(self, storage, analyzer):
        _seed_basic_data(storage)
        owners = analyzer.get_ownership("app.py")
        for o in owners:
            assert o["role"] == "original_author"

    def test_unknown_file(self, storage, analyzer):
        assert analyzer.get_ownership("nonexistent.py") == []


class TestSuggestReviewers:
    def test_returns_reviewers_from_commits(self, storage, analyzer):
        _seed_basic_data(storage)
        storage.upsert_commit("c1", "Alice", "a@b.com", "2026-03-10T00:00:00+00:00", "fix")
        storage.upsert_commit_file("c1", "app.py", 10, 2)
        storage.upsert_commit("c2", "Bob", "b@b.com", "2026-03-12T00:00:00+00:00", "refactor")
        storage.upsert_commit_file("c2", "app.py", 5, 3)
        reviewers = analyzer.suggest_reviewers("app.py")
        assert len(reviewers) > 0
        assert all(r["role"] == "suggested_reviewer" for r in reviewers)
        authors = [r["author"] for r in reviewers]
        assert "Alice" in authors
        assert "Bob" in authors

    def test_unknown_file(self, storage, analyzer):
        assert analyzer.suggest_reviewers("nonexistent.py") == []


class TestAuthorConcentration:
    def test_single_author(self):
        blocks = [{"author": "Alice", "line_start": 1, "line_end": 100}]
        assert _author_concentration(blocks) == 1.0

    def test_two_equal_authors(self):
        blocks = [
            {"author": "Alice", "line_start": 1, "line_end": 50},
            {"author": "Bob", "line_start": 51, "line_end": 100},
        ]
        conc = _author_concentration(blocks)
        assert abs(conc - 0.5) < 0.01  # HHI = 0.25 + 0.25 = 0.5

    def test_empty_blame(self):
        assert _author_concentration([]) == 1.0
