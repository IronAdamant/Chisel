# Chisel -- LLM Development Log

Chronological record of development activity on the Chisel project.

---

## v0.5.0 -- 2026-03-22 -- Improved Edge Quality, AST Robustness, PyPI Publishing

### Summary
Addressed three structural gaps identified in codebase assessment: (1) fragile regex AST extraction for newer languages, (2) name-only test edge matching creating false positives, (3) missing PyPI publish automation. Added proximity-based edge weighting, Python import-path matching, improved regex patterns for 8 languages, and comprehensive test coverage for all of them.

### Edge Quality Improvements
- **Proximity weighting**: `_compute_proximity_weight()` in `test_mapper.py` scores test-to-code edges based on directory distance (1.0 same dir → 0.4 distant). Stored in the existing `weight` column on `test_edges`.
- **Python import-path matching**: `_matches_import_path()` resolves `from myapp.utils import foo` to `myapp/utils.py:foo` specifically, preventing false edges to unrelated `foo` functions in other modules. Falls back to name-based matching for calls and non-Python languages.
- **Impact on existing behavior**: All edge weights are ≤ 1.0 (same as before for same-directory matches). `impact.py` already uses the `weight` field, so impact analysis automatically benefits from higher-precision edges.

### AST Regex Improvements
- **Nested generics** (C#, Java, C++): `(?:<[^>]*>)` → `(?:<(?:[^<>]|<[^>]*>)*>)` — handles `Dictionary<string, List<int>>`, `Map<String, List<Integer>>`
- **Annotations/attributes** (C#, Java, Swift): Added prefix patterns `^(?:\s*@\w+...)*` and `^(?:\s*\[[^\]]*\]...)*` to handle `@Override`, `@Entity`, `[Test]`, `[Serializable]`, `@objc`
- **Kotlin extension functions**: `fun\s+(?:[A-Za-z_]\w*\.)?(?P<name>...)` — `fun String.toSnake()` now extracts `toSnake` (was extracting `String`)
- **C++ template functions + destructors**: Added `template<...>` prefix, `~?` in name capture for destructors
- **Dart factory/getters/setters**: Regex accepts `factory` keyword and `get`/`set` keyword before function names

### PyPI Publishing
- Added `.github/workflows/publish.yml` — triggers on tag push (`v*`), builds with `python -m build`, publishes via OIDC trusted publishing (`pypa/gh-action-pypi-publish`)

### Documentation
- **spec-project.md**: Complete rewrite — all 15 tools specified (was missing diff_impact, update, test_gaps, record_result, stats), all 12 languages in table with AST method details, all 18 CLI subcommands listed, new "Test Edge Weighting" section
- Updated CLAUDE.md with edge weighting and AST improvement notes

### Tests
- 63 new tests for 8 newer languages (C#: 9, Java: 8, Kotlin: 8, C++: 8, Swift: 7, PHP: 6, Ruby: 8, Dart: 9)
- 9 new tests for proximity weighting and import-path matching
- 522 tests total, all passing

---

## v0.4.1 -- 2026-03-22 -- Codebase Audit: Simplify, Modernize, Fix

### Summary
Full codebase audit across all 12 source files using 6 parallel exploration agents. Removed dead code, consolidated duplicated logic, modernized syntax, fixed documentation drift, and added missing error logging.

### Dead Code Removed
- **project.py**: `self._fd = None` on `ProcessLock` — never read or assigned after init

### Code Consolidated
- **project.py**: `exclusive()` and `shared()` were near-identical 10-line methods differing only in lock type; consolidated into shared `_acquire(lock_type)` helper
- **test_mapper.py**: `parse_test_file()` duplicated the exact logic from `_check_rust_test()` and `_check_cpp_test()` inline; extracted `_check_rust_test_content()` and `_check_cpp_test_content()` content-only helpers shared by both paths

### Bugs Fixed
- **storage.py**: `timeout=30` on `sqlite3.connect()` was redundant — `PRAGMA busy_timeout=30000` (set on the next line) overrides it. Removed the dead parameter
- **storage.py**: Restructured `get_direct_impacted_tests()` condition — the old `len(changed_functions) > 0` followed by a separate `changed_functions is not None` check was logically redundant
- **mcp_server.py**: Negative `Content-Length` values were not rejected (only zero was checked)
- **mcp_stdio.py**: `call_tool()` caught exceptions but never logged them server-side, making debugging impossible
- **metrics.py**: Docstring claimed "no file path filtering needed" for unit-level churn, but the code does filter

### Modernization
- **project.py**: `str.removeprefix("./")` replaces manual `if startswith / slice` pattern (Python 3.9+)
- **git_analyzer.py**: Walrus operator (`:=`) for regex match-then-check in `_parse_blame_output()` and `_parse_diff_functions()`
- **engine.py**: `functions if functions else None` → `functions or None`
- **test_mapper.py**: `lang == "java" or lang == "kotlin"` → `lang in ("java", "kotlin")`

### Performance
- **impact.py**: `suggest_reviewers()` parsed the same ISO dates 2-3 times per commit; now caches parsed datetimes per author

### Documentation
- Updated risk formula in `spec-project.md` (was still showing old 4-component 0.4/0.3/0.2/0.1 weights instead of current 5-component 0.35/0.25/0.2/0.1/0.1)
- Updated tool count from "10" to "15" in spec-project.md
- Updated README language/framework lists to include all 12 supported languages
- Updated version across `__init__.py`, `pyproject.toml`, `COMPLETE_PROJECT_DOCUMENTATION.md`, `CHANGELOG.md`

### Tests
- All 450 tests pass, no regressions

---

## v0.3.3 -- 2026-03-17 -- Codebase Audit & Cleanup

### Summary
Full codebase audit across all 10 source files and 11 test files. Fixed bugs, removed dead code, eliminated redundancy, and improved encapsulation.

### Bug Fixes
- **engine.py**: `_scan_code_files()` was case-sensitive for extensions (`.PY` files skipped); now uses `.lower()`
- **engine.py**: `_scan_code_files()` called inside `write_lock()` in `update()`, blocking readers during filesystem walk; moved outside lock
- **git_analyzer.py**: `compute_churn()` `commit_count` included commits with unparseable dates that were skipped in analysis; now counts only analyzed commits
- **git_analyzer.py**: `_parse_diff_functions()` fell back to raw hunk context as function name (producing garbage like `class Foo:`); now skips non-function contexts
- **cli.py**: `--passed`/`--failed` flags on `record-result` were not mutually exclusive; both given silently ignored `--failed`; now uses `add_mutually_exclusive_group()`
- **cli.py**: `cmd_serve` did not clean up engine on non-KeyboardInterrupt exceptions; now uses try/finally

### Dead Code Removed
- **storage.py**: Removed `get_latest_commit_date()` — never called from any production code

### Code Improvements
- **engine.py**: `_detect_diff_base()` no longer calls private `GitAnalyzer._run_git()`; new public `get_current_branch()` and `branch_exists()` methods added to `GitAnalyzer`
- **cli.py**: Added `--no-exclude-tests` flag to `test-gaps` subcommand (was present in MCP schema but missing from CLI)
- **cli.py**: Removed duplicate `shared` parent from top-level parser (subcommands already inherit it)
- **storage.py**: Fixed `get_stats()` docstring (`blame_blocks` → `blame_cache`)
- **storage.py**: Added `ORDER BY` to `get_all_test_units()` for deterministic results
- **storage.py**: `cleanup_orphaned_test_results()` now passes tuple instead of list to `_execute()` for consistency
- **test_mapper.py**: Eliminated triple file read for Rust test files (content now read once, reused for framework detection)
- **mcp_stdio.py**: `_run_server()` now reuses `create_server()` instead of duplicating engine creation logic
- **cli.py**: Removed extra blank line between `_limit()` and command handlers

### Tests
- Updated 4 test assertions to match code changes (removed 2 dead-code tests, updated 2 CLI mock assertions)
- All 404 tests pass, ruff lint clean

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

---

## v0.3.2 -- 2026-03-17 -- Deep Code Review Round 3: Semantic Bugs, Dead Code, Refactoring

### Summary

Third comprehensive code review using 10 parallel agents to audit every module, cross-validate inter-module contracts, and identify semantic bugs. Fixed 7 bugs (including 2 logic errors that silently produced wrong results), consolidated duplicate code, and hardened error handling.

### Bugs fixed

- **`impact.py`**: `changed_functions or None` converted an empty list `[]` to `None`, causing `get_impacted_tests()` to return ALL tests when the caller explicitly said "no functions changed" (should return none). Root cause: Python's `[] or None` evaluates to `None` because empty list is falsy.
- **`impact.py`**: `get_risk_map(directory="src")` used bare `startswith("src")` which incorrectly matched paths like `src_backup/file.py`. Changed to `startswith("src/")` with proper path boundary.
- **`cli.py`**: `cmd_stale_tests` displayed a nonexistent `"reason"` field (always blank). The actual field from `detect_stale_tests()` is `"edge_type"`. This was masked by the old defensive `.get("reason", "")` fallback.
- **`mcp_server.py`**: `ChiselMCPServer.stop()` closed the engine but didn't set `self._engine = None`, leaving a stale reference. Inconsistent with `_httpd` and `_thread` cleanup.
- **`mcp_stdio.py`**: `create_server()` created a `ChiselEngine` captured in a closure with no cleanup path. Engine now stored as `server._engine` for caller cleanup.
- **`git_analyzer.py`**: `compute_churn()` called `_parse_iso_date()` without try-except, so a malformed commit date would crash the entire churn computation. `compute_co_changes()` already had the guard — now both are consistent.
- **`tests/test_cli.py`**: 6 test mocks had incorrect field names (`score` vs `relevance`, `reason` vs `edge_type`, missing `percentage`/`recent_commits`/`date`/`author`/`message`). These never failed because the old CLI code used defensive fallback chains that silently returned defaults.

### Code consolidated

- **`ast_utils.py`**: Replaced 3 near-identical functions (`_extract_js_ts`, `_extract_go`, `_extract_rust`) with a shared `_extract_brace_lang(file_path, content, patterns)` helper. Each language now defines a pattern table (`_JS_TS_PATTERNS`, `_GO_PATTERNS`, `_RS_PATTERNS`) — a list of `(regex, unit_type)` tuples where `unit_type` can be a string or a `callable(match) -> (name, type)` for dynamic extraction (Go's `kind` group, Rust's `impl` name stripping). Net reduction: ~50 lines.
- **`storage.py`**: Deduplicated the identical 6-line SELECT/JOIN clause in `get_direct_impacted_tests()` into a local `base_sql` variable shared by both query paths.

### Dead code removed

- **`cli.py`**: Removed `_print_result()` function (only used once by `cmd_churn`, dict branch was unreachable). Inlined the list iteration.
- **`cli.py`**: Stripped all `.get("x", .get("y", ...))` defensive fallback chains across 10 command handlers. The engine returns well-defined dicts — the fallbacks masked field name mismatches (proven by the test mock fixes above).

### Test fixes

- Updated 6 CLI test mocks to use correct field names matching actual engine output contracts.
- Updated `test_risk_map_with_directory` to use directory-style paths (`src/app.py`) instead of exploiting the old buggy prefix behavior.
- Fixed misleading comment on `_py_block_end` return value.
- 334 tests (count unchanged).

---

## v0.3.1 -- 2026-03-17 -- Comprehensive Code Review and Bug Fixes

### Summary

Full codebase audit using parallel agents to review every module, cross-validate all inter-module dependencies, and verify test coverage. Fixed 5 bugs, removed 5 instances of dead code, and made 4 performance/quality improvements.

### Bugs fixed

- **`git_analyzer.py`**: `_parse_diff_functions()` returned full declaration lines (e.g. `def foo():`) instead of bare function names (`foo`). This caused the `changed_functions` filter in `impact.py:get_impacted_tests()` to silently match nothing, making function-level impact filtering a no-op.
- **`cli.py`**: `cmd_suggest_tests` read `item.get("score")` but `ImpactAnalyzer.suggest_tests()` returns key `"relevance"`. Score always displayed as empty string in human output.
- **`engine.py`**: `tool_churn` fell back to returning all file churn stats even when a specific `unit_name` was requested and not found. Now returns `[]` for missing units.
- **`cli.py`**: All 10 `cmd_*` handlers created `ChiselEngine` instances without closing them, leaking SQLite connections. Changed to `with ChiselEngine(...) as engine:`.
- **`mcp_server.py`**: `ChiselMCPServer.stop()` didn't call `engine.close()`, leaking the SQLite connection.

### Dead code removed

- `Storage.delete_test_units_by_file()` and `Storage.delete_edges_for_test()` -- defined but never called from any module.
- Unreachable `framework == "rust"` branch in `TestMapper.detect_framework()` -- no pattern in `_FRAMEWORK_PATTERNS` produces `"rust"`.
- Unreachable `handler is None` guard in `cli.main()` -- argparse validates subcommands.
- Unused `_project_dir` and `_storage_dir` fields on `ChiselMCPServer`.

### Performance and quality improvements

- `engine.update()` called `parse_log()` twice (once partial, once full) -- now calls it once.
- `test_mapper.build_test_edges()` re-read the same file for every test unit from that file -- added file content cache.
- `impact.py` moved lazy `GitAnalyzer` import to module top-level (no circular import risk).
- `ast_utils.extract_code_units()` added `None` guard for extractor lookup defensively.

### Test fixes

- Engine fixture in `test_engine.py` changed from `return` to `yield` + `close()` to avoid connection leaks.
- Simplified overly complex `test_cmd_serve_human` (removed dead `_orig` import, unnecessary `sys.modules` manipulation).
- Added `_make_engine_mock()` helper for CLI handler tests to support context manager protocol.
- Updated assertions in `test_git_analyzer.py` and removed tests for deleted `Storage` methods.

### Test suite

- 334 tests (removed 2 tests for deleted methods, adjusted 3 assertions).
