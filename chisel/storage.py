"""SQLite persistence layer for Chisel's data model."""

import json
import logging
import os
import sqlite3
import threading
import time
from datetime import datetime, timezone
from pathlib import Path
from statistics import fmean, pstdev

logger = logging.getLogger(__name__)

# Retry config for cross-process SQLITE_BUSY errors.
_BUSY_RETRIES = 5
_BUSY_BACKOFF = 0.1  # seconds, doubled each retry


class Storage:
    """Manages all Chisel data in a SQLite database with WAL mode."""

    def __init__(self, base_dir=None):
        if base_dir is None:
            base_dir = Path.home() / ".chisel"
        self.base_dir = Path(base_dir)
        self.base_dir.mkdir(parents=True, exist_ok=True)
        self.db_path = self.base_dir / "chisel.db"
        self._lock = threading.Lock()
        self._local = threading.local()
        self._init_database()

    def _create_connection(self):
        """Create and configure the SQLite connection (called once).

        Uses a 30-second busy timeout so concurrent processes wait rather
        than immediately failing with SQLITE_BUSY.
        """
        conn = sqlite3.connect(str(self.db_path), check_same_thread=False)
        conn.row_factory = sqlite3.Row
        conn.execute("PRAGMA journal_mode=WAL")
        conn.execute("PRAGMA synchronous=NORMAL")
        conn.execute("PRAGMA busy_timeout=30000")
        # FK enforcement disabled — Chisel manages integrity at application level
        # and stale test detection relies on orphaned edge references.
        return conn

    def __enter__(self):
        return self

    def __exit__(self, *exc):
        self.close()

    def close(self):
        """Close the persistent database connection."""
        if hasattr(self._local, "conn") and self._local.conn is not None:
            self._local.conn.close()
            self._local.conn = None

    @property
    def _conn(self):
        """Return the SQLite connection for the current thread."""
        conn = getattr(self._local, "conn", None)
        if conn is None:
            conn = self._create_connection()
            self._local.conn = conn
        return conn

    def _ensure_main_conn(self):
        """Deprecated — kept for backward compat; returns current thread conn."""
        return self._conn

    def _init_database(self):
        # executescript auto-commits; no need for a context manager.
        self._conn.executescript("""
                CREATE TABLE IF NOT EXISTS code_units (
                    id TEXT PRIMARY KEY,
                    file_path TEXT NOT NULL,
                    name TEXT NOT NULL,
                    unit_type TEXT NOT NULL,
                    line_start INTEGER,
                    line_end INTEGER,
                    content_hash TEXT,
                    updated_at TEXT
                );

                CREATE TABLE IF NOT EXISTS test_units (
                    id TEXT PRIMARY KEY,
                    file_path TEXT NOT NULL,
                    name TEXT NOT NULL,
                    framework TEXT,
                    line_start INTEGER,
                    line_end INTEGER,
                    content_hash TEXT,
                    updated_at TEXT
                );

                CREATE TABLE IF NOT EXISTS test_edges (
                    test_id TEXT REFERENCES test_units(id),
                    code_id TEXT REFERENCES code_units(id),
                    edge_type TEXT,
                    weight REAL DEFAULT 1.0,
                    PRIMARY KEY (test_id, code_id, edge_type)
                );

                CREATE TABLE IF NOT EXISTS commits (
                    hash TEXT PRIMARY KEY,
                    author TEXT,
                    author_email TEXT,
                    date TEXT,
                    message TEXT
                );

                CREATE TABLE IF NOT EXISTS commit_files (
                    commit_hash TEXT REFERENCES commits(hash),
                    file_path TEXT,
                    insertions INTEGER,
                    deletions INTEGER,
                    PRIMARY KEY (commit_hash, file_path)
                );

                CREATE TABLE IF NOT EXISTS blame_cache (
                    file_path TEXT,
                    line_start INTEGER,
                    line_end INTEGER,
                    commit_hash TEXT,
                    author TEXT,
                    author_email TEXT,
                    date TEXT,
                    content_hash TEXT,
                    PRIMARY KEY (file_path, line_start)
                );

                CREATE TABLE IF NOT EXISTS co_changes (
                    file_a TEXT,
                    file_b TEXT,
                    co_commit_count INTEGER,
                    last_co_commit TEXT,
                    PRIMARY KEY (file_a, file_b)
                );

                CREATE TABLE IF NOT EXISTS churn_stats (
                    file_path TEXT,
                    unit_name TEXT,
                    commit_count INTEGER,
                    distinct_authors INTEGER,
                    total_insertions INTEGER,
                    total_deletions INTEGER,
                    last_changed TEXT,
                    churn_score REAL,
                    PRIMARY KEY (file_path, unit_name)
                );

                CREATE TABLE IF NOT EXISTS file_hashes (
                    file_path TEXT PRIMARY KEY,
                    content_hash TEXT,
                    updated_at TEXT
                );

                CREATE INDEX IF NOT EXISTS idx_code_units_file ON code_units(file_path);
                CREATE INDEX IF NOT EXISTS idx_test_units_file ON test_units(file_path);
                CREATE INDEX IF NOT EXISTS idx_test_edges_code ON test_edges(code_id);
                CREATE INDEX IF NOT EXISTS idx_test_edges_test ON test_edges(test_id);
                CREATE INDEX IF NOT EXISTS idx_commit_files_file ON commit_files(file_path);
                CREATE INDEX IF NOT EXISTS idx_blame_cache_hash ON blame_cache(content_hash);
                CREATE INDEX IF NOT EXISTS idx_churn_stats_file ON churn_stats(file_path);
                CREATE INDEX IF NOT EXISTS idx_co_changes_file_b ON co_changes(file_b);
                CREATE INDEX IF NOT EXISTS idx_churn_stats_file_unit ON churn_stats(file_path, unit_name);
                CREATE INDEX IF NOT EXISTS idx_co_changes_file_a ON co_changes(file_a);
                CREATE INDEX IF NOT EXISTS idx_blame_cache_lookup ON blame_cache(file_path, content_hash, line_start);
                CREATE INDEX IF NOT EXISTS idx_commits_date ON commits(date);

                CREATE TABLE IF NOT EXISTS import_edges (
                    importer_file TEXT NOT NULL,
                    imported_file TEXT NOT NULL,
                    PRIMARY KEY (importer_file, imported_file)
                );
                CREATE INDEX IF NOT EXISTS idx_import_edges_imported
                    ON import_edges(imported_file);

                CREATE TABLE IF NOT EXISTS branch_co_changes (
                    file_a TEXT,
                    file_b TEXT,
                    co_commit_count INTEGER,
                    last_co_commit TEXT,
                    PRIMARY KEY (file_a, file_b)
                );
                CREATE INDEX IF NOT EXISTS idx_branch_co_changes_b ON branch_co_changes(file_b);
                CREATE INDEX IF NOT EXISTS idx_branch_co_changes_a ON branch_co_changes(file_a);

                CREATE TABLE IF NOT EXISTS test_results (
                    test_id TEXT,
                    passed INTEGER NOT NULL,
                    duration_ms INTEGER,
                    recorded_at TEXT,
                    PRIMARY KEY (test_id, recorded_at)
                );
                CREATE INDEX IF NOT EXISTS idx_test_results_test
                    ON test_results(test_id);

                CREATE TABLE IF NOT EXISTS meta (
                    key TEXT PRIMARY KEY,
                    value TEXT NOT NULL
                );

                CREATE TABLE IF NOT EXISTS file_locks (
                    file_path    TEXT PRIMARY KEY,
                    agent_id     TEXT NOT NULL,
                    acquired_at  REAL NOT NULL,
                    expires_at   REAL NOT NULL,
                    purpose      TEXT
                );
                CREATE INDEX IF NOT EXISTS idx_file_locks_agent
                    ON file_locks(agent_id);

                CREATE TABLE IF NOT EXISTS bg_jobs (
                    id TEXT PRIMARY KEY,
                    kind TEXT NOT NULL,
                    status TEXT NOT NULL,
                    result_json TEXT,
                    error_message TEXT,
                    progress_pct INTEGER,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL
                );
                CREATE INDEX IF NOT EXISTS idx_bg_jobs_status ON bg_jobs(status);
            """)
        # Schema migration: add progress_pct if upgrading an existing DB
        try:
            self._conn.execute("ALTER TABLE bg_jobs ADD COLUMN progress_pct INTEGER")
        except sqlite3.OperationalError:
            pass  # Column already exists

        # Schema migration: add cancel_requested_at and started_at to bg_jobs
        for col in ("cancel_requested_at", "started_at"):
            try:
                self._conn.execute(f"ALTER TABLE bg_jobs ADD COLUMN {col} TEXT")
            except sqlite3.OperationalError:
                pass  # Column already exists

        # Schema migration: add job_events table
        self._conn.execute("""
            CREATE TABLE IF NOT EXISTS job_events (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                job_id TEXT NOT NULL,
                event_type TEXT NOT NULL,
                payload_json TEXT,
                created_at TEXT NOT NULL
            )
        """)
        self._conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_job_events_job_id ON job_events(job_id, created_at)"
        )

        # Schema migration: add static_test_imports table and file_hashes columns
        self._conn.execute("""
            CREATE TABLE IF NOT EXISTS static_test_imports (
                test_file_path TEXT NOT NULL,
                test_unit_name TEXT NOT NULL,
                target_file_path TEXT NOT NULL,
                gap_eligible INTEGER DEFAULT 0,
                score REAL DEFAULT 1.0,
                PRIMARY KEY (test_file_path, test_unit_name, target_file_path)
            )
        """)
        self._conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_static_test_imports_target "
            "ON static_test_imports(target_file_path)"
        )
        for col_def in ("mtime REAL", "size INTEGER"):
            try:
                self._conn.execute(f"ALTER TABLE file_hashes ADD COLUMN {col_def}")
            except sqlite3.OperationalError:
                pass  # column already exists

    # --- Query helpers ---

    @staticmethod
    def _now():
        return datetime.now(timezone.utc).isoformat()

    def _fetchall(self, sql, params=()):
        """Execute a query and return all rows as dicts."""
        with self._conn as conn:
            cur = conn.execute(sql, params)
            try:
                return [dict(r) for r in cur.fetchall()]
            finally:
                cur.close()

    def _fetchone(self, sql, params=()):
        """Execute a query and return a single row as dict, or None."""
        with self._conn as conn:
            cur = conn.execute(sql, params)
            try:
                row = cur.fetchone()
                return dict(row) if row else None
            finally:
                cur.close()

    def _execute(self, sql, params=()):
        """Execute a write query within a transaction. Returns rowcount.

        Retries on SQLITE_BUSY with exponential backoff for cross-process
        safety (e.g., two agents analyzing concurrently).
        """
        backoff = _BUSY_BACKOFF
        for attempt in range(_BUSY_RETRIES):
            try:
                with self._conn as conn:
                    cur = conn.execute(sql, params)
                    rc = cur.rowcount
                    cur.close()
                    return rc
            except sqlite3.OperationalError as exc:
                if "database is locked" in str(exc) and attempt < _BUSY_RETRIES - 1:
                    logger.debug("SQLITE_BUSY (attempt %d), retrying in %.1fs", attempt + 1, backoff)
                    time.sleep(backoff)
                    backoff *= 2
                else:
                    if "database is locked" in str(exc):
                        logger.warning(
                            "SQLite write failed after %d retries (db=%s): %s",
                            _BUSY_RETRIES, self.db_path, exc,
                        )
                    raise

    def _executemany(self, sql, params_seq):
        """Execute a write query repeatedly within a single transaction.

        Uses ``executemany`` for bulk inserts/upserts. Retries on
        SQLITE_BUSY with exponential backoff.
        """
        backoff = _BUSY_BACKOFF
        for attempt in range(_BUSY_RETRIES):
            try:
                with self._conn as conn:
                    cur = conn.executemany(sql, params_seq)
                    cur.close()
                    return cur
            except sqlite3.OperationalError as exc:
                if "database is locked" in str(exc) and attempt < _BUSY_RETRIES - 1:
                    logger.debug("SQLITE_BUSY (executemany attempt %d), retrying in %.1fs", attempt + 1, backoff)
                    time.sleep(backoff)
                    backoff *= 2
                else:
                    if "database is locked" in str(exc):
                        logger.warning(
                            "SQLite executemany failed after %d retries (db=%s): %s",
                            _BUSY_RETRIES, self.db_path, exc,
                        )
                    raise

    # --- code_units ---

    def upsert_code_unit(self, id, file_path, name, unit_type,
                         line_start=None, line_end=None, content_hash=None):
        self._execute(
            """INSERT INTO code_units (id, file_path, name, unit_type,
               line_start, line_end, content_hash, updated_at)
               VALUES (?, ?, ?, ?, ?, ?, ?, ?)
               ON CONFLICT(id) DO UPDATE SET
               file_path=excluded.file_path, name=excluded.name,
               unit_type=excluded.unit_type, line_start=excluded.line_start,
               line_end=excluded.line_end, content_hash=excluded.content_hash,
               updated_at=excluded.updated_at""",
            (id, file_path, name, unit_type, line_start, line_end,
             content_hash, self._now()),
        )

    def get_code_unit(self, id):
        return self._fetchone("SELECT * FROM code_units WHERE id = ?", (id,))

    def get_code_units_by_file(self, file_path):
        return self._fetchall(
            "SELECT * FROM code_units WHERE file_path = ?", (file_path,),
        )

    def get_code_units_by_file_stem(self, stem):
        """Find code units in source files whose basename matches *stem*.

        Matches ``src/services/searchService.js`` for stem ``searchService``.
        Excludes files containing ``.test.`` or ``.spec.`` in their path.
        """
        return self._fetchall(
            """SELECT * FROM code_units
               WHERE (file_path LIKE ? OR file_path LIKE ?)
               AND file_path NOT LIKE ? AND file_path NOT LIKE ?""",
            (f"%/{stem}.%", f"{stem}.%", "%.test.%", "%.spec.%"),
        )

    def get_distinct_code_file_paths(self):
        """All project-relative paths that have at least one code unit."""
        rows = self._fetchall("SELECT DISTINCT file_path FROM code_units")
        return {r["file_path"] for r in rows}

    def get_resolvable_code_file_paths(self):
        """Paths usable as static import targets: code_units ∪ churn_stats.

        Some analyzed files have no extractable units but still appear in
        churn_stats after analyze (e.g. a tiny ES module).
        """
        rows = self._fetchall(
            """SELECT DISTINCT file_path FROM code_units
               UNION
               SELECT DISTINCT file_path FROM churn_stats""",
        )
        return {r["file_path"] for r in rows}

    def delete_code_units_by_file(self, file_path):
        self._execute("DELETE FROM code_units WHERE file_path = ?", (file_path,))

    def upsert_code_units_batch(self, rows):
        """Batch upsert code units.

        *rows* is an iterable of (id, file_path, name, unit_type,
        line_start, line_end, content_hash) tuples.
        """
        if not rows:
            return
        now = self._now()
        self._executemany(
            """INSERT INTO code_units (id, file_path, name, unit_type,
               line_start, line_end, content_hash, updated_at)
               VALUES (?, ?, ?, ?, ?, ?, ?, ?)
               ON CONFLICT(id) DO UPDATE SET
               file_path=excluded.file_path, name=excluded.name,
               unit_type=excluded.unit_type, line_start=excluded.line_start,
               line_end=excluded.line_end, content_hash=excluded.content_hash,
               updated_at=excluded.updated_at""",
            [(r[0], r[1], r[2], r[3], r[4], r[5], r[6], now) for r in rows],
        )

    # --- test_units ---

    def upsert_test_unit(self, id, file_path, name, framework=None,
                         line_start=None, line_end=None, content_hash=None):
        self._execute(
            """INSERT INTO test_units (id, file_path, name, framework,
               line_start, line_end, content_hash, updated_at)
               VALUES (?, ?, ?, ?, ?, ?, ?, ?)
               ON CONFLICT(id) DO UPDATE SET
               file_path=excluded.file_path, name=excluded.name,
               framework=excluded.framework, line_start=excluded.line_start,
               line_end=excluded.line_end, content_hash=excluded.content_hash,
               updated_at=excluded.updated_at""",
            (id, file_path, name, framework, line_start, line_end,
             content_hash, self._now()),
        )

    def get_test_unit(self, id):
        return self._fetchone("SELECT * FROM test_units WHERE id = ?", (id,))

    def get_test_units_by_file(self, file_path):
        return self._fetchall(
            "SELECT * FROM test_units WHERE file_path = ?", (file_path,),
        )

    def get_all_test_units(self):
        return self._fetchall("SELECT * FROM test_units ORDER BY file_path, name")

    def get_test_file_paths(self):
        """Return the set of file paths that contain test units."""
        rows = self._fetchall(
            "SELECT DISTINCT file_path FROM test_units",
        )
        return {r["file_path"] for r in rows}

    def get_all_test_files(self):
        """Return all test files with their unit names, ordered by file path.

        Returns a dict mapping file_path -> [test_unit_names].
        Used by suggest_tests fallback when a file has no direct test edges.
        """
        rows = self._fetchall(
            "SELECT DISTINCT file_path FROM test_units ORDER BY file_path",
        )
        if not rows:
            return {}
        result = {r["file_path"]: [] for r in rows}
        unit_rows = self._fetchall(
            "SELECT file_path, name FROM test_units ORDER BY file_path, name",
        )
        for row in unit_rows:
            if row["file_path"] in result:
                result[row["file_path"]].append(row["name"])
        return result

    # --- test_edges ---

    def upsert_test_edge(self, test_id, code_id, edge_type, weight=1.0):
        self._execute(
            """INSERT INTO test_edges (test_id, code_id, edge_type, weight)
               VALUES (?, ?, ?, ?)
               ON CONFLICT(test_id, code_id, edge_type) DO UPDATE SET
               weight=excluded.weight""",
            (test_id, code_id, edge_type, weight),
        )

    def upsert_test_edges_batch(self, rows):
        """Batch upsert test edges.

        *rows* is an iterable of (test_id, code_id, edge_type, weight) tuples.
        """
        if not rows:
            return
        self._executemany(
            """INSERT INTO test_edges (test_id, code_id, edge_type, weight)
               VALUES (?, ?, ?, ?)
               ON CONFLICT(test_id, code_id, edge_type) DO UPDATE SET
               weight=excluded.weight""",
            list(rows),
        )

    def get_edges_for_test(self, test_id):
        return self._fetchall(
            "SELECT * FROM test_edges WHERE test_id = ?", (test_id,),
        )

    def get_edges_for_code(self, code_id):
        return self._fetchall(
            "SELECT * FROM test_edges WHERE code_id = ?", (code_id,),
        )

    # --- commits ---

    def upsert_commit(self, hash, author=None, author_email=None, date=None, message=None):
        self._execute(
            """INSERT INTO commits (hash, author, author_email, date, message)
               VALUES (?, ?, ?, ?, ?)
               ON CONFLICT(hash) DO UPDATE SET
               author=excluded.author, author_email=excluded.author_email,
               date=excluded.date, message=excluded.message""",
            (hash, author, author_email, date, message),
        )

    def get_commit(self, hash):
        return self._fetchone("SELECT * FROM commits WHERE hash = ?", (hash,))

    # --- commit_files ---

    def upsert_commit_file(self, commit_hash, file_path, insertions=0, deletions=0):
        self._execute(
            """INSERT INTO commit_files (commit_hash, file_path, insertions, deletions)
               VALUES (?, ?, ?, ?)
               ON CONFLICT(commit_hash, file_path) DO UPDATE SET
               insertions=excluded.insertions, deletions=excluded.deletions""",
            (commit_hash, file_path, insertions, deletions),
        )

    def upsert_commits_batch(self, rows):
        """Batch upsert commits.

        *rows* is an iterable of (hash, author, author_email, date, message) tuples.
        """
        if not rows:
            return
        self._executemany(
            """INSERT INTO commits (hash, author, author_email, date, message)
               VALUES (?, ?, ?, ?, ?)
               ON CONFLICT(hash) DO UPDATE SET
               author=excluded.author, author_email=excluded.author_email,
               date=excluded.date, message=excluded.message""",
            list(rows),
        )

    def upsert_commit_files_batch(self, rows):
        """Batch upsert commit_files.

        *rows* is an iterable of (commit_hash, file_path, insertions, deletions) tuples.
        """
        if not rows:
            return
        self._executemany(
            """INSERT INTO commit_files (commit_hash, file_path, insertions, deletions)
               VALUES (?, ?, ?, ?)
               ON CONFLICT(commit_hash, file_path) DO UPDATE SET
               insertions=excluded.insertions, deletions=excluded.deletions""",
            list(rows),
        )

    def get_commits_for_file(self, file_path):
        return self._fetchall(
            """SELECT c.*, cf.insertions, cf.deletions FROM commits c
               JOIN commit_files cf ON c.hash = cf.commit_hash
               WHERE cf.file_path = ? ORDER BY c.date DESC""",
            (file_path,),
        )

    # --- blame_cache ---

    def store_blame(self, file_path, line_start, line_end, commit_hash,
                    author, author_email, date, content_hash):
        self._execute(
            """INSERT INTO blame_cache (file_path, line_start, line_end,
               commit_hash, author, author_email, date, content_hash)
               VALUES (?, ?, ?, ?, ?, ?, ?, ?)
               ON CONFLICT(file_path, line_start) DO UPDATE SET
               line_end=excluded.line_end, commit_hash=excluded.commit_hash,
               author=excluded.author, author_email=excluded.author_email,
               date=excluded.date, content_hash=excluded.content_hash""",
            (file_path, line_start, line_end, commit_hash,
             author, author_email, date, content_hash),
        )

    def get_blame(self, file_path, content_hash):
        return self._fetchall(
            """SELECT * FROM blame_cache
               WHERE file_path = ? AND content_hash = ?
               ORDER BY line_start""",
            (file_path, content_hash),
        )

    def invalidate_blame(self, file_path):
        self._execute("DELETE FROM blame_cache WHERE file_path = ?", (file_path,))

    def store_blame_batch(self, rows):
        """Batch upsert blame cache entries.

        *rows* is an iterable of (file_path, line_start, line_end,
        commit_hash, author, author_email, date, content_hash) tuples.
        """
        if not rows:
            return
        self._executemany(
            """INSERT INTO blame_cache (file_path, line_start, line_end,
               commit_hash, author, author_email, date, content_hash)
               VALUES (?, ?, ?, ?, ?, ?, ?, ?)
               ON CONFLICT(file_path, line_start) DO UPDATE SET
               line_end=excluded.line_end, commit_hash=excluded.commit_hash,
               author=excluded.author, author_email=excluded.author_email,
               date=excluded.date, content_hash=excluded.content_hash""",
            list(rows),
        )

    # --- co_changes ---

    def upsert_co_change(self, file_a, file_b, co_commit_count, last_co_commit=None):
        a, b = sorted([file_a, file_b])
        self._execute(
            """INSERT INTO co_changes (file_a, file_b, co_commit_count, last_co_commit)
               VALUES (?, ?, ?, ?)
               ON CONFLICT(file_a, file_b) DO UPDATE SET
               co_commit_count=excluded.co_commit_count,
               last_co_commit=excluded.last_co_commit""",
            (a, b, co_commit_count, last_co_commit),
        )

    def upsert_co_changes_batch(self, rows):
        """Batch upsert co-changes.

        *rows* is an iterable of (file_a, file_b, co_commit_count, last_co_commit) tuples.
        file_a and file_b are sorted before insertion.
        """
        if not rows:
            return
        sorted_rows = []
        for a, b, cc, lc in rows:
            a, b = sorted([a, b])
            sorted_rows.append((a, b, cc, lc))
        self._executemany(
            """INSERT INTO co_changes (file_a, file_b, co_commit_count, last_co_commit)
               VALUES (?, ?, ?, ?)
               ON CONFLICT(file_a, file_b) DO UPDATE SET
               co_commit_count=excluded.co_commit_count,
               last_co_commit=excluded.last_co_commit""",
            sorted_rows,
        )

    def get_co_changes(self, file_path, min_count=3):
        return self._fetchall(
            """SELECT * FROM co_changes
               WHERE (file_a = ? OR file_b = ?) AND co_commit_count >= ?
               ORDER BY co_commit_count DESC""",
            (file_path, file_path, min_count),
        )

    # --- import_edges ---

    def clear_import_edges(self):
        """Remove all static import edges (full rebuild before reinsert)."""
        self._execute("DELETE FROM import_edges")

    def delete_import_edges_for_files(self, file_paths):
        """Delete all import edges where *importer_file* is in *file_paths*."""
        if not file_paths:
            return
        for chunk in self._chunked(list(file_paths), size=450):
            placeholders = ",".join("?" for _ in chunk)
            self._execute(
                f"DELETE FROM import_edges WHERE importer_file IN ({placeholders})",
                tuple(chunk),
            )

    def upsert_import_edge(self, importer_file, imported_file):
        self._execute(
            """INSERT OR IGNORE INTO import_edges (importer_file, imported_file)
               VALUES (?, ?)""",
            (importer_file, imported_file),
        )

    def upsert_import_edges_batch(self, rows):
        """Batch insert import edges (ignore duplicates).

        *rows* is an iterable of (importer_file, imported_file) tuples.
        """
        if not rows:
            return
        self._executemany(
            """INSERT OR IGNORE INTO import_edges (importer_file, imported_file)
               VALUES (?, ?)""",
            list(rows),
        )

    def get_import_neighbors_batch(self, file_paths):
        """Return distinct neighbor files (either direction) per path in *file_paths*.

        Neighbors exclude the file itself.  Used for structural coupling.
        """
        if not file_paths:
            return {}
        result = {fp: set() for fp in file_paths}
        for chunk in self._chunked_by_params(list(file_paths), params_per_item=2):
            placeholders = ",".join("?" for _ in chunk)
            sql = f"""SELECT importer_file, imported_file FROM import_edges
                      WHERE importer_file IN ({placeholders})
                         OR imported_file IN ({placeholders})"""
            rows = self._fetchall(sql, tuple(chunk) + tuple(chunk))
            for row in rows:
                a, b = row["importer_file"], row["imported_file"]
                if a in result and b != a:
                    result[a].add(b)
                if b in result and a != b:
                    result[b].add(a)
        return {fp: sorted(neighbors) for fp, neighbors in result.items()}

    def get_importers(self, imported_file):
        """Return files that statically import *imported_file* (reverse edges)."""
        rows = self._fetchall(
            "SELECT DISTINCT importer_file FROM import_edges WHERE imported_file = ?",
            (imported_file,),
        )
        return [r["importer_file"] for r in rows]

    def get_imported_files(self, importer_file):
        """Return files that *importer_file* imports (forward edges)."""
        rows = self._fetchall(
            "SELECT DISTINCT imported_file FROM import_edges WHERE importer_file = ?",
            (importer_file,),
        )
        return [r["imported_file"] for r in rows]

    # --- branch_co_changes (merge-base..HEAD only, rebuilt each analyze) ---

    def clear_branch_co_changes(self):
        self._execute("DELETE FROM branch_co_changes")

    def upsert_branch_co_change(self, file_a, file_b, co_commit_count, last_co_commit=None):
        a, b = sorted([file_a, file_b])
        self._execute(
            """INSERT INTO branch_co_changes (file_a, file_b, co_commit_count, last_co_commit)
               VALUES (?, ?, ?, ?)
               ON CONFLICT(file_a, file_b) DO UPDATE SET
               co_commit_count=excluded.co_commit_count,
               last_co_commit=excluded.last_co_commit""",
            (a, b, co_commit_count, last_co_commit),
        )

    def upsert_branch_co_changes_batch(self, rows):
        """Batch upsert branch co-changes.

        *rows* is an iterable of (file_a, file_b, co_commit_count, last_co_commit) tuples.
        file_a and file_b are sorted before insertion.
        """
        if not rows:
            return
        sorted_rows = []
        for a, b, cc, lc in rows:
            a, b = sorted([a, b])
            sorted_rows.append((a, b, cc, lc))
        self._executemany(
            """INSERT INTO branch_co_changes (file_a, file_b, co_commit_count, last_co_commit)
               VALUES (?, ?, ?, ?)
               ON CONFLICT(file_a, file_b) DO UPDATE SET
               co_commit_count=excluded.co_commit_count,
               last_co_commit=excluded.last_co_commit""",
            sorted_rows,
        )

    def get_branch_co_changes(self, file_path, min_count=1):
        return self._fetchall(
            """SELECT * FROM branch_co_changes
               WHERE (file_a = ? OR file_b = ?) AND co_commit_count >= ?
               ORDER BY co_commit_count DESC""",
            (file_path, file_path, min_count),
        )

    def get_branch_co_changes_batch(self, file_paths, min_count=1):
        if not file_paths:
            return {}
        result = {fp: [] for fp in file_paths}
        for chunk in self._chunked_by_params(list(file_paths), params_per_item=2):
            placeholders = ",".join("?" for _ in chunk)
            rows = self._fetchall(
                f"""SELECT * FROM branch_co_changes
                    WHERE (file_a IN ({placeholders}) OR file_b IN ({placeholders}))
                    AND co_commit_count >= ?
                    ORDER BY co_commit_count DESC""",
                (*chunk, *chunk, min_count),
            )
            for row in rows:
                if row["file_a"] in result:
                    result[row["file_a"]].append(row)
                if row["file_b"] in result and row["file_b"] != row["file_a"]:
                    result[row["file_b"]].append(row)
        return result

    # --- churn_stats ---

    @staticmethod
    def _normalize_unit_name(unit_name):
        return unit_name or ""

    def upsert_churn_stat(self, file_path, unit_name, commit_count=0,
                          distinct_authors=0, total_insertions=0, total_deletions=0,
                          last_changed=None, churn_score=0.0):
        unit_name = self._normalize_unit_name(unit_name)
        self._execute(
            """INSERT INTO churn_stats (file_path, unit_name, commit_count,
               distinct_authors, total_insertions, total_deletions,
               last_changed, churn_score)
               VALUES (?, ?, ?, ?, ?, ?, ?, ?)
               ON CONFLICT(file_path, unit_name) DO UPDATE SET
               commit_count=excluded.commit_count,
               distinct_authors=excluded.distinct_authors,
               total_insertions=excluded.total_insertions,
               total_deletions=excluded.total_deletions,
               last_changed=excluded.last_changed,
               churn_score=excluded.churn_score""",
            (file_path, unit_name, commit_count, distinct_authors,
             total_insertions, total_deletions, last_changed, churn_score),
        )

    def upsert_churn_stats_batch(self, rows):
        """Batch upsert churn stats.

        *rows* is an iterable of (file_path, unit_name, commit_count,
        distinct_authors, total_insertions, total_deletions, last_changed, churn_score) tuples.
        """
        if not rows:
            return
        normalized = []
        for r in rows:
            unit_name = self._normalize_unit_name(r[1])
            normalized.append((r[0], unit_name, r[2], r[3], r[4], r[5], r[6], r[7]))
        self._executemany(
            """INSERT INTO churn_stats (file_path, unit_name, commit_count,
               distinct_authors, total_insertions, total_deletions,
               last_changed, churn_score)
               VALUES (?, ?, ?, ?, ?, ?, ?, ?)
               ON CONFLICT(file_path, unit_name) DO UPDATE SET
               commit_count=excluded.commit_count,
               distinct_authors=excluded.distinct_authors,
               total_insertions=excluded.total_insertions,
               total_deletions=excluded.total_deletions,
               last_changed=excluded.last_changed,
               churn_score=excluded.churn_score""",
            normalized,
        )

    def get_churn_stat(self, file_path, unit_name=None):
        unit_name = self._normalize_unit_name(unit_name)
        return self._fetchone(
            "SELECT * FROM churn_stats WHERE file_path = ? AND unit_name = ?",
            (file_path, unit_name),
        )

    def get_all_churn_stats(self, file_path=None):
        sql = "SELECT * FROM churn_stats"
        params = ()
        if file_path:
            sql += " WHERE file_path = ?"
            params = (file_path,)
        sql += " ORDER BY churn_score DESC"
        return self._fetchall(sql, params)

    def get_stale_test_edges(self):
        """Find test edges that point to code units that no longer exist."""
        return self._fetchall(
            """SELECT te.test_id, tu.name AS test_name,
                      te.code_id, te.edge_type
               FROM test_edges te
               JOIN test_units tu ON te.test_id = tu.id
               LEFT JOIN code_units cu ON te.code_id = cu.id
               WHERE cu.id IS NULL""",
        )

    def get_edge_type_counts(self):
        """Count test edges grouped by edge_type.

        Returns:
            Dict mapping edge_type string to count.
            Keys include: call, import, dynamic_import, eval_import, tainted_import.
        """
        rows = self._fetchall(
            """SELECT edge_type, COUNT(*) AS cnt
               FROM test_edges
               GROUP BY edge_type""",
        )
        return {r["edge_type"]: r["cnt"] for r in rows}

    def get_direct_impacted_tests(self, file_path, changed_functions=None):
        """Find tests with edges to code units in a file, via a single JOIN."""
        base_sql = """SELECT tu.id AS test_id, tu.file_path,
                             tu.name, cu.name AS code_name,
                             te.edge_type, te.weight
                      FROM code_units cu
                      JOIN test_edges te ON cu.id = te.code_id
                      JOIN test_units tu ON te.test_id = tu.id
                      WHERE cu.file_path = ?"""
        if changed_functions is not None:
            if not changed_functions:
                # Explicit empty list means no functions changed — return nothing
                return []
            placeholders = ",".join("?" for _ in changed_functions)
            return self._fetchall(
                f"{base_sql} AND cu.name IN ({placeholders})",
                (file_path, *changed_functions),
            )
        return self._fetchall(base_sql, (file_path,))

    def delete_test_units_by_file(self, file_path):
        self._execute("DELETE FROM test_units WHERE file_path = ?", (file_path,))

    def upsert_test_units_batch(self, rows):
        """Batch upsert test units.

        *rows* is an iterable of (id, file_path, name, framework,
        line_start, line_end, content_hash) tuples.
        """
        if not rows:
            return
        now = self._now()
        self._executemany(
            """INSERT INTO test_units (id, file_path, name, framework,
               line_start, line_end, content_hash, updated_at)
               VALUES (?, ?, ?, ?, ?, ?, ?, ?)
               ON CONFLICT(id) DO UPDATE SET
               file_path=excluded.file_path, name=excluded.name,
               framework=excluded.framework, line_start=excluded.line_start,
               line_end=excluded.line_end, content_hash=excluded.content_hash,
               updated_at=excluded.updated_at""",
            [(r[0], r[1], r[2], r[3], r[4], r[5], r[6], now) for r in rows],
        )

    def delete_test_edges_by_test(self, test_id):
        self._execute("DELETE FROM test_edges WHERE test_id = ?", (test_id,))

    def get_untested_code_units(self, file_path=None, directory=None,
                               exclude_tests=True, limit=None):
        """Find code units with no test edges, joined with churn data.

        Args:
            exclude_tests: If True, exclude units from test files.
            limit: Optional maximum number of results to return.

        Returns list of dicts sorted by churn_score descending.
        """
        base = """SELECT cu.id, cu.file_path, cu.name, cu.unit_type,
                         cu.line_start, cu.line_end,
                         COALESCE(cs.churn_score, 0) AS churn_score,
                         COALESCE(cs.commit_count, 0) AS commit_count
                  FROM code_units cu
                  LEFT JOIN test_edges te ON cu.id = te.code_id
                  LEFT JOIN churn_stats cs
                      ON cu.file_path = cs.file_path AND cs.unit_name = cu.name
                  WHERE te.code_id IS NULL"""
        if exclude_tests:
            base += (" AND cu.file_path NOT IN"
                     " (SELECT DISTINCT file_path FROM test_units)")
        params = ()
        if file_path:
            base += " AND cu.file_path = ?"
            params = (file_path,)
        elif directory:
            prefix = directory.rstrip("/") + "/"
            base += " AND cu.file_path LIKE ?"
            params = (prefix + "%",)
        base += " ORDER BY churn_score DESC"
        if limit:
            base += " LIMIT ?"
            params = params + (int(limit),)
        return self._fetchall(base, params)

    # --- test_results ---

    def record_test_result(self, test_id, passed, duration_ms=None):
        self._execute(
            """INSERT INTO test_results (test_id, passed, duration_ms, recorded_at)
               VALUES (?, ?, ?, ?)""",
            (test_id, 1 if passed else 0, duration_ms, self._now()),
        )

    def get_test_failure_rates(self):
        """Get failure stats for all tests with recorded results.

        Returns:
            List of dicts: {test_id, total_runs, failures}
        """
        return self._fetchall(
            """SELECT test_id,
                      COUNT(*) AS total_runs,
                      SUM(CASE WHEN passed = 0 THEN 1 ELSE 0 END) AS failures
               FROM test_results
               GROUP BY test_id"""
        )

    def get_test_duration_cv_batch(self, test_ids, max_runs=20):
        """Coefficient of variation (std/mean) of duration_ms per test_id.

        Uses up to *max_runs* most recent rows with non-null *duration_ms*.
        Returns {test_id: cv} for tests with at least 3 samples.
        """
        if not test_ids:
            return {}

        result = {}
        for chunk in self._chunked(list(test_ids)):
            placeholders = ",".join("?" for _ in chunk)
            rows = self._fetchall(
                f"""SELECT test_id, duration_ms, recorded_at FROM test_results
                    WHERE test_id IN ({placeholders}) AND duration_ms IS NOT NULL""",
                tuple(chunk),
            )
            by_test = {}
            for row in rows:
                by_test.setdefault(row["test_id"], []).append(
                    (row["recorded_at"], row["duration_ms"]),
                )
            for tid, pairs in by_test.items():
                pairs.sort(key=lambda x: x[0], reverse=True)
                durs = [p[1] for p in pairs[:max_runs]]
                if len(durs) < 3:
                    continue
                mu = fmean(durs)
                if mu <= 0:
                    continue
                sigma = pstdev(durs)
                result[tid] = min(sigma / mu, 1.0)
        return result

    def cleanup_orphaned_test_results(self):
        """Delete test_results rows whose test_id no longer exists in test_units.

        Returns:
            Number of orphaned rows deleted.
        """
        return self._execute(
            """DELETE FROM test_results
               WHERE test_id NOT IN (SELECT id FROM test_units)"""
        )

    def has_analysis_data(self):
        """Check whether the database contains any analysis data.

        Returns True if at least one code unit exists (i.e. ``analyze``
        has been run).  Uses ``LIMIT 1`` for minimal cost.
        """
        row = self._fetchone("SELECT 1 FROM code_units LIMIT 1")
        return row is not None

    # --- meta (key-value) ---

    def get_meta(self, key):
        """Return stored value for *key*, or None if missing."""
        row = self._fetchone("SELECT value FROM meta WHERE key = ?", (key,))
        return row["value"] if row else None

    def set_meta(self, key, value):
        """Upsert a meta key (string values)."""
        self._execute(
            """INSERT INTO meta (key, value) VALUES (?, ?)
               ON CONFLICT(key) DO UPDATE SET value=excluded.value""",
            (key, str(value)),
        )

    # --- bg_jobs (in-process background analyze/update; stdlib threading only) ---

    def insert_bg_job(self, job_id, kind, status="running"):
        """Insert a new background job row."""
        now = self._now()
        self._execute(
            """INSERT INTO bg_jobs (id, kind, status, result_json, error_message,
                                    progress_pct, created_at, updated_at,
                                    cancel_requested_at, started_at)
               VALUES (?, ?, ?, NULL, NULL, 0, ?, ?, NULL, ?)""",
            (job_id, kind, status, now, now, now),
        )

    def update_bg_job(self, job_id, status, result_json=None, error_message=None,
                       progress_pct=None):
        """Update job status and optional result or error."""
        if progress_pct is not None:
            self._execute(
                """UPDATE bg_jobs SET status = ?, result_json = ?, error_message = ?,
                                      progress_pct = ?, updated_at = ?
                   WHERE id = ?""",
                (status, result_json, error_message, progress_pct, self._now(), job_id),
            )
        else:
            self._execute(
                """UPDATE bg_jobs SET status = ?, result_json = ?, error_message = ?,
                                      updated_at = ?
                   WHERE id = ?""",
                (status, result_json, error_message, self._now(), job_id),
            )

    def request_bg_job_cancel(self, job_id):
        """Set the cancel-requested flag on a running job."""
        self._execute(
            "UPDATE bg_jobs SET cancel_requested_at = ? WHERE id = ?",
            (self._now(), job_id),
        )

    def is_bg_job_cancel_requested(self, job_id):
        """Return True if a cancel has been requested for *job_id*."""
        row = self._fetchone(
            "SELECT cancel_requested_at FROM bg_jobs WHERE id = ?",
            (job_id,),
        )
        return row is not None and row["cancel_requested_at"] is not None

    def get_bg_job(self, job_id):
        """Return job row as dict, or None if missing."""
        row = self._fetchone("SELECT * FROM bg_jobs WHERE id = ?", (job_id,))
        return dict(row) if row else None

    def sweep_stale_bg_jobs(self, max_duration_minutes=30):
        """Mark any 'running' jobs as 'failed' (server restarted or timed out).

        Returns the number of jobs affected.
        """
        now = self._now()
        count = self._execute(
            """UPDATE bg_jobs SET status = 'failed',
                                  error_message = 'Server restarted while job was running'
               WHERE status = 'running' AND started_at IS NULL""",
        )
        count += self._execute(
            f"""UPDATE bg_jobs SET status = 'failed',
                                   error_message = 'Exceeded maximum duration ({max_duration_minutes}m)'
                WHERE status = 'running'
                  AND datetime(started_at, '+{max_duration_minutes} minutes') < datetime(?)""",
            (now,),
        )
        return count

    # --- job_events ---

    def insert_job_event(self, job_id, event_type, payload=None):
        """Record a progress or lifecycle event for a background job."""
        self._execute(
            """INSERT INTO job_events (job_id, event_type, payload_json, created_at)
               VALUES (?, ?, ?, ?)""",
            (job_id, event_type, json.dumps(payload) if payload else None, self._now()),
        )

    def get_job_events(self, job_id, limit=50):
        """Return recent events for a job, newest first."""
        rows = self._fetchall(
            """SELECT event_type, payload_json, created_at FROM job_events
               WHERE job_id = ? ORDER BY created_at DESC LIMIT ?""",
            (job_id, limit),
        )
        result = []
        for r in rows:
            item = {"event_type": r["event_type"], "created_at": r["created_at"]}
            if r["payload_json"]:
                item["payload"] = json.loads(r["payload_json"])
            result.append(item)
        return result

    # --- file_locks ---

    def acquire_file_lock(self, file_path, agent_id, ttl=300, purpose=None):
        """Acquire an exclusive advisory lock on file_path.

        Returns (acquired: bool, holder: str|None, expires_at: float).
        If already held by another agent, returns (False, holder, expires_at).
        Cleans up expired locks before checking.
        """
        self._cleanup_expired_locks()
        existing = self._fetchone(
            "SELECT agent_id, expires_at FROM file_locks WHERE file_path = ?",
            (file_path,),
        )
        if existing and existing["agent_id"] != agent_id:
            return False, existing["agent_id"], existing["expires_at"]
        now = time.time()
        self._execute(
            """INSERT INTO file_locks (file_path, agent_id, acquired_at, expires_at, purpose)
               VALUES (?, ?, ?, ?, ?)
               ON CONFLICT(file_path) DO UPDATE SET
                   agent_id=excluded.agent_id,
                   acquired_at=excluded.acquired_at,
                   expires_at=excluded.expires_at,
                   purpose=excluded.purpose""",
            (file_path, agent_id, now, now + ttl, purpose),
        )
        return True, None, now + ttl

    def release_file_lock(self, file_path, agent_id):
        """Release lock only if held by agent_id. Returns bool."""
        row = self._fetchone(
            "SELECT agent_id FROM file_locks WHERE file_path = ?", (file_path,)
        )
        if not row or row["agent_id"] != agent_id:
            return False
        self._execute("DELETE FROM file_locks WHERE file_path = ?", (file_path,))
        return True

    def refresh_file_lock(self, file_path, agent_id, ttl=300):
        """Extend TTL if lock held by agent_id. Returns (bool, new_expires_at)."""
        row = self._fetchone(
            "SELECT agent_id FROM file_locks WHERE file_path = ?", (file_path,)
        )
        if not row or row["agent_id"] != agent_id:
            return False, None
        now = time.time()
        new_expires = now + ttl
        self._execute(
            "UPDATE file_locks SET expires_at = ? WHERE file_path = ?",
            (new_expires, file_path),
        )
        return True, new_expires

    def get_file_lock(self, file_path):
        """Return lock info dict or None. Cleans up expired locks first."""
        self._cleanup_expired_locks()
        row = self._fetchone(
            "SELECT * FROM file_locks WHERE file_path = ?", (file_path,)
        )
        return dict(row) if row else None

    def list_file_locks(self, agent_id=None):
        """List all active locks, optionally filtered by agent."""
        self._cleanup_expired_locks()
        if agent_id is None:
            rows = self._fetchall(
                "SELECT * FROM file_locks ORDER BY acquired_at DESC"
            )
        else:
            rows = self._fetchall(
                "SELECT * FROM file_locks WHERE agent_id = ? ORDER BY acquired_at DESC",
                (agent_id,),
            )
        return [dict(r) for r in rows]

    def _cleanup_expired_locks(self):
        """Delete all locks past their expires_at. Called before reads/writes."""
        self._execute("DELETE FROM file_locks WHERE expires_at < ?", (time.time(),))

    def get_co_change_query_min(self):
        """Minimum co_commit_count used when querying co_changes (matches ingest).

        Default 3 for databases analyzed before co_change_query_min was stored.
        """
        raw = self.get_meta("co_change_query_min")
        if raw is None:
            return 3
        try:
            return max(1, int(raw))
        except ValueError:
            return 3

    def get_stats(self):
        """Get summary counts for all tables in a single query.

        Returns:
            Dict: {code_units, test_units, test_edges, commits,
                   commit_files, blame_cache, co_changes, churn_stats,
                   file_hashes, test_results}
        """
        rows = self._fetchall(
            """SELECT 'code_units' AS tbl, COUNT(*) AS cnt FROM code_units
               UNION ALL SELECT 'test_units', COUNT(*) FROM test_units
               UNION ALL SELECT 'test_edges', COUNT(*) FROM test_edges
               UNION ALL SELECT 'commits', COUNT(*) FROM commits
               UNION ALL SELECT 'commit_files', COUNT(*) FROM commit_files
               UNION ALL SELECT 'blame_cache', COUNT(*) FROM blame_cache
               UNION ALL SELECT 'co_changes', COUNT(*) FROM co_changes
               UNION ALL SELECT 'branch_co_changes', COUNT(*) FROM branch_co_changes
               UNION ALL SELECT 'import_edges', COUNT(*) FROM import_edges
               UNION ALL SELECT 'churn_stats', COUNT(*) FROM churn_stats
               UNION ALL SELECT 'file_hashes', COUNT(*) FROM file_hashes
               UNION ALL SELECT 'test_results', COUNT(*) FROM test_results"""
        )
        return {r["tbl"]: r["cnt"] for r in rows}

    # --- batch queries ---

    @staticmethod
    def _chunked(items, size=900):
        """Yield successive chunks of *items*, each at most *size* long."""
        for i in range(0, len(items), size):
            yield items[i:i + size]

    @staticmethod
    def _chunked_by_params(items, params_per_item, max_params=950):
        """Chunk *items* so that total SQL parameters stay under SQLite's limit.

        SQLite defaults to 999 variables per statement. We use 950 to leave
        headroom for additional trailing parameters (e.g. min_count).
        """
        size = max(1, max_params // max(1, params_per_item))
        for i in range(0, len(items), size):
            yield items[i:i + size]

    def get_edges_for_code_batch(self, code_ids):
        """Batch-fetch test edges for multiple code unit IDs.

        Returns a dict mapping each code_id to its list of edge dicts.
        IDs with no edges map to empty lists.
        """
        if not code_ids:
            return {}
        result = {cid: [] for cid in code_ids}
        for chunk in self._chunked(list(code_ids)):
            placeholders = ",".join("?" for _ in chunk)
            rows = self._fetchall(
                f"SELECT * FROM test_edges WHERE code_id IN ({placeholders})",
                tuple(chunk),
            )
            for row in rows:
                result[row["code_id"]].append(row)
        return result

    def get_code_units_by_files_batch(self, file_paths):
        """Batch-fetch code units for multiple file paths.

        Returns a dict mapping file_path to its list of code unit dicts.
        """
        if not file_paths:
            return {}
        result = {fp: [] for fp in file_paths}
        for chunk in self._chunked(list(file_paths)):
            placeholders = ",".join("?" for _ in chunk)
            rows = self._fetchall(
                f"SELECT * FROM code_units WHERE file_path IN ({placeholders})",
                tuple(chunk),
            )
            for row in rows:
                result[row["file_path"]].append(row)
        return result

    def get_files_with_test_edges(self, file_paths):
        """Return the subset of file paths that have at least one test edge."""
        if not file_paths:
            return set()
        result = set()
        for chunk in self._chunked(list(file_paths)):
            placeholders = ",".join("?" for _ in chunk)
            rows = self._fetchall(
                f"""SELECT DISTINCT cu.file_path
                    FROM code_units cu
                    JOIN test_edges te ON cu.id = te.code_id
                    WHERE cu.file_path IN ({placeholders})""",
                tuple(chunk),
            )
            for row in rows:
                result.add(row["file_path"])
        return result

    def get_co_changes_batch(self, file_paths, min_count=3):
        """Batch-fetch co-changes for multiple file paths.

        Returns a dict mapping file_path to its list of co-change dicts.
        """
        if not file_paths:
            return {}
        result = {fp: [] for fp in file_paths}
        for chunk in self._chunked_by_params(list(file_paths), params_per_item=2):
            placeholders = ",".join("?" for _ in chunk)
            rows = self._fetchall(
                f"""SELECT * FROM co_changes
                    WHERE (file_a IN ({placeholders}) OR file_b IN ({placeholders}))
                    AND co_commit_count >= ?
                    ORDER BY co_commit_count DESC""",
                (*chunk, *chunk, min_count),
            )
            for row in rows:
                if row["file_a"] in result:
                    result[row["file_a"]].append(row)
                if row["file_b"] in result and row["file_b"] != row["file_a"]:
                    result[row["file_b"]].append(row)
        return result

    def get_churn_stats_batch(self, file_paths):
        """Batch-fetch file-level churn stats (unit_name='') for multiple files.

        Returns a dict mapping file_path to its churn stat dict (or None).
        """
        if not file_paths:
            return {}
        result = {fp: None for fp in file_paths}
        for chunk in self._chunked(list(file_paths)):
            placeholders = ",".join("?" for _ in chunk)
            rows = self._fetchall(
                f"""SELECT * FROM churn_stats
                    WHERE file_path IN ({placeholders}) AND unit_name = ''""",
                tuple(chunk),
            )
            for row in rows:
                result[row["file_path"]] = row
        return result

    def get_blame_batch(self, file_hash_pairs):
        """Batch-fetch blame data for multiple (file_path, content_hash) pairs.

        Returns a dict mapping file_path to its list of blame block dicts.
        """
        if not file_hash_pairs:
            return {}
        result = {fp: [] for fp, _ in file_hash_pairs}
        for chunk in self._chunked_by_params(list(file_hash_pairs), params_per_item=2):
            conditions = " OR ".join(
                "(file_path = ? AND content_hash = ?)" for _ in chunk
            )
            params = []
            for fp, ch in chunk:
                params.extend([fp, ch])
            rows = self._fetchall(
                f"SELECT * FROM blame_cache WHERE {conditions} ORDER BY line_start",
                tuple(params),
            )
            for row in rows:
                result[row["file_path"]].append(row)
        return result

    # --- file_hashes ---

    def set_file_hash(self, file_path, content_hash, mtime=None, size=None):
        self._execute(
            """INSERT INTO file_hashes (file_path, content_hash, updated_at, mtime, size)
               VALUES (?, ?, ?, ?, ?)
               ON CONFLICT(file_path) DO UPDATE SET
               content_hash=excluded.content_hash, updated_at=excluded.updated_at,
               mtime=excluded.mtime, size=excluded.size""",
            (file_path, content_hash, self._now(), mtime, size),
        )

    def set_file_hashes_batch(self, rows):
        """Batch upsert file hashes.

        *rows* is an iterable of (file_path, content_hash, mtime, size) tuples.
        """
        if not rows:
            return
        now = self._now()
        self._executemany(
            """INSERT INTO file_hashes (file_path, content_hash, updated_at, mtime, size)
               VALUES (?, ?, ?, ?, ?)
               ON CONFLICT(file_path) DO UPDATE SET
               content_hash=excluded.content_hash, updated_at=excluded.updated_at,
               mtime=excluded.mtime, size=excluded.size""",
            [(r[0], r[1], now, r[2], r[3]) for r in rows],
        )

    def get_file_hash(self, file_path):
        row = self._fetchone(
            "SELECT content_hash FROM file_hashes WHERE file_path = ?",
            (file_path,),
        )
        return row["content_hash"] if row else None

    def get_file_hashes_batch(self, file_paths):
        """Batch-fetch file hashes with mtime and size.

        Returns a dict mapping file_path -> {"hash": ..., "mtime": ..., "size": ...}
        (or None for missing files).
        """
        if not file_paths:
            return {}
        result = {fp: None for fp in file_paths}
        for chunk in self._chunked(list(file_paths)):
            placeholders = ",".join("?" for _ in chunk)
            rows = self._fetchall(
                f"SELECT file_path, content_hash, mtime, size FROM file_hashes WHERE file_path IN ({placeholders})",
                tuple(chunk),
            )
            for row in rows:
                result[row["file_path"]] = {
                    "hash": row["content_hash"],
                    "mtime": row["mtime"],
                    "size": row["size"],
                }
        return result

    # --- static_test_imports ---

    def clear_static_test_imports(self):
        self._execute("DELETE FROM static_test_imports")

    def upsert_static_test_imports_batch(self, rows):
        """Batch upsert static test import mappings.

        *rows* is an iterable of (test_file_path, test_unit_name,
        target_file_path, gap_eligible, score) tuples.
        """
        if not rows:
            return
        self._executemany(
            """INSERT INTO static_test_imports
               (test_file_path, test_unit_name, target_file_path, gap_eligible, score)
               VALUES (?, ?, ?, ?, ?)
               ON CONFLICT(test_file_path, test_unit_name, target_file_path) DO UPDATE SET
               gap_eligible=excluded.gap_eligible, score=excluded.score""",
            list(rows),
        )

    def find_static_tests_for_target(self, target_file_path):
        """Return static test import rows that resolve to *target_file_path*."""
        return self._fetchall(
            """SELECT test_file_path, test_unit_name, gap_eligible, score
               FROM static_test_imports
               WHERE target_file_path = ?""",
            (target_file_path,),
        )

    # --- maintenance ---

    def wal_checkpoint(self):
        """Run WAL checkpoint to truncate the WAL file after large writes.

        Falls back to PASSIVE if TRUNCATE is blocked by an active read or
        prior unfinalized statement.
        """
        conn = self._conn
        conn.commit()
        try:
            conn.execute("PRAGMA wal_checkpoint(TRUNCATE)")
        except sqlite3.OperationalError:
            conn.execute("PRAGMA wal_checkpoint(PASSIVE)")

    def optimize(self, wal_size_threshold_bytes=10 * 1024 * 1024):
        """Run PRAGMA optimize and VACUUM if the WAL file is large.

        Returns a dict with the actions performed.
        """
        vacuumed = False
        with self._conn as conn:
            conn.execute("PRAGMA optimize")
        wal_path = str(self.db_path) + "-wal"
        if os.path.exists(wal_path):
            wal_size = os.path.getsize(wal_path)
            if wal_size > wal_size_threshold_bytes:
                with self._conn as conn:
                    conn.execute("VACUUM")
                vacuumed = True
        return {"optimized": True, "vacuumed": vacuumed}
