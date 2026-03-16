# Changelog

All notable changes to Chisel are documented in this file.

Format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).
This project uses [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

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
