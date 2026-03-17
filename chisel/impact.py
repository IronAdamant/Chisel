"""Impact analysis, risk scoring, stale test detection, and reviewer suggestions."""

from collections import defaultdict
from datetime import datetime, timezone

from chisel.git_analyzer import GitAnalyzer, _parse_iso_date


class ImpactAnalyzer:
    """Analyzes test impact, risk, and ownership using data from Storage."""

    def __init__(self, storage):
        self.storage = storage

    # ------------------------------------------------------------------ #
    # Impacted tests
    # ------------------------------------------------------------------ #

    def get_impacted_tests(self, changed_files, changed_functions=None):
        """Find tests affected by the given file/function changes.

        Uses both direct test edges and transitive co-change coupling.

        Args:
            changed_files: List of changed file paths.
            changed_functions: Optional list of changed function names.

        Returns:
            List of dicts: {test_id, file_path, name, reason, score}
        """
        impacted = {}

        # Direct hits via single JOIN query per file
        for file_path in changed_files:
            hits = self.storage.get_direct_impacted_tests(
                file_path, changed_functions,
            )
            for hit in hits:
                new_score = hit["weight"]
                if hit["test_id"] not in impacted or new_score > impacted[hit["test_id"]]["score"]:
                    impacted[hit["test_id"]] = {
                        "test_id": hit["test_id"],
                        "file_path": hit["file_path"],
                        "name": hit["name"],
                        "reason": f"direct edge to {hit['code_name']} ({hit['edge_type']})",
                        "score": new_score,
                    }

        # Transitive hits via co-change coupling
        for file_path in changed_files:
            co_changes = self.storage.get_co_changes(file_path, min_count=3)
            for cc in co_changes:
                coupled_file = cc["file_b"] if cc["file_a"] == file_path else cc["file_a"]
                hits = self.storage.get_direct_impacted_tests(coupled_file)
                for hit in hits:
                    new_score = hit["weight"] * 0.5
                    if hit["test_id"] not in impacted or new_score > impacted[hit["test_id"]]["score"]:
                        impacted[hit["test_id"]] = {
                            "test_id": hit["test_id"],
                            "file_path": hit["file_path"],
                            "name": hit["name"],
                            "reason": (
                                f"co-change coupling: {file_path} <-> {coupled_file}"
                                f" ({cc['co_commit_count']} commits)"
                            ),
                            "score": new_score,
                        }

        result = list(impacted.values())
        result.sort(key=lambda x: x["score"], reverse=True)
        return result

    # ------------------------------------------------------------------ #
    # Risk scoring
    # ------------------------------------------------------------------ #

    def compute_risk_score(self, file_path, unit_name=None):
        """Compute a risk score for a file or function.

        Formula: 0.35*churn + 0.25*coupling + 0.2*coverage_gap
                 + 0.1*author_concentration + 0.1*test_instability

        Returns:
            Dict: {file_path, unit_name, risk_score, breakdown}
        """
        # Churn component (0-1 normalized, cap at 1.0)
        churn_stat = self.storage.get_churn_stat(file_path, unit_name)
        churn_raw = churn_stat["churn_score"] if churn_stat else 0.0
        churn_norm = min(churn_raw / 5.0, 1.0)  # normalize: 5.0 score => 1.0

        # Coupling breadth component (0-1)
        co_changes = self.storage.get_co_changes(file_path, min_count=3)
        coupling_norm = min(len(co_changes) / 10.0, 1.0)  # 10+ coupled files => 1.0

        # Test coverage component (0-1, inverted)
        code_units = self.storage.get_code_units_by_file(file_path)
        if unit_name:
            code_units = [cu for cu in code_units if cu["name"] == unit_name]
        tested_count = 0
        covering_test_ids = set()
        for cu in code_units:
            edges = self.storage.get_edges_for_code(cu["id"])
            if edges:
                tested_count += 1
                for e in edges:
                    covering_test_ids.add(e["test_id"])
        coverage = tested_count / max(len(code_units), 1)
        coverage_gap = 1.0 - coverage

        # Author concentration component (0-1)
        blame_data = self.storage.get_blame(file_path, _latest_hash(self.storage, file_path))
        author_conc = _author_concentration(blame_data)

        # Test instability component (0-1): avg failure rate of covering tests
        instability = _test_instability(self.storage, covering_test_ids)

        risk = (
            0.35 * churn_norm
            + 0.25 * coupling_norm
            + 0.2 * coverage_gap
            + 0.1 * author_conc
            + 0.1 * instability
        )
        return {
            "file_path": file_path,
            "unit_name": unit_name,
            "risk_score": round(risk, 4),
            "breakdown": {
                "churn": round(churn_norm, 4),
                "coupling": round(coupling_norm, 4),
                "coverage_gap": round(coverage_gap, 4),
                "author_concentration": round(author_conc, 4),
                "test_instability": round(instability, 4),
            },
        }

    # ------------------------------------------------------------------ #
    # Test suggestions
    # ------------------------------------------------------------------ #

    def suggest_tests(self, file_path):
        """Suggest tests to run for a changed file, ordered by relevance.

        Uses recorded test results to boost tests with higher failure rates.

        Returns:
            List of dicts: {test_id, file_path, name, relevance, reason}
        """
        impacted = self.get_impacted_tests([file_path])

        # Boost tests that have historically failed more often
        failure_rates = {}
        for row in self.storage.get_test_failure_rates():
            if row["total_runs"] > 0:
                failure_rates[row["test_id"]] = row["failures"] / row["total_runs"]

        result = []
        for item in impacted:
            relevance = item["score"]
            fail_rate = failure_rates.get(item["test_id"], 0.0)
            # Boost by up to 50% based on historical failure rate
            relevance *= (1.0 + 0.5 * fail_rate)
            result.append({
                "test_id": item["test_id"],
                "file_path": item["file_path"],
                "name": item["name"],
                "relevance": relevance,
                "reason": item["reason"],
            })

        result.sort(key=lambda x: x["relevance"], reverse=True)
        return result

    # ------------------------------------------------------------------ #
    # Stale test detection
    # ------------------------------------------------------------------ #

    def detect_stale_tests(self):
        """Find tests whose edges point to code units that no longer exist.

        Uses a single LEFT JOIN query instead of per-test lookups.

        Returns:
            List of dicts: {test_id, test_name, missing_code_id, edge_type}
        """
        rows = self.storage.get_stale_test_edges()
        return [
            {
                "test_id": r["test_id"],
                "test_name": r["test_name"],
                "missing_code_id": r["code_id"],
                "edge_type": r["edge_type"],
            }
            for r in rows
        ]

    # ------------------------------------------------------------------ #
    # Test gap detection
    # ------------------------------------------------------------------ #

    def get_test_gaps(self, file_path=None, directory=None, exclude_tests=True):
        """Find code units that have no test coverage, prioritized by churn.

        Args:
            file_path: Scope to a single file.
            directory: Scope to a directory (file_path takes precedence).
            exclude_tests: If True (default), exclude units from test files.

        Returns:
            List of dicts: {id, file_path, name, unit_type, line_start,
                            line_end, churn_score, commit_count}
        """
        return self.storage.get_untested_code_units(
            file_path=file_path,
            directory=directory if not file_path else None,
            exclude_tests=exclude_tests,
        )

    # ------------------------------------------------------------------ #
    # Risk map
    # ------------------------------------------------------------------ #

    def get_risk_map(self, directory=None):
        """Compute risk scores for all tracked files (optionally in a directory).

        Returns:
            List of dicts: {file_path, risk_score, breakdown}
        """
        all_churn = self.storage.get_all_churn_stats()
        dir_prefix = directory.rstrip("/") + "/" if directory else ""
        files = set()
        for stat in all_churn:
            if not directory or stat["file_path"].startswith(dir_prefix):
                files.add(stat["file_path"])

        risk_map = []
        for fp in sorted(files):
            risk = self.compute_risk_score(fp)
            risk_map.append(risk)

        risk_map.sort(key=lambda x: x["risk_score"], reverse=True)
        return risk_map

    # ------------------------------------------------------------------ #
    # Ownership (blame-based)
    # ------------------------------------------------------------------ #

    def get_ownership(self, file_path):
        """Get code ownership breakdown based on git blame.

        Shows who originally authored each portion of the file.
        Delegates to GitAnalyzer.compute_ownership for the core aggregation.

        Returns:
            List of dicts sorted by line_count desc:
            {author, author_email, line_count, percentage, role}
        """
        content_hash = _latest_hash(self.storage, file_path)
        blame_data = self.storage.get_blame(file_path, content_hash)
        if not blame_data:
            return []

        result = GitAnalyzer.compute_ownership(blame_data)
        for entry in result:
            entry["role"] = "original_author"
        return result

    # ------------------------------------------------------------------ #
    # Reviewer suggestions (commit-activity-based)
    # ------------------------------------------------------------------ #

    def suggest_reviewers(self, file_path):
        """Suggest reviewers based on recent commit activity for a file.

        Unlike ownership (which shows who wrote the code), this shows who
        has been actively maintaining/modifying the file recently and is
        best positioned to review changes.

        Returns:
            List of dicts sorted by activity score desc:
            {author, author_email, recent_commits, last_commit_date,
             days_since_last_commit, insertions, deletions, percentage, role}
        """
        commits = self.storage.get_commits_for_file(file_path)
        if not commits:
            return []

        now = datetime.now(timezone.utc)
        author_stats = defaultdict(lambda: {
            "commits": 0, "email": "", "insertions": 0, "deletions": 0,
            "last_date": "", "score": 0.0,
        })

        for commit in commits:
            author = commit["author"]
            info = author_stats[author]
            info["commits"] += 1
            info["email"] = commit.get("author_email", "")
            info["insertions"] += commit.get("insertions", 0)
            info["deletions"] += commit.get("deletions", 0)
            if not info["last_date"] or commit["date"] > info["last_date"]:
                info["last_date"] = commit["date"]
            # Weight by recency: recent commits count more
            try:
                cdate = _parse_iso_date(commit["date"])
                days = max((now - cdate).total_seconds() / 86400, 0)
                info["score"] += 1.0 / (1.0 + days)
            except (ValueError, TypeError):
                info["score"] += 0.01

        total_score = sum(info["score"] for info in author_stats.values())
        result = []
        for author, info in author_stats.items():
            days_since = None
            if info["last_date"]:
                try:
                    last = _parse_iso_date(info["last_date"])
                    days_since = round((now - last).total_seconds() / 86400)
                except (ValueError, TypeError):
                    pass
            pct = (info["score"] / total_score * 100) if total_score > 0 else 0
            result.append({
                "author": author,
                "author_email": info["email"],
                "recent_commits": info["commits"],
                "last_commit_date": info["last_date"],
                "days_since_last_commit": days_since,
                "insertions": info["insertions"],
                "deletions": info["deletions"],
                "percentage": round(pct, 2),
                "role": "suggested_reviewer",
            })
        result.sort(key=lambda x: x["percentage"], reverse=True)
        return result


# ------------------------------------------------------------------ #
# Internal helpers
# ------------------------------------------------------------------ #

def _latest_hash(storage, file_path):
    """Get the most recent content hash for a file from storage."""
    return storage.get_file_hash(file_path) or ""


def _author_concentration(blame_data):
    """Compute author concentration (0 = many authors, 1 = single author).

    Uses a simple Herfindahl index: sum of squared ownership fractions.
    """
    if not blame_data:
        return 1.0  # no data => assume concentrated

    lines_by_author = defaultdict(int)
    total = 0
    for block in blame_data:
        n = block["line_end"] - block["line_start"] + 1
        lines_by_author[block["author"]] += n
        total += n

    if total == 0:
        return 1.0

    hhi = sum((count / total) ** 2 for count in lines_by_author.values())
    return round(hhi, 4)


def _test_instability(storage, test_ids):
    """Compute average failure rate for a set of tests (0 = stable, 1 = always fails).

    Returns 0.0 if no recorded results exist.
    """
    if not test_ids:
        return 0.0
    failure_rates = {
        r["test_id"]: r["failures"] / r["total_runs"]
        for r in storage.get_test_failure_rates()
        if r["total_runs"] > 0
    }
    rates = [failure_rates[tid] for tid in test_ids if tid in failure_rates]
    if not rates:
        return 0.0
    return sum(rates) / len(rates)
