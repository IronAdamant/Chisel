"""Git analysis layer for Chisel — log parsing, blame, churn, ownership, co-change."""

import re
import subprocess
from collections import defaultdict
from datetime import datetime, timezone
from itertools import combinations


class GitAnalyzer:
    """Parses git log/blame output and computes churn, ownership, and co-change metrics.

    All git interaction uses subprocess.run — no gitpython dependency.
    """

    def __init__(self, repo_dir):
        self.repo_dir = str(repo_dir)

    def _run_git(self, args):
        """Run a git command in the repo directory and return stdout.

        Raises RuntimeError on non-zero exit code or if the repo directory
        does not exist.
        """
        try:
            result = subprocess.run(
                ["git"] + args,
                cwd=self.repo_dir,
                capture_output=True,
                text=True,
            )
        except OSError as exc:
            raise RuntimeError(
                f"git {' '.join(args)} failed: {exc}"
            ) from exc
        if result.returncode != 0:
            raise RuntimeError(
                f"git {' '.join(args)} failed (rc={result.returncode}): {result.stderr.strip()}"
            )
        return result.stdout

    # ------------------------------------------------------------------ #
    # git log parsing
    # ------------------------------------------------------------------ #

    _COMMIT_SEP = "COMMIT_START"
    _LOG_FORMAT = f"{_COMMIT_SEP}%n%H|%an|%ae|%aI|%s"

    def parse_log(self, since=None, paths=None):
        """Parse git log --numstat into a list of commit dicts.

        Each dict contains:
            hash, author, author_email, date, message,
            files: [{path, insertions, deletions}, ...]

        Args:
            since: ISO date string to limit history (e.g. '2026-01-01').
            paths: List of file paths to limit history to.

        Returns:
            List of commit dicts, newest first.
        """
        args = ["log", f"--format={self._LOG_FORMAT}", "--numstat"]
        if since:
            args.append(f"--since={since}")
        if paths:
            args.append("--")
            args.extend(paths)
        raw = self._run_git(args)
        return self._parse_log_output(raw)

    def _parse_log_output(self, raw):
        """Parse raw git log --numstat output into commit dicts."""
        commits = []
        blocks = raw.split(self._COMMIT_SEP)
        for block in blocks:
            block = block.strip()
            if not block:
                continue
            lines = block.split("\n")
            header = lines[0].strip()
            if not header:
                continue
            parts = header.split("|", 4)
            if len(parts) < 5:
                continue
            commit = {
                "hash": parts[0],
                "author": parts[1],
                "author_email": parts[2],
                "date": parts[3],
                "message": parts[4],
                "files": [],
            }
            for line in lines[1:]:
                line = line.strip()
                if not line:
                    continue
                file_parts = line.split("\t")
                if len(file_parts) != 3:
                    continue
                ins_str, del_str, path = file_parts
                # Binary files show '-' for insertions/deletions
                insertions = int(ins_str) if ins_str != "-" else 0
                deletions = int(del_str) if del_str != "-" else 0
                commit["files"].append({
                    "path": path,
                    "insertions": insertions,
                    "deletions": deletions,
                })
            commits.append(commit)
        return commits

    # ------------------------------------------------------------------ #
    # git blame parsing
    # ------------------------------------------------------------------ #

    def parse_blame(self, file_path):
        """Parse git blame --porcelain for a file.

        Returns a list of blame block dicts, each containing:
            commit_hash, author, author_email, date, line_start, line_end
        """
        raw = self._run_git(["blame", "--porcelain", file_path])
        return self._parse_blame_output(raw)

    @staticmethod
    def _parse_blame_output(raw):
        """Parse raw git blame --porcelain output into blame block dicts.

        In porcelain format every source line gets its own entry.  The first
        occurrence of a commit hash includes full header lines (author,
        author-mail, author-time, etc.).  Subsequent lines from the same
        commit only have the hash line and the tab-prefixed content line.

        We cache commit metadata by hash, emit one per-line entry, then
        merge adjacent lines from the same commit into contiguous blocks
        with ``line_start`` / ``line_end`` ranges.
        """
        per_line = []  # one entry per source line
        current = None
        # Cache: commit_hash -> {author, author_email, date}
        commit_info = {}

        commit_header_re = re.compile(
            r"^([0-9a-f]{40})\s+(\d+)\s+(\d+)(?:\s+(\d+))?"
        )

        for line in raw.split("\n"):
            m = commit_header_re.match(line)
            if m:
                commit_hash = m.group(1)
                final_line = int(m.group(3))
                # Each entry represents exactly one line in the final file.
                cached = commit_info.get(commit_hash, {})
                current = {
                    "commit_hash": commit_hash,
                    "author": cached.get("author", ""),
                    "author_email": cached.get("author_email", ""),
                    "date": cached.get("date", ""),
                    "line": final_line,
                }
            elif current is not None:
                if line.startswith("author "):
                    current["author"] = line[len("author "):]
                elif line.startswith("author-mail "):
                    email = line[len("author-mail "):]
                    current["author_email"] = email.strip("<>")
                elif line.startswith("author-time "):
                    timestamp = int(line[len("author-time "):])
                    current["date"] = (
                        datetime.fromtimestamp(timestamp, tz=timezone.utc).isoformat()
                    )
                elif line.startswith("\t"):
                    # Content line marks end of this entry.
                    h = current["commit_hash"]
                    if h not in commit_info:
                        commit_info[h] = {
                            "author": current["author"],
                            "author_email": current["author_email"],
                            "date": current["date"],
                        }
                    per_line.append(current)
                    current = None

        # Merge adjacent lines from the same commit into contiguous blocks.
        blocks = []
        for entry in per_line:
            if (
                blocks
                and blocks[-1]["commit_hash"] == entry["commit_hash"]
                and blocks[-1]["line_end"] + 1 == entry["line"]
            ):
                blocks[-1]["line_end"] = entry["line"]
            else:
                blocks.append({
                    "commit_hash": entry["commit_hash"],
                    "author": entry["author"],
                    "author_email": entry["author_email"],
                    "date": entry["date"],
                    "line_start": entry["line"],
                    "line_end": entry["line"],
                })
        return blocks

    # ------------------------------------------------------------------ #
    # churn computation
    # ------------------------------------------------------------------ #

    @staticmethod
    def compute_churn(commits, file_path, unit_name=None, now=None):
        """Compute churn metrics for a file (or function within a file).

        Churn score formula: sum(1 / (1 + days_since_commit))

        Args:
            commits: List of commit dicts (from parse_log).
            file_path: File path to compute churn for.
            unit_name: Optional function/class name to filter by (not yet
                       implemented — requires git log -L support).
            now: Optional datetime for testing (defaults to utcnow).

        Returns:
            Dict with: commit_count, distinct_authors, total_insertions,
                       total_deletions, last_changed, churn_score
        """
        if now is None:
            now = datetime.now(timezone.utc)

        matching = []
        for commit in commits:
            for f in commit.get("files", []):
                if f["path"] == file_path:
                    matching.append((commit, f))
                    break

        if not matching:
            return {
                "commit_count": 0,
                "distinct_authors": 0,
                "total_insertions": 0,
                "total_deletions": 0,
                "last_changed": None,
                "churn_score": 0.0,
            }

        authors = set()
        total_ins = 0
        total_del = 0
        churn_score = 0.0
        last_changed = None

        for commit, file_info in matching:
            authors.add(commit["author"])
            total_ins += file_info["insertions"]
            total_del += file_info["deletions"]

            commit_date = datetime.fromisoformat(commit["date"])
            # Ensure timezone-aware comparison
            if commit_date.tzinfo is None:
                commit_date = commit_date.replace(tzinfo=timezone.utc)
            days_since = max((now - commit_date).total_seconds() / 86400, 0)
            churn_score += 1.0 / (1.0 + days_since)

            if last_changed is None or commit["date"] > last_changed:
                last_changed = commit["date"]

        return {
            "commit_count": len(matching),
            "distinct_authors": len(authors),
            "total_insertions": total_ins,
            "total_deletions": total_del,
            "last_changed": last_changed,
            "churn_score": round(churn_score, 6),
        }

    # ------------------------------------------------------------------ #
    # ownership computation
    # ------------------------------------------------------------------ #

    @staticmethod
    def compute_ownership(blame_blocks):
        """Compute per-author ownership from blame blocks.

        Args:
            blame_blocks: List of blame block dicts (from parse_blame).

        Returns:
            List of dicts sorted by line_count desc, each containing:
                author, author_email, line_count, percentage
        """
        if not blame_blocks:
            return []

        author_lines = defaultdict(lambda: {"line_count": 0, "email": ""})
        total_lines = 0

        for block in blame_blocks:
            num_lines = block["line_end"] - block["line_start"] + 1
            author = block["author"]
            author_lines[author]["line_count"] += num_lines
            author_lines[author]["email"] = block.get("author_email", "")
            total_lines += num_lines

        result = []
        for author, info in author_lines.items():
            pct = (info["line_count"] / total_lines * 100) if total_lines > 0 else 0
            result.append({
                "author": author,
                "author_email": info["email"],
                "line_count": info["line_count"],
                "percentage": round(pct, 2),
            })

        result.sort(key=lambda x: x["line_count"], reverse=True)
        return result

    # ------------------------------------------------------------------ #
    # co-change computation
    # ------------------------------------------------------------------ #

    @staticmethod
    def compute_co_changes(commits, min_count=3):
        """Find file pairs that frequently appear in the same commits.

        Args:
            commits: List of commit dicts (from parse_log).
            min_count: Minimum co-occurrence count to include.

        Returns:
            List of dicts sorted by co_commit_count desc, each containing:
                file_a, file_b, co_commit_count, last_co_commit
        """
        pair_counts = defaultdict(int)
        pair_last = {}

        for commit in commits:
            paths = sorted({f["path"] for f in commit.get("files", [])})
            if len(paths) < 2:
                continue
            for a, b in combinations(paths, 2):
                pair_counts[(a, b)] += 1
                existing = pair_last.get((a, b))
                if existing is None or commit["date"] > existing:
                    pair_last[(a, b)] = commit["date"]

        result = []
        for (a, b), count in pair_counts.items():
            if count >= min_count:
                result.append({
                    "file_a": a,
                    "file_b": b,
                    "co_commit_count": count,
                    "last_co_commit": pair_last[(a, b)],
                })

        result.sort(key=lambda x: x["co_commit_count"], reverse=True)
        return result

    # ------------------------------------------------------------------ #
    # changed files / functions
    # ------------------------------------------------------------------ #

    def get_changed_files(self, ref="HEAD"):
        """Return list of files changed relative to the given ref.

        Uses git diff --name-only against the ref.
        """
        raw = self._run_git(["diff", "--name-only", ref])
        return [line.strip() for line in raw.strip().split("\n") if line.strip()]

    def get_changed_functions(self, file_path, ref="HEAD~1"):
        """Extract function names from diff hunk headers.

        Parses @@ ... @@ lines from git diff -U0 for function-level context.

        Args:
            file_path: Path to the file (relative to repo root).
            ref: Git ref to diff against (default HEAD~1).

        Returns:
            List of function name strings extracted from hunk headers.
        """
        raw = self._run_git(["diff", "-U0", ref, "--", file_path])
        return self._parse_diff_functions(raw)

    @staticmethod
    def _parse_diff_functions(raw):
        """Extract function names from @@ hunk headers in unified diff output.

        Hunk headers look like: @@ -a,b +c,d @@ optional_function_context
        We extract the text after the second @@.
        """
        func_re = re.compile(r"^@@\s+[^@]+\s+@@\s*(.+)$")
        functions = []
        seen = set()
        for line in raw.split("\n"):
            m = func_re.match(line)
            if m:
                func_name = m.group(1).strip()
                if func_name and func_name not in seen:
                    seen.add(func_name)
                    functions.append(func_name)
        return functions
