"""Chisel CLI — command-line interface for all Chisel tool methods."""

import argparse
import json
import os
import re
import subprocess
import sys
import tempfile

from chisel.engine import ChiselEngine


# ------------------------------------------------------------------ #
# Parser
# ------------------------------------------------------------------ #

def create_parser():
    """Build the argparse parser with all subcommands and global flags."""
    # Shared flags inherited by every subcommand so they work in any position
    shared = argparse.ArgumentParser(add_help=False)
    shared.add_argument(
        "--project-dir", default=os.getcwd(),
        help="Project root directory (default: current directory)",
    )
    shared.add_argument(
        "--storage-dir", default=None,
        help="Storage directory for Chisel data (default: auto)",
    )
    shared.add_argument(
        "--json", action="store_true", dest="json_output",
        help="Output results as JSON",
    )
    shared.add_argument(
        "--limit", type=int, default=None,
        help="Maximum number of results to return",
    )

    parser = argparse.ArgumentParser(
        prog="chisel",
        description="Chisel — test impact analysis and code intelligence",
    )

    sub = parser.add_subparsers(dest="command")

    # analyze
    p_analyze = sub.add_parser("analyze", parents=[shared],
                               help="Run full project analysis")
    p_analyze.add_argument("directory", nargs="?", default=".",
                           help="Directory to analyze (default: .)")
    p_analyze.add_argument("--force", action="store_true",
                           help="Force full re-analysis")

    # impact
    p_impact = sub.add_parser("impact", parents=[shared],
                              help="Show impacted tests for files")
    p_impact.add_argument("files", nargs="+", help="File paths to check")
    p_impact.add_argument("--functions", nargs="*", default=None,
                          help="Optional function names to scope impact")

    # suggest-tests
    p_suggest = sub.add_parser("suggest-tests", parents=[shared],
                               help="Suggest tests for a file or directory")
    p_suggest.add_argument("file", nargs="?", default=None,
                           help="File path (optional if --directory is given)")
    p_suggest.add_argument("--directory", default=None,
                           help="Directory path; returns suggestions for all code files under it")
    p_suggest.add_argument("--fallback", action="store_true",
                           help="Also return all test files if no edges found")
    p_suggest.add_argument("--working-tree", action="store_true",
                           help="Include untracked files on disk in analysis")

    # churn
    p_churn = sub.add_parser("churn", parents=[shared],
                             help="Show churn stats for a file")
    p_churn.add_argument("file", help="File path")
    p_churn.add_argument("--unit", default=None,
                         help="Specific code unit name")

    # ownership
    p_ownership = sub.add_parser("ownership", parents=[shared],
                                 help="Show ownership breakdown for a file")
    p_ownership.add_argument("file", help="File path")

    # coupling
    p_coupling = sub.add_parser("coupling", parents=[shared],
                                help="Show co-change coupling for a file")
    p_coupling.add_argument("file", help="File path")
    p_coupling.add_argument("--min-count", type=int, default=3,
                            help="Minimum co-change count (default: 3)")

    # risk-map
    p_risk = sub.add_parser("risk-map", parents=[shared],
                            help="Show risk scores for all files")
    p_risk.add_argument("directory", nargs="?", default=None,
                        help="Directory to scope (default: all)")
    p_risk.add_argument("--no-exclude-tests", action="store_true", default=False,
                        help="Include test files in risk map")
    p_risk.add_argument("--proximity", action="store_true", default=False,
                        help="Adjust coverage_gap by import distance to tested code")
    p_risk.add_argument("--coverage-mode", choices=["unit", "line"],
                        default="unit",
                        help="Coverage mode: 'unit' weights units equally, 'line' weights by line count")

    # stale-tests
    sub.add_parser("stale-tests", parents=[shared], help="Detect stale tests")

    # history
    p_history = sub.add_parser("history", parents=[shared],
                               help="Show commit history for a file")
    p_history.add_argument("file", help="File path")

    # who-reviews
    p_who = sub.add_parser("who-reviews", parents=[shared],
                           help="Suggest reviewers for a file")
    p_who.add_argument("file", help="File path")

    # diff-impact
    p_diff = sub.add_parser("diff-impact", parents=[shared],
                            help="Auto-detect changes and show impacted tests")
    p_diff.add_argument("--ref", default=None,
                        help="Git ref to diff against (default: auto-detect)")
    p_diff.add_argument("--working-tree", action="store_true", default=False,
                        help="Full static import scan for untracked files")

    # update
    sub.add_parser("update", parents=[shared],
                   help="Incremental re-analysis of changed files")

    # test-gaps
    p_gaps = sub.add_parser("test-gaps", parents=[shared],
                            help="Find code units with no test coverage")
    p_gaps.add_argument("file", nargs="?", default=None,
                        help="Scope to a single file")
    p_gaps.add_argument("--directory", default=None,
                        help="Scope to a directory")
    p_gaps.add_argument("--no-exclude-tests", action="store_true", default=False,
                        help="Include test file units in results")
    p_gaps.add_argument("--working-tree", action="store_true",
                        help="Include untracked files on disk as gaps with churn=0")

    # record-result
    p_record = sub.add_parser("record-result", parents=[shared],
                              help="Record a test result (pass/fail)")
    p_record.add_argument("test_id", help="Test ID")
    result_group = p_record.add_mutually_exclusive_group(required=True)
    result_group.add_argument("--passed", action="store_true", default=False,
                              help="Mark test as passed")
    result_group.add_argument("--failed", action="store_true", default=False,
                              help="Mark test as failed")
    p_record.add_argument("--duration", type=int, default=None,
                          help="Duration in milliseconds")

    # run
    p_run = sub.add_parser("run", parents=[shared],
                           help="Run tests and automatically record results")
    p_run.add_argument("command", nargs="+", help="Test command to execute")

    # stats
    sub.add_parser("stats", parents=[shared],
                   help="Show database summary counts")

    # start-job (background analyze/update — same as MCP start_job)
    p_sjob = sub.add_parser("start-job", parents=[shared],
                            help="Run analyze or update in a background thread")
    p_sjob.add_argument(
        "kind", choices=["analyze", "update"],
        help="analyze or update",
    )
    p_sjob.add_argument(
        "directory", nargs="?", default=None,
        help="Subdirectory to analyze (analyze only)",
    )
    p_sjob.add_argument(
        "--force", action="store_true",
        help="Force full re-analysis (analyze only)",
    )

    p_jstat = sub.add_parser("job-status", parents=[shared],
                             help="Poll a background job from start-job")
    p_jstat.add_argument("job_id", help="Job id returned by start-job")

    p_jcancel = sub.add_parser("cancel-job", parents=[shared],
                               help="Request cancellation of a background job")
    p_jcancel.add_argument("job_id", help="Job id returned by start-job")

    # triage
    p_triage = sub.add_parser("triage", parents=[shared],
                               help="Combined risk + gap + stale triage")
    p_triage.add_argument("directory", nargs="?", default=None,
                           help="Directory to scope (default: all)")
    p_triage.add_argument("--top-n", type=int, default=10,
                           help="Number of top-risk files (default: 10)")
    p_triage.add_argument("--no-exclude-tests", action="store_true", default=False,
                           help="Include test files in risk ranking")

    # serve
    p_serve = sub.add_parser("serve", parents=[shared],
                             help="Start HTTP server")
    p_serve.add_argument("--port", type=int, default=8377,
                         help="Port number (default: 8377)")
    p_serve.add_argument("--host", default="127.0.0.1",
                         help="Host to bind to (default: 127.0.0.1)")

    # serve-mcp
    sub.add_parser("serve-mcp", parents=[shared],
                   help="Start MCP server (stdio mode)")

    # --- file_locks ---
    p_acq = sub.add_parser("acquire-lock", parents=[shared],
                           help="Acquire advisory lock on a file")
    p_acq.add_argument("file", help="File path")
    p_acq.add_argument("agent_id", help="Agent identifier")
    p_acq.add_argument("--ttl", type=int, default=300,
                       help="Lock TTL in seconds (default: 300)")
    p_acq.add_argument("--purpose", help="Purpose or description")

    p_rel = sub.add_parser("release-lock", parents=[shared],
                           help="Release advisory lock held by this agent")
    p_rel.add_argument("file", help="File path")
    p_rel.add_argument("agent_id", help="Agent identifier")

    p_ref = sub.add_parser("refresh-lock", parents=[shared],
                           help="Refresh/advisory lock TTL")
    p_ref.add_argument("file", help="File path")
    p_ref.add_argument("agent_id", help="Agent identifier")
    p_ref.add_argument("--ttl", type=int, default=300,
                        help="New TTL in seconds (default: 300)")

    sub.add_parser("check-lock", parents=[shared],
                   help="Check if a file is locked").add_argument("file", help="File path")

    p_chk = sub.add_parser("check-locks", parents=[shared],
                            help="Batch-check lock status for multiple files")
    p_chk.add_argument("files", nargs="+", help="File paths to check")

    p_lst = sub.add_parser("list-locks", parents=[shared],
                            help="List all active file locks")
    p_lst.add_argument("--agent-id", help="Filter by agent (optional)")

    return parser


# ------------------------------------------------------------------ #
# Output helpers
# ------------------------------------------------------------------ #

def _print_json(data):
    """Print data as formatted JSON."""
    print(json.dumps(data, indent=2, default=str))


def _limit(result, args):
    """Apply --limit to list results or dict-wrapped file lists."""
    if args.limit is not None:
        if isinstance(result, list):
            return result[:args.limit]
        if isinstance(result, dict) and isinstance(result.get("files"), list):
            return {**result, "files": result["files"][:args.limit]}
    return result


def _is_no_data(result):
    """Check if *result* is a status response (no-data, no-changes, etc.)."""
    return isinstance(result, dict) and result.get("status") in (
        "no_data", "no_changes", "no_edges", "git_error",
    )


def _run_tool(args, method, kwargs, formatter, use_limit=True):
    """Execute an engine tool method with standard lifecycle and output handling."""
    with ChiselEngine(args.project_dir, storage_dir=args.storage_dir) as engine:
        result = getattr(engine, method)(**kwargs)
        if use_limit:
            result = _limit(result, args)
        if args.json_output:
            _print_json(result)
        elif _is_no_data(result):
            print(result["message"])
            if result.get("hint"):
                print(result["hint"])
            if result.get("error"):
                print(f"Error: {result['error']}")
            cwd = result.get("cwd") or result.get("project_dir")
            if cwd:
                print(f"Directory: {cwd}")
        else:
            formatter(result, args)
        return result


def _fmt_kv(header):
    """Create a formatter that prints dict results as labeled key-value pairs."""
    def fmt(result, _args):
        print(header)
        for key, value in result.items():
            print(f"  {key.replace('_', ' ').title()}: {value}")
    return fmt


def _fmt_list(empty_msg, header, line_fn):
    """Create a formatter for list results with a header and per-item line.

    Args:
        empty_msg: Message to print when the result list is empty.
        header: Static string or callable(result, args) -> string.
        line_fn: Callable(item) -> string for each result item.
    """
    def fmt(result, args):
        if not result:
            print(empty_msg)
        else:
            print(header(result, args) if callable(header) else header)
            for item in result:
                print(f"  {line_fn(item)}")
    return fmt


# ------------------------------------------------------------------ #
# Command handlers
# ------------------------------------------------------------------ #

def cmd_analyze(args):
    return _run_tool(args, "tool_analyze",
                     {"directory": args.directory, "force": args.force},
                     _fmt_kv("Analysis complete:"), use_limit=False)


def cmd_impact(args):
    kwargs = {"files": args.files}
    if args.functions:
        kwargs["functions"] = args.functions
    return _run_tool(args, "tool_impact", kwargs,
                     _fmt_list("No impacted tests found.", "Impacted tests:",
                               lambda i: f"{i['test_id']}  ({i['reason']})"))


def cmd_suggest_tests(args):
    kwargs = {"fallback_to_all": args.fallback, "working_tree": args.working_tree}
    if args.directory:
        kwargs["directory"] = args.directory
    else:
        kwargs["file_path"] = args.file

    def fmt(result, args):
        if not result:
            print("No test suggestions.")
            return
        if isinstance(result, dict):
            for fp, items in result.items():
                print(f"\n{fp}:")
                for i in items:
                    print(f"  {i['name']}  (score: {i['relevance']})")
        else:
            for i in result:
                print(f"{i['name']}  (score: {i['relevance']})")

    return _run_tool(args, "tool_suggest_tests", kwargs, fmt)


def cmd_churn(args):
    def fmt(result, args):
        if not result:
            print("No churn data available.")
        else:
            print(f"Churn stats for {args.file}:")
            for item in result:
                for key, value in item.items():
                    print(f"  {key}: {value}")
                print()
    return _run_tool(args, "tool_churn",
                     {"file_path": args.file, "unit_name": args.unit}, fmt)


def cmd_ownership(args):
    return _run_tool(args, "tool_ownership", {"file_path": args.file},
                     _fmt_list("No ownership data.",
                               lambda r, a: f"Ownership for {a.file}:",
                               lambda i: f"{i['author']}: {i['percentage']}"))


def cmd_coupling(args):
    def fmt(result, _args):
        cc = result.get("co_change_partners", [])
        imp = result.get("import_partners", [])
        if not cc and not imp:
            print("No coupling data.")
            return
        if cc:
            print(f"Co-change partners for {args.file}:")
            for p in cc:
                print(f"  {p['file_b']}  ({p['co_commit_count']} co-commits)")
        if imp:
            if cc:
                print()
            print(f"Import partners for {args.file}:")
            for p in imp:
                print(f"  {p['file']}")
    return _run_tool(args, "tool_coupling",
                     {"file_path": args.file, "min_count": args.min_count},
                     fmt)


def cmd_risk_map(args):
    def fmt(result, _args):
        if isinstance(result, dict) and "files" in result:
            files = result["files"]
            meta = result.get("_meta", {})
        else:
            files = result if isinstance(result, list) else []
            meta = {}
        if not files:
            print("No risk data.")
            return
        print("Risk map:")
        for item in files:
            print(f"  {item['file_path']}: {item['risk_score']}")
        uniform = meta.get("uniform_components", {})
        if uniform:
            print("\nDiagnostics (uniform components — not differentiating):")
            for comp, info in uniform.items():
                print(f"  {comp}: {info['reason']}")
    return _run_tool(args, "tool_risk_map",
                     {"directory": args.directory,
                      "exclude_tests": not args.no_exclude_tests,
                      "proximity_adjustment": args.proximity,
                      "coverage_mode": args.coverage_mode}, fmt)


def cmd_stale_tests(args):
    return _run_tool(args, "tool_stale_tests", {},
                     _fmt_list("No stale tests found.", "Stale tests:",
                               lambda i: f"{i['test_id']}  ({i['edge_type']})"))


def cmd_history(args):
    return _run_tool(args, "tool_history", {"file_path": args.file},
                     _fmt_list("No history found.",
                               lambda r, a: f"History for {a.file}:",
                               lambda i: f"{i['hash'][:8]}  {i['date']}  {i['author']}  {i['message']}"))


def cmd_who_reviews(args):
    return _run_tool(
        args, "tool_who_reviews", {"file_path": args.file},
        _fmt_list(
            "No reviewer suggestions.",
            lambda r, a: f"Suggested reviewers for {a.file}:",
            lambda i: (f"{i['author']}: {i['percentage']}% "
                       f"({i['recent_commits']} commits, "
                       f"{i.get('days_since_last_commit', '?')}d ago)"),
        ))


def cmd_diff_impact(args):
    return _run_tool(args, "tool_diff_impact", {"ref": args.ref, "working_tree": args.working_tree},
                     _fmt_list("No impacted tests (or no changes detected).",
                               "Impacted tests from diff:",
                               lambda i: f"{i['test_id']}  ({i['reason']})"))


def cmd_update(args):
    return _run_tool(args, "tool_update", {},
                     _fmt_kv("Incremental update complete:"), use_limit=False)


def cmd_test_gaps(args):
    return _run_tool(
        args, "tool_test_gaps",
        {"file_path": args.file, "directory": args.directory,
         "exclude_tests": not args.no_exclude_tests,
         "working_tree": args.working_tree},
        _fmt_list(
            "No untested code units found.",
            lambda r, a: f"Untested code units ({len(r)}):",
            lambda i: (f"{i['file_path']}:{i['name']} "
                       f"({i['unit_type']}, lines {i['line_start']}-{i['line_end']}"
                       f", churn: {i.get('churn_score', 0)})")),
    )


def cmd_record_result(args):
    def fmt(_result, args):
        status = "PASSED" if not args.failed else "FAILED"
        print(f"Recorded: {args.test_id} — {status}")
    return _run_tool(args, "tool_record_result",
                     {"test_id": args.test_id, "passed": not args.failed,
                      "duration_ms": args.duration},
                     fmt, use_limit=False)


def cmd_triage(args):
    def fmt(result, _args):
        summary = result["summary"]
        print(f"Triage ({summary['files_triaged']} files):")
        print("\nTop risk files:")
        for r in result["top_risk_files"]:
            partners = ""
            cp = r.get("coupling_partners", [])
            if cp:
                names = [p["file"] for p in cp[:2]]
                partners = f"  coupled: {', '.join(names)}"
            print(f"  {r['file_path']}: {r['risk_score']}{partners}")
        if result["test_gaps"]:
            print(f"\nTest gaps ({summary['total_test_gaps']}):")
            for g in result["test_gaps"]:
                print(f"  {g['file_path']}:{g['name']} ({g['unit_type']})")
        else:
            print("\nNo test gaps in triaged files.")
        if result["stale_tests"]:
            print(f"\nStale tests ({summary['total_stale_tests']}):")
            for s in result["stale_tests"]:
                print(f"  {s['test_id']}  ({s['edge_type']})")
        else:
            print("\nNo stale tests found.")
    return _run_tool(args, "tool_triage",
                     {"directory": args.directory, "top_n": args.top_n,
                      "exclude_tests": not args.no_exclude_tests},
                     fmt, use_limit=False)


def cmd_stats(args):
    return _run_tool(args, "tool_stats", {},
                     _fmt_kv("Chisel database stats:"), use_limit=False)


def cmd_start_job(args):
    kwargs = {"kind": args.kind, "force": args.force}
    if args.kind == "analyze":
        kwargs["directory"] = args.directory
    return _run_tool(args, "tool_start_job", kwargs,
                     _fmt_kv("Background job:"), use_limit=False)


def cmd_job_status(args):
    return _run_tool(args, "tool_job_status", {"job_id": args.job_id},
                     _fmt_kv("Job status:"), use_limit=False)


def cmd_cancel_job(args):
    return _run_tool(args, "tool_cancel_job", {"job_id": args.job_id},
                     _fmt_kv("Cancel job:"), use_limit=False)


# --- file_locks ---

def cmd_acquire_lock(args):
    def fmt(result, _args):
        if result["acquired"]:
            print(f"Locked: {args.file}")
            print(f"Agent:  {args.agent_id}")
            print(f"TTL:    {result['expires_at']}")
        else:
            print(f"FAILED: {args.file} is already locked by {result['holder']}")
            print(f"Expires: {result['expires_at']}")
    return _run_tool(args, "tool_acquire_file_lock",
                     {"file_path": args.file, "agent_id": args.agent_id,
                      "ttl": args.ttl, "purpose": args.purpose},
                     fmt, use_limit=False)


def cmd_release_lock(args):
    def fmt(result, _args):
        if result["released"]:
            print(f"Released: {args.file}")
        else:
            print(f"Not held: {args.file} is not locked by {args.agent_id}")
    return _run_tool(args, "tool_release_file_lock",
                     {"file_path": args.file, "agent_id": args.agent_id},
                     fmt, use_limit=False)


def cmd_refresh_lock(args):
    def fmt(result, _args):
        if result["refreshed"]:
            print(f"Refreshed: {args.file}")
            print(f"New TTL:   {result['expires_at']}")
        else:
            print(f"Not held: {args.file} is not locked by {args.agent_id}")
    return _run_tool(args, "tool_refresh_file_lock",
                     {"file_path": args.file, "agent_id": args.agent_id,
                      "ttl": args.ttl},
                     fmt, use_limit=False)


def cmd_check_lock(args):
    def fmt(result, _args):
        if not result["locked"]:
            print(f"Unlocked: {args.file}")
        else:
            print(f"Locked:   {args.file}")
            print(f"Holder:   {result['holder']}")
            print(f"TTL rem:  {result['ttl_remaining']}s")
            print(f"Stale:    {result['stale']}")
            if result.get("purpose"):
                print(f"Purpose:  {result['purpose']}")
    return _run_tool(args, "tool_check_file_lock", {"file_path": args.file},
                     fmt, use_limit=False)


def cmd_check_locks(args):
    def fmt(result, _args):
        if not result["conflicts"]:
            print(f"No locks on {len(result['checked'])} checked file(s)")
        else:
            print(f"{len(result['conflicts'])} lock(s) found:")
            for c in result["conflicts"]:
                stale = " (STALE)" if c["stale"] else ""
                print(f"  {c['file_path']}: {c['holder']} ({c['ttl_remaining']}s remaining){stale}")
    return _run_tool(args, "tool_check_locks", {"file_paths": args.files},
                     fmt, use_limit=False)


def cmd_list_locks(args):
    def fmt(result, _args):
        locks = result["locks"]
        if not locks:
            print("No active locks")
        else:
            print(f"{result['total']} active lock(s):")
            for lock in locks:
                stale = " (STALE)" if lock["ttl_remaining"] < 60 else ""
                print(f"  {lock['file_path']}: {lock['agent_id']} "
                      f"({lock['ttl_remaining']}s remaining){stale}")
    agent_filter = getattr(args, "agent_id", None)
    return _run_tool(args, "tool_list_file_locks",
                     {"agent_id": agent_filter},
                     fmt, use_limit=False)


def cmd_serve(args):
    """Handle the 'serve' subcommand."""
    from chisel.mcp_server import ChiselMCPServer
    server = ChiselMCPServer(
        project_dir=args.project_dir,
        storage_dir=args.storage_dir,
        host=args.host,
        port=args.port,
    )
    print(f"Starting HTTP server on {server.get_url()}")
    try:
        server.start(blocking=True)
    finally:
        server.stop()


def cmd_serve_mcp(args):
    """Handle the 'serve-mcp' subcommand."""
    os.environ["CHISEL_PROJECT_DIR"] = args.project_dir
    if args.storage_dir:
        os.environ["CHISEL_STORAGE_DIR"] = args.storage_dir
    from chisel.mcp_stdio import main as mcp_main
    mcp_main()


# ------------------------------------------------------------------ #
# Test-run helpers (chisel run)
# ------------------------------------------------------------------ #

def _detect_test_framework(command):
    """Detect test framework from the command list."""
    if not command:
        return None
    first = command[0].lower()
    if first == "pytest" or first.endswith("/pytest"):
        return "pytest"
    if first in ("jest", "npx"):
        return "jest"
    if first == "go":
        return "go"
    if first == "cargo":
        return "rust"
    return None


# Pytest verbose output: "tests/test_app.py::test_foo PASSED"
_PYTEST_RESULT_RE = re.compile(
    r"^(.+?)\s+(PASSED|FAILED|ERROR|SKIPPED|XFAIL|XPASS)$"
)


def _parse_pytest_output(lines):
    """Parse pytest -v output and return list of (test_id, passed)."""
    results = []
    for line in lines:
        line = line.rstrip("\n")
        m = _PYTEST_RESULT_RE.match(line)
        if m:
            test_id = m.group(1).strip()
            status = m.group(2)
            passed = status in ("PASSED", "XFAIL")
            results.append((test_id, passed))
    return results


def _parse_jest_json(path, project_dir):
    """Parse Jest JSON output and return list of (test_id, passed)."""
    try:
        with open(path, "r", encoding="utf-8") as fh:
            data = json.load(fh)
    except (OSError, json.JSONDecodeError):
        return []

    results = []
    for suite in data.get("testResults", []):
        suite_path = suite.get("name", "")
        # Make path relative to project dir
        rel_path = os.path.relpath(suite_path, project_dir).replace("\\", "/")
        for assertion in suite.get("assertionResults", []):
            title = assertion.get("title", "")
            status = assertion.get("status", "")
            passed = status == "passed"
            test_id = f"{rel_path}:{title}"
            results.append((test_id, passed))
    return results


def _augment_command(command, framework):
    """Add framework-specific flags to produce machine-readable output."""
    augmented = list(command)
    if framework == "pytest":
        if "-v" not in augmented and "--verbose" not in augmented:
            augmented.append("-v")
    elif framework == "jest":
        if "--json" not in augmented:
            augmented.append("--json")
    elif framework == "go":
        if "-json" not in augmented:
            augmented.append("-json")
    elif framework == "rust":
        if "--message-format=json" not in augmented:
            augmented.append("--message-format=json")
    return augmented


def _collect_run_results(framework, stdout_lines, temp_files, project_dir):
    """Collect (test_id, passed) tuples from test output."""
    if framework == "pytest":
        return _parse_pytest_output(stdout_lines)
    if framework == "jest" and temp_files:
        return _parse_jest_json(temp_files[0], project_dir)
    return []


def cmd_run(args):
    """Run a test command and record results automatically."""
    framework = _detect_test_framework(args.command)
    if framework is None:
        print("Warning: could not detect test framework; results will not be recorded.")
        print(f"Running: {' '.join(args.command)}")
        proc = subprocess.run(args.command)
        return proc.returncode

    augmented = _augment_command(args.command, framework)
    temp_files = []
    if framework == "jest":
        # Jest needs a temp file for JSON output
        fd, tmp_path = tempfile.mkstemp(suffix=".json", prefix="chisel-jest-")
        os.close(fd)
        temp_files.append(tmp_path)
        if "--outputFile" not in augmented and "-o" not in augmented:
            augmented.extend(["--outputFile", tmp_path])

    proc = subprocess.Popen(
        augmented,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
    )
    stdout_lines = []
    for raw in proc.stdout:
        text = raw.decode("utf-8", errors="replace")
        stdout_lines.append(text)
        sys.stdout.write(text)
    proc.wait()
    exit_code = proc.returncode

    try:
        results = _collect_run_results(
            framework, stdout_lines, temp_files, args.project_dir,
        )
        if results:
            with ChiselEngine(args.project_dir, storage_dir=args.storage_dir) as engine:
                recorded = 0
                for test_id, passed in results:
                    engine.tool_record_result(test_id, passed=passed)
                    recorded += 1
            print(f"\n[chisel] Recorded {recorded} test result(s)")
        else:
            print("\n[chisel] No test results parsed from output")
    finally:
        for tmp in temp_files:
            try:
                os.remove(tmp)
            except OSError:
                pass

    return exit_code


# ------------------------------------------------------------------ #
# Dispatch table
# ------------------------------------------------------------------ #

_COMMANDS = {
    "analyze": cmd_analyze,
    "impact": cmd_impact,
    "suggest-tests": cmd_suggest_tests,
    "churn": cmd_churn,
    "ownership": cmd_ownership,
    "coupling": cmd_coupling,
    "risk-map": cmd_risk_map,
    "stale-tests": cmd_stale_tests,
    "history": cmd_history,
    "who-reviews": cmd_who_reviews,
    "diff-impact": cmd_diff_impact,
    "update": cmd_update,
    "test-gaps": cmd_test_gaps,
    "record-result": cmd_record_result,
    "run": cmd_run,
    "triage": cmd_triage,
    "stats": cmd_stats,
    "start-job": cmd_start_job,
    "job-status": cmd_job_status,
    "cancel-job": cmd_cancel_job,
    "serve": cmd_serve,
    "serve-mcp": cmd_serve_mcp,
    # --- file_locks ---
    "acquire-lock": cmd_acquire_lock,
    "release-lock": cmd_release_lock,
    "refresh-lock": cmd_refresh_lock,
    "check-lock": cmd_check_lock,
    "check-locks": cmd_check_locks,
    "list-locks": cmd_list_locks,
}


# ------------------------------------------------------------------ #
# Main entry point
# ------------------------------------------------------------------ #

def main(argv=None):
    """Parse arguments and dispatch to the appropriate handler.

    Args:
        argv: Command-line arguments (default: sys.argv[1:]).

    Returns:
        The result from the command handler, or None.
    """
    parser = create_parser()
    args = parser.parse_args(argv)

    if not args.command:
        parser.print_help()
        return None

    handler = _COMMANDS[args.command]
    return handler(args)


if __name__ == "__main__":
    main()
