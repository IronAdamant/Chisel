# Contributing to Chisel

Chisel is built for **LLM agents** and **solo developers** running **multiple agent sessions** on one repo. Contributions should preserve: **stdlib-only runtime**, **structured MCP responses** (status dicts, `next_steps` where applicable), **multi-process safety** (locks around storage), and **import-graph + test edges** as primary signals when git co-change is thin.

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
pytest tests/ -v --tb=short     # full suite
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

See `CLAUDE.md` for the full module map and dependency graph, `ARCHITECTURE.md` for the data model, and `wiki-local/spec-project.md` for MCP tool contracts.

The core modules are:

| Module | Role |
|---|---|
| `engine.py` | Orchestrator; owns all subsystems; `tool_*()` for MCP |
| `storage.py` | SQLite persistence (WAL mode, single persistent connection, batch queries) |
| `ast_utils.py` | Multi-language AST extraction (12 languages) + pluggable extractor registry |
| `git_analyzer.py` | Git log/blame parsing via subprocess |
| `metrics.py` | Pure computation: churn scoring, ownership, co-change detection |
| `test_mapper.py` | Test discovery, framework detection, dependency extraction, edge building |
| `impact.py` | Impact analysis (direct + co-change + import-graph tests), risk scoring, stale tests, git-derived ownership/review hints |
| `project.py` | Project root, path normalization, **ProcessLock** (multi-process / multi-agent) |
| `schemas.py` | JSON Schema + dispatch for **26 MCP tools** (20 functional + 6 advisory file-lock) |
| `next_steps.py` | Contextual follow-up hints for agent clients |
| `cli.py` | argparse CLI (core tools + serve + lock subcommands) |
| `mcp_server.py` | HTTP MCP server (`dispatch_tool`, `next_steps`) |
| `mcp_stdio.py` | stdio MCP server |
| `rwlock.py` | In-process read/write lock (pairs with ProcessLock) |

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

To add a new tool, wire it through these layers:

1. **Engine method**: Add `tool_<name>(self, ...)` in `engine.py`. Wrap with `self._process_lock.shared()` + `self.lock.read_lock()` for reads, or `exclusive()` + `write_lock()` for writes. Prefer **explicit statuses** (`no_data`, `git_error`, etc.) over ambiguous empty lists for agents.
2. **Schema + dispatch**: Add the tool schema to `_TOOL_SCHEMAS` and the dispatch entry to `_TOOL_DISPATCH` in `schemas.py`. Use **prescriptive** descriptions (“Use when…”). Both HTTP and stdio servers import these automatically; HTTP responses include **`next_steps`** — add a handler in `next_steps.py` if the new tool should suggest follow-ups.
3. **CLI handler**: Add a subcommand in `cli.py` when the tool should be human-invokable from the terminal (optional for agent-only tools).
4. **Tests**: Add tests in `tests/` — at minimum an engine integration test; add CLI tests if you added a subcommand.

Keep **multi-agent safety** in mind: long-running writes (`analyze`, `update`) must stay under the process exclusive lock; readers should not block writers longer than necessary.

### Release checklist

1. Bump **`pyproject.toml`** `[project].version` and **`chisel/__init__.py`** `__version__` together (CI runs `python scripts/check_version.py`).
2. Add a **`CHANGELOG.md`** section for the release.
3. Tag **`vX.Y.Z`** and push — PyPI publishes via `.github/workflows/publish.yml` (trusted publishing).
4. Create a **GitHub Release** with notes from the changelog.

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

See **`docs/CUSTOM_EXTRACTORS.md`** for **`CHISEL_BOOTSTRAP`** and third-party parsers (tree-sitter, etc.) in **user** environments — Chisel stays stdlib-only.

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
