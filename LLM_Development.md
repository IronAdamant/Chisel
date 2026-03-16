# Chisel -- LLM Development Log

Chronological record of development activity on the Chisel project.

---

## v0.1.0 -- 2026-03-16 -- Initial Implementation

### Summary

Built the complete Chisel system from scratch: a zero-dependency test impact analysis and code intelligence tool for LLM agents.

### What was built

- **SQLite persistence layer** (`storage.py`): WAL-mode database with 9 tables (`code_units`, `test_units`, `test_edges`, `commits`, `commit_files`, `blame_cache`, `co_changes`, `churn_stats`, `file_hashes`). All CRUD operations with upsert semantics. Foreign key enforcement intentionally disabled to support stale test detection via orphaned edge references.
- **Multi-language AST extraction** (`ast_utils.py`): Python extraction using the `ast` module with regex fallback for syntax errors. JavaScript/TypeScript, Go, and Rust extraction via regex patterns. `CodeUnit` dataclass for representing functions, classes, structs, enums, and impl blocks. Shared `_SKIP_DIRS` constant for directory filtering.
- **Git analysis** (`git_analyzer.py`): Parsing of `git log --numstat` and `git blame --porcelain` output via subprocess (no gitpython dependency). Churn score computation using the formula `sum(1 / (1 + days_since_commit))`. Ownership computation from blame blocks. Co-change coupling detection across file pairs (threshold: >= 3 co-commits).
- **Test mapper** (`test_mapper.py`): Automatic test file discovery with framework detection for pytest, Jest, Go test, Rust `#[test]`, and Playwright. Dependency extraction (imports and function calls) per language. Test-to-code edge building by matching extracted dependencies against known code units.
- **Impact analysis** (`impact.py`): Finding impacted tests for changed files via direct test edges and transitive co-change coupling. Risk scoring with formula: `0.4*churn + 0.3*coupling_breadth + 0.2*(1-test_coverage) + 0.1*author_concentration`. Stale test detection (tests referencing removed code units). Reviewer suggestions based on commit activity.
- **Engine** (`engine.py`): Orchestrator class tying together Storage, GitAnalyzer, TestMapper, ImpactAnalyzer, and RWLock. Full `analyze()` pipeline: scan code files, extract code units, discover tests, parse git history, compute churn and co-changes, run blame, build test edges. Incremental `update()` method using file content hashes. 10 `tool_*()` methods, one per MCP tool.
- **CLI** (`cli.py`): argparse-based CLI with 12 subcommands (`analyze`, `impact`, `suggest-tests`, `churn`, `ownership`, `coupling`, `risk-map`, `stale-tests`, `history`, `who-reviews`, `serve`, `serve-mcp`). JSON output mode via `--json` flag.
- **HTTP MCP server** (`mcp_server.py`): ThreadedHTTPServer with `GET /tools`, `GET /health`, `POST /call` endpoints. JSON Schema definitions for all 10 tools. Tool dispatch table mapping tool names to engine methods.
- **stdio MCP server** (`mcp_stdio.py`): Async MCP-compliant server using the optional `mcp` Python package. Communicates over stdin/stdout for Claude Desktop and Cursor integration.
- **Read-write lock** (`rwlock.py`): Multiple concurrent readers or one exclusive writer, used by the engine for thread-safe storage access.
- **Test suite**: 305 tests covering all modules.

### Design decisions established

- Zero external dependencies (stdlib only).
- Git as the sole source of truth (subprocess, not gitpython).
- Incremental analysis via file content hashing.
- Blame caching keyed by file content hash.

---

## v0.2.0 -- 2026-03-16 -- MIT License, Unit-Level Churn

### Summary

Added MIT license and extended churn analysis to the function level.

### What changed

- **MIT license**: Added `LICENSE` file to the project root.
- **Function-level git log** (`git_analyzer.py`): New `get_function_log()` method using `git log -L :funcname:file` to retrieve commits that touched a specific function.
- **Unit-level churn** (`engine.py`): `analyze()` now computes churn stats per function (not just per file). For each code unit of type `function` or `async_function`, the engine calls `get_function_log()` and stores the resulting churn stats with the unit name. The `compute_churn()` method was updated to accept a `unit_name` parameter: when provided, all commits are assumed pre-filtered by `git log -L` and used directly without file-path filtering.

---

## v0.3.0 -- 2026-03-16 -- Codebase Cleanup, Storage Refactor, Ownership/Reviewers Differentiation

### Summary

Major cleanup pass: refactored storage to use a single persistent connection, differentiated ownership from reviewer suggestions, removed dead code, and fixed multiple bugs.

### What changed

- **Storage refactor** (`storage.py`): Replaced per-method connection creation with a single persistent SQLite connection (`check_same_thread=False`). WAL mode and PRAGMA settings applied once at init. Added `close()` method for proper lifecycle management. `_connect()` now returns the persistent connection rather than creating a new one.
- **Ownership vs. reviewers differentiation** (`impact.py`):
  - `get_ownership()` returns blame-based authorship with `role: "original_author"` -- shows who wrote the code.
  - `suggest_reviewers()` returns commit-activity-based suggestions with `role: "suggested_reviewer"` -- shows who has been actively maintaining the file and is best positioned to review.
  - MCP tool descriptions in `mcp_server.py` updated to clarify the distinction.
- **Shared constants** (`ast_utils.py`): Moved `_SKIP_DIRS` to `ast_utils.py` as the canonical location. Both `engine.py` and `test_mapper.py` now import it from there instead of defining their own copies.
- **Scoped analysis** (`engine.py`): `tool_analyze()` / `analyze()` now accepts a `directory` parameter to scope code scanning to a subdirectory while keeping git log and test discovery project-wide.
- **Helper extraction** (`impact.py`): New `_aggregate_blame_lines()` helper to deduplicate blame aggregation logic used by both `get_ownership()` and `_author_concentration()`.
- **Import consolidation** (`mcp_stdio.py`): `_TOOL_DISPATCH` and `_TOOL_SCHEMAS` now imported from `mcp_server.py` instead of being duplicated.
- **Module-level compilation**: Blame header regex in `git_analyzer.py` compiled once at module level. `defaultdict` imports in `impact.py` moved to module level.

### Bugs fixed

- Redundant `compute_file_hash` call per code unit during analysis (was called once per unit instead of once per file).
- First-write-wins logic in `get_impacted_tests()` was dropping higher-score test edges; changed to keep the highest score.
- `_strip_strings_and_comments()` incorrectly treated `#` as a comment for JS/TS/Go/Rust (only valid for Python, which uses `_py_block_end` instead).
- `cli.main()` discarded handler return values.
- Go import parsing failed on aliased imports.

### Dead code removed

- Unreachable loop in `engine.py` (lines 98-102).
- Unused `_print_table` function in `cli.py`.
- Unused imports across test files.
- Dead `framework` parameter in `extract_test_dependencies`.

### Test suite

- 3 new tests added (313 total).
