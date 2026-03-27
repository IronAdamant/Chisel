"""Impact analysis, risk scoring, stale test detection, and reviewer suggestions."""

import re
from collections import defaultdict, deque
from datetime import datetime, timezone

from chisel.metrics import _parse_iso_date, compute_ownership

# Co-change coupling: breadth of partners (normalized by this count).
_COCHANGE_COUPLING_CAP = 10
# Static import-graph coupling: distinct neighbor files (either direction).
_IMPORT_COUPLING_CAP = 8

_PROXIMITY_ALPHA = 0.15
_PROXIMITY_CAP_HOPS = 5

_QUANTIZE_STEPS = 10

# Static import-graph test impact: multiplier and hop decay (see get_impacted_tests).
_IMPORT_GRAPH_TEST_WEIGHT = 0.45
_IMPORT_HOP_DECAY = 0.88
_MAX_IMPORT_CLOSURE_HOPS = 32


def _quantize_gap(value, steps=4):
    """Quantize coverage_gap to fixed steps for graduated risk levels."""
    return round(value * steps) / steps


def _import_hops_to_tested(all_files, tested_files, neighbors_batch):
    """BFS distance from multi-source *tested_files* over undirected import edges."""
    dist = {fp: 0 for fp in tested_files}
    q = deque(tested_files)
    while q:
        u = q.popleft()
        for v in neighbors_batch.get(u, []):
            if v in all_files and v not in dist:
                dist[v] = dist[u] + 1
                q.append(v)
    return dist


def _apply_coverage_proximity(coverage_gap, min_hops):
    """Slightly reduce coverage_gap when *min_hops* from a tested file (import graph)."""
    factor = 1.0 - _PROXIMITY_ALPHA * max(
        0, (_PROXIMITY_CAP_HOPS - min_hops),
    ) / float(_PROXIMITY_CAP_HOPS)
    return coverage_gap * max(0.0, factor)


def _tarjan_scc(nodes, neighbors_func):
    """Tarjan's SCC algorithm.

    Args:
        nodes: iterable of node identifiers.
        neighbors_func: callable(node) -> list of neighbor nodes.

    Returns:
        List of SCCs, each SCC is a list of nodes. SCCs with size > 1 are cycles.
    """
    index_counter = [0]
    stack = []
    lowlinks = {}
    index = {}
    on_stack = {}
    sccs = []

    def strongconnect(v):
        index[v] = lowlinks[v] = index_counter[0]
        index_counter[0] += 1
        stack.append(v)
        on_stack[v] = True
        for w in neighbors_func(v):
            if w not in index:
                strongconnect(w)
                lowlinks[v] = min(lowlinks[v], lowlinks[w])
            elif on_stack.get(w, False):
                lowlinks[v] = min(lowlinks[v], index[w])
        if lowlinks[v] == index[v]:
            scc = []
            while stack[-1] != v:
                w = stack.pop()
                on_stack[w] = False
                scc.append(w)
            stack.pop()
            on_stack[v] = False
            scc.append(v)
            sccs.append(scc)

    for v in nodes:
        if v not in index:
            strongconnect(v)
    return sccs


def _find_circular_dependencies(all_files, neighbors_batch):
    """Find circular dependencies in the import graph via Tarjan's SCC.

    Returns top-3 cycles sorted by severity (largest first), each as
    ``{"files": [...], "length": N}``.
    """
    def neighbors(fp):
        return neighbors_batch.get(fp, [])

    sccs = _tarjan_scc(list(all_files), neighbors)
    cycles = [
        {"files": sorted(scc), "length": len(scc)}
        for scc in sccs if len(scc) > 1
    ]
    cycles.sort(key=lambda c: c["length"], reverse=True)
    return cycles[:3]


# ------------------------------------------------------------------ #
# Plugin / dynamic require awareness
# ------------------------------------------------------------------ #

_PLUGIN_REGISTRY_FUNCS = re.compile(
    r"\b(?:registerPlugin|loadPlugin|addPlugin|"
    r"installPlugin|enabledPlugins?|getPlugin|hasPlugin)\s*\(",
)
_PLUGIN_MANAGER_CLASSES = re.compile(
    r"\b(?:PluginManager|ModuleRegistry|ExtensionRegistry|"
    r"PluginLoader|ModuleLoader)\b",
)
_PLUGIN_DIRS = re.compile(
    r"\b(?:plugins?|extensions?|addons?)\b",
)
_PLUGIN_CONFIG_RE = re.compile(
    r"""require\s*\(\s*['\"](?:[^'\"]*[/\\])?(?:plugins?|extensions?|builtin)['\"]\s*\)""",
)


def detect_plugin_signals(content):
    """Return plugin-related signals found in file content.

    For awareness only — not used in coupling/risk scoring.
    ``require(variable)`` and dynamic ``import()`` are fundamentally invisible
    to static analysis.
    """
    return {
        "has_plugin_registry": bool(_PLUGIN_REGISTRY_FUNCS.search(content)),
        "has_plugin_manager": bool(_PLUGIN_MANAGER_CLASSES.search(content)),
        "has_plugin_dir_ref": bool(_PLUGIN_DIRS.search(content)),
        "has_plugin_config": bool(_PLUGIN_CONFIG_RE.search(content)),
    }


class ImpactAnalyzer:
    """Analyzes test impact, risk, and ownership using data from Storage."""

    def __init__(self, storage):
        self.storage = storage

    def _import_graph_undirected_neighbors(self, file_path):
        """One-hop neighbors on the static import graph (both directions)."""
        im = self.storage.get_importers(file_path)
        out = self.storage.get_imported_files(file_path)
        return set(im) | set(out)

    def _import_static_closure(self, start):
        """Shortest-hop distances from *start* over undirected import edges.

        Used to find tests that cover modules reachable from *start* via
        imports (e.g. inner module only exercised through a facade test).
        """
        visited = {start: 0}
        q = deque([start])
        while q:
            u = q.popleft()
            du = visited[u]
            if du >= _MAX_IMPORT_CLOSURE_HOPS:
                continue
            for v in self._import_graph_undirected_neighbors(u):
                if v not in visited:
                    visited[v] = du + 1
                    q.append(v)
        return visited

    # ------------------------------------------------------------------ #
    # Impacted tests
    # ------------------------------------------------------------------ #

    def get_impacted_tests(self, changed_files, changed_functions=None,
                           untracked_files=None):
        """Find tests affected by the given file/function changes.

        Uses direct test edges, transitive co-change coupling, and static
        import-graph reachability (tests that cover modules connected via imports).

        Args:
            changed_files: List of changed file paths.
            changed_functions: Optional list of changed function names (tracked
                diffs).  Ignored for paths in *untracked_files* (whole-file).
            untracked_files: Optional set of paths that are untracked; those
                use full-file impact (no function-level diff available).

        Returns:
            List of dicts: {test_id, file_path, name, reason, score, source}
            ``source`` is ``direct`` | ``co_change`` | ``import_graph``.
        """
        impacted = {}
        untracked = untracked_files or set()

        # Direct hits via single JOIN query per file
        for file_path in changed_files:
            fn = None if file_path in untracked else changed_functions
            hits = self.storage.get_direct_impacted_tests(
                file_path, fn,
            )
            for hit in hits:
                new_score = hit["weight"]
                if hit["test_id"] not in impacted or new_score > impacted[hit["test_id"]]["score"]:
                    impacted[hit["test_id"]] = {
                        "test_id": hit["test_id"],
                        "file_path": hit["file_path"],
                        "name": hit["name"],
                        "reason": f"direct edge to {hit['code_name']} ({hit['edge_type']})",
                        "score": new_score,
                        "source": "direct",
                    }

        query_min = self.storage.get_co_change_query_min()
        # Transitive hits via co-change coupling
        for file_path in changed_files:
            co_changes = self.storage.get_co_changes(file_path, min_count=query_min)
            for cc in co_changes:
                coupled_file = cc["file_b"] if cc["file_a"] == file_path else cc["file_a"]
                hits = self.storage.get_direct_impacted_tests(coupled_file)
                for hit in hits:
                    new_score = hit["weight"] * 0.5
                    if hit["test_id"] not in impacted or new_score > impacted[hit["test_id"]]["score"]:
                        impacted[hit["test_id"]] = {
                            "test_id": hit["test_id"],
                            "file_path": hit["file_path"],
                            "name": hit["name"],
                            "reason": (
                                f"co-change coupling: {file_path} <-> {coupled_file}"
                                f" ({cc['co_commit_count']} commits)"
                            ),
                            "score": new_score,
                            "source": "co_change",
                        }

        # Transitive hits via static import graph (facade / barrel patterns)
        for file_path in changed_files:
            if file_path in untracked:
                continue
            closure = self._import_static_closure(file_path)
            for other, hops in closure.items():
                if other == file_path:
                    continue
                hits = self.storage.get_direct_impacted_tests(other, None)
                decay = _IMPORT_HOP_DECAY ** hops
                for hit in hits:
                    new_score = hit["weight"] * _IMPORT_GRAPH_TEST_WEIGHT * decay
                    reason = (
                        f"import graph: tests cover {other} "
                        f"({hops} hop{'s' if hops != 1 else ''} from {file_path})"
                    )
                    if hit["test_id"] not in impacted or new_score > impacted[hit["test_id"]]["score"]:
                        impacted[hit["test_id"]] = {
                            "test_id": hit["test_id"],
                            "file_path": hit["file_path"],
                            "name": hit["name"],
                            "reason": reason,
                            "score": new_score,
                            "source": "import_graph",
                        }

        result = list(impacted.values())
        result.sort(key=lambda x: x["score"], reverse=True)
        return result

    # ------------------------------------------------------------------ #
    # Risk scoring
    # ------------------------------------------------------------------ #

    def compute_risk_score(self, file_path, unit_name=None, failure_rates=None,
                           coverage_mode="unit"):
        """Compute a risk score for a file or function.

        Formula: 0.35*churn + 0.25*coupling + 0.2*coverage_gap
                 + 0.1*author_concentration + 0.1*test_instability

        Args:
            failure_rates: Optional pre-fetched dict of {test_id: rate}.
                           Fetched from storage if not provided.
            coverage_mode: "unit" (default) weights each code unit equally;
                           "line" weights by line count so large untested
                           units have proportionally higher coverage_gap.

        Returns:
            Dict: {file_path, unit_name, risk_score, breakdown}
        """
        # Churn component (0-1 normalized, cap at 1.0)
        churn_stat = self.storage.get_churn_stat(file_path, unit_name)
        churn_raw = churn_stat["churn_score"] if churn_stat else 0.0
        churn_norm = min(churn_raw / 5.0, 1.0)  # normalize: 5.0 score => 1.0

        # Coupling: max(git co-change breadth, static import-graph breadth)
        query_min = self.storage.get_co_change_query_min()
        co_global = self.storage.get_co_changes(file_path, min_count=query_min)
        co_branch = self.storage.get_branch_co_changes_batch(
            [file_path], min_count=1,
        ).get(file_path, [])
        cochange_global_norm = min(
            len(co_global) / float(_COCHANGE_COUPLING_CAP), 1.0,
        )
        cochange_branch_norm = min(
            len(co_branch) / float(_COCHANGE_COUPLING_CAP), 1.0,
        )
        cochange_coupling_norm = max(cochange_global_norm, cochange_branch_norm)
        import_neighbors = self.storage.get_import_neighbors_batch(
            [file_path],
        ).get(file_path, [])
        import_coupling_norm = min(
            len(import_neighbors) / float(_IMPORT_COUPLING_CAP), 1.0,
        )
        # Additive hybrid: both signals boost coupling, but co-change dominates
        # when present (it reflects actual change-history coupling).
        # Formula: use co-change if non-zero; otherwise import coupling;
        # if both are non-zero, use co-change + 0.25 * import (additive boost).
        if cochange_coupling_norm > 0:
            coupling_norm = max(
                cochange_coupling_norm,
                cochange_coupling_norm + 0.25 * import_coupling_norm,
            )
        else:
            coupling_norm = import_coupling_norm

        # Test coverage component (0-1, inverted)
        code_units = self.storage.get_code_units_by_file(file_path)
        if unit_name:
            code_units = [cu for cu in code_units if cu["name"] == unit_name]
        tested_count = 0
        tested_lines = 0
        total_lines = 0
        covering_test_ids = set()
        edge_type_counts = {"call": 0, "import": 0}
        for cu in code_units:
            unit_lines = cu["line_end"] - cu["line_start"] + 1
            total_lines += unit_lines
            edges = self.storage.get_edges_for_code(cu["id"])
            if edges:
                tested_count += 1
                tested_lines += unit_lines
                for e in edges:
                    covering_test_ids.add(e["test_id"])
                    edge_type_counts[e.get("edge_type", "import")] += 1
        if coverage_mode == "line" and total_lines > 0:
            coverage = tested_lines / total_lines
        else:
            coverage = tested_count / max(len(code_units), 1)
        coverage_gap = _quantize_gap(1.0 - coverage)
        coverage_fraction = round(coverage, 4)

        # Multi-dimensional coverage signals
        total_edges = sum(edge_type_counts.values())
        coverage_depth = round(min(len(covering_test_ids) / 5.0, 1.0), 4)
        edge_type_quality = round(
            edge_type_counts.get("call", 0) / max(total_edges, 1), 4,
        ) if total_edges > 0 else 0.0

        # Author concentration component (0-1)
        blame_data = self.storage.get_blame(file_path, _latest_hash(self.storage, file_path))
        author_conc = _author_concentration(blame_data)

        # Test instability: failure rate + duration CV of covering tests
        if failure_rates is None:
            failure_rates = _fetch_failure_rates(self.storage)
        duration_cv = self.storage.get_test_duration_cv_batch(
            list(covering_test_ids),
        )
        instability = _test_instability(
            covering_test_ids, failure_rates, duration_cv,
        )

        risk = (
            0.35 * churn_norm
            + 0.25 * coupling_norm
            + 0.2 * coverage_gap
            + 0.1 * author_conc
            + 0.1 * instability
        )
        return {
            "file_path": file_path,
            "unit_name": unit_name,
            "risk_score": round(risk, 4),
            "breakdown": {
                "churn": round(churn_norm, 4),
                "coupling": round(coupling_norm, 4),
                "import_coupling": round(import_coupling_norm, 4),
                "cochange_coupling": round(cochange_coupling_norm, 4),
                "cochange_global": round(cochange_global_norm, 4),
                "cochange_branch": round(cochange_branch_norm, 4),
                "coverage_gap": round(coverage_gap, 4),
                "coverage_fraction": coverage_fraction,
                "coverage_depth": coverage_depth,
                "edge_type_quality": edge_type_quality,
                "author_concentration": round(author_conc, 4),
                "test_instability": round(instability, 4),
            },
        }

    # ------------------------------------------------------------------ #
    # Test suggestions
    # ------------------------------------------------------------------ #

    def suggest_tests(self, file_path, fallback_to_all=False):
        """Suggest tests to run for a changed file, ordered by relevance.

        Uses ``get_impacted_tests`` (direct edges, co-change, import graph) and
        boosts by recorded test failure rates.

        Args:
            fallback_to_all: If True and no test edges exist for this file,
                return all known test files ranked by stem-match relevance.

        Returns:
            List of dicts: {test_id, file_path, name, relevance, reason, source}
        """
        impacted = self.get_impacted_tests([file_path])

        # Fallback: if no impacted tests and fallback requested, return all test files
        if not impacted and fallback_to_all:
            return self._fallback_suggest_tests(file_path)

        # Boost tests that have historically failed more often
        failure_rates = _fetch_failure_rates(self.storage)

        result = []
        for item in impacted:
            relevance = item["score"]
            fail_rate = failure_rates.get(item["test_id"], 0.0)
            # Boost by up to 50% based on historical failure rate
            relevance *= (1.0 + 0.5 * fail_rate)
            result.append({
                "test_id": item["test_id"],
                "file_path": item["file_path"],
                "name": item["name"],
                "relevance": relevance,
                "reason": item["reason"],
                "source": item.get("source", "direct"),
            })

        result.sort(key=lambda x: x["relevance"], reverse=True)
        return result

    def _fallback_suggest_tests(self, file_path):
        """Return all test files ranked by stem similarity to file_path.

        Used when a file has no test edges (new/unanalyzed files).
        """
        import os
        source_stem = os.path.splitext(os.path.basename(file_path))[0]
        all_test_files = self.storage.get_all_test_files()
        if not all_test_files:
            return []

        scored = []
        for test_file, test_names in all_test_files.items():
            test_stem = os.path.splitext(os.path.basename(test_file))[0]
            # Scoring: 1.0 exact stem match, 0.5 partial, 0.1 for all
            if test_stem == source_stem:
                score = 1.0
            elif source_stem in test_stem or test_stem in source_stem:
                score = 0.5
            else:
                score = 0.1

            for name in test_names:
                scored.append({
                    "test_id": f"{test_file}:{name}",
                    "file_path": test_file,
                    "name": name,
                    "relevance": score,
                    "reason": "fallback: stem-matched test file",
                    "source": "fallback",
                })

        scored.sort(key=lambda x: x["relevance"], reverse=True)
        return scored

    # ------------------------------------------------------------------ #
    # Stale test detection
    # ------------------------------------------------------------------ #

    def detect_stale_tests(self):
        """Find tests whose edges point to code units that no longer exist.

        Uses a single LEFT JOIN query instead of per-test lookups.

        Returns:
            List of dicts: {test_id, test_name, missing_code_id, edge_type}
        """
        rows = self.storage.get_stale_test_edges()
        return [
            {
                "test_id": r["test_id"],
                "test_name": r["test_name"],
                "missing_code_id": r["code_id"],
                "edge_type": r["edge_type"],
            }
            for r in rows
        ]

    # ------------------------------------------------------------------ #
    # Test gap detection
    # ------------------------------------------------------------------ #

    def get_test_gaps(self, file_path=None, directory=None, exclude_tests=True):
        """Find code units that have no test coverage, prioritized by churn.

        Args:
            file_path: Scope to a single file.
            directory: Scope to a directory (file_path takes precedence).
            exclude_tests: If True (default), exclude units from test files.

        Returns:
            List of dicts: {id, file_path, name, unit_type, line_start,
                            line_end, churn_score, commit_count}
        """
        return self.storage.get_untested_code_units(
            file_path=file_path,
            directory=directory if not file_path else None,
            exclude_tests=exclude_tests,
        )

    # ------------------------------------------------------------------ #
    # Risk map
    # ------------------------------------------------------------------ #

    def get_risk_map(self, directory=None, exclude_tests=True,
                     proximity_adjustment=False, coverage_mode="unit"):
        """Compute risk scores for all tracked files (optionally in a directory).

        Uses batch queries to avoid the N+1 pattern: fetches churn, coupling,
        code units, edges, and blame for all files in a small number of queries.

        Args:
            directory: Optional subdirectory to scope the risk map.
            exclude_tests: If True (default), exclude test files from the
                risk map.  Test files always score coverage_gap=1.0 (edges
                go *from* tests, never *to* test-file code units), which
                adds noise and masks real coverage differences.
            proximity_adjustment: If True, reduce ``coverage_gap`` slightly for
                files that are a few import hops from tested code.
            coverage_mode: "unit" (default) weights each code unit equally;
                "line" weights by line count so large untested units have
                proportionally higher coverage_gap.

        Returns:
            List of dicts: {file_path, risk_score, breakdown}
        """
        all_churn = self.storage.get_all_churn_stats()
        dir_prefix = directory.rstrip("/") + "/" if directory else ""
        test_files = self.storage.get_test_file_paths() if exclude_tests else set()
        files = sorted({
            stat["file_path"] for stat in all_churn
            if (not directory or stat["file_path"].startswith(dir_prefix))
            and stat["file_path"] not in test_files
        })
        if not files:
            return []

        failure_rates = _fetch_failure_rates(self.storage)
        query_min = self.storage.get_co_change_query_min()
        churn_batch = self.storage.get_churn_stats_batch(files)
        co_changes_batch = self.storage.get_co_changes_batch(files, min_count=query_min)
        branch_cc_batch = self.storage.get_branch_co_changes_batch(files, min_count=1)
        import_neighbors_batch = self.storage.get_import_neighbors_batch(files)
        code_units_batch = self.storage.get_code_units_by_files_batch(files)

        all_code_ids = [
            cu["id"] for cus in code_units_batch.values() for cu in cus
        ]
        edges_batch = self.storage.get_edges_for_code_batch(all_code_ids)

        all_test_ids = set()
        for cid in all_code_ids:
            for e in edges_batch.get(cid, []):
                all_test_ids.add(e["test_id"])
        duration_cv_by_test = self.storage.get_test_duration_cv_batch(
            list(all_test_ids),
        )

        tested_files = set()
        for fp in files:
            for cu in code_units_batch.get(fp, []):
                if edges_batch.get(cu["id"]):
                    tested_files.add(fp)
                    break

        hop_dist = {}
        if proximity_adjustment:
            hop_dist = _import_hops_to_tested(
                set(files), tested_files, import_neighbors_batch,
            )

        file_hash_pairs = [
            (fp, _latest_hash(self.storage, fp)) for fp in files
        ]
        blame_batch = self.storage.get_blame_batch(file_hash_pairs)

        risk_map = []
        for fp in files:
            churn_stat = churn_batch.get(fp)
            churn_raw = churn_stat["churn_score"] if churn_stat else 0.0
            churn_norm = min(churn_raw / 5.0, 1.0)

            co_changes = co_changes_batch.get(fp, [])
            branch_cc = branch_cc_batch.get(fp, [])
            cochange_global_norm = min(
                len(co_changes) / float(_COCHANGE_COUPLING_CAP), 1.0,
            )
            cochange_branch_norm = min(
                len(branch_cc) / float(_COCHANGE_COUPLING_CAP), 1.0,
            )
            cochange_coupling_norm = max(cochange_global_norm, cochange_branch_norm)

            import_neighbors = import_neighbors_batch.get(fp, [])
            import_coupling_norm = min(
                len(import_neighbors) / float(_IMPORT_COUPLING_CAP), 1.0,
            )
            # Additive hybrid: co-change dominates when present; import coupling
            # is the sole source in single-author projects. When both are non-zero,
            # additive boost reflects that files with both coupling types are more
            # tightly integrated than either alone.
            if cochange_coupling_norm > 0:
                coupling_norm = max(
                    cochange_coupling_norm,
                    cochange_coupling_norm + 0.25 * import_coupling_norm,
                )
            else:
                coupling_norm = import_coupling_norm

            sorted_cc = sorted(
                co_changes, key=lambda c: c["co_commit_count"], reverse=True,
            )[:3]
            coupling_partners = [
                {
                    "file": cc["file_b"] if cc["file_a"] == fp else cc["file_a"],
                    "co_commits": cc["co_commit_count"],
                }
                for cc in sorted_cc
            ]
            import_partners = [{"file": n} for n in import_neighbors[:3]]

            code_units = code_units_batch.get(fp, [])
            tested_count = 0
            tested_lines = 0
            total_lines = 0
            covering_test_ids = set()
            edge_type_counts = {"call": 0, "import": 0}
            for cu in code_units:
                unit_lines = cu["line_end"] - cu["line_start"] + 1
                total_lines += unit_lines
                edges = edges_batch.get(cu["id"], [])
                if edges:
                    tested_count += 1
                    tested_lines += unit_lines
                    for e in edges:
                        covering_test_ids.add(e["test_id"])
                        edge_type_counts[e.get("edge_type", "import")] += 1
            if coverage_mode == "line" and total_lines > 0:
                coverage = tested_lines / total_lines
            else:
                coverage = tested_count / max(len(code_units), 1)
            coverage_gap = _quantize_gap(1.0 - coverage)
            coverage_fraction = round(coverage, 4)

            # Multi-dimensional coverage signals
            total_edges = sum(edge_type_counts.values())
            coverage_depth = round(min(len(covering_test_ids) / 5.0, 1.0), 4)
            edge_type_quality = round(
                edge_type_counts.get("call", 0) / max(total_edges, 1), 4,
            ) if total_edges > 0 else 0.0

            if proximity_adjustment and fp not in tested_files:
                mh = hop_dist.get(fp)
                if mh is not None and mh > 0:
                    coverage_gap = _apply_coverage_proximity(coverage_gap, mh)

            blame_data = blame_batch.get(fp, [])
            author_conc = _author_concentration(blame_data)

            instability = _test_instability(
                covering_test_ids, failure_rates, duration_cv_by_test,
            )

            risk = (
                0.35 * churn_norm
                + 0.25 * coupling_norm
                + 0.2 * coverage_gap
                + 0.1 * author_conc
                + 0.1 * instability
            )
            risk_map.append({
                "file_path": fp,
                "unit_name": None,
                "risk_score": round(risk, 4),
                "coupling_partners": coupling_partners,
                "import_partners": import_partners,
                "breakdown": {
                    "churn": round(churn_norm, 4),
                    "coupling": round(coupling_norm, 4),
                    "import_coupling": round(import_coupling_norm, 4),
                    "cochange_coupling": round(cochange_coupling_norm, 4),
                    "cochange_global": round(cochange_global_norm, 4),
                    "cochange_branch": round(cochange_branch_norm, 4),
                    "coverage_gap": round(coverage_gap, 4),
                    "coverage_fraction": coverage_fraction,
                    "coverage_depth": coverage_depth,
                    "edge_type_quality": edge_type_quality,
                    "author_concentration": round(author_conc, 4),
                    "test_instability": round(instability, 4),
                },
            })

        risk_map.sort(key=lambda x: x["risk_score"], reverse=True)
        return risk_map

    # ------------------------------------------------------------------ #
    # Ownership (blame-based)
    # ------------------------------------------------------------------ #

    def get_ownership(self, file_path):
        """Get code ownership breakdown based on git blame.

        Shows who originally authored each portion of the file.

        Returns:
            List of dicts sorted by line_count desc:
            {author, author_email, line_count, percentage, role}
        """
        content_hash = _latest_hash(self.storage, file_path)
        blame_data = self.storage.get_blame(file_path, content_hash)
        if not blame_data:
            return []

        result = compute_ownership(blame_data)
        for entry in result:
            entry["role"] = "original_author"
        return result

    # ------------------------------------------------------------------ #
    # Reviewer suggestions (commit-activity-based)
    # ------------------------------------------------------------------ #

    def suggest_reviewers(self, file_path):
        """Suggest reviewers based on recent commit activity for a file.

        Unlike ownership (which shows who wrote the code), this shows who
        has been actively maintaining/modifying the file recently and is
        best positioned to review changes.

        Returns:
            List of dicts sorted by activity score desc:
            {author, author_email, recent_commits, last_commit_date,
             days_since_last_commit, insertions, deletions, percentage, role}
        """
        commits = self.storage.get_commits_for_file(file_path)
        if not commits:
            return []

        now = datetime.now(timezone.utc)
        author_stats = defaultdict(lambda: {
            "commits": 0, "email": "", "insertions": 0, "deletions": 0,
            "last_date": "", "score": 0.0,
        })

        # Track parsed last_date per author to avoid redundant parsing
        author_last_dt = {}

        for commit in commits:
            author = commit["author"]
            info = author_stats[author]
            info["commits"] += 1
            info["email"] = commit.get("author_email", "")
            info["insertions"] += commit.get("insertions", 0)
            info["deletions"] += commit.get("deletions", 0)
            try:
                cdate = _parse_iso_date(commit["date"])
            except (ValueError, TypeError):
                info["score"] += 0.01
                continue
            prev_dt = author_last_dt.get(author)
            if prev_dt is None or cdate > prev_dt:
                author_last_dt[author] = cdate
                info["last_date"] = commit["date"]
            # Weight by recency: recent commits count more
            days = max((now - cdate).total_seconds() / 86400, 0)
            info["score"] += 1.0 / (1.0 + days)

        total_score = sum(info["score"] for info in author_stats.values())
        result = []
        for author, info in author_stats.items():
            last_dt = author_last_dt.get(author)
            days_since = round((now - last_dt).total_seconds() / 86400) if last_dt else None
            pct = (info["score"] / total_score * 100) if total_score > 0 else 0
            result.append({
                "author": author,
                "author_email": info["email"],
                "recent_commits": info["commits"],
                "last_commit_date": info["last_date"],
                "days_since_last_commit": days_since,
                "insertions": info["insertions"],
                "deletions": info["deletions"],
                "percentage": round(pct, 2),
                "role": "suggested_reviewer",
            })
        result.sort(key=lambda x: x["percentage"], reverse=True)
        return result


# ------------------------------------------------------------------ #
# Internal helpers
# ------------------------------------------------------------------ #

def _latest_hash(storage, file_path):
    """Get the most recent content hash for a file from storage."""
    return storage.get_file_hash(file_path) or ""


def _author_concentration(blame_data):
    """Compute author concentration (0 = many authors, 1 = single author).

    Uses a simple Herfindahl index: sum of squared ownership fractions.
    """
    if not blame_data:
        return 1.0  # no data => assume concentrated

    lines_by_author = defaultdict(int)
    total = 0
    for block in blame_data:
        n = block["line_end"] - block["line_start"] + 1
        lines_by_author[block["author"]] += n
        total += n

    if total == 0:
        return 1.0

    hhi = sum((count / total) ** 2 for count in lines_by_author.values())
    return round(hhi, 4)


def _fetch_failure_rates(storage):
    """Fetch test failure rates from storage as a dict of test_id -> rate."""
    return {
        r["test_id"]: r["failures"] / r["total_runs"]
        for r in storage.get_test_failure_rates()
        if r["total_runs"] > 0
    }


def _test_instability(test_ids, failure_rates, duration_cv_by_test=None):
    """Blend failure rate with duration coefficient-of-variation when available."""
    duration_cv_by_test = duration_cv_by_test or {}
    if not test_ids:
        return 0.0
    rates = [failure_rates[tid] for tid in test_ids if tid in failure_rates]
    fail_component = sum(rates) / len(rates) if rates else 0.0
    cvs = [duration_cv_by_test[tid] for tid in test_ids if tid in duration_cv_by_test]
    cv_component = sum(cvs) / len(cvs) if cvs else 0.0
    if not rates and not cvs:
        return 0.0
    if not rates:
        return min(cv_component, 1.0)
    if not cvs:
        return fail_component
    return min(1.0, 0.65 * fail_component + 0.35 * cv_component)
