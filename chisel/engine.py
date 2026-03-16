"""ChiselEngine — main orchestrator tying together all subsystems."""

import os
from pathlib import Path

from chisel.ast_utils import _SKIP_DIRS, compute_file_hash, extract_code_units
from chisel.git_analyzer import GitAnalyzer
from chisel.impact import ImpactAnalyzer
from chisel.rwlock import RWLock
from chisel.storage import Storage
from chisel.test_mapper import TestMapper


# File extensions we scan for code units.
_CODE_EXTENSIONS = {
    ".py", ".pyw", ".js", ".jsx", ".mjs", ".cjs",
    ".ts", ".tsx", ".go", ".rs",
}

class ChiselEngine:
    """High-level orchestrator for Chisel operations.

    Owns Storage, GitAnalyzer, TestMapper, ImpactAnalyzer, and an RWLock
    for thread-safe access.
    """

    def __init__(self, project_dir, storage_dir=None):
        self.project_dir = str(project_dir)
        self.storage = Storage(base_dir=storage_dir)
        self.git = GitAnalyzer(self.project_dir)
        self.mapper = TestMapper(self.project_dir)
        self.impact = ImpactAnalyzer(self.storage)
        self.lock = RWLock()

    # ------------------------------------------------------------------ #
    # Full analysis
    # ------------------------------------------------------------------ #

    def analyze(self, directory=None, force=False):
        """Full rebuild of all data under a write lock.

        Args:
            directory: Optional subdirectory to scope code scanning.
                       Git log and test discovery remain project-wide.
            force: Force re-analysis of all files even if unchanged.

        Returns:
            Dict summarizing the analysis results.
        """
        with self.lock.write_lock():
            code_files = self._scan_code_files(directory=directory)
            changed_files = self._find_changed_files(code_files, force=force)

            stats = {
                "code_files_scanned": len(code_files),
                "code_units_found": self._parse_and_store_code_units(changed_files),
                "test_files_found": 0,
                "test_units_found": 0,
                "test_edges_built": 0,
                "commits_parsed": 0,
            }

            try:
                commits = self.git.parse_log()
                stats["commits_parsed"] = len(commits)
                self._store_commits(commits)
                self._compute_churn_and_coupling(commits, code_files)
            except RuntimeError:
                pass  # Not a git repo or git not available

            self._store_blame(changed_files)

            test_units, tf_count, edge_count = self._discover_and_build_edges(code_files)
            stats["test_files_found"] = tf_count
            stats["test_units_found"] = len(test_units)
            stats["test_edges_built"] = edge_count

            return stats

    # ------------------------------------------------------------------ #
    # Incremental update
    # ------------------------------------------------------------------ #

    def update(self):
        """Incremental update — only re-process changed files + new commits.

        Returns:
            Dict summarizing what was updated.
        """
        with self.lock.write_lock():
            code_files = self._scan_code_files()
            changed_files = self._find_changed_files(code_files)
            self._parse_and_store_code_units(changed_files)
            self._store_blame(changed_files)

            stats = {"files_updated": len(changed_files), "new_commits": 0}

            try:
                all_commits = self.git.parse_log()
                new_commits = [
                    c for c in all_commits
                    if self.storage.get_commit(c["hash"]) is None
                ]
                self._store_commits(new_commits)
                stats["new_commits"] = len(new_commits)

                # Recompute churn/coupling when anything changed
                if new_commits or changed_files:
                    self._compute_churn_and_coupling(all_commits, code_files)
            except RuntimeError:
                pass

            self._discover_and_build_edges(code_files)
            return stats

    # ------------------------------------------------------------------ #
    # Tool methods (one per MCP tool)
    # ------------------------------------------------------------------ #

    def tool_analyze(self, directory=None, force=False):
        """MCP tool: full analysis."""
        return self.analyze(directory=directory, force=force)

    def tool_impact(self, files, functions=None):
        """MCP tool: get impacted tests for changed files."""
        with self.lock.read_lock():
            return self.impact.get_impacted_tests(files, functions)

    def tool_suggest_tests(self, file_path, diff=None):
        """MCP tool: suggest tests for a file."""
        with self.lock.read_lock():
            return self.impact.suggest_tests(file_path, diff)

    def tool_churn(self, file_path, unit_name=None):
        """MCP tool: get churn stats. Always returns a list."""
        with self.lock.read_lock():
            stat = self.storage.get_churn_stat(file_path, unit_name)
            if stat:
                return [stat]
            # Only fall back to all stats when no specific unit was requested
            if unit_name is None:
                return self.storage.get_all_churn_stats(file_path)
            return []

    def tool_ownership(self, file_path):
        """MCP tool: get blame-based code ownership."""
        with self.lock.read_lock():
            return self.impact.get_ownership(file_path)

    def tool_coupling(self, file_path, min_count=3):
        """MCP tool: get co-change coupling partners."""
        with self.lock.read_lock():
            return self.storage.get_co_changes(file_path, min_count)

    def tool_risk_map(self, directory=None):
        """MCP tool: risk scores for all files."""
        with self.lock.read_lock():
            return self.impact.get_risk_map(directory)

    def tool_stale_tests(self):
        """MCP tool: detect stale tests."""
        with self.lock.read_lock():
            return self.impact.detect_stale_tests()

    def tool_history(self, file_path):
        """MCP tool: commit history for a file."""
        with self.lock.read_lock():
            return self.storage.get_commits_for_file(file_path)

    def tool_who_reviews(self, file_path):
        """MCP tool: suggest reviewers based on recent commit activity."""
        with self.lock.read_lock():
            return self.impact.suggest_reviewers(file_path)

    # ------------------------------------------------------------------ #
    # Shared internal helpers
    # ------------------------------------------------------------------ #

    def _find_changed_files(self, code_files, force=False):
        """Compare content hashes to identify changed code files.

        Returns list of (abs_path, rel_path, new_hash) tuples.
        """
        changed = []
        for fpath in code_files:
            rel = os.path.relpath(fpath, self.project_dir)
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
            content = Path(fpath).read_text(encoding="utf-8", errors="replace")
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

    def _compute_churn_and_coupling(self, commits, code_files):
        """Compute file-level and unit-level churn stats, plus co-change coupling."""
        for fpath in code_files:
            rel = os.path.relpath(fpath, self.project_dir)
            churn = self.git.compute_churn(commits, rel)
            self.storage.upsert_churn_stat(
                rel, "", churn["commit_count"], churn["distinct_authors"],
                churn["total_insertions"], churn["total_deletions"],
                churn["last_changed"], churn["churn_score"],
            )
            # Unit-level churn via git log -L
            for cu in self.storage.get_code_units_by_file(rel):
                if cu["unit_type"] in ("function", "async_function"):
                    bare_name = cu["name"].rsplit(".", 1)[-1]
                    func_commits = self.git.get_function_log(rel, bare_name)
                    if func_commits:
                        fc = self.git.compute_churn(
                            func_commits, rel, unit_name=cu["name"],
                        )
                        self.storage.upsert_churn_stat(
                            rel, cu["name"], fc["commit_count"],
                            fc["distinct_authors"], fc["total_insertions"],
                            fc["total_deletions"], fc["last_changed"],
                            fc["churn_score"],
                        )

        co_changes = self.git.compute_co_changes(commits, min_count=3)
        for cc in co_changes:
            self.storage.upsert_co_change(
                cc["file_a"], cc["file_b"],
                cc["co_commit_count"], cc["last_co_commit"],
            )

    def _discover_and_build_edges(self, code_files):
        """Discover test files, parse them, build test edges.

        Returns (all_test_units, test_file_count, edge_count).
        """
        test_files = self.mapper.discover_test_files()
        all_test_units = []
        for tf in test_files:
            for tu in self.mapper.parse_test_file(tf):
                self.storage.upsert_test_unit(
                    tu["id"], tu["file_path"], tu["name"], tu["framework"],
                    tu["line_start"], tu["line_end"], tu["content_hash"],
                )
                all_test_units.append(tu)

        all_cu_dicts = []
        for fpath in code_files:
            rel = os.path.relpath(fpath, self.project_dir)
            all_cu_dicts.extend(self.storage.get_code_units_by_file(rel))

        edges = self.mapper.build_test_edges(all_test_units, all_cu_dicts)
        for edge in edges:
            self.storage.upsert_test_edge(
                edge["test_id"], edge["code_id"],
                edge["edge_type"], edge["weight"],
            )
        return all_test_units, len(test_files), len(edges)

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
                if Path(fname).suffix in _CODE_EXTENSIONS:
                    files.append(os.path.join(root, fname))
        return sorted(files)
