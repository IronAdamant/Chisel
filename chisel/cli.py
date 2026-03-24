"""Chisel CLI — command-line interface for all Chisel tool methods."""

import argparse
import json
import os

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
                               help="Suggest tests for a file")
    p_suggest.add_argument("file", help="File path")

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

    # stats
    sub.add_parser("stats", parents=[shared],
                   help="Show database summary counts")

    # triage
    p_triage = sub.add_parser("triage", parents=[shared],
                               help="Combined risk + gap + stale triage")
    p_triage.add_argument("directory", nargs="?", default=None,
                           help="Directory to scope (default: all)")
    p_triage.add_argument("--top-n", type=int, default=10,
                           help="Number of top-risk files (default: 10)")

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

    return parser


# ------------------------------------------------------------------ #
# Output helpers
# ------------------------------------------------------------------ #

def _print_json(data):
    """Print data as formatted JSON."""
    print(json.dumps(data, indent=2, default=str))


def _limit(result, args):
    """Apply --limit to list results."""
    if args.limit is not None and isinstance(result, list):
        return result[:args.limit]
    return result


def _is_no_data(result):
    """Check if *result* is a status response (no-data, no-changes, etc.)."""
    return isinstance(result, dict) and result.get("status") in ("no_data", "no_changes")


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
    return _run_tool(args, "tool_suggest_tests", {"file_path": args.file},
                     _fmt_list("No test suggestions.", "Suggested tests:",
                               lambda i: f"{i['name']}  (score: {i['relevance']})"))


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
    return _run_tool(args, "tool_coupling",
                     {"file_path": args.file, "min_count": args.min_count},
                     _fmt_list("No coupling data.",
                               lambda r, a: f"Co-change coupling for {a.file}:",
                               lambda i: f"{i['file_b']}  ({i['co_commit_count']} co-commits)"))


def cmd_risk_map(args):
    return _run_tool(args, "tool_risk_map", {"directory": args.directory},
                     _fmt_list("No risk data.", "Risk map:",
                               lambda i: f"{i['file_path']}: {i['risk_score']}"))


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
    return _run_tool(args, "tool_diff_impact", {"ref": args.ref},
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
         "exclude_tests": not args.no_exclude_tests},
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
                     {"directory": args.directory, "top_n": args.top_n},
                     fmt, use_limit=False)


def cmd_stats(args):
    return _run_tool(args, "tool_stats", {},
                     _fmt_kv("Chisel database stats:"), use_limit=False)


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
    "triage": cmd_triage,
    "stats": cmd_stats,
    "serve": cmd_serve,
    "serve-mcp": cmd_serve_mcp,
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
