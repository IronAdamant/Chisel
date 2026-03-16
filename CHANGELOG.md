# Changelog

All notable changes to Chisel are documented in this file.

Format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).
This project uses [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

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
