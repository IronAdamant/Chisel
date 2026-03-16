# Contributing to Chisel

## Prerequisites

- Python 3.9 or later
- git (used at runtime for log, blame, and diff operations)

## Development Setup

```bash
git clone <repo-url>
cd Chisel
pip install -e ".[dev]" --break-system-packages   # --break-system-packages needed on Arch Linux
```

This installs Chisel in editable mode along with dev dependencies (pytest, pytest-cov, ruff).

## Running Tests

```bash
pytest tests/ -v --tb=short     # full suite (313 tests)
pytest tests/test_engine.py     # single module
pytest -k "test_risk"           # by name pattern
```

All tests must pass before submitting changes.

## Code Style

This project uses [ruff](https://docs.astral.sh/ruff/) for linting.

- Line length: 100 characters
- Run `ruff check chisel/` before committing
- No type stubs required, but type hints are welcome

Configuration lives in `pyproject.toml` under `[tool.ruff]`.

## Architecture Overview

See `CLAUDE.md` for the full module map and dependency graph, and `ARCHITECTURE.md` for detailed design documentation.

The core modules are:

| Module | Role |
|---|---|
| `engine.py` | Orchestrator; owns all subsystems |
| `storage.py` | SQLite persistence (WAL mode, single persistent connection) |
| `ast_utils.py` | Multi-language AST extraction |
| `git_analyzer.py` | Git log/blame parsing via subprocess |
| `test_mapper.py` | Test discovery, framework detection, dependency extraction |
| `impact.py` | Impact analysis, risk scoring, stale test detection |
| `cli.py` | argparse CLI entry point |
| `mcp_server.py` | HTTP MCP server |
| `mcp_stdio.py` | stdio MCP server |
| `rwlock.py` | Read-write lock |

## Guidelines

### Module Size

Maximum 500 lines of code per file. If a module grows beyond this, split it along responsibility boundaries.

### Zero External Dependencies

Chisel's runtime has zero external dependencies. This is a firm design constraint. All functionality uses the Python standard library:

- `ast` for Python parsing
- Regular expressions for JS/TS/Go/Rust parsing
- `subprocess.run(["git", ...])` for git operations
- `sqlite3` for persistence

The only exceptions are optional extras declared in `pyproject.toml`:

- `mcp` extra: required only for the stdio MCP server
- `dev` extra: pytest, pytest-cov, ruff (development only)

Do not add runtime dependencies.

### Adding a New MCP Tool

To add a new tool, wire it through three layers:

1. **Engine method**: Add `tool_<name>(self, ...)` in `engine.py`. This contains the business logic.
2. **CLI handler**: Add a subcommand in `cli.py` that calls the engine method and formats output.
3. **MCP dispatch**: Add the tool name and schema to `_TOOL_DISPATCH` in `mcp_server.py`. The stdio server imports this dict automatically.

Each tool should have corresponding tests in `tests/`.

## Commit Messages

Use imperative mood in the subject line. Keep the subject under 72 characters. Include a body when the change is non-trivial, explaining what changed and why.

Examples:

```
Add unit-level churn via git log -L

Wire get_function_log() into engine.analyze() so each
function gets its own churn score alongside file-level stats.
```

```
Fix Go import parsing for aliased imports
```
