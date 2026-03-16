"""Tests for chisel.storage — all CRUD operations per table."""

import sqlite3

import pytest

from chisel.storage import Storage


@pytest.fixture
def storage(tmp_path):
    s = Storage(base_dir=tmp_path / "chisel_data")
    yield s
    s.close()


class TestDatabaseInit:
    def test_creates_database_file(self, storage):
        assert storage.db_path.exists()

    def test_wal_mode(self, storage):
        conn = sqlite3.connect(str(storage.db_path))
        mode = conn.execute("PRAGMA journal_mode").fetchone()[0]
        conn.close()
        assert mode == "wal"

    def test_all_tables_exist(self, storage):
        expected = {
            "code_units", "test_units", "test_edges", "commits",
            "commit_files", "blame_cache", "co_changes", "churn_stats",
            "file_hashes",
        }
        conn = sqlite3.connect(str(storage.db_path))
        tables = {
            r[0] for r in conn.execute(
                "SELECT name FROM sqlite_master WHERE type='table'"
            ).fetchall()
        }
        conn.close()
        assert expected.issubset(tables)

    def test_indexes_exist(self, storage):
        expected_indexes = {
            "idx_code_units_file", "idx_test_units_file", "idx_test_edges_code",
            "idx_commit_files_file", "idx_blame_cache_hash", "idx_churn_stats_file",
            "idx_co_changes_file_b",
        }
        conn = sqlite3.connect(str(storage.db_path))
        indexes = {
            r[0] for r in conn.execute(
                "SELECT name FROM sqlite_master WHERE type='index'"
            ).fetchall()
        }
        conn.close()
        assert expected_indexes.issubset(indexes)


class TestCodeUnits:
    def test_upsert_and_get(self, storage):
        storage.upsert_code_unit("f.py:foo:func", "f.py", "foo", "func", 1, 10, "abc")
        unit = storage.get_code_unit("f.py:foo:func")
        assert unit["name"] == "foo"
        assert unit["unit_type"] == "func"
        assert unit["line_start"] == 1
        assert unit["line_end"] == 10

    def test_upsert_overwrites(self, storage):
        storage.upsert_code_unit("f.py:foo:func", "f.py", "foo", "func", 1, 10, "abc")
        storage.upsert_code_unit("f.py:foo:func", "f.py", "foo", "func", 5, 20, "def")
        unit = storage.get_code_unit("f.py:foo:func")
        assert unit["line_start"] == 5
        assert unit["content_hash"] == "def"

    def test_get_by_file(self, storage):
        storage.upsert_code_unit("f.py:a:func", "f.py", "a", "func")
        storage.upsert_code_unit("f.py:b:func", "f.py", "b", "func")
        storage.upsert_code_unit("g.py:c:func", "g.py", "c", "func")
        units = storage.get_code_units_by_file("f.py")
        assert len(units) == 2

    def test_delete_by_file(self, storage):
        storage.upsert_code_unit("f.py:a:func", "f.py", "a", "func")
        storage.upsert_code_unit("f.py:b:func", "f.py", "b", "func")
        storage.delete_code_units_by_file("f.py")
        assert storage.get_code_units_by_file("f.py") == []

    def test_get_nonexistent(self, storage):
        assert storage.get_code_unit("nope") is None


class TestTestUnits:
    def test_upsert_and_get(self, storage):
        storage.upsert_test_unit("t.py:test_x", "t.py", "test_x", "pytest", 1, 5, "h1")
        unit = storage.get_test_unit("t.py:test_x")
        assert unit["framework"] == "pytest"

    def test_get_all(self, storage):
        storage.upsert_test_unit("t1.py:a", "t1.py", "a", "pytest")
        storage.upsert_test_unit("t2.py:b", "t2.py", "b", "jest")
        all_units = storage.get_all_test_units()
        assert len(all_units) == 2


class TestTestEdges:
    def test_upsert_and_get_for_test(self, storage):
        storage.upsert_code_unit("c1", "f.py", "foo", "func")
        storage.upsert_test_unit("t1", "t.py", "test_foo")
        storage.upsert_test_edge("t1", "c1", "import", 2.0)
        edges = storage.get_edges_for_test("t1")
        assert len(edges) == 1
        assert edges[0]["weight"] == 2.0

    def test_get_for_code(self, storage):
        storage.upsert_code_unit("c1", "f.py", "foo", "func")
        storage.upsert_test_unit("t1", "t.py", "test_a")
        storage.upsert_test_unit("t2", "t.py", "test_b")
        storage.upsert_test_edge("t1", "c1", "call")
        storage.upsert_test_edge("t2", "c1", "import")
        edges = storage.get_edges_for_code("c1")
        assert len(edges) == 2

    def test_upsert_updates_weight(self, storage):
        storage.upsert_code_unit("c1", "f.py", "foo", "func")
        storage.upsert_test_unit("t1", "t.py", "test_a")
        storage.upsert_test_edge("t1", "c1", "call", 1.0)
        storage.upsert_test_edge("t1", "c1", "call", 5.0)
        edges = storage.get_edges_for_test("t1")
        assert edges[0]["weight"] == 5.0


class TestCommits:
    def test_upsert_and_get(self, storage):
        storage.upsert_commit("abc123", "Alice", "a@b.com", "2026-01-01", "fix bug")
        c = storage.get_commit("abc123")
        assert c["author"] == "Alice"
        assert c["message"] == "fix bug"

    def test_get_latest_date(self, storage):
        storage.upsert_commit("a", date="2026-01-01")
        storage.upsert_commit("b", date="2026-03-15")
        storage.upsert_commit("c", date="2026-02-10")
        assert storage.get_latest_commit_date() == "2026-03-15"

    def test_get_latest_date_empty(self, storage):
        assert storage.get_latest_commit_date() is None


class TestCommitFiles:
    def test_upsert_and_get_for_file(self, storage):
        storage.upsert_commit("abc", "A", "a@b.com", "2026-01-01", "msg")
        storage.upsert_commit_file("abc", "f.py", 10, 3)
        commits = storage.get_commits_for_file("f.py")
        assert len(commits) == 1
        assert commits[0]["insertions"] == 10
        assert commits[0]["deletions"] == 3

    def test_multiple_commits_for_file(self, storage):
        storage.upsert_commit("a", date="2026-01-01")
        storage.upsert_commit("b", date="2026-02-01")
        storage.upsert_commit_file("a", "f.py", 5, 2)
        storage.upsert_commit_file("b", "f.py", 3, 1)
        commits = storage.get_commits_for_file("f.py")
        assert len(commits) == 2
        assert commits[0]["date"] > commits[1]["date"]


class TestBlameCache:
    def test_store_and_get(self, storage):
        storage.store_blame("f.py", 1, 10, "abc", "Alice", "a@b.com", "2026-01-01", "hash1")
        blocks = storage.get_blame("f.py", "hash1")
        assert len(blocks) == 1
        assert blocks[0]["author"] == "Alice"

    def test_get_wrong_hash_returns_empty(self, storage):
        storage.store_blame("f.py", 1, 10, "abc", "Alice", "a@b.com", "2026-01-01", "hash1")
        assert storage.get_blame("f.py", "wrong_hash") == []

    def test_invalidate(self, storage):
        storage.store_blame("f.py", 1, 10, "abc", "Alice", "a@b.com", "2026-01-01", "hash1")
        storage.invalidate_blame("f.py")
        assert storage.get_blame("f.py", "hash1") == []

    def test_upsert_same_line_start(self, storage):
        storage.store_blame("f.py", 1, 5, "abc", "Alice", "a@b.com", "2026-01-01", "h1")
        storage.store_blame("f.py", 1, 8, "def", "Bob", "b@b.com", "2026-02-01", "h2")
        blocks = storage.get_blame("f.py", "h2")
        assert len(blocks) == 1
        assert blocks[0]["author"] == "Bob"


class TestCoChanges:
    def test_upsert_and_get(self, storage):
        storage.upsert_co_change("a.py", "b.py", 5, "abc123")
        results = storage.get_co_changes("a.py", min_count=3)
        assert len(results) == 1
        assert results[0]["co_commit_count"] == 5

    def test_min_count_filter(self, storage):
        storage.upsert_co_change("a.py", "b.py", 2)
        assert storage.get_co_changes("a.py", min_count=3) == []

    def test_sorted_pair_keys(self, storage):
        storage.upsert_co_change("z.py", "a.py", 4)
        results = storage.get_co_changes("z.py", min_count=1)
        assert len(results) == 1
        assert results[0]["file_a"] == "a.py"

    def test_query_from_either_side(self, storage):
        storage.upsert_co_change("a.py", "b.py", 5)
        assert len(storage.get_co_changes("b.py", min_count=1)) == 1


class TestChurnStats:
    def test_upsert_and_get(self, storage):
        storage.upsert_churn_stat("f.py", "foo", 10, 3, 50, 20, "2026-01-01", 0.82)
        stat = storage.get_churn_stat("f.py", "foo")
        assert stat["commit_count"] == 10
        assert stat["churn_score"] == 0.82

    def test_file_level_stat(self, storage):
        storage.upsert_churn_stat("f.py", None, 5, 2, 30, 10, "2026-01-01", 0.5)
        stat = storage.get_churn_stat("f.py")
        assert stat["commit_count"] == 5

    def test_get_all(self, storage):
        storage.upsert_churn_stat("a.py", "", 5, 2, 10, 5, "2026-01-01", 0.8)
        storage.upsert_churn_stat("b.py", "", 3, 1, 5, 2, "2026-01-01", 0.3)
        all_stats = storage.get_all_churn_stats()
        assert len(all_stats) == 2
        assert all_stats[0]["churn_score"] >= all_stats[1]["churn_score"]

    def test_get_all_for_file(self, storage):
        storage.upsert_churn_stat("f.py", "", 5, 2, 10, 5, "2026-01-01", 0.8)
        storage.upsert_churn_stat("f.py", "foo", 3, 1, 5, 2, "2026-01-01", 0.3)
        storage.upsert_churn_stat("g.py", "", 1, 1, 1, 0, "2026-01-01", 0.1)
        stats = storage.get_all_churn_stats("f.py")
        assert len(stats) == 2


class TestFileHashes:
    def test_set_and_get(self, storage):
        storage.set_file_hash("f.py", "abc123")
        assert storage.get_file_hash("f.py") == "abc123"

    def test_update(self, storage):
        storage.set_file_hash("f.py", "old")
        storage.set_file_hash("f.py", "new")
        assert storage.get_file_hash("f.py") == "new"

    def test_get_nonexistent(self, storage):
        assert storage.get_file_hash("nope.py") is None
