# Contributing to Chisel

## Prerequisites

- Python 3.11 or later
- git (used at runtime for log, blame, and diff operations)

## Development Setup

```bash
git clone https://github.com/IronAdamant/Chisel.git
cd Chisel
pip install -e ".[dev]" --break-system-packages   # --break-system-packages needed on Arch Linux
```

This installs Chisel in editable mode along with dev dependencies (pytest, pytest-cov, ruff).

## Running Tests

```bash
pytest tests/ -v --tb=short     # full suite (553 tests)
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
| `storage.py` | SQLite persistence (WAL mode, single persistent connection, batch queries) |
| `ast_utils.py` | Multi-language AST extraction (12 languages) + pluggable extractor registry |
| `git_analyzer.py` | Git log/blame parsing via subprocess |
| `metrics.py` | Pure computation: churn scoring, ownership, co-change detection |
| `test_mapper.py` | Test discovery, framework detection, dependency extraction, edge building |
| `impact.py` | Impact analysis, risk scoring, stale test detection, reviewer suggestions |
| `project.py` | Project root detection, path normalization, cross-platform ProcessLock |
| `schemas.py` | JSON Schema definitions for all 15 tools + dispatch table |
| `cli.py` | argparse CLI (17 subcommands) |
| `mcp_server.py` | HTTP MCP server |
| `mcp_stdio.py` | stdio MCP server |
| `rwlock.py` | Read-write lock for concurrent access |

## Guidelines

### Module Size

Maximum 500 lines of code per file. If a module grows beyond this, split it along responsibility boundaries.

### Zero External Dependencies

Chisel's runtime has zero external dependencies. This is a firm design constraint. All functionality uses the Python standard library:

- `ast` for Python parsing
- Regular expressions for 11 other languages (JS/TS, Go, Rust, C#, Java, Kotlin, C/C++, Swift, PHP, Ruby, Dart)
- `subprocess.run(["git", ...])` for git operations
- `sqlite3` for persistence
- `fcntl` (Unix) / `ctypes` + `msvcrt` (Windows) for cross-process file locking

The only exceptions are optional extras declared in `pyproject.toml`:

- `mcp` extra: required only for the stdio MCP server
- `dev` extra: pytest, pytest-cov, ruff (development only)

Do not add runtime dependencies.

### Adding a New MCP Tool

To add a new tool, wire it through four layers:

1. **Engine method**: Add `tool_<name>(self, ...)` in `engine.py`. Wrap with `self._process_lock.shared()` + `self.lock.read_lock()` for reads, or `exclusive()` + `write_lock()` for writes.
2. **Schema + dispatch**: Add the tool schema to `_TOOL_SCHEMAS` and the dispatch entry to `_TOOL_DISPATCH` in `schemas.py`. Both HTTP and stdio servers import these automatically.
3. **CLI handler**: Add a subcommand in `cli.py` that calls the engine method and formats output.
4. **Tests**: Add tests in `tests/` — at minimum an engine integration test and a CLI mock test.

### Adding a Custom Extractor

Users can register custom AST extractors (e.g., tree-sitter-backed) without modifying Chisel:

```python
from chisel.ast_utils import register_extractor, CodeUnit

def my_python_extractor(file_path, content):
    # Your tree-sitter/LSP logic here
    return [CodeUnit(file_path, "func_name", "function", 1, 10)]

register_extractor("python", my_python_extractor)
```

Custom extractors override the built-in regex ones for the registered language.

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
