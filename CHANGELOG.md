# Changelog

All notable changes to Chisel are documented in this file.

Format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).
This project uses [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.4.0] - 2026-03-18

### Added

- `metrics.py` — extracted pure computation functions (churn, ownership, co-change) from `git_analyzer.py` into a standalone module with zero internal dependencies
- `schemas.py` — extracted tool JSON Schema definitions and dispatch tables from `mcp_server.py` into a shared module
- Co-change computation cap: commits touching >50 files are skipped (bulk operations are not meaningful coupling signals)
- Failure rate caching in `impact.py`: `get_risk_map` and `compute_risk_score` now fetch failure rates once instead of per-file, eliminating N redundant full-table scans
- 14 new tests (405 → 419): CLI output formatting for all 15 commands, metrics module tests, co-change cap verification
- `_fetch_failure_rates()` public helper for pre-fetching test instability data
- `_fmt_kv()` and `_fmt_list()` formatter factories in CLI for consistent output
- PyPI-ready metadata: classifiers, keywords, project URLs, readme field
- README: MCP-first structure with Claude Code/Cursor config snippets, self-analysis example, 15-tool table, Stele ecosystem mention

### Changed

- `git_analyzer.py` now focused on git subprocess interaction only (517 → 319 LOC)
- `mcp_server.py` now focused on HTTP server logic only (504 → 226 LOC)
- `cli.py` consolidated via `_run_tool()` shared handler (485 → 403 LOC)
- `ast_utils.py` uses `functools.partial` instead of 3 trivial wrapper functions
- `test_mapper.py` keyword blacklists converted from tuple to frozenset for O(1) lookup
- `test_mapper.py` inline regex precompiled as module-level `_JS_NAMED_IMPORT_RE`
- `mcp_stdio.py` imports `_TOOL_SCHEMAS` from `schemas.py` instead of `mcp_server.py`
- `_test_instability()` accepts a pre-built failure rates dict instead of querying storage directly
- `compute_risk_score()` accepts optional `failure_rates` parameter for batch use
- Shared `storage` fixture moved from `test_impact.py`/`test_storage.py` into `conftest.py`
- `_make_args()` in test_cli.py now includes `limit=None` default
- Package name changed to `chisel-test-impact` for PyPI (bare `chisel` is taken)

### Fixed

- `storage.py`: `cleanup_orphaned_test_results` TOCTOU race — replaced two-step SELECT+DELETE with atomic `DELETE ... WHERE test_id NOT IN (SELECT id FROM test_units)`
- `storage.py`: `_init_database` misleading context manager around `executescript` (which auto-commits independently)
- `storage.py`: `upsert_churn_stat`/`get_churn_stat` used `or ""` which could coerce legitimate falsy values — changed to `if x is None`
- `engine.py`: `update()` discarded return value of `_parse_and_store_code_units`, making update stats incomplete vs `analyze()`
- `mcp_stdio.py`: `create_server()` leaked engine if `_configure_server()` raised — added try/except cleanup
- `mcp_server.py`: loop variables `_name`, `_schema` leaked into module namespace after schema injection loop
- `test_mcp_stdio.py`: orphaned duplicate section comment removed

## [0.3.2] - 2026-03-17

### Fixed

- `impact.py`: `changed_functions or None` silently converted empty list `[]` to `None`, causing `get_impacted_tests()` to return ALL tests instead of none when no functions changed
- `impact.py`: `get_risk_map(directory="src")` matched files like `src_backup/file.py` due to bare prefix check — now uses path-boundary-safe `startswith(dir + "/")`
- `cli.py`: `cmd_stale_tests` printed nonexistent `"reason"` field (always empty) instead of `"edge_type"`
- `mcp_server.py`: `ChiselMCPServer.stop()` did not set `self._engine = None` after close, inconsistent with `_httpd`/`_thread` cleanup
- `mcp_stdio.py`: `create_server()` leaked engine with no cleanup path — engine now stored as `server._engine`
- `git_analyzer.py`: `compute_churn()` crashed on malformed commit dates — added try-except consistent with `compute_co_changes()`
- `tests/test_cli.py`: 6 test mocks had wrong field names (`score` instead of `relevance`, `reason` instead of `edge_type`, missing required fields), masked by now-removed defensive fallbacks

### Changed

- `ast_utils.py`: Consolidated 3 near-identical brace-language extractors (`_extract_js_ts`, `_extract_go`, `_extract_rust`) into shared `_extract_brace_lang()` with per-language pattern tables
- `storage.py`: Deduplicated identical SELECT/JOIN clause in `get_direct_impacted_tests()` into local `base_sql` variable
- `cli.py`: Simplified all output handlers by removing defensive `.get("x", .get("y", ...))` fallback chains — data contracts from engine are well-defined
- `ast_utils.py`: Fixed misleading comment on `_py_block_end` return value

### Removed

- `cli.py`: `_print_result()` function — used only once, dict branch was unreachable dead code

## [0.3.1] - 2026-03-17

### Fixed

- `_parse_diff_functions` now extracts bare function names instead of full declaration lines, fixing function-level impact filtering that silently matched nothing
- `cmd_suggest_tests` CLI handler read `item.get("score")` but `suggest_tests` returns key `"relevance"` — score always displayed as empty string
- `tool_churn` fallback returned all file churn stats even when a specific `unit_name` was requested and not found — now returns empty list
- All 10 CLI command handlers now use `with ChiselEngine(...) as engine:` to properly close SQLite connections
- `ChiselMCPServer.stop()` now calls `engine.close()` to release the SQLite connection
- Engine fixture in `test_engine.py` now uses `yield` + `close()` to avoid connection leaks

### Changed

- `impact.py` imports `GitAnalyzer` at module level instead of lazy import inside `get_ownership()`
- `build_test_edges` in `test_mapper.py` caches file contents to avoid re-reading the same file per test unit
- `engine.update()` calls `parse_log()` once instead of twice (once partial, once full)
- `extract_code_units` in `ast_utils.py` adds a `None` guard for the extractor lookup
- Simplified `test_cmd_serve_human` test (removed dead `_orig` import, unnecessary `sys.modules` clearing)

### Removed

- Dead code: `Storage.delete_test_units_by_file()` and `Storage.delete_edges_for_test()` (never called)
- Dead code: unreachable `framework == "rust"` branch in `TestMapper.detect_framework()`
- Dead code: unreachable `handler is None` guard in `cli.main()`
- Dead code: unused `_project_dir` and `_storage_dir` fields on `ChiselMCPServer`
- 334 tests (removed 2 tests for deleted methods, adjusted 3 test assertions)

## [0.3.0] - 2026-03-16

### Added

- `tool_ownership` returns blame-based authorship (`role: original_author`)
- `tool_who_reviews` returns commit-activity-based reviewer suggestions (`role: suggested_reviewer`)
- `tool_analyze` now accepts a directory parameter to scope code scanning
- `Storage.close()` for proper connection lifecycle management
- `_aggregate_blame_lines` helper to deduplicate blame aggregation logic
- `_SKIP_DIRS` shared constant in `ast_utils.py`, imported by `engine.py` and `test_mapper.py`
- 3 new tests (313 total)

### Fixed

- Redundant `compute_file_hash` call per code unit during analysis
- First-write-wins logic was dropping higher-score test edges in impact analysis
- `#` incorrectly treated as a comment character for JS/TS/Go/Rust in `_strip_strings_and_comments`
- `cli.main()` was discarding handler return values
- Go import parsing broke on aliased imports

### Changed

- Storage refactored to a single persistent SQLite connection (`check_same_thread=False`, WAL mode set once) instead of opening a new connection per method call
- `_TOOL_DISPATCH` in `mcp_stdio.py` now imported from `mcp_server.py` instead of duplicated
- `defaultdict` imports moved to module level in `impact.py`
- Blame header regex in `git_analyzer.py` compiled once at module level

### Removed

- Dead loop in `engine.py` (lines 98-102)
- Unused `_print_table` function in `cli.py`
- Unused imports across test files
- Dead `framework` parameter in `extract_test_dependencies`

## [0.2.0] - 2026-03-16

### Added

- MIT license (`LICENSE` file)
- `get_function_log()` using `git log -L :funcname:file` for per-function commit history
- Unit-level churn stats wired into `engine.analyze()` so each function gets its own churn score alongside file-level stats

## [0.1.0] - 2026-03-16

### Added

- SQLite persistence layer with WAL mode (9 tables)
- Multi-language AST extraction for Python, JavaScript, TypeScript, Go, and Rust
- Git log and blame analysis via subprocess
- Test file discovery with framework detection (pytest, Jest, Go test, Rust `#[test]`, Playwright)
- Test-to-code dependency extraction and edge building
- Impact analysis with risk scoring
- Stale test detection
- Reviewer suggestion engine
- CLI with 12 subcommands (`analyze`, `impact`, `suggest-tests`, `churn`, `ownership`, `coupling`, `risk-map`, `stale-tests`, `history`, `who-reviews`, `serve`, `serve-stdio`)
- HTTP MCP server (`GET /tools`, `/health`, `POST /call`)
- stdio MCP server (requires optional `mcp` package)
- Read-write lock for concurrent access
- Incremental analysis via file content hashing
- 305 tests, zero external dependencies
