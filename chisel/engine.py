"""ChiselEngine — main orchestrator tying together all subsystems."""

import json
import logging
import os
import threading
import time
import uuid
from contextlib import contextmanager

try:
    import tomllib
except ImportError:  # pragma: no cover
    tomllib = None

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

logger = logging.getLogger(__name__)


class JobCancelledError(Exception):
    """Raised when a background analyze/update job is cancelled mid-flight."""


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
        self._bg_job_progress = {}
        self._job_event_buffer: dict[str, list[dict]] = {}
        self._shard_engines = {}
        self._shard_config = self._load_shard_config()
        self._init_shard_engines(storage_dir)
        self._check_project_fingerprint()
        # Mark any jobs left as 'running' from a previous server/process crash
        self.storage.sweep_stale_bg_jobs()
        for _storage, *_ in self._shard_engines.values():
            _storage.sweep_stale_bg_jobs()

    # ------------------------------------------------------------------ #
    # Shard routing
    # ------------------------------------------------------------------ #

    def _load_shard_config(self):
        """Load shard definitions from env var or .chisel/shards.toml."""
        env = os.environ.get("CHISEL_SHARDS", "")
        if env:
            return self._parse_shard_config(env)

        toml_path = os.path.join(self.project_dir, ".chisel", "shards.toml")
        if tomllib and os.path.isfile(toml_path):
            try:
                with open(toml_path, "rb") as fh:
                    data = tomllib.load(fh)
                shards = data.get("shards", [])
                if isinstance(shards, list):
                    return self._parse_shard_config(",".join(shards))
            except (OSError, tomllib.TOMLDecodeError, ValueError) as exc:
                logger.warning("Failed to load shard config from %s: %s", toml_path, exc)
        return []

    def _parse_shard_config(self, config_str):
        """Parse a comma-separated shard config into normalized keys."""
        shards = set()
        for part in config_str.split(","):
            part = part.strip().strip("/")
            if not part:
                continue
            if part.endswith("/*"):
                base = part[:-2]
                base_path = os.path.join(self.project_dir, base)
                if os.path.isdir(base_path):
                    for name in os.listdir(base_path):
                        child = os.path.join(base_path, name)
                        if os.path.isdir(child):
                            shards.add(f"{base}/{name}")
            else:
                shards.add(part)
        return sorted(shards)

    def _init_shard_engines(self, storage_dir):
        """Initialize per-shard storage, locks, and impact analyzers."""
        self._shard_engines = {
            None: (self.storage, self.lock, self._process_lock, self.impact),
        }
        for shard in self._shard_config:
            shard_dir = resolve_storage_dir(self.project_dir, storage_dir, shard=shard)
            storage = Storage(base_dir=shard_dir)
            lock = RWLock()
            process_lock = ProcessLock(shard_dir)
            impact = ImpactAnalyzer(storage, self.project_dir)
            self._shard_engines[shard] = (storage, lock, process_lock, impact)

    @contextmanager
    def _with_shard(self, shard):
        """Temporarily swap self.storage/lock/process_lock/impact to a shard."""
        if shard is None or shard not in self._shard_engines:
            yield
            return
        old_storage = self.storage
        old_lock = self.lock
        old_process_lock = self._process_lock
        old_impact = self.impact
        self.storage, self.lock, self._process_lock, self.impact = self._shard_engines[shard]
        try:
            yield
        finally:
            self.storage = old_storage
            self.lock = old_lock
            self._process_lock = old_process_lock
            self.impact = old_impact

    def _shard_for_path(self, file_path):
        """Return the shard key for a project-relative file path."""
        if not self._shard_config:
            return None
        norm = file_path.replace("\\", "/")
        best = None
        best_len = 0
        for shard in self._shard_config:
            prefix = shard + "/"
            if norm.startswith(prefix) or norm == shard:
                if len(shard) > best_len:
                    best = shard
                    best_len = len(shard)
        return best

    def _shards_for_directory(self, directory):
        """Return shard keys that fall under a project-relative directory."""
        if not self._shard_config:
            return [None]
        dir_norm = directory.replace("\\", "/").rstrip("/")
        shards = [s for s in self._shard_config if s.startswith(dir_norm + "/") or s == dir_norm]
        return shards if shards else [None]

    def _check_analysis_data(self):
        """Return a no-data warning dict if the DB is empty, else ``None``."""
        if not self.storage.has_analysis_data():
            return dict(_NO_DATA_RESPONSE)
        return None

    def _check_any_shard_analysis_data(self):
        """Return no-data only if *all* shards are empty."""
        for storage, *_ in self._shard_engines.values():
            if storage.has_analysis_data():
                return None
        return dict(_NO_DATA_RESPONSE)

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
        self._record_job_event("phase_start", {"phase": "scan", "directory": directory})
        code_files = self._scan_code_files(directory=directory)
        self._check_job_cancelled()

        commits = None
        git_warning = None
        try:
            commits = self.git.parse_log()
        except RuntimeError as exc:
            git_warning = str(exc)

        total_files = len(code_files)
        self._set_bg_progress(0)

        # Phase 2: write to storage under both process lock (cross-process)
        # and RWLock (in-process threads).
        with self._process_lock.exclusive():
            with self.lock.write_lock():
                self._record_job_event("phase_start", {"phase": "find_changed_files"})
                changed_files = self._find_changed_files(code_files, force=force)
                self._check_job_cancelled()

                stats = {
                    "code_files_scanned": total_files,
                    "code_units_found": 0,
                    "test_files_found": 0,
                    "test_units_found": 0,
                    "test_edges_built": 0,
                    "commits_parsed": 0,
                }
                self._record_job_event("phase_start", {"phase": "parse_and_store_code_units"})
                stats["code_units_found"] = self._parse_and_store_code_units(changed_files)
                self._check_job_cancelled()
                self._set_bg_progress(20)

                if commits is not None:
                    stats["commits_parsed"] = len(commits)
                    self._record_job_event("phase_start", {"phase": "store_commits", "count": len(commits)})
                    self._store_commits(commits)
                    if commits:
                        self.storage.set_meta("last_commit_date", commits[0]["date"])
                    self._check_job_cancelled()
                    self._record_job_event("phase_start", {"phase": "churn_and_coupling"})
                    self._compute_churn_and_coupling(commits, code_files)
                    self._check_job_cancelled()
                    self._record_job_event("phase_start", {"phase": "blame"})
                    self._store_blame(changed_files)
                    self._check_job_cancelled()
                elif git_warning:
                    stats["git_warning"] = (
                        f"Git unavailable ({git_warning}); "
                        "churn, blame, and coupling will be missing."
                    )
                self._set_bg_progress(50)

                self._record_job_event("phase_start", {"phase": "discover_and_build_edges"})
                test_units, tf_count, edge_count = self._discover_and_build_edges(code_files)
                stats["test_files_found"] = tf_count
                stats["test_units_found"] = len(test_units)
                stats["test_edges_built"] = edge_count
                self._check_job_cancelled()
                self._set_bg_progress(75)

                self._record_job_event("phase_start", {"phase": "rebuild_import_edges"})
                self._rebuild_import_edges(code_files, changed_files)
                self._check_job_cancelled()
                self._record_job_event("phase_start", {"phase": "backfill_heuristic_edges"})
                self._backfill_heuristic_edges()
                self.storage.set_meta("project_fingerprint", self.project_dir)
                self._set_bg_progress(100)

                self._record_job_event("phase_start", {"phase": "cleanup"})
                stats["orphaned_results_cleaned"] = self.storage.cleanup_orphaned_test_results()
                self.storage.wal_checkpoint()
                self._record_job_event("phase_end", {"phase": "analyze", "stats": stats})
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
        self._record_job_event("phase_start", {"phase": "scan"})
        code_files = self._scan_code_files()
        self._check_job_cancelled()
        self._set_bg_progress(0)
        with self._process_lock.exclusive():
            with self.lock.write_lock():
                self._record_job_event("phase_start", {"phase": "find_changed_files"})
                changed_files = self._find_changed_files(code_files)
                self._check_job_cancelled()
                self._record_job_event("phase_start", {"phase": "parse_and_store_code_units"})
                code_units_found = self._parse_and_store_code_units(changed_files)
                self._check_job_cancelled()
                self._set_bg_progress(30)

                stats = {
                    "files_updated": len(changed_files),
                    "code_units_found": code_units_found,
                    "new_commits": 0,
                }

                try:
                    self._record_job_event("phase_start", {"phase": "commits"})
                    since = self.storage.get_meta("last_commit_date")
                    all_commits = self.git.parse_log(since=since)
                    new_commits = [
                        c for c in all_commits
                        if self.storage.get_commit(c["hash"]) is None
                    ]
                    self._store_commits(new_commits)
                    stats["new_commits"] = len(new_commits)
                    if new_commits:
                        latest_date = new_commits[0]["date"]
                        self.storage.set_meta("last_commit_date", latest_date)
                    self._check_job_cancelled()

                    if new_commits or changed_files:
                        self._record_job_event("phase_start", {"phase": "churn_and_coupling"})
                        self._compute_churn_and_coupling(all_commits, code_files)
                        self._check_job_cancelled()
                    self._record_job_event("phase_start", {"phase": "blame"})
                    self._store_blame(changed_files)
                    self._check_job_cancelled()
                except RuntimeError as exc:
                    stats["git_warning"] = (
                        f"Git unavailable ({exc}); "
                        "churn, blame, and coupling will be missing."
                    )
                self._set_bg_progress(60)

                self._record_job_event("phase_start", {"phase": "discover_and_build_edges"})
                self._discover_and_build_edges(code_files)
                self._check_job_cancelled()
                self._record_job_event("phase_start", {"phase": "rebuild_import_edges"})
                self._rebuild_import_edges(code_files, changed_files)
                self._check_job_cancelled()
                self._record_job_event("phase_start", {"phase": "backfill_heuristic_edges"})
                self._backfill_heuristic_edges()
                self.storage.set_meta("project_fingerprint", self.project_dir)
                self._set_bg_progress(100)

                self._record_job_event("phase_start", {"phase": "cleanup"})
                stats["orphaned_results_cleaned"] = self.storage.cleanup_orphaned_test_results()
                self.storage.wal_checkpoint()
                self._record_job_event("phase_end", {"phase": "update", "stats": stats})
                return stats

    # ------------------------------------------------------------------ #
    # Tool methods (one per MCP tool)
    # ------------------------------------------------------------------ #

    _AUTO_BG_JOB_THRESHOLD = 300
    _AUTO_UPDATE_MAX_FILES = 50

    # Human-readable hints for each reason _try_auto_update may skip.
    # Used by callers that return stale_db or similar envelopes so agents can
    # distinguish "DB is current" from "DB is stale but auto-refresh was skipped".
    _AUTO_UPDATE_SKIP_HINTS = {
        "background_job_running": (
            "Auto-update skipped: a background analyze/update is in progress. "
            "Poll job_status and retry when it completes."
        ),
        "too_many_changed_files": (
            f"Auto-update skipped: more than {_AUTO_UPDATE_MAX_FILES} files changed. "
            "Run 'chisel update' or start_job(kind='update') to refresh the DB."
        ),
    }

    def _try_auto_update(self):
        """Attempt a lightweight inline update when DB is stale.

        Returns:
            tuple: (performed: bool, reason: str | None)
        """
        with self._bg_jobs_lock:
            if self._bg_job_in_progress:
                return False, "background_job_running"

        # Scan + change-check must happen under the same lock window so concurrent
        # writers cannot insert new files between scan and hash comparison.
        with self._process_lock.exclusive():
            with self.lock.write_lock():
                code_files = self._scan_code_files()
                changed_files = self._find_changed_files(code_files)
                if not changed_files:
                    return False, "no_changes"
                if len(changed_files) > self._AUTO_UPDATE_MAX_FILES:
                    return False, "too_many_changed_files"

        self.update()
        return True, None

    def tool_analyze(self, directory=None, force=False, shard=None):
        """MCP tool: full analysis.

        For large repositories (>300 code files with force=True), automatically
        falls back to a background job via start_job to avoid MCP timeouts.

        Args:
            shard: Optional shard key to scope the analysis to a shard DB.
        """
        # Quick size check to decide whether to auto-fallback to background job
        code_files = self._scan_code_files(directory=directory)
        if force and len(code_files) > self._AUTO_BG_JOB_THRESHOLD:
            job = self.tool_start_job(kind="analyze", directory=directory, force=force, shard=shard)
            return {
                **job,
                "status": "auto_queued",
                "message": (
                    f"Repository has {len(code_files)} code files; "
                    "synchronous analyze with force=True would likely time out. "
                    "A background job has been queued automatically."
                ),
            }
        target_shard = shard
        if target_shard is not None and target_shard not in self._shard_engines:
            return {
                "status": "error",
                "message": f"Unknown shard: {target_shard!r}. Known: {sorted(self._shard_engines)}",
            }
        with self._with_shard(target_shard):
            result = self.analyze(directory=directory, force=force)
            result.setdefault(
                "hint",
                "For large repos, use start_job with kind='analyze' to avoid MCP timeouts.",
            )
            if target_shard:
                result["shard"] = target_shard
            return result

    def tool_start_job(self, kind, directory=None, force=False, shard=None):
        """MCP tool: run ``analyze`` or ``update`` in a background thread.

        Poll ``job_status`` until ``status`` is ``completed`` or ``failed``.
        Avoids MCP client timeouts on large repos (zero extra dependencies).

        Args:
            shard: Optional shard key to scope the job to a shard DB.
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
        with self._bg_jobs_lock:
            self._active_bg_job_id = job_id
        self.storage.insert_bg_job(job_id, kind, "running")

        def run():
            try:
                if kind == "analyze":
                    with self._with_shard(shard):
                        out = self.analyze(directory=directory, force=force)
                else:
                    with self._with_shard(shard):
                        out = self.update()
                self.storage.update_bg_job(
                    job_id, "completed", result_json=json.dumps(out, default=str),
                    progress_pct=100,
                )
            except JobCancelledError as exc:
                self.storage.update_bg_job(
                    job_id, "failed", error_message=str(exc),
                    progress_pct=100,
                )
            except Exception as exc:
                self.storage.update_bg_job(
                    job_id, "failed", error_message=str(exc),
                    progress_pct=100,
                )
            finally:
                with self._bg_jobs_lock:
                    self._bg_job_in_progress = False
                    self._active_bg_job_id = None
                    mem_events = self._job_event_buffer.pop(job_id, [])
                for ev in mem_events:
                    self.storage.insert_job_event(
                        job_id, ev["event_type"], ev.get("payload"),
                    )

        threading.Thread(
            target=run, name=f"chisel-bg-{kind}", daemon=True,
        ).start()
        result = {
            "job_id": job_id,
            "status": "running",
            "kind": kind,
            "hint": "Poll job_status with job_id until completed or failed.",
        }
        if shard:
            result["shard"] = shard
        return result

    def tool_cancel_job(self, job_id):
        """MCP tool: request cancellation of a running background job."""
        row = self.storage.get_bg_job(job_id)
        if not row:
            return {
                "status": "not_found",
                "job_id": job_id,
                "message": "No job with this id",
            }
        if row["status"] != "running":
            return {
                "status": "error",
                "job_id": job_id,
                "message": f"Job is already {row['status']}",
            }
        self.storage.request_bg_job_cancel(job_id)
        return {
            "status": "ok",
            "job_id": job_id,
            "message": "Cancellation requested",
        }

    def tool_job_status(self, job_id):
        """MCP tool: poll a job started with ``tool_start_job`` ."""
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
            "cancel_requested": row.get("cancel_requested_at") is not None,
        }
        # Prefer in-memory progress; fall back to DB column
        with self._bg_jobs_lock:
            mem_pct = self._bg_job_progress.get(job_id)
        db_pct = row.get("progress_pct")
        if mem_pct is not None:
            out["progress_pct"] = mem_pct
        elif db_pct is not None:
            out["progress_pct"] = db_pct
        if row["error_message"]:
            out["error"] = row["error_message"]
        if row["result_json"]:
            out["result"] = json.loads(row["result_json"])
        if row["status"] == "running":
            with self._bg_jobs_lock:
                mem_events = [
                    {"event_type": e["event_type"], "created_at": e["created_at"],
                     **({"payload": e["payload"]} if e.get("payload") else {})}
                    for e in self._job_event_buffer.get(job_id, [])
                ]
            out["events"] = mem_events[-20:]
        else:
            out["events"] = self.storage.get_job_events(job_id, limit=20)
        return out

    def tool_impact(self, files, functions=None):
        """MCP tool: get impacted tests for changed files."""
        if not self._shard_config:
            with self._process_lock.shared():
                with self.lock.read_lock():
                    empty = self._check_analysis_data()
                    if empty is not None:
                        return empty
                    return self.impact.get_impacted_tests(files, functions)

        # Shard-aware: group files by shard, aggregate results
        shard_files = {}
        for fp in files:
            shard = self._shard_for_path(fp)
            shard_files.setdefault(shard, []).append(fp)

        all_results = []
        for shard, sfiles in shard_files.items():
            with self._with_shard(shard):
                with self._process_lock.shared():
                    with self.lock.read_lock():
                        if self._check_analysis_data() is not None:
                            continue
                        result = self.impact.get_impacted_tests(sfiles, functions)
                        all_results.extend(result)
        return all_results

    def _suggest_single_file(self, file_path, fallback_to_all, working_tree,
                              disk, extra):
        """Suggest tests for a single file (used by file_path and directory modes)."""
        # Fast path for working-tree files with no DB edges:
        if working_tree and self.storage.get_file_hash(file_path) is None:
            result = []
            if not fallback_to_all:
                result = self.impact._fallback_suggest_tests(file_path)
            if not result:
                result = self._working_tree_suggest(file_path, disk_test_files=disk)
            return result[:_WORKING_TREE_SUGGEST_LIMIT] if len(result) > _WORKING_TREE_SUGGEST_LIMIT else result

        if working_tree:
            quick_db = self.impact.get_impacted_tests([file_path])
            if not quick_db:
                result = []
                if not fallback_to_all:
                    result = self.impact._fallback_suggest_tests(file_path)
                if not result:
                    result = self._working_tree_suggest(file_path, disk_test_files=disk)
                return result[:_WORKING_TREE_SUGGEST_LIMIT] if len(result) > _WORKING_TREE_SUGGEST_LIMIT else result

        result = self.impact.suggest_tests(
            file_path,
            fallback_to_all=fallback_to_all,
            disk_test_files=disk,
            extra_code_paths=extra,
        )
        if not result and not fallback_to_all:
            result = self.impact._fallback_suggest_tests(file_path)
        if not result and not working_tree:
            result = self._working_tree_suggest(file_path, disk_test_files=disk)
        if working_tree and not result:
            result = self._working_tree_suggest(file_path, disk_test_files=disk)
        return result[:_WORKING_TREE_SUGGEST_LIMIT] if len(result) > _WORKING_TREE_SUGGEST_LIMIT else result

    def _suggest_tests_impl(self, file_path, directory, fallback_to_all,
                            working_tree, auto_update, disk, extra):
        """Core suggest_tests logic assuming shard context is already set."""
        with self._process_lock.shared():
            with self.lock.read_lock():
                empty = self._check_analysis_data()
                if empty is not None:
                    return empty

                if file_path:
                    file_missing = self.storage.get_file_hash(file_path) is None and not working_tree
                else:
                    file_missing = False

        if file_path and file_missing and auto_update:
            performed, reason = self._try_auto_update()
            if performed:
                return self._suggest_tests_impl(
                    file_path, directory, fallback_to_all, working_tree,
                    False, disk, extra,
                )
            hint = self._AUTO_UPDATE_SKIP_HINTS.get(
                reason,
                "Run 'chisel update' first to include new or changed files.",
            )
            result = {
                "status": "stale_db",
                "file_path": file_path,
                "message": "File not found in analysis database.",
                "hint": hint,
            }
            if reason:
                result["auto_update_skip_reason"] = reason
            return result

        with self._process_lock.shared():
            with self.lock.read_lock():
                empty = self._check_analysis_data()
                if empty is not None:
                    return empty

                if file_path:
                    if self.storage.get_file_hash(file_path) is None and not working_tree:
                        return {
                            "status": "stale_db",
                            "file_path": file_path,
                            "message": "File not found in analysis database.",
                            "hint": "Run 'chisel update' first to include new or changed files.",
                        }
                    return self._suggest_single_file(
                        file_path, fallback_to_all, working_tree, disk, extra,
                    )

                # Directory mode
                dir_norm = directory.replace("\\", "/").rstrip("/") + "/"
                db_paths = self.storage.get_distinct_code_file_paths()
                targets = {
                    p for p in db_paths
                    if p.replace("\\", "/").startswith(dir_norm)
                }
                if working_tree and extra:
                    targets.update(
                        p for p in extra
                        if p.replace("\\", "/").startswith(dir_norm)
                    )
                if not targets:
                    return {}

                result = {}
                for fp in sorted(targets):
                    suggestions = self._suggest_single_file(
                        fp, fallback_to_all, working_tree, disk, extra,
                    )
                    if suggestions:
                        result[fp] = suggestions
                return result

    def tool_suggest_tests(self, file_path=None, directory=None,
                           fallback_to_all=False, working_tree=False,
                           auto_update=False):
        """MCP tool: suggest tests for a file or all files under a directory.

        Args:
            file_path: Specific code file to query (mutually exclusive with
                       *directory*).
            directory: Directory path; returns a mapping of ``{file_path: [suggestions]}``
                       for every code file found under that directory.
            fallback_to_all: If True and no test edges exist, return all known
                test files ranked by stem-match relevance.
            working_tree: If True, also check untracked files on disk and use
                stem-matching to find relevant tests when the file has no DB edges.
                Useful during active development before files are committed.
            auto_update: If True, attempt an incremental update when the DB
                is stale before returning results.
        """
        if not file_path and not directory:
            return {
                "status": "error",
                "message": "Provide either file_path or directory.",
            }
        if file_path and directory:
            return {
                "status": "error",
                "message": "Provide only one of file_path or directory, not both.",
            }

        disk = self._disk_only_test_map() if working_tree else None
        extra = self._git_untracked_code_paths() if working_tree else None

        if not self._shard_config:
            return self._suggest_tests_impl(
                file_path, directory, fallback_to_all, working_tree,
                auto_update, disk, extra,
            )

        if file_path:
            shard = self._shard_for_path(file_path)
            with self._with_shard(shard):
                return self._suggest_tests_impl(
                    file_path, directory, fallback_to_all, working_tree,
                    auto_update, disk, extra,
                )

        # Directory mode: aggregate across matching shards
        shards = self._shards_for_directory(directory) if directory else list(self._shard_engines.keys())
        merged = {}
        for shard in shards:
            with self._with_shard(shard):
                partial = self._suggest_tests_impl(
                    file_path, directory, fallback_to_all, working_tree,
                    auto_update, disk, extra,
                )
                if isinstance(partial, dict):
                    merged.update(partial)
        return merged

    def tool_churn(self, file_path, unit_name=None):
        """MCP tool: get churn stats. Always returns a list."""
        shard = self._shard_for_path(file_path)
        with self._with_shard(shard):
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
        shard = self._shard_for_path(file_path)
        with self._with_shard(shard):
            with self._process_lock.shared():
                with self.lock.read_lock():
                    empty = self._check_analysis_data()
                    if empty is not None:
                        return empty
                    return self.impact.get_ownership(file_path)

    def tool_coupling(self, file_path, min_count=3):
        """MCP tool: get co-change and import coupling partners."""
        shard = self._shard_for_path(file_path)
        with self._with_shard(shard):
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
                    # First-class hybrid: import-graph coupling is treated as an equal
                    # signal to co-change, not a minor additive boost.
                    effective_coupling = max(
                        co_norm,
                        import_norm,
                        0.5 * co_norm + 0.5 * import_norm,
                    )
                    return {
                        "co_change_partners": co_change_partners,
                        "import_partners": [{"file": path} for path in import_neighbors],
                        "co_change_breadth": co_len,
                        "import_breadth": imp_len,
                        "cochange_coupling": round(co_norm, 4),
                        "import_coupling": round(import_norm, 4),
                        "effective_coupling": round(effective_coupling, 4),
                    }

    def _risk_map_impl(self, directory, exclude_tests, proximity_adjustment,
                       coverage_mode, extra, exclude_new_file_boost):
        """Core risk_map computation for the currently active shard."""
        with self._process_lock.shared():
            with self.lock.read_lock():
                empty = self._check_analysis_data()
                if empty is not None:
                    return empty
                files = self.impact.get_risk_map(
                    directory, exclude_tests, proximity_adjustment, coverage_mode,
                    extra_files=extra,
                    exclude_new_file_boost=exclude_new_file_boost,
                )
                files, rw_meta = apply_risk_reweighting(files)
                stats = self.storage.get_stats()
                meta = build_risk_meta(files, stats)
                meta.update(rw_meta)
                meta["coverage_gap_mode"] = (
                    "proximity_adjusted" if proximity_adjustment else coverage_mode
                )
                # Compute cycles from import graph for inclusion in _meta
                all_files = {f["file_path"] for f in files}
                import_neighbors = self.storage.get_import_neighbors_batch(list(all_files))
                from chisel.impact import _find_circular_dependencies
                cycles = _find_circular_dependencies(all_files, import_neighbors)
                meta["cycles"] = cycles
                return {"files": files, "_meta": meta}

    def tool_risk_map(self, directory=None, exclude_tests=True,
                      proximity_adjustment=True, coverage_mode="line",
                      working_tree=False, exclude_new_file_boost=False,
                      auto_update=False):
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
            working_tree: If True, include untracked files in the risk map
                so newly created files are visible.
            exclude_new_file_boost: If True, suppress the 0.5 new-file boost
                so long-term risk rankings are not skewed toward recently
                touched files.
            auto_update: If True, attempt an incremental update before
                scoring so recent changes are reflected.
        """
        auto_update_performed = False
        auto_update_reason = None
        if auto_update:
            auto_update_performed, auto_update_reason = self._try_auto_update()

        extra = None
        untracked_warning = None
        if working_tree:
            extra = sorted(self._git_untracked_code_paths())
        else:
            untracked = self._git_untracked_code_paths()
            if untracked:
                untracked_warning = (
                    f"{len(untracked)} untracked code files excluded "
                    "from risk scoring due to lack of git history"
                )

        if not self._shard_config:
            result = self._risk_map_impl(
                directory, exclude_tests, proximity_adjustment,
                coverage_mode, extra, exclude_new_file_boost,
            )
            if isinstance(result, dict) and "_meta" in result:
                meta = result["_meta"]
                if auto_update:
                    meta["auto_update_performed"] = auto_update_performed
                    if not auto_update_performed:
                        meta["auto_update_skip_reason"] = auto_update_reason
                if extra:
                    meta["working_tree_files_included"] = len(extra)
                if untracked_warning:
                    meta.setdefault("warnings", []).append(untracked_warning)
                bc = self.storage.get_meta("branch_coupling_commits")
                if bc is not None:
                    try:
                        meta["branch_coupling_commits"] = int(bc)
                    except ValueError:
                        pass
            return result

        # Shard-aware aggregation
        shards = self._shards_for_directory(directory) if directory else list(self._shard_engines.keys())
        all_files = []
        all_cycles = []
        total_stats = {}
        for shard in shards:
            with self._with_shard(shard):
                partial = self._risk_map_impl(
                    directory, exclude_tests, proximity_adjustment,
                    coverage_mode, extra, exclude_new_file_boost,
                )
                if isinstance(partial, dict) and "files" in partial:
                    all_files.extend(partial["files"])
                    all_cycles.extend(partial.get("_meta", {}).get("cycles", []))
                    stats = self.storage.get_stats()
                    for k, v in stats.items():
                        if isinstance(v, int):
                            total_stats[k] = total_stats.get(k, 0) + v

        if not all_files:
            empty = self._check_any_shard_analysis_data()
            if empty is not None:
                return empty

        all_files.sort(key=lambda x: x["risk_score"], reverse=True)
        all_files, rw_meta = apply_risk_reweighting(all_files)
        meta = build_risk_meta(all_files, total_stats)
        meta.update(rw_meta)
        meta["coverage_gap_mode"] = "proximity_adjusted" if proximity_adjustment else coverage_mode
        seen_cycles = set()
        unique_cycles = []
        for c in all_cycles:
            key = tuple(sorted(c.get("files", [])))
            if key not in seen_cycles:
                seen_cycles.add(key)
                unique_cycles.append(c)
        meta["cycles"] = unique_cycles
        if auto_update:
            meta["auto_update_performed"] = auto_update_performed
            if not auto_update_performed:
                meta["auto_update_skip_reason"] = auto_update_reason
        if extra:
            meta["working_tree_files_included"] = len(extra)
        if untracked_warning:
            meta.setdefault("warnings", []).append(untracked_warning)
        with self._with_shard(None):
            bc = self.storage.get_meta("branch_coupling_commits")
            if bc is not None:
                try:
                    meta["branch_coupling_commits"] = int(bc)
                except ValueError:
                    pass
        return {"files": all_files, "_meta": meta}

    def tool_stale_tests(self):
        """MCP tool: detect stale tests.

        Returns a diagnostic dict (status="no_edges") when no test edges
        exist, so agents can distinguish "no stale tests" from "nothing
        to evaluate".
        """
        if not self._shard_config:
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

        # Shard-aware aggregation
        all_stale = []
        total_test_edges = 0
        for shard in self._shard_engines:
            with self._with_shard(shard):
                with self._process_lock.shared():
                    with self.lock.read_lock():
                        if self._check_analysis_data() is not None:
                            continue
                        result = self.impact.detect_stale_tests()
                        all_stale.extend(result)
                        total_test_edges += self.storage.get_stats().get("test_edges", 0)
        if not all_stale and total_test_edges == 0:
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
        return all_stale

    def tool_history(self, file_path):
        """MCP tool: commit history for a file."""
        shard = self._shard_for_path(file_path)
        with self._with_shard(shard):
            with self._process_lock.shared():
                with self.lock.read_lock():
                    empty = self._check_analysis_data()
                    if empty is not None:
                        return empty
                    return self.storage.get_commits_for_file(file_path)

    def tool_who_reviews(self, file_path):
        """MCP tool: suggest reviewers based on recent commit activity."""
        shard = self._shard_for_path(file_path)
        with self._with_shard(shard):
            with self._process_lock.shared():
                with self.lock.read_lock():
                    empty = self._check_analysis_data()
                    if empty is not None:
                        return empty
                    return self.impact.suggest_reviewers(file_path)

    def _missing_from_db(self, changed_files):
        """Return list of changed files missing from the current shard DB."""
        with self._process_lock.shared():
            with self.lock.read_lock():
                return [
                    cf for cf in changed_files
                    if self.storage.get_file_hash(cf) is None
                ]

    def tool_diff_impact(self, ref=None, working_tree=False,
                         auto_update=False):
        """MCP tool: auto-detect changes from git diff and return impacted tests.

        If ref is not provided, auto-detects: on a feature branch diffs against
        main/master; on main diffs against HEAD (unstaged changes).

        Args:
            ref: Git ref to diff against.
            working_tree: If True, perform full static import scanning for
                untracked files (matching suggest_tests behavior) instead of
                only stem-match fallback.
            auto_update: If True, attempt an incremental update when changed
                files are missing from the DB before returning results.

        Returns a diagnostic dict (status="no_changes") instead of bare []
        when no diff is found, so LLM agents can reason about why.
        """
        if not self._shard_config:
            with self._process_lock.shared():
                with self.lock.read_lock():
                    empty = self._check_analysis_data()
                    if empty is not None:
                        return empty
        else:
            empty = self._check_any_shard_analysis_data()
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
        functions_per_file = {}
        for fp in changed_files:
            if fp in untracked_code:
                continue
            try:
                funcs = self.git.get_changed_functions(fp, ref)
                # Treat empty list as None so files with no detectable function
                # context still get whole-file direct-edge impact.
                functions_per_file[fp] = funcs if funcs else None
            except RuntimeError:
                functions_per_file[fp] = None
        disk = self._disk_only_test_map() if working_tree else None
        extra = self._git_untracked_code_paths() if working_tree else None

        # Stale-DB detection across shards
        if not self._shard_config:
            missing_from_db = self._missing_from_db(changed_files)
        else:
            missing_from_db = []
            for cf in changed_files:
                shard = self._shard_for_path(cf)
                with self._with_shard(shard):
                    if self.storage.get_file_hash(cf) is None:
                        missing_from_db.append(cf)

        auto_update_reason = None
        if missing_from_db and auto_update:
            performed, auto_update_reason = self._try_auto_update()
            if performed:
                return self.tool_diff_impact(
                    ref=ref,
                    working_tree=working_tree,
                    auto_update=False,
                )

        if missing_from_db:
            hint = self._AUTO_UPDATE_SKIP_HINTS.get(
                auto_update_reason,
                "Run 'chisel update' first to include new or changed files.",
            )
            result = {
                "status": "stale_db",
                "changed_files": changed_files,
                "missing_from_db": missing_from_db,
                "message": "Some changed files are missing from the analysis database.",
                "hint": hint,
            }
            if auto_update_reason:
                result["auto_update_skip_reason"] = auto_update_reason
            return result

        if not self._shard_config:
            with self._process_lock.shared():
                with self.lock.read_lock():
                    empty = self._check_analysis_data()
                    if empty is not None:
                        return empty
                    missing_from_db = self._missing_from_db(changed_files)
                    if missing_from_db:
                        return {
                            "status": "stale_db",
                            "changed_files": changed_files,
                            "missing_from_db": missing_from_db,
                            "message": "Some changed files are missing from the analysis database.",
                            "hint": "Run 'chisel update' first to include new or changed files.",
                        }
                    result = self.impact.get_impacted_tests(
                        changed_files,
                        functions_per_file or None,
                        untracked_files=untracked_code,
                    )
                    if working_tree:
                        idx = self.impact._get_static_index(
                            disk_test_files=disk,
                            extra_code_paths=extra,
                        )
                        covered = {item["test_id"] for item in result}
                        tracked_changed = [cf for cf in changed_files if cf not in untracked_code]
                        for cf in tracked_changed:
                            static_hits = idx.find_tests(cf)
                            for sh in static_hits:
                                tid = sh["test_id"]
                                if tid not in covered:
                                    covered.add(tid)
                                    result.append({
                                        "test_id": tid,
                                        "file_path": sh["file_path"],
                                        "name": sh["name"],
                                        "reason": f"static import of {cf}",
                                        "score": sh["score"],
                                        "source": "working_tree",
                                    })
                    if untracked_code or working_tree:
                        covered = {item["test_id"] for item in result}
                        files_with_hits = set()
                        for item in result:
                            reason = item.get("reason", "")
                            for cf in changed_files:
                                if cf in reason:
                                    files_with_hits.add(cf)
                                    break
                        target_files = changed_files if working_tree else sorted(untracked_code)
                        for cf in target_files:
                            if cf in files_with_hits:
                                continue
                            stem_hits = self._working_tree_suggest(cf, disk_test_files=disk)
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
                                        "reason": f"stem-match for {cf}",
                                        "score": sh["relevance"] * 0.4,
                                        "source": "working_tree",
                                    })
                    return result

        # Shard-aware aggregation
        shard_files = {}
        for fp in changed_files:
            shard = self._shard_for_path(fp)
            shard_files.setdefault(shard, []).append(fp)

        all_results = []
        for shard, sfiles in shard_files.items():
            with self._with_shard(shard):
                with self._process_lock.shared():
                    with self.lock.read_lock():
                        if self._check_analysis_data() is not None:
                            continue
                        result = self.impact.get_impacted_tests(
                            sfiles, functions_per_file or None, untracked_files=untracked_code,
                        )
                        all_results.extend(result)

        if working_tree:
            covered = {item["test_id"] for item in all_results}
            tracked_changed = [cf for cf in changed_files if cf not in untracked_code]
            for cf in tracked_changed:
                # Query each shard's static index for hits
                for shard in self._shard_engines:
                    with self._with_shard(shard):
                        idx = self.impact._get_static_index(
                            disk_test_files=disk,
                            extra_code_paths=extra,
                        )
                        static_hits = idx.find_tests(cf)
                        for sh in static_hits:
                            tid = sh["test_id"]
                            if tid not in covered:
                                covered.add(tid)
                                all_results.append({
                                    "test_id": tid,
                                    "file_path": sh["file_path"],
                                    "name": sh["name"],
                                    "reason": f"static import of {cf}",
                                    "score": sh["score"],
                                    "source": "working_tree",
                                })

        if untracked_code or working_tree:
            covered = {item["test_id"] for item in all_results}
            files_with_hits = set()
            for item in all_results:
                reason = item.get("reason", "")
                for cf in changed_files:
                    if cf in reason:
                        files_with_hits.add(cf)
                        break
            target_files = changed_files if working_tree else sorted(untracked_code)
            for cf in target_files:
                if cf in files_with_hits:
                    continue
                stem_hits = self._working_tree_suggest(cf, disk_test_files=disk)
                for sh in stem_hits:
                    if sh.get("relevance", 0) < 0.5:
                        continue
                    tid = sh["test_id"]
                    if tid not in covered:
                        covered.add(tid)
                        all_results.append({
                            "test_id": tid,
                            "file_path": sh["file_path"],
                            "name": sh["name"],
                            "reason": f"stem-match for {cf}",
                            "score": sh["relevance"] * 0.4,
                            "source": "working_tree",
                        })
        return all_results

    def tool_update(self, shard=None):
        """MCP tool: incremental re-analysis of changed files.

        Args:
            shard: Optional shard key to scope the update to a shard DB.
        """
        target_shard = shard
        if target_shard is not None and target_shard not in self._shard_engines:
            return {
                "status": "error",
                "message": f"Unknown shard: {target_shard!r}. Known: {sorted(self._shard_engines)}",
            }
        with self._with_shard(target_shard):
            result = self.update()
            result.setdefault(
                "hint",
                "For large repos, use start_job with kind='update' to avoid MCP timeouts.",
            )
            if target_shard:
                result["shard"] = target_shard
            return result

    def tool_test_gaps(self, file_path=None, directory=None, exclude_tests=True,
                       working_tree=False, limit=None, auto_update=False):
        """MCP tool: find code units with no test coverage.

        Args:
            working_tree: If True, also scan untracked (uncommitted) files from
                disk and include their code units as gaps with churn=0. Useful
                for identifying coverage gaps in files that haven't been committed.
            limit: Optional maximum number of results to return.
            auto_update: If True, attempt an incremental update before
                querying so recent changes are reflected.
        """
        if auto_update:
            performed, reason = self._try_auto_update()
            if not performed and reason in self._AUTO_UPDATE_SKIP_HINTS:
                logger.warning("test_gaps %s", self._AUTO_UPDATE_SKIP_HINTS[reason])

        disk = self._disk_only_test_map() if working_tree else None
        extra = self._git_untracked_code_paths() if working_tree else None

        if not self._shard_config:
            with self._process_lock.shared():
                with self.lock.read_lock():
                    empty = self._check_analysis_data()
                    if empty is not None:
                        return empty
                    gaps = list(self.impact.get_test_gaps(
                        file_path, directory, exclude_tests,
                        disk_test_files=disk,
                        extra_code_paths=extra,
                        limit=limit,
                    ))
                    if working_tree:
                        gaps.extend(self._working_tree_gaps(file_path, directory, exclude_tests))
                        gaps.sort(key=lambda g: (not g.get("_working_tree"), -g.get("churn_score", 0)))
                    return gaps

        # Shard-aware aggregation
        shards = self._shards_for_directory(directory) if directory else list(self._shard_engines.keys())
        if file_path:
            shards = [self._shard_for_path(file_path)]
        all_gaps = []
        for shard in shards:
            with self._with_shard(shard):
                with self._process_lock.shared():
                    with self.lock.read_lock():
                        if self._check_analysis_data() is not None:
                            continue
                        partial = list(self.impact.get_test_gaps(
                            file_path, directory, exclude_tests,
                            disk_test_files=disk,
                            extra_code_paths=extra,
                            limit=limit,
                        ))
                        all_gaps.extend(partial)
        if working_tree:
            all_gaps.extend(self._working_tree_gaps(file_path, directory, exclude_tests))
            all_gaps.sort(key=lambda g: (not g.get("_working_tree"), -g.get("churn_score", 0)))
        return all_gaps

    def tool_record_result(self, test_id, passed, duration_ms=None):
        """MCP tool: record a test result (pass/fail) for future prioritization.

        Also attempts heuristic edge creation when no edges exist for
        the test file — matches the test filename to a source file and
        links all code units in the match.
        """
        test_file = test_id.split(":")[0] if ":" in test_id else test_id
        shard = self._shard_for_path(test_file)
        with self._with_shard(shard):
            with self._process_lock.exclusive():
                with self.lock.write_lock():
                    self.storage.record_test_result(test_id, passed, duration_ms)
                    edges_created = self._create_heuristic_edges(test_id)
                    result = {"test_id": test_id, "passed": passed, "recorded": True}
                    if edges_created:
                        result["heuristic_edges_created"] = edges_created
                    return result

    def tool_triage(self, directory=None, top_n=10, exclude_tests=True,
                    working_tree=False, exclude_new_file_boost=False,
                    auto_update=False):
        """MCP tool: combined risk_map + test_gaps + stale_tests triage."""
        auto_update_performed = False
        auto_update_reason = None
        if auto_update:
            auto_update_performed, auto_update_reason = self._try_auto_update()

        extra = self._git_untracked_code_paths() if working_tree else None
        disk = self._disk_only_test_map() if working_tree else None

        if not self._shard_config:
            with self._process_lock.shared():
                with self.lock.read_lock():
                    empty = self._check_analysis_data()
                    if empty is not None:
                        return empty
                    risk_map = self.impact.get_risk_map(
                        directory, exclude_tests,
                        extra_files=sorted(extra) if extra else None,
                        exclude_new_file_boost=exclude_new_file_boost,
                    )
                    risk_map = risk_map[:top_n]
                    test_gaps = self.impact.get_test_gaps(
                        directory=directory,
                        disk_test_files=disk,
                        extra_code_paths=extra,
                    )
                    if working_tree:
                        test_gaps.extend(self._working_tree_gaps(directory=directory))
                        test_gaps.sort(key=lambda g: (not g.get("_working_tree"), -g.get("churn_score", 0)))
                    stale = self.impact.detect_stale_tests()
                    stats = self.storage.get_stats()

                    top_files = {r["file_path"] for r in risk_map}
                    relevant_gaps = [g for g in test_gaps if g["file_path"] in top_files]

                    commit_count = stats.get("commits", 0)
                    summary = {
                        "files_triaged": len(risk_map),
                        "total_test_gaps": len(relevant_gaps),
                        "total_stale_tests": len(stale),
                        "test_edge_count": stats.get("test_edges", 0),
                        "test_result_count": stats.get("test_results", 0),
                        "coupling_threshold": _coupling_threshold(commit_count),
                    }
                    if auto_update:
                        summary["auto_update_performed"] = auto_update_performed
                        if not auto_update_performed:
                            summary["auto_update_skip_reason"] = auto_update_reason
                    return {
                        "top_risk_files": risk_map,
                        "test_gaps": relevant_gaps,
                        "stale_tests": stale,
                        "summary": summary,
                    }

        # Shard-aware aggregation
        shards = self._shards_for_directory(directory) if directory else list(self._shard_engines.keys())
        all_risk = []
        all_gaps = []
        all_stale = []
        total_stats = {}
        for shard in shards:
            with self._with_shard(shard):
                with self._process_lock.shared():
                    with self.lock.read_lock():
                        if self._check_analysis_data() is not None:
                            continue
                        partial_risk = self.impact.get_risk_map(
                            directory, exclude_tests,
                            extra_files=sorted(extra) if extra else None,
                            exclude_new_file_boost=exclude_new_file_boost,
                        )
                        all_risk.extend(partial_risk)
                        partial_gaps = list(self.impact.get_test_gaps(
                            directory=directory,
                            disk_test_files=disk,
                            extra_code_paths=extra,
                        ))
                        all_gaps.extend(partial_gaps)
                        all_stale.extend(self.impact.detect_stale_tests())
                        stats = self.storage.get_stats()
                        for k, v in stats.items():
                            if isinstance(v, int):
                                total_stats[k] = total_stats.get(k, 0) + v

        all_risk.sort(key=lambda x: x["risk_score"], reverse=True)
        all_risk = all_risk[:top_n]
        if working_tree:
            all_gaps.extend(self._working_tree_gaps(directory=directory))
            all_gaps.sort(key=lambda g: (not g.get("_working_tree"), -g.get("churn_score", 0)))
        top_files = {r["file_path"] for r in all_risk}
        relevant_gaps = [g for g in all_gaps if g["file_path"] in top_files]
        commit_count = total_stats.get("commits", 0)
        summary = {
            "files_triaged": len(all_risk),
            "total_test_gaps": len(relevant_gaps),
            "total_stale_tests": len(all_stale),
            "test_edge_count": total_stats.get("test_edges", 0),
            "test_result_count": total_stats.get("test_results", 0),
            "coupling_threshold": _coupling_threshold(commit_count),
        }
        if auto_update:
            summary["auto_update_performed"] = auto_update_performed
            if not auto_update_performed:
                summary["auto_update_skip_reason"] = auto_update_reason
        return {
            "top_risk_files": all_risk,
            "test_gaps": relevant_gaps,
            "stale_tests": all_stale,
            "summary": summary,
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
        if not self._shard_config:
            return self._stats_for_storage()

        # Aggregate stats across all shards
        merged = {}
        total_edge_counts = {}
        any_data = False
        for shard in self._shard_engines:
            with self._with_shard(shard):
                with self._process_lock.shared():
                    with self.lock.read_lock():
                        shard_stats = self.storage.get_stats()
                        if any(v != 0 for v in shard_stats.values()):
                            any_data = True
                        for key, value in shard_stats.items():
                            if isinstance(value, int):
                                merged[key] = merged.get(key, 0) + value
                        # Aggregate edge counts
                        edge_counts = self.storage.get_edge_type_counts()
                        for k, v in edge_counts.items():
                            total_edge_counts[k] = total_edge_counts.get(k, 0) + v

        if not any_data:
            merged["hint"] = "All counts are zero. Run 'chisel analyze' to populate."
            return merged

        commit_count = merged.get("commits", 0)
        if commit_count > 0:
            merged["coupling_threshold"] = _coupling_threshold(commit_count)

        # Use default storage's query min / branch coupling (they're global git facts)
        with self._with_shard(None):
            qm = self.storage.get_co_change_query_min()
            merged["co_change_query_min"] = qm
            bc = self.storage.get_meta("branch_coupling_commits")
            if bc is not None:
                try:
                    merged["branch_coupling_commits"] = int(bc)
                except ValueError:
                    pass

        if total_edge_counts:
            merged["shadow_graph"] = {
                "total_edges": sum(total_edge_counts.values()),
                "call_edges": total_edge_counts.get("call", 0),
                "import_edges": total_edge_counts.get("import", 0),
                "dynamic_import_edges": (
                    total_edge_counts.get("dynamic_import", 0)
                    + total_edge_counts.get("eval_import", 0)
                ),
                "eval_import_edges": total_edge_counts.get("eval_import", 0),
                "tainted_import_edges": total_edge_counts.get("tainted_import", 0),
                "unknown_shadow_ratio": round(
                    (
                        total_edge_counts.get("dynamic_import", 0)
                        + total_edge_counts.get("eval_import", 0)
                    )
                    / max(sum(total_edge_counts.values()), 1),
                    4,
                ),
            }
        return merged

    def _stats_for_storage(self):
        """Internal stats helper for the currently active storage."""
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
                    fp = self.storage.get_meta("project_fingerprint")
                    if fp and fp != self.project_dir:
                        stats["warning"] = (
                            f"Database fingerprint mismatch: stored '{fp}' "
                            f"vs current '{self.project_dir}'. "
                            f"Use --storage-dir to isolate projects."
                        )
                return stats

    def tool_optimize_storage(self):
        """MCP tool: run PRAGMA optimize and VACUUM if the WAL is large."""
        with self._process_lock.exclusive():
            with self.lock.write_lock():
                return self.storage.optimize()

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

        Uses mtime/size as a fast-path to avoid re-hashing unchanged files.
        Returns list of (abs_path, rel_path, new_hash) tuples.
        """
        rels = [normalize_path(fpath, self.project_dir) for fpath in code_files]
        hash_batch = self.storage.get_file_hashes_batch(rels)
        changed = []
        hash_updates = []
        for fpath, rel in zip(code_files, rels):
            try:
                st = os.stat(fpath)
            except OSError:
                continue
            old = hash_batch.get(rel)
            if (
                not force
                and old is not None
                and old.get("mtime") == st.st_mtime
                and old.get("size") == st.st_size
            ):
                continue
            new_hash = compute_file_hash(fpath)
            hash_updates.append((rel, new_hash, st.st_mtime, st.st_size))
            if force or (old is None or old.get("hash") != new_hash):
                changed.append((fpath, rel, new_hash))
        if hash_updates:
            self.storage.set_file_hashes_batch(hash_updates)
        return changed

    def _parse_and_store_code_units(self, changed_files):
        """Re-extract and upsert code units for changed files.

        Returns the total number of code units found.
        """
        count = 0
        for fpath, rel, new_hash in changed_files:
            self.storage.delete_code_units_by_file(rel)
            try:
                with open(fpath, encoding="utf-8", errors="replace") as f:
                    content = f.read()
            except OSError:
                continue
            units = extract_code_units(fpath, content)
            batch = []
            for u in units:
                cid = f"{rel}:{u.name}:{u.unit_type}"
                batch.append(
                    (cid, rel, u.name, u.unit_type, u.line_start, u.line_end, new_hash)
                )
            if batch:
                self.storage.upsert_code_units_batch(batch)
            count += len(units)
        return count

    def _store_blame(self, changed_files):
        """Parse and store git blame data for changed files.

        Uses a thread pool to parallelize blame subprocess calls, then
        bulk-inserts the results.
        """
        from concurrent.futures import ThreadPoolExecutor

        def _blame_for_file(rel):
            try:
                return self.git.parse_blame(rel)
            except RuntimeError:
                return []

        # Collect blame data in parallel (subprocess-bound, not DB-bound)
        rels = [rel for _, rel, _ in changed_files]
        rel_to_new_hash = {rel: new_hash for _, rel, new_hash in changed_files}
        results = {}
        with ThreadPoolExecutor(max_workers=8) as pool:
            for rel, blocks in zip(rels, pool.map(_blame_for_file, rels)):
                results[rel] = blocks

        # All DB writes happen in the main thread
        all_rows = []
        for rel in rels:
            self.storage.invalidate_blame(rel)
            new_hash = rel_to_new_hash[rel]
            for block in results.get(rel, []):
                all_rows.append((
                    rel, block["line_start"], block["line_end"],
                    block["commit_hash"], block["author"],
                    block["author_email"], block["date"], new_hash,
                ))
        if all_rows:
            self.storage.store_blame_batch(all_rows)

    def _store_commits(self, commits):
        """Upsert commits and their file entries into storage."""
        commit_rows = []
        file_rows = []
        for commit in commits:
            commit_rows.append((
                commit["hash"], commit.get("author"), commit.get("author_email"),
                commit["date"], commit.get("message"),
            ))
            for f in commit.get("files", []):
                file_rows.append((
                    commit["hash"], f["path"], f.get("insertions", 0), f.get("deletions", 0),
                ))
        if commit_rows:
            self.storage.upsert_commits_batch(commit_rows)
        if file_rows:
            self.storage.upsert_commit_files_batch(file_rows)

    # Unit-level churn is computed in parallel using a thread pool to avoid
    # serial subprocess overhead. Limit workers to keep system load reasonable.
    _UNIT_CHURN_WORKERS = 8

    def _compute_churn_and_coupling(self, commits, code_files):
        """Compute file-level and unit-level churn stats, plus co-change coupling."""
        from concurrent.futures import ThreadPoolExecutor

        # Batch file-level churn
        file_churn_rows = []
        for fpath in code_files:
            rel = normalize_path(fpath, self.project_dir)
            churn = compute_churn(commits, rel)
            file_churn_rows.append((
                rel, "", churn["commit_count"], churn["distinct_authors"],
                churn["total_insertions"], churn["total_deletions"],
                churn["last_changed"], churn["churn_score"],
            ))
        if file_churn_rows:
            self.storage.upsert_churn_stats_batch(file_churn_rows)

        # Unit-level churn via git log -L, parallelized with a thread pool
        unit_tasks = []
        for fpath in code_files:
            rel = normalize_path(fpath, self.project_dir)
            for cu in self.storage.get_code_units_by_file(rel):
                if cu["unit_type"] in ("function", "async_function"):
                    bare_name = cu["name"].rsplit(".", 1)[-1]
                    unit_tasks.append((rel, cu["name"], bare_name))

        def _churn_for_unit(task):
            rel, unit_name, bare_name = task
            try:
                func_commits = self.git.get_function_log(rel, bare_name)
                if func_commits:
                    fc = compute_churn(func_commits, rel, unit_name=unit_name)
                    return (
                        rel, unit_name, fc["commit_count"], fc["distinct_authors"],
                        fc["total_insertions"], fc["total_deletions"],
                        fc["last_changed"], fc["churn_score"],
                    )
            except RuntimeError:
                pass
            return None

        unit_churn_rows = []
        if unit_tasks:
            with ThreadPoolExecutor(max_workers=self._UNIT_CHURN_WORKERS) as pool:
                for row in pool.map(_churn_for_unit, unit_tasks):
                    if row is not None:
                        unit_churn_rows.append(row)
        if unit_churn_rows:
            self.storage.upsert_churn_stats_batch(unit_churn_rows)

        distinct_authors = len({c["author"] for c in commits if c.get("author")})
        adaptive_min = _coupling_threshold(len(commits))
        # Single-author projects produce fewer, larger commits — lower the
        # threshold so a solo developer's own commit patterns still surface
        # coupling signal instead of returning 0.0 for all file pairs.
        if distinct_authors <= 1:
            adaptive_min = max(1, adaptive_min // 2)
        self.storage.set_meta("co_change_query_min", str(adaptive_min))
        self.storage.set_meta("distinct_authors", str(distinct_authors))
        co_changes = compute_co_changes(commits, min_count=adaptive_min)
        if co_changes:
            self.storage.upsert_co_changes_batch([
                (cc["file_a"], cc["file_b"], cc["co_commit_count"], cc["last_co_commit"])
                for cc in co_changes
            ])

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
                    if br_cc:
                        self.storage.upsert_branch_co_changes_batch([
                            (cc["file_a"], cc["file_b"], cc["co_commit_count"], cc["last_co_commit"])
                            for cc in br_cc
                        ])
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
        test_unit_batch = []
        static_import_rows = []
        for tf in test_files:
            rel_tf = normalize_path(tf, self.project_dir)
            # Remove stale test units/edges before reinserting
            old_tests = self.storage.get_test_units_by_file(rel_tf)
            for ot in old_tests:
                self.storage.delete_test_edges_by_test(ot["id"])
            self.storage.delete_test_units_by_file(rel_tf)
            parsed = self.mapper.parse_test_file(tf)
            for tu in parsed:
                test_unit_batch.append((
                    tu["id"], tu["file_path"], tu["name"], tu["framework"],
                    tu["line_start"], tu["line_end"], tu["content_hash"],
                ))
                all_test_units.append(tu)
            # Persist static imports for working-tree fast-path
            try:
                with open(tf, encoding="utf-8", errors="replace") as fh:
                    content = fh.read()
                deps = self.mapper.extract_test_dependencies(rel_tf, content)
                all_paths = set(self.storage.get_resolvable_code_file_paths())
                py_imp = rel_tf.endswith(".py")
                from chisel.import_graph import _resolve_import_targets
                for dep in deps:
                    if dep.get("dep_type") != "import":
                        continue
                    mp = dep.get("module_path")
                    if py_imp:
                        gap_eligible = False
                    elif rel_tf.endswith(".go") and mp:
                        gap_eligible = True
                    else:
                        gap_eligible = bool(mp) and mp.startswith(".")
                    for tgt in _resolve_import_targets(rel_tf, dep, mp, all_paths):
                        for tu in parsed:
                            static_import_rows.append((
                                rel_tf, tu["name"], tgt.replace("\\", "/"),
                                1 if gap_eligible else 0,
                                1.0,
                            ))
            except OSError:
                pass
        if test_unit_batch:
            self.storage.upsert_test_units_batch(test_unit_batch)
        self.storage.clear_static_test_imports()
        if static_import_rows:
            self.storage.upsert_static_test_imports_batch(static_import_rows)

        all_cu_dicts = []
        for fpath in code_files:
            rel = normalize_path(fpath, self.project_dir)
            all_cu_dicts.extend(self.storage.get_code_units_by_file(rel))

        edges = self.mapper.build_test_edges(all_test_units, all_cu_dicts)
        if edges:
            self.storage.upsert_test_edges_batch([
                (e["test_id"], e["code_id"], e["edge_type"], e["weight"])
                for e in edges
            ])
        return all_test_units, len(test_files), len(edges)

    def _rebuild_import_edges(self, all_code_files, changed_files):
        """Rebuild static import_edges incrementally for changed files.

        Deletes existing edges whose importer_file is in *changed_files*,
        then re-scans only those files while resolving imports against the
        full set of *all_code_files*.
        """
        changed_rels = {rel for _fpath, rel, _hsh in changed_files}
        if changed_rels:
            self.storage.delete_import_edges_for_files(changed_rels)
        test_paths = set(self.storage.get_test_file_paths())
        all_rels = [normalize_path(f, self.project_dir) for f in all_code_files]
        edges = build_import_edges(
            self.mapper, self.project_dir, all_rels, test_paths, changed_rels,
        )
        if edges:
            self.storage.upsert_import_edges_batch([
                (e["importer_file"], e["imported_file"]) for e in edges
            ])

    # ------------------------------------------------------------------ #
    # Lifecycle
    # ------------------------------------------------------------------ #

    def __enter__(self):
        return self

    def __exit__(self, *exc):
        self.close()

    def close(self):
        """Close the underlying storage connection and all shard storages."""
        closed = set()
        for storage, *_ in self._shard_engines.values():
            if id(storage) not in closed:
                storage.close()
                closed.add(id(storage))
        # Ensure default storage is closed even if not in _shard_engines
        if id(self.storage) not in closed:
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
                        tu["id"], cu["id"], "heuristic", 0.2,
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

    def _working_tree_suggest(self, file_path, disk_test_files=None):
        """Suggest tests for an untracked file using stem-matching.

        Reads the file from disk, extracts code units, and matches
        against known test files in storage by stem similarity.
        Optionally merges disk-only (untracked) test files.
        Returns an empty list if no matching tests are found.
        """
        import os
        source_stem = os.path.splitext(os.path.basename(file_path))[0]
        source_dir = os.path.dirname(file_path)
        all_test_files = dict(self.storage.get_all_test_files())
        if disk_test_files:
            for rel, names in disk_test_files.items():
                if rel not in all_test_files:
                    all_test_files[rel] = names
                else:
                    merged = list(dict.fromkeys(all_test_files[rel] + names))
                    all_test_files[rel] = merged
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

            # Directory affinity boost: prefer tests in a directory whose
            # last component matches the source file's directory.
            test_dir = os.path.dirname(test_file)
            if source_dir and test_dir:
                source_last = os.path.basename(source_dir)
                test_last = os.path.basename(test_dir)
                if source_last == test_last:
                    score = min(1.0, score + 0.3)

            for name in test_names:
                src = "working_tree" if (disk_test_files and test_file in disk_test_files) else "working_tree"
                scored.append({
                    "test_id": f"{test_file}:{name}",
                    "file_path": test_file,
                    "name": name,
                    "relevance": score,
                    "reason": "working-tree: stem-matched test (file not yet committed)",
                    "source": src,
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

    def _set_bg_progress(self, pct):
        """Update in-memory progress percentage for the current background job."""
        with self._bg_jobs_lock:
            active = getattr(self, "_active_bg_job_id", None)
            if active:
                self._bg_job_progress[active] = pct
        if active:
            self.storage.update_bg_job(
                active, status="running", progress_pct=pct,
            )

    def _record_job_event(self, event_type, payload=None):
        """Write a progress/lifecycle event for the active background job."""
        active = getattr(self, "_active_bg_job_id", None)
        if not active:
            return
        with self._bg_jobs_lock:
            self._job_event_buffer.setdefault(active, []).append({
                "event_type": event_type,
                "payload": payload,
                "created_at": self.storage._now(),
            })

    def _check_job_cancelled(self):
        """Raise JobCancelledError if the active background job has a cancel request."""
        active = getattr(self, "_active_bg_job_id", None)
        if active and self.storage.is_bg_job_cancel_requested(active):
            self.storage.update_bg_job(
                active, status="failed", error_message="Cancelled by user",
            )
            raise JobCancelledError(f"Background job {active} was cancelled")

    def _check_project_fingerprint(self):
        """Warn if the stored project fingerprint does not match the current directory."""
        stored = self.storage.get_meta("project_fingerprint")
        if stored and stored != self.project_dir:
            import logging
            logging.getLogger(__name__).warning(
                "Chisel DB fingerprint mismatch: stored '%s' vs current '%s'",
                stored, self.project_dir,
            )

    def _backfill_heuristic_edges(self):
        """Create heuristic test edges for tests that have zero DB edges.

        Runs after analyze/update so filename-based matches are established
        even for test files the static scanner missed.
        """
        all_tests = self.storage.get_all_test_units()
        for tu in all_tests:
            if not self.storage.get_edges_for_test(tu["id"]):
                self._create_heuristic_edges(tu["id"])

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
