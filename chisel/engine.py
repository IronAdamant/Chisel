"""ChiselEngine — main orchestrator tying together all subsystems."""

import json
import os
import threading
import time
import uuid

from chisel.ast_utils import (
    _EXTENSION_MAP,
    _SKIP_DIRS,
    compute_file_hash,
    extract_code_units,
    path_has_code_extension,
)
from chisel.bootstrap import load_user_bootstrap
from chisel.git_analyzer import GitAnalyzer
from chisel.import_graph import build_import_edges
from chisel.impact import (
    _COCHANGE_COUPLING_CAP,
    _IMPORT_COUPLING_CAP,
    ImpactAnalyzer,
)
from chisel.metrics import compute_churn, compute_co_changes, coupling_threshold as _coupling_threshold
from chisel.project import ProcessLock, detect_project_root, normalize_path, resolve_storage_dir
from chisel.risk_meta import apply_risk_reweighting, build_risk_meta
from chisel.rwlock import RWLock
from chisel.storage import Storage
from chisel.test_mapper import TestMapper


# Derived from ast_utils._EXTENSION_MAP to avoid duplication.
_CODE_EXTENSIONS = frozenset(_EXTENSION_MAP)

# Default cap on suggest_tests results when working_tree=True to prevent
# output explosion (600K+ chars observed in Review Ten with 40+ new files).
_WORKING_TREE_SUGGEST_LIMIT = 30


def _test_to_source_stem(test_file_path):
    """Extract the source filename stem from a test file path.

    ``tests/services/nutritionService.test.js`` → ``nutritionService``
    ``test_utils.py`` → ``utils``
    """
    base = os.path.basename(test_file_path)
    # Strip all extensions: foo.test.js -> foo, foo.spec.ts -> foo
    stem = base.split(".")[0]
    # Remove test prefixes/suffixes
    for affix in ("_test", "Test", "_spec", "Spec"):
        if stem.endswith(affix):
            stem = stem[: -len(affix)]
            break
    for affix in ("test_", "Test"):
        if stem.startswith(affix):
            stem = stem[len(affix):]
            break
    return stem or None


_NO_DATA_RESPONSE = {
    "status": "no_data",
    "message": "No analysis data. Run 'chisel analyze' on this project first.",
    "hint": "chisel analyze",
}


def _git_tool_error(project_dir, message):
    """Structured failure when git subprocess fails (wrong cwd, not a repo, etc.)."""
    low = message.lower()
    if "not a git repository" in low or "not a git repo" in low:
        err = "not_a_git_repo"
    else:
        err = "git_command_failed"
    return {
        "status": "git_error",
        "error": err,
        "message": message,
        "project_dir": project_dir,
        "cwd": project_dir,
        "hint": (
            "Use the repository root as the project directory: "
            "`chisel ... --project-dir /path/to/repo` or start the MCP server "
            "with cwd set to the git checkout."
        ),
    }


class ChiselEngine:
    """High-level orchestrator for Chisel operations.

    Owns Storage, GitAnalyzer, TestMapper, ImpactAnalyzer, an RWLock
    for in-process thread safety, and a ProcessLock for cross-process
    coordination.

    Multi-agent safety:
        - project_dir is canonicalized via detect_project_root() so all
          agents (including those in git worktrees) resolve to the same root.
        - Storage defaults to project-local (<root>/.chisel/) so different
          projects never collide.
        - ProcessLock prevents concurrent analyze/update from interleaving
          destructive writes across separate processes.
        - All stored paths are normalized via normalize_path() so agents
          in different worktrees produce identical relative paths.
    """

    def __init__(self, project_dir, storage_dir=None):
        load_user_bootstrap()
        self.project_dir = detect_project_root(str(project_dir))
        resolved_storage = resolve_storage_dir(self.project_dir, storage_dir)
        self.storage = Storage(base_dir=resolved_storage)
        self.git = GitAnalyzer(self.project_dir)
        self.mapper = TestMapper(self.project_dir)
        self.impact = ImpactAnalyzer(self.storage, self.project_dir)
        self.lock = RWLock()
        self._process_lock = ProcessLock(resolved_storage)
        self._bg_jobs_lock = threading.Lock()
        self._bg_job_in_progress = False

    # ------------------------------------------------------------------ #
    # Full analysis
    # ------------------------------------------------------------------ #

    def analyze(self, directory=None, force=False):
        """Full rebuild of all data.

        Git subprocess calls and file scanning run outside the write lock.
        Only storage mutations hold the lock, minimizing read-blocking time.

        Args:
            directory: Optional subdirectory to scope code scanning.
                       Git log and test discovery remain project-wide.
            force: Force re-analysis of all files even if unchanged.

        Returns:
            Dict summarizing the analysis results.
        """
        # Phase 1: collect data (no lock needed — only reads from git/filesystem)
        code_files = self._scan_code_files(directory=directory)

        commits = None
        try:
            commits = self.git.parse_log()
        except RuntimeError:
            pass  # Not a git repo or git not available

        # Phase 2: write to storage under both process lock (cross-process)
        # and RWLock (in-process threads).
        with self._process_lock.exclusive():
            with self.lock.write_lock():
                changed_files = self._find_changed_files(code_files, force=force)

                stats = {
                    "code_files_scanned": len(code_files),
                    "code_units_found": self._parse_and_store_code_units(changed_files),
                    "test_files_found": 0,
                    "test_units_found": 0,
                    "test_edges_built": 0,
                    "commits_parsed": 0,
                }

                if commits is not None:
                    stats["commits_parsed"] = len(commits)
                    self._store_commits(commits)
                    self._compute_churn_and_coupling(commits, code_files)
                    self._store_blame(changed_files)

                test_units, tf_count, edge_count = self._discover_and_build_edges(code_files)
                stats["test_files_found"] = tf_count
                stats["test_units_found"] = len(test_units)
                stats["test_edges_built"] = edge_count

                self._rebuild_import_edges(code_files)

                stats["orphaned_results_cleaned"] = self.storage.cleanup_orphaned_test_results()
                return stats

    # ------------------------------------------------------------------ #
    # Incremental update
    # ------------------------------------------------------------------ #

    def update(self):
        """Incremental update — only re-process changed files + new commits.

        Returns:
            Dict summarizing what was updated.
        """
        # Scan filesystem outside locks to avoid blocking
        code_files = self._scan_code_files()
        with self._process_lock.exclusive():
            with self.lock.write_lock():
                changed_files = self._find_changed_files(code_files)
                code_units_found = self._parse_and_store_code_units(changed_files)

                stats = {
                    "files_updated": len(changed_files),
                    "code_units_found": code_units_found,
                    "new_commits": 0,
                }

                try:
                    all_commits = self.git.parse_log()
                    new_commits = [
                        c for c in all_commits
                        if self.storage.get_commit(c["hash"]) is None
                    ]
                    self._store_commits(new_commits)
                    stats["new_commits"] = len(new_commits)

                    if new_commits or changed_files:
                        self._compute_churn_and_coupling(all_commits, code_files)
                    self._store_blame(changed_files)
                except RuntimeError:
                    pass

                self._discover_and_build_edges(code_files)
                self._rebuild_import_edges(code_files)
                stats["orphaned_results_cleaned"] = self.storage.cleanup_orphaned_test_results()
                return stats

    # ------------------------------------------------------------------ #
    # Tool methods (one per MCP tool)
    # ------------------------------------------------------------------ #

    def _check_analysis_data(self):
        """Return a no-data warning dict if the DB is empty, else ``None``."""
        if not self.storage.has_analysis_data():
            return dict(_NO_DATA_RESPONSE)
        return None

    def tool_analyze(self, directory=None, force=False):
        """MCP tool: full analysis."""
        return self.analyze(directory=directory, force=force)

    def tool_start_job(self, kind, directory=None, force=False):
        """MCP tool: run ``analyze`` or ``update`` in a background thread.

        Poll ``job_status`` until ``status`` is ``completed`` or ``failed``.
        Avoids MCP client timeouts on large repos (zero extra dependencies).
        """
        if kind not in ("analyze", "update"):
            return {
                "status": "error",
                "message": f"kind must be 'analyze' or 'update', got {kind!r}",
            }
        with self._bg_jobs_lock:
            if self._bg_job_in_progress:
                return {
                    "status": "busy",
                    "message": (
                        "Another background analyze or update is already running "
                        "in this engine"
                    ),
                    "hint": "Poll job_status with the existing job_id, or wait.",
                }
            self._bg_job_in_progress = True
        job_id = uuid.uuid4().hex
        self.storage.insert_bg_job(job_id, kind, "running")

        def run():
            try:
                if kind == "analyze":
                    out = self.analyze(directory=directory, force=force)
                else:
                    out = self.update()
                self.storage.update_bg_job(
                    job_id, "completed", result_json=json.dumps(out, default=str),
                )
            except Exception as exc:
                self.storage.update_bg_job(
                    job_id, "failed", error_message=str(exc),
                )
            finally:
                with self._bg_jobs_lock:
                    self._bg_job_in_progress = False

        threading.Thread(
            target=run, name=f"chisel-bg-{kind}", daemon=True,
        ).start()
        return {
            "job_id": job_id,
            "status": "running",
            "kind": kind,
            "hint": "Poll job_status with job_id until completed or failed.",
        }

    def tool_job_status(self, job_id):
        """MCP tool: poll a job started with ``tool_start_job``."""
        row = self.storage.get_bg_job(job_id)
        if not row:
            return {
                "status": "not_found",
                "job_id": job_id,
                "message": "No job with this id",
            }
        out = {
            "job_id": row["id"],
            "kind": row["kind"],
            "status": row["status"],
            "created_at": row["created_at"],
            "updated_at": row["updated_at"],
        }
        if row["error_message"]:
            out["error"] = row["error_message"]
        if row["result_json"]:
            out["result"] = json.loads(row["result_json"])
        return out

    def tool_impact(self, files, functions=None):
        """MCP tool: get impacted tests for changed files."""
        with self._process_lock.shared():
            with self.lock.read_lock():
                empty = self._check_analysis_data()
                if empty is not None:
                    return empty
                return self.impact.get_impacted_tests(files, functions)

    def tool_suggest_tests(self, file_path, fallback_to_all=False,
                           working_tree=False):
        """MCP tool: suggest tests for a file.

        Args:
            fallback_to_all: If True and no test edges exist, return all known
                test files ranked by stem-match relevance.
            working_tree: If True, also check untracked files on disk and use
                stem-matching to find relevant tests when the file has no DB edges.
                Useful during active development before files are committed.
        """
        with self._process_lock.shared():
            with self.lock.read_lock():
                empty = self._check_analysis_data()
                if empty is not None:
                    return empty
                disk = self._disk_only_test_map() if working_tree else None
                extra = self._git_untracked_code_paths() if working_tree else None
                result = self.impact.suggest_tests(
                    file_path,
                    fallback_to_all=fallback_to_all,
                    disk_test_files=disk,
                    extra_code_paths=extra,
                )
                if working_tree and not result:
                    # File may be untracked — try stem-matching against known test files
                    result = self._working_tree_suggest(file_path)
                if working_tree and len(result) > _WORKING_TREE_SUGGEST_LIMIT:
                    result = result[:_WORKING_TREE_SUGGEST_LIMIT]
                return result

    def tool_churn(self, file_path, unit_name=None):
        """MCP tool: get churn stats. Always returns a list."""
        with self._process_lock.shared():
            with self.lock.read_lock():
                empty = self._check_analysis_data()
                if empty is not None:
                    return empty
                stat = self.storage.get_churn_stat(file_path, unit_name)
                if stat:
                    return [stat]
                if unit_name is None:
                    return self.storage.get_all_churn_stats(file_path)
                return []

    def tool_ownership(self, file_path):
        """MCP tool: get blame-based code ownership."""
        with self._process_lock.shared():
            with self.lock.read_lock():
                empty = self._check_analysis_data()
                if empty is not None:
                    return empty
                return self.impact.get_ownership(file_path)

    def tool_coupling(self, file_path, min_count=3):
        """MCP tool: get co-change and import coupling partners."""
        with self._process_lock.shared():
            with self.lock.read_lock():
                empty = self._check_analysis_data()
                if empty is not None:
                    return empty
                query_min = self.storage.get_co_change_query_min()
                effective = max(min_count, query_min)
                co_change_partners = self.storage.get_co_changes(file_path, min_count=effective)
                import_neighbors = self.storage.get_import_neighbors_batch([file_path]).get(file_path, [])
                co_len = len(co_change_partners)
                imp_len = len(import_neighbors)
                co_norm = min(co_len / float(_COCHANGE_COUPLING_CAP), 1.0)
                import_norm = min(imp_len / float(_IMPORT_COUPLING_CAP), 1.0)
                if co_norm > 0:
                    effective_coupling = max(
                        co_norm, co_norm + 0.25 * import_norm,
                    )
                else:
                    effective_coupling = import_norm
                return {
                    "co_change_partners": co_change_partners,
                    "import_partners": [{"file": path} for path in import_neighbors],
                    "co_change_breadth": co_len,
                    "import_breadth": imp_len,
                    "cochange_coupling": round(co_norm, 4),
                    "import_coupling": round(import_norm, 4),
                    "effective_coupling": round(effective_coupling, 4),
                }

    def tool_risk_map(self, directory=None, exclude_tests=True,
                      proximity_adjustment=False, coverage_mode="unit"):
        """MCP tool: risk scores for all files.

        Returns ``{"files": [...], "_meta": {...}}`` so LLM agents can
        inspect which risk components are differentiating vs uniform noise.

        Args:
            directory: Optional subdirectory to scope the risk map.
            exclude_tests: If True (default), exclude test files.  Test
                files always score coverage_gap=1.0 (no edges point *to*
                test-file code units), which adds noise and masks real
                coverage signal.
            proximity_adjustment: If True, slightly reduce coverage_gap for
                files a few import hops from tested code (see ``_meta``).
            coverage_mode: "unit" (default) weights each code unit equally;
                "line" weights by line count so large untested units have
                proportionally higher coverage_gap.
        """
        with self._process_lock.shared():
            with self.lock.read_lock():
                empty = self._check_analysis_data()
                if empty is not None:
                    return empty
                files = self.impact.get_risk_map(
                    directory, exclude_tests, proximity_adjustment, coverage_mode,
                )
                files, rw_meta = apply_risk_reweighting(files)
                stats = self.storage.get_stats()
                meta = build_risk_meta(files, stats)
                meta.update(rw_meta)
                meta["coverage_gap_mode"] = (
                    "proximity_adjusted" if proximity_adjustment else coverage_mode
                )
                bc = self.storage.get_meta("branch_coupling_commits")
                if bc is not None:
                    try:
                        meta["branch_coupling_commits"] = int(bc)
                    except ValueError:
                        pass
                # Compute cycles from import graph for inclusion in _meta
                all_files = {f["file_path"] for f in files}
                import_neighbors = self.storage.get_import_neighbors_batch(list(all_files))
                from chisel.impact import _find_circular_dependencies
                cycles = _find_circular_dependencies(all_files, import_neighbors)
                meta["cycles"] = cycles
                return {"files": files, "_meta": meta}

    def tool_stale_tests(self):
        """MCP tool: detect stale tests.

        Returns a diagnostic dict (status="no_edges") when no test edges
        exist, so agents can distinguish "no stale tests" from "nothing
        to evaluate".
        """
        with self._process_lock.shared():
            with self.lock.read_lock():
                empty = self._check_analysis_data()
                if empty is not None:
                    return empty
                result = self.impact.detect_stale_tests()
                if not result:
                    stats = self.storage.get_stats()
                    if stats.get("test_edges", 0) == 0:
                        return {
                            "status": "no_edges",
                            "message": (
                                "No test edges exist — cannot evaluate "
                                "test staleness. Edge builder may not match "
                                "this project's import/require patterns."
                            ),
                            "hint": "chisel analyze",
                            "stale_tests": [],
                        }
                return result

    def tool_history(self, file_path):
        """MCP tool: commit history for a file."""
        with self._process_lock.shared():
            with self.lock.read_lock():
                empty = self._check_analysis_data()
                if empty is not None:
                    return empty
                return self.storage.get_commits_for_file(file_path)

    def tool_who_reviews(self, file_path):
        """MCP tool: suggest reviewers based on recent commit activity."""
        with self._process_lock.shared():
            with self.lock.read_lock():
                empty = self._check_analysis_data()
                if empty is not None:
                    return empty
                return self.impact.suggest_reviewers(file_path)

    def tool_diff_impact(self, ref=None):
        """MCP tool: auto-detect changes from git diff and return impacted tests.

        If ref is not provided, auto-detects: on a feature branch diffs against
        main/master; on main diffs against HEAD (unstaged changes).

        Returns a diagnostic dict (status="no_changes") instead of bare []
        when no diff is found, so LLM agents can reason about why.
        """
        with self._process_lock.shared():
            with self.lock.read_lock():
                empty = self._check_analysis_data()
                if empty is not None:
                    return empty
        if ref is None:
            ref = self._detect_diff_base()
        try:
            diff_files = self.git.get_changed_files(ref)
            untracked_raw = self.git.get_untracked_files()
        except RuntimeError as exc:
            return _git_tool_error(self.project_dir, str(exc))
        untracked_code = {
            p for p in untracked_raw
            if path_has_code_extension(p)
        }
        changed_files = sorted(set(diff_files) | untracked_code)
        if not changed_files:
            try:
                branch = self.git.get_current_branch()
            except RuntimeError:
                branch = None
            return {
                "status": "no_changes",
                "ref": ref,
                "branch": branch,
                "message": f"No files differ against '{ref}' and no untracked code files",
            }
        functions = []
        for fp in changed_files:
            if fp in untracked_code:
                continue
            try:
                functions.extend(self.git.get_changed_functions(fp, ref))
            except RuntimeError:
                pass
        with self._process_lock.shared():
            with self.lock.read_lock():
                result = self.impact.get_impacted_tests(
                    changed_files,
                    functions or None,
                    untracked_files=untracked_code,
                )
                # Stem-match fallback for untracked files with no DB edges
                if untracked_code:
                    covered = {item["test_id"] for item in result}
                    for uf in untracked_code:
                        has_hit = any(
                            uf in item.get("reason", "")
                            for item in result
                        )
                        if has_hit:
                            continue
                        stem_hits = self._working_tree_suggest(uf)
                        for sh in stem_hits:
                            if sh.get("relevance", 0) < 0.5:
                                continue
                            tid = sh["test_id"]
                            if tid not in covered:
                                covered.add(tid)
                                result.append({
                                    "test_id": tid,
                                    "file_path": sh["file_path"],
                                    "name": sh["name"],
                                    "reason": f"stem-match for untracked file {uf}",
                                    "score": sh["relevance"] * 0.4,
                                    "source": "working_tree",
                                })
                return result

    def tool_update(self):
        """MCP tool: incremental re-analysis of changed files."""
        return self.update()

    def tool_test_gaps(self, file_path=None, directory=None, exclude_tests=True,
                       working_tree=False):
        """MCP tool: find code units with no test coverage.

        Args:
            working_tree: If True, also scan untracked (uncommitted) files from
                disk and include their code units as gaps with churn=0. Useful
                for identifying coverage gaps in files that haven't been committed.
        """
        with self._process_lock.shared():
            with self.lock.read_lock():
                empty = self._check_analysis_data()
                if empty is not None:
                    return empty
                disk = self._disk_only_test_map() if working_tree else None
                extra = self._git_untracked_code_paths() if working_tree else None
                gaps = list(self.impact.get_test_gaps(
                    file_path, directory, exclude_tests,
                    disk_test_files=disk,
                    extra_code_paths=extra,
                ))
                if working_tree:
                    gaps.extend(self._working_tree_gaps(file_path, directory, exclude_tests))
                return gaps

    def tool_record_result(self, test_id, passed, duration_ms=None):
        """MCP tool: record a test result (pass/fail) for future prioritization.

        Also attempts heuristic edge creation when no edges exist for
        the test file — matches the test filename to a source file and
        links all code units in the match.
        """
        with self._process_lock.exclusive():
            with self.lock.write_lock():
                self.storage.record_test_result(test_id, passed, duration_ms)
                edges_created = self._create_heuristic_edges(test_id)
                result = {"test_id": test_id, "passed": passed, "recorded": True}
                if edges_created:
                    result["heuristic_edges_created"] = edges_created
                return result

    def tool_triage(self, directory=None, top_n=10, exclude_tests=True):
        """MCP tool: combined risk_map + test_gaps + stale_tests triage."""
        with self._process_lock.shared():
            with self.lock.read_lock():
                empty = self._check_analysis_data()
                if empty is not None:
                    return empty
                risk_map = self.impact.get_risk_map(
                    directory, exclude_tests,
                )
                risk_map = risk_map[:top_n]
                test_gaps = self.impact.get_test_gaps(directory=directory)
                stale = self.impact.detect_stale_tests()
                stats = self.storage.get_stats()

                top_files = {r["file_path"] for r in risk_map}
                relevant_gaps = [g for g in test_gaps if g["file_path"] in top_files]

                commit_count = stats.get("commits", 0)
                return {
                    "top_risk_files": risk_map,
                    "test_gaps": relevant_gaps,
                    "stale_tests": stale,
                    "summary": {
                        "files_triaged": len(risk_map),
                        "total_test_gaps": len(relevant_gaps),
                        "total_stale_tests": len(stale),
                        "test_edge_count": stats.get("test_edges", 0),
                        "test_result_count": stats.get("test_results", 0),
                        "coupling_threshold": _coupling_threshold(commit_count),
                    },
                }

    # --- file_locks (advisory multi-agent locking) ---

    def tool_acquire_file_lock(self, file_path, agent_id, ttl=300, purpose=None):
        """MCP tool: acquire an advisory lock on a file."""
        with self._process_lock.exclusive():
            with self.lock.write_lock():
                acquired, holder, expires_at = self.storage.acquire_file_lock(
                    file_path, agent_id, ttl, purpose,
                )
                return {
                    "acquired": acquired,
                    "holder": holder,
                    "expires_at": expires_at,
                }

    def tool_release_file_lock(self, file_path, agent_id):
        """MCP tool: release an advisory lock held by this agent."""
        with self._process_lock.exclusive():
            with self.lock.write_lock():
                released = self.storage.release_file_lock(file_path, agent_id)
                return {"released": released}

    def tool_refresh_file_lock(self, file_path, agent_id, ttl=300):
        """MCP tool: extend the TTL of a lock held by this agent."""
        with self._process_lock.exclusive():
            with self.lock.write_lock():
                ok, new_expires = self.storage.refresh_file_lock(
                    file_path, agent_id, ttl,
                )
                return {"refreshed": ok, "expires_at": new_expires}

    def tool_check_file_lock(self, file_path):
        """MCP tool: check if a file is currently locked."""
        with self._process_lock.shared():
            with self.lock.read_lock():
                lock = self.storage.get_file_lock(file_path)
                if lock is None:
                    return {
                        "locked": False,
                        "holder": None,
                        "expires_at": None,
                        "ttl_remaining": None,
                    }
                now = time.time()
                ttl_remaining = max(0.0, lock["expires_at"] - now)
                stale = ttl_remaining < 60.0
                return {
                    "locked": True,
                    "holder": lock["agent_id"],
                    "acquired_at": lock["acquired_at"],
                    "expires_at": lock["expires_at"],
                    "ttl_remaining": round(ttl_remaining, 1),
                    "stale": stale,
                    "purpose": lock.get("purpose"),
                }

    def tool_check_locks(self, file_paths):
        """MCP tool: batch-check lock status for multiple files."""
        with self._process_lock.shared():
            with self.lock.read_lock():
                conflicts = []
                for fp in file_paths:
                    lock = self.storage.get_file_lock(fp)
                    if lock:
                        now = time.time()
                        ttl_remaining = max(0.0, lock["expires_at"] - now)
                        conflicts.append({
                            "file_path": fp,
                            "holder": lock["agent_id"],
                            "expires_at": lock["expires_at"],
                            "ttl_remaining": round(ttl_remaining, 1),
                            "stale": ttl_remaining < 60.0,
                        })
                return {"conflicts": conflicts, "checked": len(file_paths)}

    def tool_list_file_locks(self, agent_id=None):
        """MCP tool: list all active file locks, optionally filtered by agent."""
        with self._process_lock.shared():
            with self.lock.read_lock():
                locks = self.storage.list_file_locks(agent_id)
                now = time.time()
                for lock in locks:
                    lock["ttl_remaining"] = round(
                        max(0.0, lock["expires_at"] - now), 1,
                    )
                return {"locks": locks, "total": len(locks)}

    def tool_stats(self):
        """MCP tool: get summary counts for the Chisel database."""
        with self._process_lock.shared():
            with self.lock.read_lock():
                stats = self.storage.get_stats()
                if all(v == 0 for v in stats.values()):
                    stats["hint"] = "All counts are zero. Run 'chisel analyze' to populate."
                else:
                    commit_count = stats.get("commits", 0)
                    if commit_count > 0:
                        stats["coupling_threshold"] = _coupling_threshold(commit_count)
                    qm = self.storage.get_co_change_query_min()
                    stats["co_change_query_min"] = qm
                    bc = self.storage.get_meta("branch_coupling_commits")
                    if bc is not None:
                        try:
                            stats["branch_coupling_commits"] = int(bc)
                        except ValueError:
                            pass
                    # Shadow graph summary: edge type breakdown for dynamic require visibility
                    edge_counts = self.storage.get_edge_type_counts()
                    if edge_counts:
                        stats["shadow_graph"] = {
                            "total_edges": sum(edge_counts.values()),
                            "call_edges": edge_counts.get("call", 0),
                            "import_edges": edge_counts.get("import", 0),
                            "dynamic_import_edges": (
                                edge_counts.get("dynamic_import", 0)
                                + edge_counts.get("eval_import", 0)
                            ),
                            "eval_import_edges": edge_counts.get("eval_import", 0),
                            "tainted_import_edges": edge_counts.get("tainted_import", 0),
                            "unknown_shadow_ratio": round(
                                (
                                    edge_counts.get("dynamic_import", 0)
                                    + edge_counts.get("eval_import", 0)
                                )
                                / max(sum(edge_counts.values()), 1),
                                4,
                            ),
                        }
                return stats

    # ------------------------------------------------------------------ #
    # Shared internal helpers
    # ------------------------------------------------------------------ #

    def _detect_diff_base(self):
        """Auto-detect the best git ref for diff_impact.

        On a feature branch, diffs against main/master for full branch impact.
        On main/master, diffs against HEAD for unstaged changes.
        """
        try:
            branch = self.git.get_current_branch()
            if branch in ("main", "master"):
                return "HEAD"
            return next(
                (name for name in ("main", "master") if self.git.branch_exists(name)),
                "HEAD",
            )
        except RuntimeError:
            return "HEAD"

    def _find_changed_files(self, code_files, force=False):
        """Compare content hashes to identify changed code files.

        Returns list of (abs_path, rel_path, new_hash) tuples.
        """
        changed = []
        for fpath in code_files:
            rel = normalize_path(fpath, self.project_dir)
            new_hash = compute_file_hash(fpath)
            old_hash = self.storage.get_file_hash(rel)
            if force or old_hash != new_hash:
                changed.append((fpath, rel, new_hash))
        return changed

    def _parse_and_store_code_units(self, changed_files):
        """Re-extract and upsert code units for changed files.

        Returns the total number of code units found.
        """
        count = 0
        for fpath, rel, new_hash in changed_files:
            self.storage.set_file_hash(rel, new_hash)
            self.storage.delete_code_units_by_file(rel)
            try:
                with open(fpath, encoding="utf-8", errors="replace") as f:
                    content = f.read()
            except OSError:
                continue
            units = extract_code_units(fpath, content)
            for u in units:
                cid = f"{rel}:{u.name}:{u.unit_type}"
                self.storage.upsert_code_unit(
                    cid, rel, u.name, u.unit_type,
                    u.line_start, u.line_end, new_hash,
                )
            count += len(units)
        return count

    def _store_blame(self, changed_files):
        """Parse and store git blame data for changed files."""
        for _, rel, new_hash in changed_files:
            try:
                self.storage.invalidate_blame(rel)
                blame_blocks = self.git.parse_blame(rel)
                for block in blame_blocks:
                    self.storage.store_blame(
                        rel, block["line_start"], block["line_end"],
                        block["commit_hash"], block["author"],
                        block["author_email"], block["date"], new_hash,
                    )
            except RuntimeError:
                pass

    def _store_commits(self, commits):
        """Upsert commits and their file entries into storage."""
        for commit in commits:
            self.storage.upsert_commit(
                commit["hash"], commit["author"], commit["author_email"],
                commit["date"], commit["message"],
            )
            for f in commit.get("files", []):
                self.storage.upsert_commit_file(
                    commit["hash"], f["path"], f["insertions"], f["deletions"],
                )

    # Skip unit-level churn (git log -L per function) for repos above this
    # file count — each function spawns a subprocess, so 10k+ files with
    # multiple functions each would mean tens of thousands of subprocess calls.
    _UNIT_CHURN_FILE_LIMIT = 2000

    def _compute_churn_and_coupling(self, commits, code_files):
        """Compute file-level and unit-level churn stats, plus co-change coupling."""
        for fpath in code_files:
            rel = normalize_path(fpath, self.project_dir)
            churn = compute_churn(commits, rel)
            self.storage.upsert_churn_stat(
                rel, "", churn["commit_count"], churn["distinct_authors"],
                churn["total_insertions"], churn["total_deletions"],
                churn["last_changed"], churn["churn_score"],
            )

        # Unit-level churn via git log -L (expensive: one subprocess per function)
        if len(code_files) <= self._UNIT_CHURN_FILE_LIMIT:
            for fpath in code_files:
                rel = normalize_path(fpath, self.project_dir)
                for cu in self.storage.get_code_units_by_file(rel):
                    if cu["unit_type"] in ("function", "async_function"):
                        bare_name = cu["name"].rsplit(".", 1)[-1]
                        func_commits = self.git.get_function_log(rel, bare_name)
                        if func_commits:
                            fc = compute_churn(
                                func_commits, rel, unit_name=cu["name"],
                            )
                            self.storage.upsert_churn_stat(
                                rel, cu["name"], fc["commit_count"],
                                fc["distinct_authors"], fc["total_insertions"],
                                fc["total_deletions"], fc["last_changed"],
                                fc["churn_score"],
                            )

        adaptive_min = _coupling_threshold(len(commits))
        self.storage.set_meta("co_change_query_min", str(adaptive_min))
        co_changes = compute_co_changes(commits, min_count=adaptive_min)
        for cc in co_changes:
            self.storage.upsert_co_change(
                cc["file_a"], cc["file_b"],
                cc["co_commit_count"], cc["last_co_commit"],
            )

        self.storage.clear_branch_co_changes()
        try:
            branch = self.git.get_current_branch()
            if branch in ("main", "master"):
                self.storage.set_meta("branch_coupling_commits", "0")
            else:
                base = next(
                    (n for n in ("main", "master") if self.git.branch_exists(n)),
                    None,
                )
                if base:
                    mb = self.git.merge_base_with(base)
                    branch_commits = self.git.parse_log_range(f"{mb}..HEAD")
                    self.storage.set_meta(
                        "branch_coupling_commits", str(len(branch_commits)),
                    )
                    br_cc = compute_co_changes(branch_commits, min_count=1)
                    for cc in br_cc:
                        self.storage.upsert_branch_co_change(
                            cc["file_a"], cc["file_b"],
                            cc["co_commit_count"], cc["last_co_commit"],
                        )
                else:
                    self.storage.set_meta("branch_coupling_commits", "0")
        except RuntimeError:
            self.storage.set_meta("branch_coupling_commits", "0")

    def _discover_and_build_edges(self, code_files):
        """Discover test files, parse them, build test edges.

        Returns (all_test_units, test_file_count, edge_count).
        """
        test_files = self.mapper.discover_test_files()
        all_test_units = []
        for tf in test_files:
            rel_tf = normalize_path(tf, self.project_dir)
            # Remove stale test units/edges before reinserting
            old_tests = self.storage.get_test_units_by_file(rel_tf)
            for ot in old_tests:
                self.storage.delete_test_edges_by_test(ot["id"])
            self.storage.delete_test_units_by_file(rel_tf)
            for tu in self.mapper.parse_test_file(tf):
                self.storage.upsert_test_unit(
                    tu["id"], tu["file_path"], tu["name"], tu["framework"],
                    tu["line_start"], tu["line_end"], tu["content_hash"],
                )
                all_test_units.append(tu)

        all_cu_dicts = []
        for fpath in code_files:
            rel = normalize_path(fpath, self.project_dir)
            all_cu_dicts.extend(self.storage.get_code_units_by_file(rel))

        edges = self.mapper.build_test_edges(all_test_units, all_cu_dicts)
        for edge in edges:
            self.storage.upsert_test_edge(
                edge["test_id"], edge["code_id"],
                edge["edge_type"], edge["weight"],
            )
        return all_test_units, len(test_files), len(edges)

    def _rebuild_import_edges(self, code_files):
        """Rebuild static import_edges for all scanned code files."""
        self.storage.clear_import_edges()
        test_paths = set(self.storage.get_test_file_paths())
        rels = [normalize_path(f, self.project_dir) for f in code_files]
        edges = build_import_edges(self.mapper, self.project_dir, rels, test_paths)
        for e in edges:
            self.storage.upsert_import_edge(
                e["importer_file"], e["imported_file"],
            )

    # ------------------------------------------------------------------ #
    # Lifecycle
    # ------------------------------------------------------------------ #

    def __enter__(self):
        return self

    def __exit__(self, *exc):
        self.close()

    def close(self):
        """Close the underlying storage connection."""
        self.storage.close()

    def _create_heuristic_edges(self, test_id):
        """Attempt filename-based edge creation for a recorded test.

        When ``test_id`` is ``tests/services/nutritionService.test.js``,
        finds test units in that file, extracts the source stem
        (``nutritionService``), locates matching source-file code units,
        and creates heuristic edges if none already exist.

        Returns the number of edges created.
        """
        test_file = test_id.split(":")[0]
        test_units = self.storage.get_test_units_by_file(test_file)
        if not test_units:
            return 0
        # Skip if any test unit already has edges (analyzer already ran)
        for tu in test_units:
            if self.storage.get_edges_for_test(tu["id"]):
                return 0
        stem = _test_to_source_stem(test_file)
        if not stem:
            return 0
        source_units = self.storage.get_code_units_by_file_stem(stem)
        if not source_units:
            return 0
        count = 0
        for tu in test_units:
            for cu in source_units:
                if cu["file_path"] != test_file:
                    self.storage.upsert_test_edge(
                        tu["id"], cu["id"], "heuristic", 0.5,
                    )
                    count += 1
        return count

    def _git_untracked_code_paths(self):
        """Project-relative untracked code paths (for static import resolution)."""
        try:
            raw = self.git.get_untracked_files()
        except RuntimeError:
            return set()
        return {p for p in raw if path_has_code_extension(p)}

    def _disk_only_test_map(self):
        """Test files on disk not yet represented in ``test_units`` (e.g. untracked)."""
        mapper = self.mapper
        known = self.storage.get_test_file_paths()
        out = {}
        for abs_path in mapper.discover_test_files():
            rel = normalize_path(abs_path, self.project_dir)
            if rel in known:
                continue
            units = mapper.parse_test_file(abs_path)
            if not units:
                continue
            out[rel] = [u["name"] for u in units]
        return out

    def _working_tree_suggest(self, file_path):
        """Suggest tests for an untracked file using stem-matching.

        Reads the file from disk, extracts code units, and matches
        against known test files in storage by stem similarity.
        Returns an empty list if no matching tests are found.
        """
        import os
        source_stem = os.path.splitext(os.path.basename(file_path))[0]
        all_test_files = self.storage.get_all_test_files()
        if not all_test_files:
            return []

        scored = []
        for test_file, test_names in all_test_files.items():
            test_stem = os.path.splitext(os.path.basename(test_file))[0]
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
                    "reason": "working-tree: stem-matched test (file not yet committed)",
                    "source": "working_tree",
                })

        scored.sort(key=lambda x: x["relevance"], reverse=True)
        return scored

    def _working_tree_gaps(self, file_path=None, directory=None, exclude_tests=True):
        """Find coverage gaps in untracked files by scanning disk directly.

        Untracked files have no git history, so churn=0 for all their code units.
        Only returns gaps for files that are genuinely untracked (not in DB).
        """
        try:
            untracked = set(self.git.get_untracked_files())
        except RuntimeError:
            return []

        if not untracked:
            return []

        # Filter to code files, optionally scoped to directory
        dir_prefix = directory.rstrip("/") + "/" if directory else ""
        code_exts = _CODE_EXTENSIONS
        gaps = []
        for ufp in untracked:
            if not any(ufp.endswith(ext) for ext in code_exts):
                continue
            if dir_prefix and not ufp.startswith(dir_prefix):
                continue
            # Skip test files if exclude_tests
            if exclude_tests and ufp.startswith("test"):
                continue
            # Skip if already in DB (analyzed file — not truly untracked)
            if self.storage.get_code_units_by_file(ufp):
                continue
            # Read from disk and extract code units
            abs_path = os.path.join(self.project_dir, ufp)
            try:
                with open(abs_path, encoding="utf-8", errors="replace") as f:
                    content = f.read()
            except OSError:
                continue
            units = extract_code_units(abs_path, content)
            for u in units:
                gaps.append({
                    "id": f"{ufp}:{u.name}:{u.unit_type}",
                    "file_path": ufp,
                    "name": u.name,
                    "unit_type": u.unit_type,
                    "line_start": u.line_start,
                    "line_end": u.line_end,
                    "churn_score": 0.0,
                    "commit_count": 0,
                    "_working_tree": True,
                })
        return gaps

    def _scan_code_files(self, directory=None):
        """Walk project tree and return all code file paths.

        Args:
            directory: Optional subdirectory to scope the scan.
                       Resolved relative to project_dir.
        """
        start_dir = self.project_dir
        if directory and directory != ".":
            candidate = os.path.normpath(os.path.join(self.project_dir, directory))
            if os.path.isdir(candidate):
                start_dir = candidate
        files = []
        for root, dirs, filenames in os.walk(start_dir):
            dirs[:] = [d for d in dirs if d not in _SKIP_DIRS]
            for fname in filenames:
                if os.path.splitext(fname)[1].lower() in _CODE_EXTENSIONS:
                    files.append(os.path.join(root, fname))
        return sorted(files)
