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
        parents=[shared],
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

    # record-result
    p_record = sub.add_parser("record-result", parents=[shared],
                              help="Record a test result (pass/fail)")
    p_record.add_argument("test_id", help="Test ID")
    p_record.add_argument("--passed", action="store_true", default=False,
                          help="Mark test as passed")
    p_record.add_argument("--failed", action="store_true", default=False,
                          help="Mark test as failed")
    p_record.add_argument("--duration", type=int, default=None,
                          help="Duration in milliseconds")

    # stats
    sub.add_parser("stats", parents=[shared],
                   help="Show database summary counts")

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



# ------------------------------------------------------------------ #
# Command handlers
# ------------------------------------------------------------------ #

def cmd_analyze(args):
    """Handle the 'analyze' subcommand."""
    with ChiselEngine(args.project_dir, storage_dir=args.storage_dir) as engine:
        result = engine.tool_analyze(directory=args.directory, force=args.force)
        if args.json_output:
            _print_json(result)
        else:
            print("Analysis complete:")
            for key, value in result.items():
                label = key.replace("_", " ").title()
                print(f"  {label}: {value}")
        return result


def cmd_impact(args):
    """Handle the 'impact' subcommand."""
    with ChiselEngine(args.project_dir, storage_dir=args.storage_dir) as engine:
        result = _limit(engine.tool_impact(args.files), args)
        if args.json_output:
            _print_json(result)
        else:
            if not result:
                print("No impacted tests found.")
            else:
                print("Impacted tests:")
                for item in result:
                    print(f"  {item['test_id']}  ({item['reason']})")
        return result


def cmd_suggest_tests(args):
    """Handle the 'suggest-tests' subcommand."""
    with ChiselEngine(args.project_dir, storage_dir=args.storage_dir) as engine:
        result = _limit(engine.tool_suggest_tests(args.file), args)
        if args.json_output:
            _print_json(result)
        else:
            if not result:
                print("No test suggestions.")
            else:
                print("Suggested tests:")
                for item in result:
                    print(f"  {item['name']}  (score: {item['relevance']})")
        return result


def cmd_churn(args):
    """Handle the 'churn' subcommand."""
    with ChiselEngine(args.project_dir, storage_dir=args.storage_dir) as engine:
        result = _limit(engine.tool_churn(args.file, unit_name=args.unit), args)
        if args.json_output:
            _print_json(result)
        else:
            if not result:
                print("No churn data available.")
            else:
                print(f"Churn stats for {args.file}:")
                for item in result:
                    for key, value in item.items():
                        print(f"  {key}: {value}")
                    print()
        return result


def cmd_ownership(args):
    """Handle the 'ownership' subcommand."""
    with ChiselEngine(args.project_dir, storage_dir=args.storage_dir) as engine:
        result = _limit(engine.tool_ownership(args.file), args)
        if args.json_output:
            _print_json(result)
        else:
            if not result:
                print("No ownership data.")
            else:
                print(f"Ownership for {args.file}:")
                for item in result:
                    print(f"  {item['author']}: {item['percentage']}")
        return result


def cmd_coupling(args):
    """Handle the 'coupling' subcommand."""
    with ChiselEngine(args.project_dir, storage_dir=args.storage_dir) as engine:
        result = _limit(engine.tool_coupling(args.file, min_count=args.min_count), args)
        if args.json_output:
            _print_json(result)
        else:
            if not result:
                print("No coupling data.")
            else:
                print(f"Co-change coupling for {args.file}:")
                for item in result:
                    print(f"  {item['file_b']}  ({item['co_commit_count']} co-commits)")
        return result


def cmd_risk_map(args):
    """Handle the 'risk-map' subcommand."""
    with ChiselEngine(args.project_dir, storage_dir=args.storage_dir) as engine:
        result = _limit(engine.tool_risk_map(directory=args.directory), args)
        if args.json_output:
            _print_json(result)
        else:
            if not result:
                print("No risk data.")
            else:
                print("Risk map:")
                for item in result:
                    print(f"  {item['file_path']}: {item['risk_score']}")
        return result


def cmd_stale_tests(args):
    """Handle the 'stale-tests' subcommand."""
    with ChiselEngine(args.project_dir, storage_dir=args.storage_dir) as engine:
        result = _limit(engine.tool_stale_tests(), args)
        if args.json_output:
            _print_json(result)
        else:
            if not result:
                print("No stale tests found.")
            else:
                print("Stale tests:")
                for item in result:
                    print(f"  {item['test_id']}  ({item['edge_type']})")
        return result


def cmd_history(args):
    """Handle the 'history' subcommand."""
    with ChiselEngine(args.project_dir, storage_dir=args.storage_dir) as engine:
        result = _limit(engine.tool_history(args.file), args)
        if args.json_output:
            _print_json(result)
        else:
            if not result:
                print("No history found.")
            else:
                print(f"History for {args.file}:")
                for item in result:
                    short = item["hash"][:8]
                    print(f"  {short}  {item['date']}  {item['author']}  {item['message']}")
        return result


def cmd_who_reviews(args):
    """Handle the 'who-reviews' subcommand."""
    with ChiselEngine(args.project_dir, storage_dir=args.storage_dir) as engine:
        result = _limit(engine.tool_who_reviews(args.file), args)
        if args.json_output:
            _print_json(result)
        else:
            if not result:
                print("No reviewer suggestions.")
            else:
                print(f"Suggested reviewers for {args.file}:")
                for item in result:
                    days = item.get("days_since_last_commit", "?")
                    print(f"  {item['author']}: {item['percentage']}% ({item['recent_commits']} commits, {days}d ago)")
        return result


def cmd_diff_impact(args):
    """Handle the 'diff-impact' subcommand."""
    with ChiselEngine(args.project_dir, storage_dir=args.storage_dir) as engine:
        result = _limit(engine.tool_diff_impact(ref=args.ref), args)
        if args.json_output:
            _print_json(result)
        else:
            if not result:
                print("No impacted tests (or no changes detected).")
            else:
                print("Impacted tests from diff:")
                for item in result:
                    print(f"  {item['test_id']}  ({item['reason']})")
        return result


def cmd_update(args):
    """Handle the 'update' subcommand."""
    with ChiselEngine(args.project_dir, storage_dir=args.storage_dir) as engine:
        result = engine.tool_update()
        if args.json_output:
            _print_json(result)
        else:
            print("Incremental update complete:")
            for key, value in result.items():
                label = key.replace("_", " ").title()
                print(f"  {label}: {value}")
        return result


def cmd_test_gaps(args):
    """Handle the 'test-gaps' subcommand."""
    with ChiselEngine(args.project_dir, storage_dir=args.storage_dir) as engine:
        result = _limit(engine.tool_test_gaps(file_path=args.file, directory=args.directory), args)
        if args.json_output:
            _print_json(result)
        else:
            if not result:
                print("No untested code units found.")
            else:
                print(f"Untested code units ({len(result)}):")
                for item in result:
                    churn = item.get("churn_score", 0)
                    print(f"  {item['file_path']}:{item['name']} "
                          f"({item['unit_type']}, lines {item['line_start']}-{item['line_end']}"
                          f", churn: {churn})")
        return result


def cmd_record_result(args):
    """Handle the 'record-result' subcommand."""
    passed = args.passed or not args.failed  # default to passed if neither flag
    with ChiselEngine(args.project_dir, storage_dir=args.storage_dir) as engine:
        result = engine.tool_record_result(
            args.test_id, passed, duration_ms=args.duration,
        )
        if args.json_output:
            _print_json(result)
        else:
            status = "PASSED" if passed else "FAILED"
            print(f"Recorded: {args.test_id} — {status}")
        return result


def cmd_stats(args):
    """Handle the 'stats' subcommand."""
    with ChiselEngine(args.project_dir, storage_dir=args.storage_dir) as engine:
        result = engine.tool_stats()
        if args.json_output:
            _print_json(result)
        else:
            print("Chisel database stats:")
            for table, count in result.items():
                label = table.replace("_", " ").title()
                print(f"  {label}: {count}")
        return result


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
    server.start(blocking=True)


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
