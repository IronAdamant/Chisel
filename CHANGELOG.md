# Changelog

All notable changes to Chisel are documented in this file.

Format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).
This project uses [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.6.1] - 2026-03-22

### Fixed

- `ast_utils.py`: `_strip_strings_and_comments` now tracks multi-line `/* */` block comment state across lines — braces inside multi-line comments were being counted, potentially returning wrong block end positions in C/C++/Java/Go/Rust/etc.
- `metrics.py`: Removed dead `Z`-suffix workaround in `_parse_iso_date` — `fromisoformat` handles `Z` natively since Python 3.11

### Changed

- Minimum Python version bumped from 3.9 to 3.11 (Python 3.9 is EOL, 3.10 EOL October 2026)
- `_strip_strings_and_comments` now returns `(cleaned_line, in_block_comment)` tuple for state propagation
- CI matrix updated from 3.9-3.13 to 3.11-3.14
- Removed 3.9 and 3.10 classifiers from pyproject.toml

### Added

- Tests for `tool_record_result` and `tool_stats` at engine integration level (were only tested via CLI mocks)
- Tests for `--limit` / `limit` parameter: CLI `_limit()` helper, CLI command truncation, MCP server pass-through
- Tests for multi-line block comment handling (6 tests)
- 553 tests pass (up from 540)

## [0.6.0] - 2026-03-22

### Added

- **Pluggable AST extractors**: `register_extractor(language, fn)` lets users override built-in regex extractors with tree-sitter, LSP, or other backends. `unregister_extractor()` reverts to built-in. `get_registered_extractors()` for introspection. Custom extractors checked before built-ins in `extract_code_units()`. Zero new dependencies.
- **Batch SQL queries**: 5 new `get_*_batch()` methods in `storage.py` for edges, code units, co-changes, churn stats, and blame. `_chunked()` helper splits large batches to stay under SQLite's variable limit.
- **Process-level read locks**: All read tool methods in `engine.py` now acquire `_process_lock.shared()` + `lock.read_lock()`. Write tools (`record_result`, `analyze`, `update`) acquire `_process_lock.exclusive()` + `lock.write_lock()`. Concurrent reads from multiple processes are now safe.
- **Cross-platform ProcessLock**: `project.py` uses `fcntl.flock` on Unix and `LockFileEx`/`UnlockFileEx` via ctypes on Windows. Both support shared and exclusive locks.
- 18 new tests: extractor registry (6), batch queries (7), process lock (3), engine lock wiring (2)

### Changed

- `impact.get_risk_map()` rewritten to use batch queries — computes all risk scores in ~5 queries instead of N*5 (eliminates N+1 pattern)
- `ProcessLock._acquire()` takes `exclusive: bool` instead of a platform-specific lock type constant
- 540 tests pass (up from 522)

## [0.5.4] - 2026-03-22

### Fixed

- `engine.py`: Unhandled `OSError` when a file vanishes between scan and parse in `_parse_and_store_code_units()` — now gracefully skips the file
- `schemas.py`: All tool schemas shared a single mutable `_LIMIT_PROP` dict reference — mutation by any consumer would silently corrupt all schemas; now each schema gets its own copy
- `cli.py`: `record-result` without `--passed` or `--failed` silently defaulted to "passed" — the mutually exclusive group is now `required=True`
- `wiki-local/glossary.md`: Co-change coupling risk weight was listed as 0.3 (old formula) instead of current 0.25
- `wiki-local/glossary.md`: Tool dispatch table reference pointed to `mcp_server.py` instead of `schemas.py`

### Changed

- `engine.py`: Simplified `_detect_diff_base()` using `next()` with generator expression (replaces for-loop + early return)
- `engine.py`: Removed `pathlib.Path` import — replaced single `Path.read_text()` usage with `open()` + error handling
- `ast_utils.py`: Removed unnecessary `getattr(node, "end_lineno", None)` guards — `end_lineno` is guaranteed on all AST nodes in Python 3.9+
- `ast_utils.py`: Removed redundant `lang is None` check in `extract_code_units()` — `None` is never a key in `_EXTRACTORS`
- `git_analyzer.py`: Used walrus operator in `get_changed_files()` to eliminate double `.strip()` call per line
- `storage.py`: Simplified `_normalize_unit_name` from explicit `if is not None` to `or ""`
- `impact.py`: Replaced loop-building-a-set with set comprehension in `get_risk_map()`
- `test_mapper.py`: Converted `extract_test_dependencies()` from instance method to `@staticmethod` with dispatch dict (replaces 11-branch if-chain)
- `test_mapper.py`: Uses `normalize_path()` from `project.py` instead of `os.path.relpath()` — ensures consistent forward-slash paths across platforms
- `pyproject.toml`: Added Python 3.14 classifier
- Updated all documentation: `COMPLETE_PROJECT_DOCUMENTATION.md`, `CLAUDE.md`, `CHANGELOG.md`, `LLM_Development.md`, glossary, spec-project
- 522 tests pass, no regressions

## [0.5.3] - 2026-03-22

### Fixed

- `mcp_stdio.py`: Moved chisel imports above the optional `mcp` try/except block to fix ruff E402 (module-level import not at top of file) — CI was failing on Python 3.11

## [0.5.2] - 2026-03-22

### Fixed

- `metrics.py`: Fragile tuple default `(None,)` in `compute_co_changes()` changed to `(None, None)` for consistency with the 2-element tuple structure
- `wiki-local/glossary.md`: Stale 4-component risk formula updated to current 5-component formula (0.35/0.25/0.2/0.1/0.1 with test_instability)
- `CLAUDE.md`: Corrected CLI subcommand count from 18 to 17

## [0.5.1] - 2026-03-22

### Fixed

- `storage.py`: Added missing index `idx_test_edges_test` on `test_edges(test_id)` — `get_edges_for_test()` and `delete_test_edges_by_test()` were doing full table scans
- `cli.py`: Unused `result` parameter in `cmd_record_result` formatter renamed to `_result`
- `cli.py`: Removed misleading "(default)" from `--passed` help text
- `ast_utils.py`: Bare `list` and `dict` type hints replaced with generic forms (`list[str]`, `dict[int, str]`)

## [0.5.0] - 2026-03-22

### Added

- **Proximity-based edge weighting**: Test edges now carry weights (0.4-1.0) based on file-path proximity. Same directory = 1.0, sibling dirs = 0.8, shared ancestor = 0.6, distant = 0.4. Reduces false positive edges from name collisions in multi-package projects.
- **Python import-path matching**: `from myapp.utils import foo` now matches specifically to `myapp/utils.py:foo` rather than any `foo` in any file. Falls back to name-based matching when import path doesn't resolve. New helpers: `_compute_proximity_weight()`, `_matches_import_path()` in `test_mapper.py`.
- **C# regex: nested generics and attributes**: `Dictionary<string, List<int>> Build()` and `[Test] public void Run()` now correctly extracted. Patterns handle `[Attribute]` prefixes and `<A<B>>` nesting.
- **Java regex: annotations and nested generics**: `@Override public void process()` and `Map<String, List<Integer>> build()` now correctly extracted. Patterns handle `@Annotation` prefixes.
- **Kotlin regex: extension functions**: `fun String.toSnake()` now extracts `toSnake` as the function name (was incorrectly extracting `String`). `inline` added to class modifiers.
- **C++ regex: template functions and destructors**: `template<typename T> void process(T)` and `void ~Foo()` now extracted. Nested template generics supported.
- **Swift regex: @attributes**: `@objc func setup()` and `@objc class Bridge` now correctly extracted.
- **Dart regex: factory constructors, getters/setters**: `factory Foo.fromJson()` and `String get name` now extracted.
- **PyPI publish workflow**: `.github/workflows/publish.yml` publishes to PyPI on tag push using OIDC trusted publishing.
- 63 new AST extraction tests for all 8 newer languages (C#, Java, Kotlin, C++, Swift, PHP, Ruby, Dart)
- 9 new edge weighting and import-path matching tests
- **spec-project.md**: Full rewrite — all 15 tools documented, all 12 languages in table, 18 CLI subcommands listed, test edge weighting section added

### Changed

- `test_mapper.py`: `build_test_edges()` now computes proximity-based weights instead of hardcoded 1.0
- `test_mapper.py`: `_extract_python_deps()` and `_extract_python_deps_regex()` now return `module_path` field for import-path matching
- 522 tests pass (up from 450)

## [0.4.1] - 2026-03-22

### Fixed

- `project.py`: Removed dead `self._fd = None` attribute on `ProcessLock` (never read or assigned after init)
- `storage.py`: Removed redundant `timeout=30` from `sqlite3.connect()` — the `PRAGMA busy_timeout=30000` already controls this and takes precedence
- `storage.py`: Simplified redundant condition logic in `get_direct_impacted_tests()` — early-return for empty list now groups naturally with the non-empty branch
- `mcp_server.py`: Negative `Content-Length` values no longer bypass validation (now rejected alongside zero)
- `metrics.py`: Fixed inaccurate docstring in `compute_churn()` that claimed "no file path filtering needed" when filtering is actually performed
- `mcp_stdio.py`: Added missing `logger.exception()` call in `call_tool()` — exceptions were silently swallowed with no server-side logging

### Changed

- `project.py`: Consolidated duplicated `exclusive()`/`shared()` lock methods into shared `_acquire(lock_type)` helper
- `project.py`: Uses `str.removeprefix("./")` instead of manual slicing (Python 3.9+)
- `engine.py`: `functions if functions else None` simplified to `functions or None`
- `test_mapper.py`: Extracted `_check_rust_test_content()` and `_check_cpp_test_content()` helpers; `parse_test_file()` now reuses them instead of duplicating detection logic
- `test_mapper.py`: `lang == "java" or lang == "kotlin"` changed to `lang in ("java", "kotlin")` for consistency
- `impact.py`: `suggest_reviewers()` caches parsed datetimes per author instead of re-parsing on every comparison and recency calculation
- `git_analyzer.py`: Uses walrus operator (`:=`) for regex matches in `_parse_blame_output()` and `_parse_diff_functions()`
- `wiki-local/spec-project.md`: Updated risk formula to current 5-component weights (was still showing old 4-component formula)
- `README.md`: Updated language and framework lists to include all 12 supported languages
- 450 tests pass, no regressions

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
