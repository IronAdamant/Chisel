"""Git analysis layer for Chisel — log parsing, blame, changed files/functions."""

import re
import subprocess
from datetime import datetime, timezone


_BLAME_HEADER_RE = re.compile(
    r"^([0-9a-f]{40})\s+\d+\s+(\d+)(?:\s+\d+)?"
)

_HUNK_RE = re.compile(r"^@@\s+[^@]+\s+@@\s*(.+)$")
# Patterns to extract the bare name from common declaration styles.
# Covers Python (def/async def), JS/TS (function/async function),
# Go (func), Rust (fn), Kotlin (fun), plus a fallback for return-type
# declarations in C#/Java/C++/Dart/Swift.
_FUNC_NAME_RE = re.compile(
    r"(?:"
    r"(?:def|func|fn|fun|function|async\s+def|async\s+function)"
    r"\s+(?:\([^)]*\)\s+)?(\w+)"
    r"|"
    r"(?:(?:public|private|protected|internal|static|virtual|override|abstract"
    r"|async|final|synchronized|open|sealed|inline)\s+)*"
    r"(?:\w+(?:<[^>]*>)?(?:\[\])*\??\s+)(\w+)\s*\("
    r")"
)


class GitAnalyzer:
    """Parses git log/blame output and extracts changed files/functions.

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
                timeout=120,
            )
        except subprocess.TimeoutExpired as exc:
            raise RuntimeError(
                f"git {' '.join(args)} timed out after 120s"
            ) from exc
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
        """Parse raw git log output into commit dicts.

        Handles both ``--numstat`` output (tab-separated stats) and
        inline diff output from ``git log -L`` (counts ``+``/``-`` lines).
        """
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
            # Count diff +/- lines as fallback for git log -L output
            diff_ins = 0
            diff_del = 0
            has_numstat = False
            for raw_line in lines[1:]:
                line = raw_line.strip()
                if not line:
                    continue
                file_parts = line.split("\t")
                if len(file_parts) == 3:
                    ins_str, del_str, path = file_parts
                    # Validate numstat format: first two fields must be digits or '-'
                    if not (ins_str == "-" or ins_str.isdigit()) or \
                       not (del_str == "-" or del_str.isdigit()):
                        # Not numstat — likely a diff line with tabs
                        if not has_numstat:
                            if line.startswith("+") and not line.startswith("+++"):
                                diff_ins += 1
                            elif line.startswith("-") and not line.startswith("---"):
                                diff_del += 1
                        continue
                    insertions = int(ins_str) if ins_str != "-" else 0
                    deletions = int(del_str) if del_str != "-" else 0
                    commit["files"].append({
                        "path": path,
                        "insertions": insertions,
                        "deletions": deletions,
                    })
                    has_numstat = True
                elif not has_numstat:
                    # Count unified diff +/- lines (skip --- and +++ headers)
                    if line.startswith("+") and not line.startswith("+++"):
                        diff_ins += 1
                    elif line.startswith("-") and not line.startswith("---"):
                        diff_del += 1
            # Store diff-counted stats when numstat wasn't available
            if not has_numstat and (diff_ins or diff_del):
                commit["_diff_insertions"] = diff_ins
                commit["_diff_deletions"] = diff_del
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

        for line in raw.split("\n"):
            if m := _BLAME_HEADER_RE.match(line):
                commit_hash = m.group(1)
                final_line = int(m.group(2))
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
    # Function-level log
    # ------------------------------------------------------------------ #

    def get_function_log(self, file_path, func_name):
        """Get commits that touched a specific function using git log -L.

        Args:
            file_path: Path to the file (relative to repo root).
            func_name: Function/method name to search for.

        Returns:
            List of commit dicts (same format as parse_log).
        """
        try:
            raw = self._run_git([
                "log", f"--format={self._LOG_FORMAT}",
                f"-L:{func_name}:{file_path}",
            ])
        except RuntimeError:
            return []
        return self._parse_log_output(raw)

    # ------------------------------------------------------------------ #
    # Branch and diff queries
    # ------------------------------------------------------------------ #

    def get_current_branch(self):
        """Return the name of the currently checked-out branch."""
        return self._run_git(["rev-parse", "--abbrev-ref", "HEAD"]).strip()

    def branch_exists(self, name):
        """Return True if a branch with the given name exists."""
        try:
            self._run_git(["rev-parse", "--verify", name])
            return True
        except RuntimeError:
            return False

    def get_changed_files(self, ref="HEAD"):
        """Return list of files changed relative to the given ref.

        Uses git diff --name-only against the ref.
        """
        raw = self._run_git(["diff", "--name-only", ref])
        return [s for line in raw.strip().splitlines() if (s := line.strip())]

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
        We extract the text after the second @@ and parse out just the
        bare function/method name (e.g. ``def foo(...)`` -> ``foo``).
        """
        functions = []
        seen = set()
        for line in raw.split("\n"):
            if m := _HUNK_RE.match(line):
                context = m.group(1).strip()
                # Only extract recognized function declarations
                nm = _FUNC_NAME_RE.search(context)
                if not nm:
                    continue
                func_name = nm.group(1) or nm.group(2)
                if func_name and func_name not in seen:
                    seen.add(func_name)
                    functions.append(func_name)
        return functions
