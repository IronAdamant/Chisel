"""Tests for chisel.impact — impacted tests, risk scoring, stale detection."""

import pytest

from chisel.impact import ImpactAnalyzer, _author_concentration, _fetch_failure_rates, _test_instability


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

    def test_untracked_ignores_function_filter(self, storage, analyzer):
        _seed_basic_data(storage)
        result = analyzer.get_impacted_tests(
            ["app.py"],
            changed_functions=["foo"],
            untracked_files={"app.py"},
        )
        test_ids = [r["test_id"] for r in result]
        assert "test_app.py:test_foo" in test_ids
        assert "test_app.py:test_bar" in test_ids

    def test_empty_changed_files(self, storage, analyzer):
        _seed_basic_data(storage)
        assert analyzer.get_impacted_tests([]) == []

    def test_unknown_file(self, storage, analyzer):
        _seed_basic_data(storage)
        result = analyzer.get_impacted_tests(["nonexistent.py"])
        assert result == []

    def test_transitive_via_import_graph(self, storage, analyzer):
        """Inner file has no test edges; facade imports it and has tests."""
        storage.upsert_code_unit("deep.py:core:func", "deep.py", "core", "func", 1, 5)
        storage.upsert_code_unit("facade.py:run:func", "facade.py", "run", "func", 1, 10)
        storage.upsert_test_unit("test_facade.py:test_e2e", "test_facade.py", "test_e2e", "pytest")
        storage.upsert_test_edge(
            "test_facade.py:test_e2e", "facade.py:run:func", "import", 1.0,
        )
        storage.upsert_import_edge("facade.py", "deep.py")

        result = analyzer.get_impacted_tests(["deep.py"])
        ids = [r["test_id"] for r in result]
        assert "test_facade.py:test_e2e" in ids
        assert any("import graph" in r["reason"] for r in result)

    def test_sorted_by_score(self, storage, analyzer):
        _seed_basic_data(storage)
        result = analyzer.get_impacted_tests(["app.py"])
        assert len(result) >= 2
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
        assert "import_coupling" in bd
        assert "cochange_coupling" in bd
        assert "cochange_global" in bd
        assert "cochange_branch" in bd
        assert "coverage_gap" in bd
        assert "coverage_fraction" in bd
        assert "coverage_depth" in bd
        assert "edge_type_quality" in bd
        assert "author_concentration" in bd
        assert "test_instability" in bd

    def test_coverage_depth_and_quality(self, storage, analyzer):
        """Coverage depth and edge type quality are computed from edge data."""
        _seed_basic_data(storage)
        risk = analyzer.compute_risk_score("app.py")
        bd = risk["breakdown"]
        # app.py: foo has import edge, bar has call edge, 2 distinct test files
        assert 0.0 <= bd["coverage_depth"] <= 1.0
        assert 0.0 <= bd["edge_type_quality"] <= 1.0
        # edge_type_quality = call_edges / total_edges = 1/2 = 0.5
        assert bd["edge_type_quality"] == 0.5

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
            assert "source" in s


class TestSuggestTestsFailureBoost:
    def test_failure_boosts_relevance(self, storage, analyzer):
        _seed_basic_data(storage)
        # Get baseline suggestions without any recorded results
        baseline = analyzer.suggest_tests("app.py")
        assert len(baseline) > 0
        baseline_scores = {s["test_id"]: s["relevance"] for s in baseline}

        # Record failures for one test
        storage.record_test_result("test_app.py:test_foo", False)
        storage.record_test_result("test_app.py:test_foo", False)

        boosted = analyzer.suggest_tests("app.py")
        boosted_scores = {s["test_id"]: s["relevance"] for s in boosted}

        # The failed test should have a higher relevance than baseline
        assert boosted_scores["test_app.py:test_foo"] > baseline_scores["test_app.py:test_foo"]

    def test_no_results_no_boost(self, storage, analyzer):
        _seed_basic_data(storage)
        # With no recorded results, suggest_tests should still work
        suggestions = analyzer.suggest_tests("app.py")
        assert len(suggestions) > 0


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
        assert len(risk_map) >= 2
        assert risk_map[0]["risk_score"] >= risk_map[1]["risk_score"]

    def test_risk_map_with_directory(self, storage, analyzer):
        # Seed with directory-style paths to test proper path boundary matching
        storage.upsert_churn_stat("src/app.py", "", churn_score=3.0)
        storage.upsert_churn_stat("lib/helper.py", "", churn_score=2.0)
        risk_map = analyzer.get_risk_map(directory="src")
        files = [r["file_path"] for r in risk_map]
        assert "src/app.py" in files
        assert "lib/helper.py" not in files

    def test_risk_map_includes_coupling_partners(self, storage, analyzer):
        _seed_basic_data(storage)
        risk_map = analyzer.get_risk_map()
        app_entry = next(r for r in risk_map if r["file_path"] == "app.py")
        assert "coupling_partners" in app_entry
        # app.py <-> lib.py has 5 co-commits in seed data
        partners = app_entry["coupling_partners"]
        assert len(partners) >= 1
        assert partners[0]["file"] == "lib.py"
        assert partners[0]["co_commits"] == 5

    def test_risk_map_coupling_partners_empty_when_no_coupling(self, storage, analyzer):
        # File with churn but no co-changes
        storage.upsert_churn_stat("solo.py", "", churn_score=1.0)
        risk_map = analyzer.get_risk_map()
        solo = next(r for r in risk_map if r["file_path"] == "solo.py")
        assert solo["coupling_partners"] == []

    def test_coverage_gap_reflects_test_edges(self, storage, analyzer):
        """Files with test edges should have coverage_gap < 1.0."""
        _seed_basic_data(storage)
        risk_map = analyzer.get_risk_map()
        app = next(r for r in risk_map if r["file_path"] == "app.py")
        lib = next(r for r in risk_map if r["file_path"] == "lib.py")
        # app.py: foo and bar both have edges → 0/2 gap → 0.0
        assert app["breakdown"]["coverage_gap"] == 0.0
        # lib.py: helper has edge → 0/1 gap → 0.0
        assert lib["breakdown"]["coverage_gap"] == 0.0

    def test_coverage_gap_partial_coverage(self, storage, analyzer):
        """File with some tested and some untested units."""
        storage.upsert_code_unit("m.py:a:func", "m.py", "a", "func", 1, 5)
        storage.upsert_code_unit("m.py:b:func", "m.py", "b", "func", 6, 10)
        storage.upsert_code_unit("m.py:c:func", "m.py", "c", "func", 11, 15)
        storage.upsert_test_unit("test_m.py:t1", "test_m.py", "t1", "pytest")
        storage.upsert_test_edge("test_m.py:t1", "m.py:a:func", "import")
        storage.upsert_churn_stat("m.py", "", churn_score=1.0)
        risk_map = analyzer.get_risk_map()
        entry = next(r for r in risk_map if r["file_path"] == "m.py")
        # 1 of 3 tested → coverage 0.333, gap 0.667 → quantized to 0.75
        assert entry["breakdown"]["coverage_gap"] == 0.75

    def test_exclude_tests_filters_test_files(self, storage, analyzer):
        """Test files should be excluded from risk_map by default."""
        _seed_basic_data(storage)
        # test_app.py has churn (from seed) — would appear without filtering
        storage.upsert_churn_stat("test_app.py", "", churn_score=1.0)
        risk_map = analyzer.get_risk_map()
        files = [r["file_path"] for r in risk_map]
        assert "test_app.py" not in files
        assert "app.py" in files

    def test_exclude_tests_false_includes_test_files(self, storage, analyzer):
        """exclude_tests=False includes test files."""
        _seed_basic_data(storage)
        storage.upsert_churn_stat("test_app.py", "", churn_score=1.0)
        risk_map = analyzer.get_risk_map(exclude_tests=False)
        files = [r["file_path"] for r in risk_map]
        assert "test_app.py" in files


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


class TestTestInstability:
    def test_no_results(self):
        assert _test_instability({"t1", "t2"}, {}) == 0.0

    def test_no_test_ids(self):
        assert _test_instability(set(), {}) == 0.0

    def test_with_failures(self):
        rate = _test_instability({"t1"}, {"t1": 1.0})
        assert rate == 1.0

    def test_mixed_results(self):
        rate = _test_instability({"t1"}, {"t1": 0.5})
        assert abs(rate - 0.5) < 0.01

    def test_duration_cv_when_no_fail_rates(self):
        cv = _test_instability(
            {"t1"}, {}, {"t1": 0.4, "t2": 0.2},
        )
        assert 0 < cv <= 1.0

    def test_fetch_failure_rates(self, storage):
        storage.record_test_result("t1", False)
        storage.record_test_result("t1", True)
        rates = _fetch_failure_rates(storage)
        assert abs(rates["t1"] - 0.5) < 0.01

    def test_risk_score_includes_instability(self, storage, analyzer):
        _seed_basic_data(storage)
        # Record failures for a test covering app.py
        storage.record_test_result("test_app.py:test_foo", False)
        storage.record_test_result("test_app.py:test_foo", False)
        risk = analyzer.compute_risk_score("app.py")
        assert risk["breakdown"]["test_instability"] > 0


class TestTarjanSCC:
    """Tests for _tarjan_scc and _find_circular_dependencies."""

    def test_detects_simple_cycle(self):
        from chisel.impact import _tarjan_scc
        # a -> b -> c -> a
        nodes = ["a", "b", "c"]
        neighbors = {"a": ["b"], "b": ["c"], "c": ["a"]}
        sccs = _tarjan_scc(nodes, lambda n: neighbors.get(n, []))
        cycle_sccs = [s for s in sccs if len(s) > 1]
        assert len(cycle_sccs) == 1
        assert set(cycle_sccs[0]) == {"a", "b", "c"}

    def test_no_cycle(self):
        from chisel.impact import _tarjan_scc
        # a -> b -> c (linear, no cycle)
        nodes = ["a", "b", "c"]
        neighbors = {"a": ["b"], "b": ["c"]}
        sccs = _tarjan_scc(nodes, lambda n: neighbors.get(n, []))
        cycle_sccs = [s for s in sccs if len(s) > 1]
        assert cycle_sccs == []

    def test_two_independent_cycles(self):
        from chisel.impact import _tarjan_scc
        # a -> b -> a and c -> d -> c
        nodes = ["a", "b", "c", "d"]
        neighbors = {"a": ["b"], "b": ["a"], "c": ["d"], "d": ["c"]}
        sccs = _tarjan_scc(nodes, lambda n: neighbors.get(n, []))
        cycle_sccs = [s for s in sccs if len(s) > 1]
        assert len(cycle_sccs) == 2

    def test_find_circular_dependencies_returns_top_3(self):
        from chisel.impact import _find_circular_dependencies
        import_neighbors = {
            "a": ["b"], "b": ["c"], "c": ["a"],  # 3-cycle
            "x": ["y"], "y": ["z"], "z": ["x"],  # 3-cycle
            "m": ["n"], "n": ["o"], "o": ["m"],  # 3-cycle
            "p": [],  # no cycle
        }
        cycles = _find_circular_dependencies(
            {"a", "b", "c", "x", "y", "z", "m", "n", "o", "p"}, import_neighbors,
        )
        assert len(cycles) == 3  # top-3
        # All cycles should be length 3 and sorted by length desc
        assert all(c["length"] == 3 for c in cycles)

    def test_find_circular_dependencies_skips_orphans(self):
        from chisel.impact import _find_circular_dependencies
        import_neighbors = {
            "a": ["b"],
            "b": ["a"],  # cycle
            "c": [],  # no cycle
        }
        cycles = _find_circular_dependencies({"a", "b", "c"}, import_neighbors)
        assert len(cycles) == 1
        assert set(cycles[0]["files"]) == {"a", "b"}


class TestDetectPluginSignals:
    """Tests for detect_plugin_signals utility function."""

    def test_detects_plugin_registry_calls(self):
        from chisel.impact import detect_plugin_signals
        content = "registerPlugin('my-plugin', handler);"
        signals = detect_plugin_signals(content)
        assert signals["has_plugin_registry"] is True

    def test_detects_plugin_manager_class(self):
        from chisel.impact import detect_plugin_signals
        content = "class PluginManager { load() {} }"
        signals = detect_plugin_signals(content)
        assert signals["has_plugin_manager"] is True

    def test_detects_plugin_config_require(self):
        from chisel.impact import detect_plugin_signals
        content = "const plugins = require('./config/plugins');"
        signals = detect_plugin_signals(content)
        assert signals["has_plugin_config"] is True

    def test_detects_plugin_dir_reference(self):
        from chisel.impact import detect_plugin_signals
        content = "const ext = await import('./extensions/my-ext');"
        signals = detect_plugin_signals(content)
        assert signals["has_plugin_dir_ref"] is True

    def test_no_false_positives_on_clean_code(self):
        from chisel.impact import detect_plugin_signals
        content = "function processData(input) { return input.map(x => x * 2); }"
        signals = detect_plugin_signals(content)
        assert all(v is False for v in signals.values())
