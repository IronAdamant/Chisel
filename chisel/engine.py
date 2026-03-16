"""ChiselEngine — main orchestrator tying together all subsystems."""

import os
from pathlib import Path

from chisel.ast_utils import compute_file_hash, extract_code_units
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

_SKIP_DIRS = {
    ".git", "node_modules", "__pycache__", ".tox", ".venv", "venv",
    "env", ".mypy_cache", ".pytest_cache", ".ruff_cache", "dist",
    "build", ".eggs", "target",
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

    def analyze(self, force=False):
        """Full rebuild of all data under a write lock.

        Steps:
        1. Scan files, compute hashes
        2. Parse code units for changed files (or all if force)
        3. Discover + parse test files
        4. Parse git log, compute churn/co-changes
        5. Parse git blame for changed files
        6. Build test edges

        Returns:
            Dict summarizing the analysis results.
        """
        with self.lock.write_lock():
            stats = {
                "code_files_scanned": 0,
                "code_units_found": 0,
                "test_files_found": 0,
                "test_units_found": 0,
                "test_edges_built": 0,
                "commits_parsed": 0,
            }

            # 1. Scan code files
            code_files = self._scan_code_files()
            changed_files = []
            for fpath in code_files:
                rel = os.path.relpath(fpath, self.project_dir)
                new_hash = compute_file_hash(fpath)
                old_hash = self.storage.get_file_hash(rel)
                if force or old_hash != new_hash:
                    changed_files.append((fpath, rel, new_hash))
                    self.storage.set_file_hash(rel, new_hash)
            stats["code_files_scanned"] = len(code_files)

            # 2. Parse code units for changed files
            all_code_units = []
            for fpath, rel, _ in changed_files:
                content = Path(fpath).read_text(encoding="utf-8", errors="replace")
                self.storage.delete_code_units_by_file(rel)
                units = extract_code_units(fpath, content)
                for u in units:
                    cid = f"{rel}:{u.name}:{u.unit_type}"
                    self.storage.upsert_code_unit(
                        cid, rel, u.name, u.unit_type,
                        u.line_start, u.line_end,
                        compute_file_hash(fpath),
                    )
                    all_code_units.append(u)
                stats["code_units_found"] += len(units)

            # Also gather unchanged code units from storage
            for fpath in code_files:
                rel = os.path.relpath(fpath, self.project_dir)
                existing = self.storage.get_code_units_by_file(rel)
                all_code_units_dicts = existing  # for edge building later

            # 3. Discover + parse test files
            test_files = self.mapper.discover_test_files()
            stats["test_files_found"] = len(test_files)
            all_test_units = []
            for tf in test_files:
                test_units = self.mapper.parse_test_file(tf)
                for tu in test_units:
                    self.storage.upsert_test_unit(
                        tu["id"], tu["file_path"], tu["name"], tu["framework"],
                        tu["line_start"], tu["line_end"], tu["content_hash"],
                    )
                    all_test_units.append(tu)
            stats["test_units_found"] = len(all_test_units)

            # 4. Parse git log, compute churn/co-changes
            try:
                commits = self.git.parse_log()
                stats["commits_parsed"] = len(commits)

                for commit in commits:
                    self.storage.upsert_commit(
                        commit["hash"], commit["author"], commit["author_email"],
                        commit["date"], commit["message"],
                    )
                    for f in commit.get("files", []):
                        self.storage.upsert_commit_file(
                            commit["hash"], f["path"], f["insertions"], f["deletions"],
                        )

                # Churn stats for all tracked files (file-level + unit-level)
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
                            # Strip class prefix for git log -L (it uses bare names)
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

                # Co-change coupling
                co_changes = self.git.compute_co_changes(commits, min_count=3)
                for cc in co_changes:
                    self.storage.upsert_co_change(
                        cc["file_a"], cc["file_b"],
                        cc["co_commit_count"], cc["last_co_commit"],
                    )
            except RuntimeError:
                pass  # Not a git repo or git not available

            # 5. Parse git blame for changed files
            for _, rel, new_hash in changed_files:
                try:
                    blame_blocks = self.git.parse_blame(rel)
                    self.storage.invalidate_blame(rel)
                    for block in blame_blocks:
                        self.storage.store_blame(
                            rel, block["line_start"], block["line_end"],
                            block["commit_hash"], block["author"],
                            block["author_email"], block["date"], new_hash,
                        )
                except RuntimeError:
                    pass

            # 6. Build test edges
            # Gather all code units from storage
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
            stats["test_edges_built"] = len(edges)

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
            stats = {"files_updated": 0, "new_commits": 0}

            # Find changed code files
            code_files = self._scan_code_files()
            changed = []
            for fpath in code_files:
                rel = os.path.relpath(fpath, self.project_dir)
                new_hash = compute_file_hash(fpath)
                old_hash = self.storage.get_file_hash(rel)
                if old_hash != new_hash:
                    changed.append((fpath, rel, new_hash))

            # Re-parse changed files
            for fpath, rel, new_hash in changed:
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

                # Re-blame changed files
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

            stats["files_updated"] = len(changed)

            # Parse new commits since last known
            try:
                since = self.storage.get_latest_commit_date()
                commits = self.git.parse_log(since=since)
                new_count = 0
                for commit in commits:
                    if self.storage.get_commit(commit["hash"]) is None:
                        new_count += 1
                        self.storage.upsert_commit(
                            commit["hash"], commit["author"],
                            commit["author_email"], commit["date"],
                            commit["message"],
                        )
                        for f in commit.get("files", []):
                            self.storage.upsert_commit_file(
                                commit["hash"], f["path"],
                                f["insertions"], f["deletions"],
                            )
                stats["new_commits"] = new_count
            except RuntimeError:
                pass

            # Re-discover test files and rebuild edges
            test_files = self.mapper.discover_test_files()
            all_test_units = []
            for tf in test_files:
                tus = self.mapper.parse_test_file(tf)
                for tu in tus:
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

            return stats

    # ------------------------------------------------------------------ #
    # Tool methods (one per MCP tool)
    # ------------------------------------------------------------------ #

    def tool_analyze(self, directory=None, force=False):
        """MCP tool: full analysis."""
        return self.analyze(force=force)

    def tool_impact(self, files, functions=None):
        """MCP tool: get impacted tests for changed files."""
        with self.lock.read_lock():
            return self.impact.get_impacted_tests(files, functions)

    def tool_suggest_tests(self, file_path, diff=None):
        """MCP tool: suggest tests for a file."""
        with self.lock.read_lock():
            return self.impact.suggest_tests(file_path, diff)

    def tool_churn(self, file_path, unit_name=None):
        """MCP tool: get churn stats."""
        with self.lock.read_lock():
            stat = self.storage.get_churn_stat(file_path, unit_name)
            if stat:
                return stat
            return self.storage.get_all_churn_stats(file_path)

    def tool_ownership(self, file_path):
        """MCP tool: get ownership breakdown."""
        with self.lock.read_lock():
            return self.impact.who_reviews(file_path)

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
        """MCP tool: reviewer suggestions."""
        with self.lock.read_lock():
            return self.impact.who_reviews(file_path)

    # ------------------------------------------------------------------ #
    # Internal helpers
    # ------------------------------------------------------------------ #

    def _scan_code_files(self):
        """Walk project tree and return all code file paths."""
        files = []
        for root, dirs, filenames in os.walk(self.project_dir):
            dirs[:] = [d for d in dirs if d not in _SKIP_DIRS]
            for fname in filenames:
                if Path(fname).suffix in _CODE_EXTENSIONS:
                    files.append(os.path.join(root, fname))
        return sorted(files)
