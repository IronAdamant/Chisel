"""SQLite persistence layer for Chisel's data model."""

import sqlite3
from datetime import datetime, timezone
from pathlib import Path


class Storage:
    """Manages all Chisel data in a SQLite database with WAL mode."""

    def __init__(self, base_dir=None):
        if base_dir is None:
            base_dir = Path.home() / ".chisel"
        self.base_dir = Path(base_dir)
        self.base_dir.mkdir(parents=True, exist_ok=True)
        self.db_path = self.base_dir / "chisel.db"
        self._conn = self._create_connection()
        self._init_database()

    def _create_connection(self):
        """Create and configure the SQLite connection (called once)."""
        conn = sqlite3.connect(str(self.db_path), timeout=10, check_same_thread=False)
        conn.row_factory = sqlite3.Row
        conn.execute("PRAGMA journal_mode=WAL")
        conn.execute("PRAGMA synchronous=NORMAL")
        # FK enforcement disabled — Chisel manages integrity at application level
        # and stale test detection relies on orphaned edge references.
        return conn

    def __enter__(self):
        return self

    def __exit__(self, *exc):
        self.close()

    def close(self):
        """Close the persistent database connection."""
        if self._conn is not None:
            self._conn.close()
            self._conn = None

    def _init_database(self):
        with self._conn as conn:
            conn.executescript("""
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
                CREATE INDEX IF NOT EXISTS idx_commit_files_file ON commit_files(file_path);
                CREATE INDEX IF NOT EXISTS idx_blame_cache_hash ON blame_cache(content_hash);
                CREATE INDEX IF NOT EXISTS idx_churn_stats_file ON churn_stats(file_path);
            """)

    def _now(self):
        return datetime.now(timezone.utc).isoformat()

    # --- code_units ---

    def upsert_code_unit(self, id, file_path, name, unit_type,
                         line_start=None, line_end=None, content_hash=None):
        with self._conn as conn:
            conn.execute(
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
        with self._conn as conn:
            row = conn.execute("SELECT * FROM code_units WHERE id = ?", (id,)).fetchone()
            return dict(row) if row else None

    def get_code_units_by_file(self, file_path):
        with self._conn as conn:
            rows = conn.execute(
                "SELECT * FROM code_units WHERE file_path = ?", (file_path,)
            ).fetchall()
            return [dict(r) for r in rows]

    def delete_code_units_by_file(self, file_path):
        with self._conn as conn:
            conn.execute("DELETE FROM code_units WHERE file_path = ?", (file_path,))

    # --- test_units ---

    def upsert_test_unit(self, id, file_path, name, framework=None,
                         line_start=None, line_end=None, content_hash=None):
        with self._conn as conn:
            conn.execute(
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
        with self._conn as conn:
            row = conn.execute("SELECT * FROM test_units WHERE id = ?", (id,)).fetchone()
            return dict(row) if row else None

    def get_all_test_units(self):
        with self._conn as conn:
            rows = conn.execute("SELECT * FROM test_units").fetchall()
            return [dict(r) for r in rows]

    # --- test_edges ---

    def upsert_test_edge(self, test_id, code_id, edge_type, weight=1.0):
        with self._conn as conn:
            conn.execute(
                """INSERT INTO test_edges (test_id, code_id, edge_type, weight)
                   VALUES (?, ?, ?, ?)
                   ON CONFLICT(test_id, code_id, edge_type) DO UPDATE SET
                   weight=excluded.weight""",
                (test_id, code_id, edge_type, weight),
            )

    def get_edges_for_test(self, test_id):
        with self._conn as conn:
            rows = conn.execute(
                "SELECT * FROM test_edges WHERE test_id = ?", (test_id,)
            ).fetchall()
            return [dict(r) for r in rows]

    def get_edges_for_code(self, code_id):
        with self._conn as conn:
            rows = conn.execute(
                "SELECT * FROM test_edges WHERE code_id = ?", (code_id,)
            ).fetchall()
            return [dict(r) for r in rows]

    # --- commits ---

    def upsert_commit(self, hash, author=None, author_email=None, date=None, message=None):
        with self._conn as conn:
            conn.execute(
                """INSERT INTO commits (hash, author, author_email, date, message)
                   VALUES (?, ?, ?, ?, ?)
                   ON CONFLICT(hash) DO UPDATE SET
                   author=excluded.author, author_email=excluded.author_email,
                   date=excluded.date, message=excluded.message""",
                (hash, author, author_email, date, message),
            )

    def get_commit(self, hash):
        with self._conn as conn:
            row = conn.execute("SELECT * FROM commits WHERE hash = ?", (hash,)).fetchone()
            return dict(row) if row else None

    def get_latest_commit_date(self):
        with self._conn as conn:
            row = conn.execute(
                "SELECT MAX(date) as max_date FROM commits"
            ).fetchone()
            return row["max_date"]

    # --- commit_files ---

    def upsert_commit_file(self, commit_hash, file_path, insertions=0, deletions=0):
        with self._conn as conn:
            conn.execute(
                """INSERT INTO commit_files (commit_hash, file_path, insertions, deletions)
                   VALUES (?, ?, ?, ?)
                   ON CONFLICT(commit_hash, file_path) DO UPDATE SET
                   insertions=excluded.insertions, deletions=excluded.deletions""",
                (commit_hash, file_path, insertions, deletions),
            )

    def get_commits_for_file(self, file_path):
        with self._conn as conn:
            rows = conn.execute(
                """SELECT c.*, cf.insertions, cf.deletions FROM commits c
                   JOIN commit_files cf ON c.hash = cf.commit_hash
                   WHERE cf.file_path = ? ORDER BY c.date DESC""",
                (file_path,),
            ).fetchall()
            return [dict(r) for r in rows]

    # --- blame_cache ---

    def store_blame(self, file_path, line_start, line_end, commit_hash,
                    author, author_email, date, content_hash):
        with self._conn as conn:
            conn.execute(
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
        with self._conn as conn:
            rows = conn.execute(
                """SELECT * FROM blame_cache
                   WHERE file_path = ? AND content_hash = ?
                   ORDER BY line_start""",
                (file_path, content_hash),
            ).fetchall()
            return [dict(r) for r in rows]

    def invalidate_blame(self, file_path):
        with self._conn as conn:
            conn.execute("DELETE FROM blame_cache WHERE file_path = ?", (file_path,))

    # --- co_changes ---

    def upsert_co_change(self, file_a, file_b, co_commit_count, last_co_commit=None):
        a, b = sorted([file_a, file_b])
        with self._conn as conn:
            conn.execute(
                """INSERT INTO co_changes (file_a, file_b, co_commit_count, last_co_commit)
                   VALUES (?, ?, ?, ?)
                   ON CONFLICT(file_a, file_b) DO UPDATE SET
                   co_commit_count=excluded.co_commit_count,
                   last_co_commit=excluded.last_co_commit""",
                (a, b, co_commit_count, last_co_commit),
            )

    def get_co_changes(self, file_path, min_count=3):
        with self._conn as conn:
            rows = conn.execute(
                """SELECT * FROM co_changes
                   WHERE (file_a = ? OR file_b = ?) AND co_commit_count >= ?
                   ORDER BY co_commit_count DESC""",
                (file_path, file_path, min_count),
            ).fetchall()
            return [dict(r) for r in rows]

    # --- churn_stats ---

    def upsert_churn_stat(self, file_path, unit_name, commit_count=0,
                          distinct_authors=0, total_insertions=0, total_deletions=0,
                          last_changed=None, churn_score=0.0):
        unit_name = unit_name or ""
        with self._conn as conn:
            conn.execute(
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

    def get_churn_stat(self, file_path, unit_name=None):
        unit_name = unit_name or ""
        with self._conn as conn:
            row = conn.execute(
                "SELECT * FROM churn_stats WHERE file_path = ? AND unit_name = ?",
                (file_path, unit_name),
            ).fetchone()
            return dict(row) if row else None

    def get_all_churn_stats(self, file_path=None):
        with self._conn as conn:
            if file_path:
                rows = conn.execute(
                    "SELECT * FROM churn_stats WHERE file_path = ? ORDER BY churn_score DESC",
                    (file_path,),
                ).fetchall()
            else:
                rows = conn.execute(
                    "SELECT * FROM churn_stats ORDER BY churn_score DESC"
                ).fetchall()
            return [dict(r) for r in rows]

    # --- file_hashes ---

    def set_file_hash(self, file_path, content_hash):
        with self._conn as conn:
            conn.execute(
                """INSERT INTO file_hashes (file_path, content_hash, updated_at)
                   VALUES (?, ?, ?)
                   ON CONFLICT(file_path) DO UPDATE SET
                   content_hash=excluded.content_hash, updated_at=excluded.updated_at""",
                (file_path, content_hash, self._now()),
            )

    def get_file_hash(self, file_path):
        with self._conn as conn:
            row = conn.execute(
                "SELECT content_hash FROM file_hashes WHERE file_path = ?",
                (file_path,),
            ).fetchone()
            return row["content_hash"] if row else None
