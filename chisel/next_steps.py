"""Contextual next-step suggestions for MCP tool responses.

Computes follow-up tool suggestions based on what a tool returned,
so LLM agents know what to invoke next. Only used by MCP servers
(HTTP and stdio), not the CLI.
"""


def compute_next_steps(tool_name, result):
    """Return a list of next-step suggestion strings for a tool result.

    Args:
        tool_name: Name of the tool that produced the result.
        result: The tool's return value (dict or list).

    Returns:
        List of strings, each a brief actionable suggestion. Empty list
        if no suggestions apply.
    """
    fn = _TOOL_HINTS.get(tool_name)
    if fn is None:
        return []
    return fn(result)


# ------------------------------------------------------------------ #
# Per-tool hint functions
# ------------------------------------------------------------------ #

def _hints_analyze(result):
    if isinstance(result, dict) and "code_files_scanned" in result:
        return [
            "Run 'risk_map' to identify high-risk files.",
            "Run 'test_gaps' to find untested code.",
            "Run 'triage' for a combined risk + gap + stale overview.",
        ]
    return []


def _hints_update(result):
    if isinstance(result, dict) and result.get("files_updated", 0) > 0:
        return [
            "Run 'diff_impact' to see which tests are affected by the changes.",
            "Run 'risk_map' to check updated risk scores.",
        ]
    return []


def _hints_risk_map(result):
    if isinstance(result, list) and result:
        top = result[:3]
        files = [r["file_path"] for r in top]
        steps = [
            "Run 'test_gaps' to find missing test coverage for high-risk files.",
        ]
        # Suggest coupling drilldown for files with high coupling scores
        high_coupling = [
            r["file_path"] for r in top
            if r.get("breakdown", {}).get("coupling", 0) > 0.3
        ]
        if high_coupling:
            steps.append(
                f"Run 'coupling {high_coupling[0]}' to see co-change partners."
            )
        # Suggest churn drilldown for high-churn files
        high_churn = [
            r["file_path"] for r in top
            if r.get("breakdown", {}).get("churn", 0) > 0.5
        ]
        if high_churn:
            steps.append(
                f"Run 'churn {high_churn[0]}' for detailed change history."
            )
        steps.append(
            f"Run 'suggest_tests {files[0]}' for test recommendations on the riskiest file."
        )
        return steps
    if isinstance(result, list):
        return ["Run 'analyze' to populate risk data."]
    return []


def _hints_diff_impact(result):
    if isinstance(result, list) and result:
        return [
            "Run the listed tests to verify your changes.",
            "Use 'record_result' to log outcomes for future prioritization.",
            "Run 'coupling' on changed files to check for hidden dependents.",
        ]
    if isinstance(result, list):
        return [
            "Run 'test_gaps' to check if new code needs tests.",
            "Run 'update' if you've made changes since last analysis.",
        ]
    return []


def _hints_test_gaps(result):
    if isinstance(result, list) and result:
        top_file = result[0]["file_path"]
        return [
            "Write tests for the highest-churn untested units first.",
            f"Run 'churn {top_file}' to see change frequency.",
            f"Run 'ownership {top_file}' to find who can help write tests.",
        ]
    if isinstance(result, list):
        return ["All code units have test coverage."]
    return []


def _hints_stale_tests(result):
    if isinstance(result, list) and result:
        return [
            "Update or remove the stale tests listed above.",
            "Run 'update' to re-analyze after fixing test files.",
        ]
    if isinstance(result, list):
        return ["All tests reference current code."]
    return []


def _hints_impact(result):
    if isinstance(result, list) and result:
        return [
            "Run the impacted tests to verify correctness.",
            "Use 'record_result' to log outcomes for future prioritization.",
        ]
    return []


def _hints_suggest_tests(result):
    if isinstance(result, list) and result:
        return [
            "Run the suggested tests in order of relevance.",
            "Use 'record_result' to log outcomes for future prioritization.",
        ]
    return []


def _hints_triage(result):
    if isinstance(result, dict) and "summary" in result:
        steps = []
        if result["summary"].get("total_test_gaps", 0) > 0:
            steps.append(
                "Focus on files appearing in both risk and gap sections."
            )
        if result["top_risk_files"]:
            top = result["top_risk_files"][0]["file_path"]
            steps.append(f"Run 'suggest_tests {top}' on the highest-risk file.")
            steps.append(f"Run 'ownership {top}' to find who owns the riskiest code.")
        return steps
    return []


# ------------------------------------------------------------------ #
# Dispatch table
# ------------------------------------------------------------------ #

_TOOL_HINTS = {
    "analyze": _hints_analyze,
    "update": _hints_update,
    "risk_map": _hints_risk_map,
    "diff_impact": _hints_diff_impact,
    "test_gaps": _hints_test_gaps,
    "stale_tests": _hints_stale_tests,
    "impact": _hints_impact,
    "suggest_tests": _hints_suggest_tests,
    "triage": _hints_triage,
}
