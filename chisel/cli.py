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


def _print_result(data):
    """Print a dict or list of dicts as key: value lines."""
    if not data:
        print("No results.")
        return
    if isinstance(data, dict):
        for key, value in data.items():
            print(f"{key}: {value}")
    elif isinstance(data, list):
        for item in data:
            if isinstance(item, dict):
                for key, value in item.items():
                    print(f"  {key}: {value}")
                print()
            else:
                print(f"  {item}")


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
        result = engine.tool_impact(args.files)
        if args.json_output:
            _print_json(result)
        else:
            if not result:
                print("No impacted tests found.")
            else:
                print("Impacted tests:")
                for item in result:
                    test_id = item.get("test_id", item.get("id", "unknown"))
                    reason = item.get("reason", item.get("edge_type", ""))
                    print(f"  {test_id}  ({reason})")
        return result


def cmd_suggest_tests(args):
    """Handle the 'suggest-tests' subcommand."""
    with ChiselEngine(args.project_dir, storage_dir=args.storage_dir) as engine:
        result = engine.tool_suggest_tests(args.file)
        if args.json_output:
            _print_json(result)
        else:
            if not result:
                print("No test suggestions.")
            else:
                print("Suggested tests:")
                for item in result:
                    name = item.get("name", item.get("test_id", "unknown"))
                    score = item.get("relevance", item.get("score", ""))
                    print(f"  {name}  (score: {score})")
        return result


def cmd_churn(args):
    """Handle the 'churn' subcommand."""
    with ChiselEngine(args.project_dir, storage_dir=args.storage_dir) as engine:
        result = engine.tool_churn(args.file, unit_name=args.unit)
        if args.json_output:
            _print_json(result)
        else:
            if not result:
                print("No churn data available.")
            else:
                print(f"Churn stats for {args.file}:")
                _print_result(result)
        return result


def cmd_ownership(args):
    """Handle the 'ownership' subcommand."""
    with ChiselEngine(args.project_dir, storage_dir=args.storage_dir) as engine:
        result = engine.tool_ownership(args.file)
        if args.json_output:
            _print_json(result)
        else:
            if not result:
                print("No ownership data.")
            else:
                print(f"Ownership for {args.file}:")
                for item in result:
                    author = item.get("author", "unknown")
                    pct = item.get("percentage", item.get("line_count", ""))
                    print(f"  {author}: {pct}")
        return result


def cmd_coupling(args):
    """Handle the 'coupling' subcommand."""
    with ChiselEngine(args.project_dir, storage_dir=args.storage_dir) as engine:
        result = engine.tool_coupling(args.file, min_count=args.min_count)
        if args.json_output:
            _print_json(result)
        else:
            if not result:
                print("No coupling data.")
            else:
                print(f"Co-change coupling for {args.file}:")
                for item in result:
                    partner = item.get("file_b", item.get("partner", "unknown"))
                    count = item.get("co_commit_count", item.get("count", ""))
                    print(f"  {partner}  ({count} co-commits)")
        return result


def cmd_risk_map(args):
    """Handle the 'risk-map' subcommand."""
    with ChiselEngine(args.project_dir, storage_dir=args.storage_dir) as engine:
        result = engine.tool_risk_map(directory=args.directory)
        if args.json_output:
            _print_json(result)
        else:
            if not result:
                print("No risk data.")
            else:
                print("Risk map:")
                for item in result:
                    fpath = item.get("file_path", item.get("file", "unknown"))
                    score = item.get("risk_score", item.get("score", ""))
                    print(f"  {fpath}: {score}")
        return result


def cmd_stale_tests(args):
    """Handle the 'stale-tests' subcommand."""
    with ChiselEngine(args.project_dir, storage_dir=args.storage_dir) as engine:
        result = engine.tool_stale_tests()
        if args.json_output:
            _print_json(result)
        else:
            if not result:
                print("No stale tests found.")
            else:
                print("Stale tests:")
                for item in result:
                    test_id = item.get("test_id", item.get("id", "unknown"))
                    reason = item.get("reason", "")
                    print(f"  {test_id}  ({reason})")
        return result


def cmd_history(args):
    """Handle the 'history' subcommand."""
    with ChiselEngine(args.project_dir, storage_dir=args.storage_dir) as engine:
        result = engine.tool_history(args.file)
        if args.json_output:
            _print_json(result)
        else:
            if not result:
                print("No history found.")
            else:
                print(f"History for {args.file}:")
                for item in result:
                    commit_hash = item.get("hash", item.get("commit_hash", ""))
                    short = commit_hash[:8] if commit_hash else "?"
                    author = item.get("author", "")
                    date = item.get("date", "")
                    msg = item.get("message", "")
                    print(f"  {short}  {date}  {author}  {msg}")
        return result


def cmd_who_reviews(args):
    """Handle the 'who-reviews' subcommand."""
    with ChiselEngine(args.project_dir, storage_dir=args.storage_dir) as engine:
        result = engine.tool_who_reviews(args.file)
        if args.json_output:
            _print_json(result)
        else:
            if not result:
                print("No reviewer suggestions.")
            else:
                print(f"Suggested reviewers for {args.file}:")
                for item in result:
                    author = item.get("author", "unknown")
                    commits = item.get("recent_commits", "")
                    days = item.get("days_since_last_commit", "?")
                    pct = item.get("percentage", "")
                    print(f"  {author}: {pct}% ({commits} commits, {days}d ago)")
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
