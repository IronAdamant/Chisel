# Chisel -- Complete Project Documentation

Test impact analysis and code intelligence for LLM agents. Zero external dependencies.

**Version:** 0.3.1
**License:** MIT
**Python:** >= 3.9

## File Table

| Path | Purpose | Dependencies | Wiki Link |
|------|---------|--------------|-----------|
| `pyproject.toml` | Build config, version, scripts, dev deps | setuptools >= 68.0 | -- |
| `LICENSE` | MIT license text | -- | -- |
| `.gitignore` | Git ignore rules for caches, DBs, virtualenvs | -- | -- |
| `.github/workflows/ci.yml` | GitHub Actions CI workflow | -- | -- |
| `CLAUDE.md` | Claude Code agent instructions for this project | -- | -- |
| `ARCHITECTURE.md` | Data model, SQL schemas, design decisions | -- | [spec-project](wiki-local/spec-project.md) |
| `CHANGELOG.md` | Versioned changelog (Keep a Changelog format) | -- | -- |
| `README.md` | Overview, install, quickstart, features | -- | -- |
| `COMPLETE_PROJECT_DOCUMENTATION.md` | This file -- full file table per global agent guidelines | -- | -- |
| `LLM_Development.md` | Chronological development log | -- | -- |

### chisel/ -- Core Package

| Path | Purpose | Dependencies | Wiki Link |
|------|---------|--------------|-----------|
| `chisel/__init__.py` | Package init, exports `__version__` | -- | -- |
| `chisel/ast_utils.py` | Multi-language AST extraction (Python/JS/TS/Go/Rust), `CodeUnit` dataclass, `_SKIP_DIRS` constant, `compute_file_hash`, `detect_language` | `ast`, `hashlib`, `re`, `dataclasses`, `pathlib` | [glossary: code unit](wiki-local/glossary.md) |
| `chisel/storage.py` | SQLite persistence layer (WAL mode, 9 tables, single persistent connection), all CRUD operations | `sqlite3`, `datetime`, `pathlib` | [glossary: blame cache](wiki-local/glossary.md) |
| `chisel/git_analyzer.py` | Git log/blame parsing via subprocess, churn computation, ownership computation, co-change coupling, diff function extraction | `re`, `subprocess`, `collections`, `datetime`, `itertools` | [glossary: churn score](wiki-local/glossary.md) |
| `chisel/test_mapper.py` | Test file discovery, framework detection (pytest/Jest/Go/Rust/Playwright), dependency extraction, test edge building | `ast`, `os`, `re`, `pathlib`, `chisel.ast_utils` | [glossary: test edge](wiki-local/glossary.md) |
| `chisel/impact.py` | Impact analysis, risk scoring, stale test detection, ownership queries, reviewer suggestions | `collections`, `datetime`, `chisel.git_analyzer`, `chisel.storage` (via constructor injection) | [glossary: risk score](wiki-local/glossary.md) |
| `chisel/engine.py` | Orchestrator -- owns Storage, GitAnalyzer, TestMapper, ImpactAnalyzer, RWLock; exposes `tool_*()` methods for all 10 MCP tools | `os`, `pathlib`, `chisel.ast_utils`, `chisel.git_analyzer`, `chisel.impact`, `chisel.rwlock`, `chisel.storage`, `chisel.test_mapper` | [spec-project](wiki-local/spec-project.md) |
| `chisel/cli.py` | argparse CLI with 12 subcommands, dispatch table, output formatting | `argparse`, `json`, `os`, `sys`, `chisel.engine` | [spec-project: CLI](wiki-local/spec-project.md) |
| `chisel/mcp_server.py` | HTTP MCP server (GET /tools, /health; POST /call), ThreadedHTTPServer, tool schemas and dispatch table | `json`, `logging`, `threading`, `http.server`, `socketserver`, `chisel.engine` | [spec-project: MCP tools](wiki-local/spec-project.md) |
| `chisel/mcp_stdio.py` | stdio MCP server for Claude Desktop/Cursor integration, requires optional `mcp` package | `asyncio`, `json`, `os`, `sys`, `chisel.engine`, `chisel.mcp_server` (imports `_TOOL_DISPATCH`, `_TOOL_SCHEMAS`) | [spec-project: MCP tools](wiki-local/spec-project.md) |
| `chisel/rwlock.py` | Read-write lock (multiple readers or one exclusive writer) for concurrent access | `threading`, `contextlib` | -- |

### tests/ -- Test Suite

| Path | Purpose | Dependencies | Wiki Link |
|------|---------|--------------|-----------|
| `tests/__init__.py` | Test package init (empty) | -- | -- |
| `tests/test_ast_utils.py` | Tests for multi-language AST extraction, language detection, file hashing, brace matching | `pytest`, `hashlib`, `textwrap`, `chisel.ast_utils` | -- |
| `tests/test_storage.py` | Tests for all Storage CRUD operations per table, WAL mode, table existence | `pytest`, `sqlite3`, `chisel.storage` | -- |
| `tests/test_git_analyzer.py` | Tests for git log/blame parsing, churn computation, ownership, co-change, diff functions (uses temp git repos) | `pytest`, `os`, `subprocess`, `datetime`, `chisel.git_analyzer` | -- |
| `tests/test_test_mapper.py` | Tests for framework detection, test file discovery, dependency extraction, edge building | `pytest`, `os`, `chisel.ast_utils`, `chisel.test_mapper` | -- |
| `tests/test_impact.py` | Tests for impact analysis, risk scoring, stale test detection, ownership, reviewer suggestions | `pytest`, `chisel.impact`, `chisel.storage` | -- |
| `tests/test_engine.py` | Integration tests for ChiselEngine with temp git repos and test files | `pytest`, `os`, `subprocess`, `chisel.engine` | -- |
| `tests/test_cli.py` | Tests for CLI parser, all command handlers, main dispatch | `pytest`, `json`, `unittest.mock`, `chisel.cli` | -- |
| `tests/test_mcp_server.py` | Tests for HTTP MCP server endpoints using a real server on OS-assigned port | `pytest`, `json`, `os`, `subprocess`, `urllib`, `chisel.mcp_server` | -- |
| `tests/test_mcp_stdio.py` | Tests for stdio MCP server creation, tool listing, tool dispatch | `pytest`, `json`, `unittest.mock`, `chisel.mcp_stdio` | -- |
| `tests/test_rwlock.py` | Tests for RWLock concurrent reader/writer semantics, ordering, starvation | `pytest`, `threading`, `time`, `chisel.rwlock` | -- |
| `tests/conftest.py` | Shared pytest fixtures: temp git repos, `run_git` helper | `pytest`, `os`, `subprocess` | -- |

## Module Dependency Graph

```
engine.py --> storage.py, ast_utils.py, git_analyzer.py, test_mapper.py, impact.py, rwlock.py
test_mapper.py --> ast_utils.py
impact.py --> storage.py (injected), git_analyzer.py
cli.py --> engine.py, mcp_server.py (lazy), mcp_stdio.py (lazy)
mcp_server.py --> engine.py
mcp_stdio.py --> engine.py, mcp_server.py
```

## SQLite Tables (9)

`code_units`, `test_units`, `test_edges`, `commits`, `commit_files`, `blame_cache`, `co_changes`, `churn_stats`, `file_hashes`

## Entry Points

| Script | Target | Description |
|--------|--------|-------------|
| `chisel` | `chisel.cli:main` | CLI with 12 subcommands |
| `chisel-mcp` | `chisel.mcp_stdio:main` | stdio MCP server |
