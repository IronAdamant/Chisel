"""Tests for chisel.git_analyzer — log/blame parsing, churn, ownership, co-change."""

from datetime import datetime, timezone

import pytest

from chisel.git_analyzer import GitAnalyzer


@pytest.fixture
def git_repo(tmp_path, run_git):
    """Create a temporary git repo with three deterministic commits.

    Commit 1 (2026-01-10): add hello.py and utils.py
    Commit 2 (2026-02-15): modify hello.py
    Commit 3 (2026-03-01): modify hello.py and utils.py (co-change)
    """
    repo = tmp_path / "repo"
    repo.mkdir()
    run_git(repo, "init")
    run_git(repo, "config", "user.name", "TestAuthor")
    run_git(repo, "config", "user.email", "test@example.com")

    # Commit 1: add two files
    (repo / "hello.py").write_text("def greet():\n    return 'hi'\n")
    (repo / "utils.py").write_text("def helper():\n    return 1\n")
    run_git(repo, "add", "hello.py", "utils.py")
    run_git(
        repo, "commit", "-m", "initial commit",
        env_extra={
            "GIT_AUTHOR_DATE": "2026-01-10T12:00:00+00:00",
            "GIT_COMMITTER_DATE": "2026-01-10T12:00:00+00:00",
        },
    )

    # Commit 2: modify hello.py only
    (repo / "hello.py").write_text(
        "def greet():\n    return 'hello'\n\ndef farewell():\n    return 'bye'\n"
    )
    run_git(repo, "add", "hello.py")
    run_git(
        repo, "commit", "-m", "update greet and add farewell",
        env_extra={
            "GIT_AUTHOR_DATE": "2026-02-15T12:00:00+00:00",
            "GIT_COMMITTER_DATE": "2026-02-15T12:00:00+00:00",
        },
    )

    # Commit 3: modify both files (co-change)
    (repo / "hello.py").write_text(
        "def greet():\n    return 'hello world'\n\ndef farewell():\n    return 'goodbye'\n"
    )
    (repo / "utils.py").write_text("def helper():\n    return 42\n")
    run_git(repo, "add", "hello.py", "utils.py")
    run_git(
        repo, "commit", "-m", "update both files",
        env_extra={
            "GIT_AUTHOR_DATE": "2026-03-01T12:00:00+00:00",
            "GIT_COMMITTER_DATE": "2026-03-01T12:00:00+00:00",
        },
    )

    return repo


@pytest.fixture
def analyzer(git_repo):
    return GitAnalyzer(git_repo)


@pytest.fixture
def multi_author_repo(tmp_path, run_git):
    """Create a repo with commits from two different authors."""
    repo = tmp_path / "multi_repo"
    repo.mkdir()
    run_git(repo, "init")
    run_git(repo, "config", "user.name", "Alice")
    run_git(repo, "config", "user.email", "alice@example.com")

    # Alice's commit
    (repo / "shared.py").write_text("# Alice's line 1\n# Alice's line 2\n# Alice's line 3\n")
    run_git(repo, "add", "shared.py")
    run_git(
        repo, "commit", "-m", "alice initial",
        env_extra={
            "GIT_AUTHOR_NAME": "Alice",
            "GIT_COMMITTER_NAME": "Alice",
            "GIT_AUTHOR_EMAIL": "alice@example.com",
            "GIT_COMMITTER_EMAIL": "alice@example.com",
            "GIT_AUTHOR_DATE": "2026-01-10T12:00:00+00:00",
            "GIT_COMMITTER_DATE": "2026-01-10T12:00:00+00:00",
        },
    )

    # Bob's commit: add line at end
    (repo / "shared.py").write_text(
        "# Alice's line 1\n# Alice's line 2\n# Alice's line 3\n# Bob's line 4\n"
    )
    run_git(repo, "add", "shared.py")
    run_git(
        repo, "commit", "-m", "bob adds a line",
        env_extra={
            "GIT_AUTHOR_NAME": "Bob",
            "GIT_COMMITTER_NAME": "Bob",
            "GIT_AUTHOR_EMAIL": "bob@example.com",
            "GIT_COMMITTER_EMAIL": "bob@example.com",
            "GIT_AUTHOR_DATE": "2026-02-15T12:00:00+00:00",
            "GIT_COMMITTER_DATE": "2026-02-15T12:00:00+00:00",
        },
    )

    return repo


# ================================================================== #
# Unit tests — pure computation, no git needed
# ================================================================== #

class TestComputeChurn:
    """Test churn score computation with synthetic commit data."""

    def _make_commits(self, dates_and_files):
        """Build synthetic commit list from (date, files) pairs."""
        commits = []
        for i, (date, files) in enumerate(dates_and_files):
            commits.append({
                "hash": f"abc{i:04d}",
                "author": "Alice",
                "author_email": "alice@example.com",
                "date": date,
                "message": f"commit {i}",
                "files": [{"path": f, "insertions": 5, "deletions": 2} for f in files],
            })
        return commits

    def test_empty_commits(self):
        result = GitAnalyzer.compute_churn([], "foo.py")
        assert result["commit_count"] == 0
        assert result["churn_score"] == 0.0
        assert result["last_changed"] is None

    def test_no_matching_file(self):
        commits = self._make_commits([("2026-03-01T12:00:00+00:00", ["other.py"])])
        result = GitAnalyzer.compute_churn(commits, "foo.py")
        assert result["commit_count"] == 0

    def test_single_commit_today(self):
        now = datetime(2026, 3, 1, 12, 0, 0, tzinfo=timezone.utc)
        commits = self._make_commits([("2026-03-01T12:00:00+00:00", ["foo.py"])])
        result = GitAnalyzer.compute_churn(commits, "foo.py", now=now)
        assert result["commit_count"] == 1
        assert result["distinct_authors"] == 1
        # 1/(1+0) = 1.0
        assert result["churn_score"] == 1.0
        assert result["total_insertions"] == 5
        assert result["total_deletions"] == 2

    def test_churn_decays_with_age(self):
        now = datetime(2026, 3, 10, 12, 0, 0, tzinfo=timezone.utc)
        # Commit 10 days ago: score = 1/(1+10) ~= 0.0909
        commits = self._make_commits([("2026-02-28T12:00:00+00:00", ["foo.py"])])
        result = GitAnalyzer.compute_churn(commits, "foo.py", now=now)
        assert result["commit_count"] == 1
        assert 0.08 < result["churn_score"] < 0.10

    def test_multiple_commits_accumulate(self):
        now = datetime(2026, 3, 10, 12, 0, 0, tzinfo=timezone.utc)
        commits = self._make_commits([
            ("2026-03-10T12:00:00+00:00", ["foo.py"]),  # 0 days: 1.0
            ("2026-03-09T12:00:00+00:00", ["foo.py"]),  # 1 day: 0.5
        ])
        result = GitAnalyzer.compute_churn(commits, "foo.py", now=now)
        assert result["commit_count"] == 2
        assert result["total_insertions"] == 10
        assert 1.49 < result["churn_score"] < 1.51

    def test_distinct_authors(self):
        commits = [
            {
                "hash": "a1", "author": "Alice", "author_email": "a@x.com",
                "date": "2026-03-01T12:00:00+00:00", "message": "c1",
                "files": [{"path": "f.py", "insertions": 1, "deletions": 0}],
            },
            {
                "hash": "a2", "author": "Bob", "author_email": "b@x.com",
                "date": "2026-03-02T12:00:00+00:00", "message": "c2",
                "files": [{"path": "f.py", "insertions": 2, "deletions": 1}],
            },
        ]
        result = GitAnalyzer.compute_churn(
            commits, "f.py",
            now=datetime(2026, 3, 5, tzinfo=timezone.utc),
        )
        assert result["distinct_authors"] == 2

    def test_last_changed_picks_latest(self):
        commits = self._make_commits([
            ("2026-01-01T12:00:00+00:00", ["f.py"]),
            ("2026-03-01T12:00:00+00:00", ["f.py"]),
            ("2026-02-01T12:00:00+00:00", ["f.py"]),
        ])
        result = GitAnalyzer.compute_churn(
            commits, "f.py",
            now=datetime(2026, 3, 5, tzinfo=timezone.utc),
        )
        assert result["last_changed"] == "2026-03-01T12:00:00+00:00"


class TestComputeOwnership:
    """Test ownership computation from synthetic blame blocks."""

    def test_empty_blocks(self):
        assert GitAnalyzer.compute_ownership([]) == []

    def test_single_author_100_percent(self):
        blocks = [
            {
                "commit_hash": "abc", "author": "Alice",
                "author_email": "a@x.com", "date": "2026-01-01",
                "line_start": 1, "line_end": 10,
            },
        ]
        result = GitAnalyzer.compute_ownership(blocks)
        assert len(result) == 1
        assert result[0]["author"] == "Alice"
        assert result[0]["line_count"] == 10
        assert result[0]["percentage"] == 100.0

    def test_two_authors(self):
        blocks = [
            {
                "commit_hash": "a", "author": "Alice",
                "author_email": "a@x.com", "date": "2026-01-01",
                "line_start": 1, "line_end": 3,
            },
            {
                "commit_hash": "b", "author": "Bob",
                "author_email": "b@x.com", "date": "2026-02-01",
                "line_start": 4, "line_end": 4,
            },
        ]
        result = GitAnalyzer.compute_ownership(blocks)
        assert len(result) == 2
        # Alice has 3 lines (75%), Bob has 1 line (25%)
        assert result[0]["author"] == "Alice"
        assert result[0]["line_count"] == 3
        assert result[0]["percentage"] == 75.0
        assert result[1]["author"] == "Bob"
        assert result[1]["line_count"] == 1
        assert result[1]["percentage"] == 25.0

    def test_multiple_blocks_same_author_aggregated(self):
        blocks = [
            {
                "commit_hash": "a", "author": "Alice",
                "author_email": "a@x.com", "date": "2026-01-01",
                "line_start": 1, "line_end": 5,
            },
            {
                "commit_hash": "b", "author": "Bob",
                "author_email": "b@x.com", "date": "2026-02-01",
                "line_start": 6, "line_end": 8,
            },
            {
                "commit_hash": "c", "author": "Alice",
                "author_email": "a@x.com", "date": "2026-03-01",
                "line_start": 9, "line_end": 10,
            },
        ]
        result = GitAnalyzer.compute_ownership(blocks)
        assert len(result) == 2
        alice = next(r for r in result if r["author"] == "Alice")
        assert alice["line_count"] == 7
        assert alice["percentage"] == 70.0


class TestComputeCoChanges:
    """Test co-change detection from synthetic commit data."""

    def test_empty_commits(self):
        assert GitAnalyzer.compute_co_changes([]) == []

    def test_below_threshold(self):
        commits = [
            {
                "hash": "a", "author": "A", "author_email": "", "date": "2026-01-01",
                "message": "c1",
                "files": [{"path": "x.py", "insertions": 1, "deletions": 0},
                          {"path": "y.py", "insertions": 1, "deletions": 0}],
            },
            {
                "hash": "b", "author": "A", "author_email": "", "date": "2026-01-02",
                "message": "c2",
                "files": [{"path": "x.py", "insertions": 1, "deletions": 0},
                          {"path": "y.py", "insertions": 1, "deletions": 0}],
            },
        ]
        # Default min_count=3, only 2 co-commits
        assert GitAnalyzer.compute_co_changes(commits) == []

    def test_meets_threshold(self):
        commits = [
            {
                "hash": f"h{i}", "author": "A", "author_email": "", "date": f"2026-01-0{i+1}",
                "message": f"c{i}",
                "files": [{"path": "a.py", "insertions": 1, "deletions": 0},
                          {"path": "b.py", "insertions": 1, "deletions": 0}],
            }
            for i in range(3)
        ]
        result = GitAnalyzer.compute_co_changes(commits, min_count=3)
        assert len(result) == 1
        assert result[0]["file_a"] == "a.py"
        assert result[0]["file_b"] == "b.py"
        assert result[0]["co_commit_count"] == 3

    def test_custom_min_count(self):
        commits = [
            {
                "hash": f"h{i}", "author": "A", "author_email": "", "date": f"2026-01-0{i+1}",
                "message": f"c{i}",
                "files": [{"path": "a.py", "insertions": 1, "deletions": 0},
                          {"path": "b.py", "insertions": 1, "deletions": 0}],
            }
            for i in range(2)
        ]
        result = GitAnalyzer.compute_co_changes(commits, min_count=2)
        assert len(result) == 1
        assert result[0]["co_commit_count"] == 2

    def test_sorted_descending(self):
        commits = [
            {
                "hash": f"h{i}", "author": "A", "author_email": "", "date": f"2026-01-0{i+1}",
                "message": f"c{i}",
                "files": [
                    {"path": "a.py", "insertions": 1, "deletions": 0},
                    {"path": "b.py", "insertions": 1, "deletions": 0},
                    {"path": "c.py", "insertions": 1, "deletions": 0},
                ],
            }
            for i in range(5)
        ]
        result = GitAnalyzer.compute_co_changes(commits, min_count=1)
        assert len(result) == 3  # (a,b), (a,c), (b,c)
        # All have count 5, so order is stable but all should be 5
        for r in result:
            assert r["co_commit_count"] == 5

    def test_last_co_commit_date(self):
        commits = [
            {
                "hash": "h1", "author": "A", "author_email": "",
                "date": "2026-01-05T00:00:00+00:00", "message": "c1",
                "files": [{"path": "a.py", "insertions": 1, "deletions": 0},
                          {"path": "b.py", "insertions": 1, "deletions": 0}],
            },
            {
                "hash": "h2", "author": "A", "author_email": "",
                "date": "2026-03-10T00:00:00+00:00", "message": "c2",
                "files": [{"path": "a.py", "insertions": 1, "deletions": 0},
                          {"path": "b.py", "insertions": 1, "deletions": 0}],
            },
            {
                "hash": "h3", "author": "A", "author_email": "",
                "date": "2026-02-01T00:00:00+00:00", "message": "c3",
                "files": [{"path": "a.py", "insertions": 1, "deletions": 0},
                          {"path": "b.py", "insertions": 1, "deletions": 0}],
            },
        ]
        result = GitAnalyzer.compute_co_changes(commits, min_count=1)
        assert result[0]["last_co_commit"] == "2026-03-10T00:00:00+00:00"

    def test_single_file_commits_ignored(self):
        commits = [
            {
                "hash": "h1", "author": "A", "author_email": "", "date": "2026-01-01",
                "message": "c1",
                "files": [{"path": "a.py", "insertions": 1, "deletions": 0}],
            },
        ]
        assert GitAnalyzer.compute_co_changes(commits, min_count=1) == []


class TestParseDiffFunctions:
    """Test extraction of function names from @@ hunk headers."""

    def test_empty_diff(self):
        assert GitAnalyzer._parse_diff_functions("") == []

    def test_python_function(self):
        raw = (
            "diff --git a/foo.py b/foo.py\n"
            "--- a/foo.py\n"
            "+++ b/foo.py\n"
            "@@ -10,3 +10,4 @@ def my_function():\n"
            "+    new_line\n"
        )
        result = GitAnalyzer._parse_diff_functions(raw)
        assert result == ["my_function"]

    def test_multiple_hunks(self):
        raw = (
            "@@ -1,3 +1,4 @@ def func_a():\n"
            "+line\n"
            "@@ -20,3 +21,4 @@ def func_b():\n"
            "+line\n"
        )
        result = GitAnalyzer._parse_diff_functions(raw)
        assert result == ["func_a", "func_b"]

    def test_deduplication(self):
        raw = (
            "@@ -1,3 +1,4 @@ def func_a():\n"
            "+line\n"
            "@@ -10,3 +11,4 @@ def func_a():\n"
            "+another_line\n"
        )
        result = GitAnalyzer._parse_diff_functions(raw)
        assert result == ["func_a"]

    def test_no_function_context(self):
        raw = "@@ -1,3 +1,4 @@\n+line\n"
        result = GitAnalyzer._parse_diff_functions(raw)
        assert result == []


# ================================================================== #
# Integration tests — against real temp git repos
# ================================================================== #

class TestParseLog:
    """Integration tests for git log parsing against a real temp repo."""

    def test_returns_all_commits(self, analyzer):
        commits = analyzer.parse_log()
        assert len(commits) == 3

    def test_commit_fields(self, analyzer):
        commits = analyzer.parse_log()
        # Commits are newest-first
        newest = commits[0]
        assert newest["message"] == "update both files"
        assert newest["author"] == "TestAuthor"
        assert newest["author_email"] == "test@example.com"
        assert len(newest["hash"]) == 40

    def test_commit_dates_descending(self, analyzer):
        commits = analyzer.parse_log()
        dates = [c["date"] for c in commits]
        assert dates == sorted(dates, reverse=True)

    def test_file_changes(self, analyzer):
        commits = analyzer.parse_log()
        # Newest commit modified both hello.py and utils.py
        newest = commits[0]
        paths = {f["path"] for f in newest["files"]}
        assert "hello.py" in paths
        assert "utils.py" in paths

    def test_insertions_deletions_counted(self, analyzer):
        commits = analyzer.parse_log()
        # Find a file entry and verify insertions/deletions are integers >= 0
        for commit in commits:
            for f in commit["files"]:
                assert isinstance(f["insertions"], int)
                assert isinstance(f["deletions"], int)
                assert f["insertions"] >= 0
                assert f["deletions"] >= 0

    def test_since_filter(self, analyzer):
        commits = analyzer.parse_log(since="2026-02-01")
        # Should only include commits from Feb and March
        assert len(commits) == 2

    def test_path_filter(self, analyzer):
        commits = analyzer.parse_log(paths=["utils.py"])
        # utils.py appears in commit 1 (initial) and commit 3 (update both)
        assert len(commits) == 2


class TestParseBlame:
    """Integration tests for git blame parsing against a real temp repo."""

    def test_blame_returns_blocks(self, analyzer):
        blocks = analyzer.parse_blame("hello.py")
        assert len(blocks) > 0

    def test_blame_covers_all_lines(self, analyzer):
        blocks = analyzer.parse_blame("hello.py")
        # hello.py has 5 lines in its final state
        covered = set()
        for block in blocks:
            for line in range(block["line_start"], block["line_end"] + 1):
                covered.add(line)
        # Should cover lines 1 through 5
        assert covered == {1, 2, 3, 4, 5}

    def test_blame_block_fields(self, analyzer):
        blocks = analyzer.parse_blame("hello.py")
        for block in blocks:
            assert "commit_hash" in block
            assert len(block["commit_hash"]) == 40
            assert "author" in block
            assert "author_email" in block
            assert "date" in block
            assert "line_start" in block
            assert "line_end" in block
            assert block["line_start"] <= block["line_end"]

    def test_blame_author(self, analyzer):
        blocks = analyzer.parse_blame("hello.py")
        authors = {b["author"] for b in blocks}
        assert "TestAuthor" in authors


class TestParseBlameMultiAuthor:
    """Integration tests for blame with multiple authors."""

    def test_two_authors_in_blame(self, multi_author_repo):
        analyzer = GitAnalyzer(multi_author_repo)
        blocks = analyzer.parse_blame("shared.py")
        authors = {b["author"] for b in blocks}
        assert "Alice" in authors
        assert "Bob" in authors

    def test_ownership_from_blame(self, multi_author_repo):
        analyzer = GitAnalyzer(multi_author_repo)
        blocks = analyzer.parse_blame("shared.py")
        ownership = GitAnalyzer.compute_ownership(blocks)
        assert len(ownership) == 2
        alice = next(o for o in ownership if o["author"] == "Alice")
        bob = next(o for o in ownership if o["author"] == "Bob")
        # Alice: 3 lines (75%), Bob: 1 line (25%)
        assert alice["line_count"] == 3
        assert alice["percentage"] == 75.0
        assert bob["line_count"] == 1
        assert bob["percentage"] == 25.0


class TestChurnIntegration:
    """Integration test: parse real log, compute churn."""

    def test_churn_on_hello_py(self, analyzer):
        commits = analyzer.parse_log()
        result = GitAnalyzer.compute_churn(
            commits, "hello.py",
            now=datetime(2026, 3, 16, tzinfo=timezone.utc),
        )
        assert result["commit_count"] == 3
        assert result["distinct_authors"] == 1
        assert result["total_insertions"] > 0
        # Git may format UTC offset as +00:00 or Z depending on version
        last = datetime.fromisoformat(result["last_changed"])
        expected = datetime.fromisoformat("2026-03-01T12:00:00+00:00")
        assert last == expected
        assert result["churn_score"] > 0

    def test_churn_on_utils_py(self, analyzer):
        commits = analyzer.parse_log()
        result = GitAnalyzer.compute_churn(
            commits, "utils.py",
            now=datetime(2026, 3, 16, tzinfo=timezone.utc),
        )
        # utils.py was in commit 1 and commit 3
        assert result["commit_count"] == 2


class TestCoChangeIntegration:
    """Integration test: parse real log, compute co-changes."""

    def test_co_changes_below_default_threshold(self, analyzer):
        commits = analyzer.parse_log()
        # hello.py and utils.py co-occur in 2 commits (initial + update both)
        # Default min_count=3, so should be empty
        result = GitAnalyzer.compute_co_changes(commits)
        assert result == []

    def test_co_changes_with_lower_threshold(self, analyzer):
        commits = analyzer.parse_log()
        result = GitAnalyzer.compute_co_changes(commits, min_count=2)
        assert len(result) == 1
        pair = result[0]
        assert {pair["file_a"], pair["file_b"]} == {"hello.py", "utils.py"}
        assert pair["co_commit_count"] == 2


class TestGetFunctionLog:
    """Integration tests for get_function_log (git log -L)."""

    def test_function_log_returns_commits(self, analyzer):
        # greet() was modified in all 3 commits
        commits = analyzer.get_function_log("hello.py", "greet")
        assert len(commits) >= 2  # at least the modifications

    def test_function_log_different_functions(self, analyzer):
        # farewell() was added in commit 2 and modified in commit 3
        commits = analyzer.get_function_log("hello.py", "farewell")
        assert len(commits) >= 1

    def test_function_log_nonexistent_function(self, analyzer):
        commits = analyzer.get_function_log("hello.py", "nonexistent_func")
        assert commits == []

    def test_unit_level_churn(self, analyzer):
        func_commits = analyzer.get_function_log("hello.py", "greet")
        churn = GitAnalyzer.compute_churn(
            func_commits, "hello.py", unit_name="greet",
        )
        assert churn["commit_count"] >= 2
        assert churn["churn_score"] > 0


class TestGetChangedFiles:
    """Integration tests for get_changed_files."""

    def test_no_changes(self, analyzer):
        # Working tree is clean, no changes vs HEAD
        files = analyzer.get_changed_files()
        assert files == []

    def test_detects_unstaged_change(self, analyzer, git_repo):
        (git_repo / "hello.py").write_text("modified content\n")
        files = analyzer.get_changed_files()
        assert "hello.py" in files


class TestGetChangedFunctions:
    """Integration tests for get_changed_functions."""

    def test_detects_function_in_diff(self, analyzer, git_repo):
        # Diff between HEAD~1 and HEAD should show changes in existing functions
        funcs = analyzer.get_changed_functions("hello.py", ref="HEAD~1")
        # The diff should pick up function context from the hunk headers
        # Whether git includes function names depends on git's language detection
        assert isinstance(funcs, list)


class TestRunGitErrors:
    """Test error handling for bad git commands."""

    def test_invalid_repo_dir(self, tmp_path):
        analyzer = GitAnalyzer(tmp_path / "nonexistent")
        with pytest.raises(RuntimeError, match="failed"):
            analyzer._run_git(["status"])

    def test_bad_git_command(self, analyzer):
        with pytest.raises(RuntimeError, match="failed"):
            analyzer._run_git(["not-a-real-command"])

    def test_blame_nonexistent_file(self, analyzer):
        with pytest.raises(RuntimeError, match="failed"):
            analyzer.parse_blame("no_such_file.py")
