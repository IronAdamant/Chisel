"""Contextual next-step suggestions for MCP tool responses.

Computes follow-up tool suggestions based on what a tool returned,
so LLM agents can directly invoke the suggested next tool. Only used
by MCP servers (HTTP and stdio), not the CLI.

Each suggestion is a dict with:
    - tool: Chisel tool name to invoke (omitted for non-tool actions)
    - args: Arguments dict for the tool call (may be partial — LLM
            fills remaining required args from context)
    - action: Descriptive action label (only for non-tool suggestions)
    - reason: Why this step is recommended
"""


def compute_next_steps(tool_name, result):
    """Return a list of structured next-step suggestions for a tool result.

    Args:
        tool_name: Name of the tool that produced the result.
        result: The tool's return value (dict or list).

    Returns:
        List of dicts, each a structured suggestion with ``tool``/``args``
        or ``action`` plus ``reason``. Empty list if no suggestions apply.
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
            {"tool": "risk_map", "args": {}, "reason": "Identify high-risk files"},
            {"tool": "test_gaps", "args": {}, "reason": "Find untested code"},
            {"tool": "triage", "args": {}, "reason": "Combined risk + gap + stale overview"},
        ]
    return []


def _hints_update(result):
    if isinstance(result, dict) and result.get("files_updated", 0) > 0:
        return [
            {"tool": "diff_impact", "args": {}, "reason": "See which tests are affected by changes"},
            {"tool": "risk_map", "args": {}, "reason": "Check updated risk scores"},
        ]
    return []


def _hints_risk_map(result):
    if isinstance(result, list) and result:
        top = result[:3]
        files = [r["file_path"] for r in top]
        steps = [
            {"tool": "test_gaps", "args": {}, "reason": "Find missing coverage for high-risk files"},
        ]
        high_coupling = [
            r["file_path"] for r in top
            if r.get("breakdown", {}).get("coupling", 0) > 0.3
        ]
        if high_coupling:
            steps.append(
                {"tool": "coupling", "args": {"file_path": high_coupling[0]}, "reason": "Investigate co-change partners"}
            )
        high_churn = [
            r["file_path"] for r in top
            if r.get("breakdown", {}).get("churn", 0) > 0.5
        ]
        if high_churn:
            steps.append(
                {"tool": "churn", "args": {"file_path": high_churn[0]}, "reason": "Detailed change history"}
            )
        steps.append(
            {"tool": "suggest_tests", "args": {"file_path": files[0]}, "reason": "Test recommendations for riskiest file"}
        )
        return steps
    if isinstance(result, list):
        return [{"tool": "analyze", "args": {}, "reason": "Populate risk data"}]
    return []


def _hints_diff_impact(result):
    # Diagnostic dict when no changes detected
    if isinstance(result, dict) and result.get("status") == "no_changes":
        return [
            {"tool": "diff_impact", "args": {"ref": "HEAD~1"}, "reason": "Try diffing against previous commit"},
            {"tool": "update", "args": {}, "reason": "Re-analyze if working tree has new files"},
        ]
    if isinstance(result, list) and result:
        return [
            {"action": "run_tests", "reason": "Execute impacted tests to verify changes"},
            {"tool": "record_result", "args": {}, "reason": "Log outcomes for future prioritization"},
            {"tool": "coupling", "args": {}, "reason": "Check changed files for hidden dependents"},
        ]
    if isinstance(result, list):
        return [
            {"tool": "test_gaps", "args": {}, "reason": "Check if new code needs tests"},
            {"tool": "update", "args": {}, "reason": "Re-analyze if changes were made since last analysis"},
        ]
    return []


def _hints_test_gaps(result):
    if isinstance(result, list) and result:
        top_file = result[0]["file_path"]
        return [
            {"action": "write_tests", "reason": "Prioritize highest-churn untested units"},
            {"tool": "churn", "args": {"file_path": top_file}, "reason": "Check change frequency"},
            {"tool": "ownership", "args": {"file_path": top_file}, "reason": "Find who can help write tests"},
        ]
    if isinstance(result, list):
        return [{"action": "complete", "reason": "All code units have test coverage"}]
    return []


def _hints_stale_tests(result):
    if isinstance(result, list) and result:
        return [
            {"action": "fix_tests", "reason": "Update or remove stale tests listed above"},
            {"tool": "update", "args": {}, "reason": "Re-analyze after fixing test files"},
        ]
    if isinstance(result, list):
        return [{"action": "complete", "reason": "All tests reference current code"}]
    return []


def _hints_impact(result):
    if isinstance(result, list) and result:
        return [
            {"action": "run_tests", "reason": "Execute impacted tests to verify correctness"},
            {"tool": "record_result", "args": {}, "reason": "Log outcomes for future prioritization"},
        ]
    return []


def _hints_suggest_tests(result):
    if isinstance(result, list) and result:
        return [
            {"action": "run_tests", "reason": "Execute suggested tests in order of relevance"},
            {"tool": "record_result", "args": {}, "reason": "Log outcomes for future prioritization"},
        ]
    return []


def _hints_triage(result):
    if isinstance(result, dict) and "summary" in result:
        steps = []
        if result["summary"].get("total_test_gaps", 0) > 0:
            steps.append(
                {"action": "prioritize", "reason": "Focus on files appearing in both risk and gap sections"}
            )
        if result["top_risk_files"]:
            top = result["top_risk_files"][0]["file_path"]
            steps.append({"tool": "suggest_tests", "args": {"file_path": top}, "reason": "Test recommendations for riskiest file"})
            steps.append({"tool": "ownership", "args": {"file_path": top}, "reason": "Identify who owns the riskiest code"})
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
