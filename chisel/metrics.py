"""Pure computation functions for churn, ownership, and co-change metrics.

Stateless functions extracted from GitAnalyzer to keep git_analyzer.py
focused on git subprocess interaction.
"""

from collections import defaultdict
from datetime import datetime, timezone
from itertools import combinations


def _parse_iso_date(date_str):
    """Parse an ISO 8601 date string into a timezone-aware datetime.

    Handles the ``Z`` suffix (not supported by ``fromisoformat`` before
    Python 3.11) and ensures the result is always timezone-aware (defaults
    to UTC when no timezone info is present).
    """
    if date_str.endswith("Z"):
        date_str = date_str[:-1] + "+00:00"
    dt = datetime.fromisoformat(date_str)
    if dt.tzinfo is None:
        dt = dt.replace(tzinfo=timezone.utc)
    return dt


def compute_churn(commits, file_path, unit_name=None, now=None):
    """Compute churn metrics for a file (or function within a file).

    Churn score formula: sum(1 / (1 + days_since_commit))

    When unit_name is provided and commits were obtained via
    get_function_log(), each commit is counted directly (no file
    path filtering needed since git log -L already scoped them).

    Args:
        commits: List of commit dicts (from parse_log or get_function_log).
        file_path: File path to compute churn for.
        unit_name: Optional function/class name. When set, all provided
                   commits are assumed to be relevant (pre-filtered).
        now: Optional datetime for testing (defaults to utcnow).

    Returns:
        Dict with: commit_count, distinct_authors, total_insertions,
                   total_deletions, last_changed, churn_score
    """
    if now is None:
        now = datetime.now(timezone.utc)

    # When unit_name is provided, commits are pre-filtered by git log -L.
    # Use all commits directly instead of filtering by file path.
    if unit_name:
        matching = []
        for c in commits:
            file_stats = {"insertions": 0, "deletions": 0}
            # Try numstat first, then fall back to diff-counted stats
            for f in c.get("files", []):
                if f["path"] == file_path:
                    file_stats = f
                    break
            else:
                file_stats = {
                    "insertions": c.get("_diff_insertions", 0),
                    "deletions": c.get("_diff_deletions", 0),
                }
            matching.append((c, file_stats))
    else:
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
    last_changed_date = None
    analyzed_count = 0

    for commit, file_info in matching:
        try:
            commit_date = _parse_iso_date(commit["date"])
        except (ValueError, TypeError):
            continue

        analyzed_count += 1
        authors.add(commit["author"])
        total_ins += file_info["insertions"]
        total_del += file_info["deletions"]

        days_since = max((now - commit_date).total_seconds() / 86400, 0)
        churn_score += 1.0 / (1.0 + days_since)

        if last_changed_date is None or commit_date > last_changed_date:
            last_changed_date = commit_date
            last_changed = commit["date"]

    return {
        "commit_count": analyzed_count,
        "distinct_authors": len(authors),
        "total_insertions": total_ins,
        "total_deletions": total_del,
        "last_changed": last_changed,
        "churn_score": round(churn_score, 6),
    }


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


_MAX_CO_CHANGE_FILES = 50


def compute_co_changes(commits, min_count=3):
    """Find file pairs that frequently appear in the same commits.

    Commits touching more than 50 files are skipped — bulk operations
    (renames, formatting, dependency bumps) are not meaningful coupling signals.

    Args:
        commits: List of commit dicts (from parse_log).
        min_count: Minimum co-occurrence count to include.

    Returns:
        List of dicts sorted by co_commit_count desc, each containing:
            file_a, file_b, co_commit_count, last_co_commit
    """
    pair_counts = defaultdict(int)
    pair_last = {}  # (file_a, file_b) -> (date_str, parsed_datetime)

    for commit in commits:
        paths = sorted({f["path"] for f in commit.get("files", [])})
        if len(paths) < 2 or len(paths) > _MAX_CO_CHANGE_FILES:
            continue
        try:
            commit_dt = _parse_iso_date(commit["date"])
        except (ValueError, TypeError):
            commit_dt = None
        for a, b in combinations(paths, 2):
            pair_counts[(a, b)] += 1
            existing = pair_last.get((a, b))
            if commit_dt and (existing is None or commit_dt > existing[1]):
                pair_last[(a, b)] = (commit["date"], commit_dt)

    result = []
    for (a, b), count in pair_counts.items():
        if count >= min_count:
            result.append({
                "file_a": a,
                "file_b": b,
                "co_commit_count": count,
                "last_co_commit": pair_last.get((a, b), (None,))[0],
            })

    result.sort(key=lambda x: x["co_commit_count"], reverse=True)
    return result
