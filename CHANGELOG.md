# Changelog

All notable changes to Chisel are documented in this file.

Format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).
This project uses [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.8.3] — 2026-04-14

### Fixed

- **Version sync**: `chisel.__version__` was out of sync with `pyproject.toml` in the 0.8.2 release, causing CI failures.

## [0.8.2] — 2026-04-14

### Added

- **`optimize_storage` MCP tool**: Runs `PRAGMA optimize` and conditionally `VACUUM` when the WAL file grows large, reducing fragmentation and stale query plans after months of incremental updates.
- **Incremental import graph rebuilds**: `_rebuild_import_edges()` now only deletes and rebuilds edges for changed files instead of clearing the entire table. This turns an `O(all_files)` operation into `O(changed_files)` and keeps 1k+ file monorepo updates under 3 seconds.
- **Directory-scoped `suggest_tests`**: `tool_suggest_tests` accepts a `directory` parameter and returns aggregated suggestions for all code files under that path.
- **Background job cancellation & events**: Added `cancel_job` tool, `cancel_requested_at` flag on `bg_jobs`, `JobCancelledError`, and a `job_events` table. `analyze()` / `update()` check for cancellation at phase boundaries and record progress events.
- **Framework fixture test suite**: `tests/test_language_frameworks.py` covers C# (`[Fact]`, `[Theory]`), Java (`@Test`), Rust (`#[test]`, `#[tokio::test]`), Swift (`@Test`), and Go module-aware import resolution.
- **Query pagination hard bounds**: `limit` is enforced server-side (max 1000) for all list-returning tools.
- **`risk_map` `working_tree` parameter**: When `working_tree=true`, untracked code files are included in risk scoring. New files with zero commits and zero test coverage receive a `new_file_boost` of 0.5 so they surface in rankings instead of being invisible.
- **`risk_map` missing-file warning**: When untracked files exist but `working_tree=false`, `_meta.warnings` includes a message indicating how many files were excluded from scoring.
- **`test_gaps` working-tree elevation**: Gaps from untracked files now sort to the top of the list when `working_tree=true`, preventing the `limit` parameter from cutting them off.
- **`suggest_tests` / `diff_impact` directory-aware stem matching**: Same-directory tests (e.g. `tests/services/X.test.js` for `src/services/X.js`) are strongly preferred over fuzzy substring matches.
- **`diff_impact` expanded working-tree fallback**: Static import scan and stem-match fallback now apply to ALL changed files (including newly staged files), not just untracked ones.
- **`diff_impact` stale-DB detection**: Returns `{"status": "stale_db", "file_path": ..., "hint": "chisel analyze"}` when changed files are not present in the database, instead of silently returning empty results.
- **`suggest_tests` auto-fallback**: When a file has no DB test edges and no static import edges, `suggest_tests` now automatically falls back to stem-matching across all known test files (previously required `fallback_to_all=True`). This makes the tool self-healing for newly tracked or newly created files.
- **`triage` `working_tree` passthrough**: `tool_triage()` now forwards `working_tree` to `risk_map` and `test_gaps`, so composite triage calls include untracked files.
- **`start_job` / `job_status` progress tracking**: Background jobs now report `progress_pct` (0–100). `job_status` includes this field; `start_job` updates it during long-running `analyze`/`update` operations.
- **Heuristic edge backfill during `analyze`/`update`**: After edge discovery, test files that still have no test edges get filename-based heuristic edges automatically created via `_backfill_heuristic_edges()`. This improves coverage for projects where static dependency extraction misses test-to-source links.
- **Project fingerprint**: `analyze` stores the canonical project root in `meta.project_fingerprint`. Subsequent `suggest_tests` / `diff_impact` calls warn when the DB was created for a different project path, preventing accidental cross-project analysis reuse.
- **Git warnings in `analyze`/`update`**: When git is unavailable, the returned stats dict includes `git_warning` so agents know churn/blame/coupling will be missing.
- **MCP timeout hints**: `tool_analyze` and `tool_update` now include a `hint` recommending `start_job` for large repos.

### Changed

- **Risk formula**: Added `new_file_boost` (0.0 or 0.5) to both `compute_risk_score` and `get_risk_map`. Files with no git history and no tests score ~0.75 instead of ~0.25.
- **Coupling formula**: Import-graph coupling is now first-class. Changed from `max(cochange, cochange + 0.25 * import)` to `max(cochange, import, 0.5*cochange + 0.5*import)` in `tool_coupling`, `compute_risk_score`, and `get_risk_map`. In single-author/low-commit repos, import coupling now dominates instead of being a minor boost.
- **`risk_map` defaults**: Changed default `coverage_mode` from `"unit"` to `"line"` and default `proximity_adjustment` from `False` to `True`, so coverage gaps are graduated by default.
- **`test_gaps` static-import filter**: Now only removes files with NO DB test edges at all. Previously, a file with partial DB coverage could be incorrectly excluded from gaps just because a static import existed.

### Fixed

- **`storage.py` SQLite concurrency stability**: Restored `with self._conn as conn:` wrappers in `_fetchall`, `_fetchone`, `_execute`, and `_executemany` to prevent indefinite implicit-transaction hangs in background job threads.
- **`risk_map` crash with `working_tree=true`**: Fixed `KeyError: 'heuristic'` when heuristic test edges exist (created by `_backfill_heuristic_edges`) by adding `"heuristic"` to `edge_type_counts` in both `compute_risk_score` and `get_risk_map`.
- **`suggest_tests` timeout on working-tree files**: Added `StaticImportIndex` caching to `ImpactAnalyzer` and a fast-path in `tool_suggest_tests` that skips the expensive static index build when there are no DB edges for a working-tree file, falling back directly to stem-matching.
- **`diff_impact` timeout under working-tree load**: `StaticImportIndex` is now reused via cache, and `tool_diff_impact` only performs static import scanning on tracked changed files, using stem-matching for untracked files to avoid timeouts with many uncommitted files.
- **`storage.py` read-only transaction error**: `_fetchone()` and `_execute()` no longer use `with self._conn as conn:` for reads, which caused `sqlite3.OperationalError: cannot commit - no transaction is active` when SQLite implicitly opened no transaction for SELECT queries.

## [0.8.2] - 2026-04-10

### Added

- **`diff_impact` `working_tree` parameter**: Performs full static import scanning for untracked files via `StaticImportIndex`, finding tests that import new files by path. CLI: `chisel diff-impact --working-tree`. MCP: `working_tree: true`. Previously only `suggest_tests` supported this.

### Changed

- **Single-author co-change threshold halving**: When all commits have one distinct author, the adaptive coupling threshold is halved (`max(1, threshold // 2)`) so solo developer commit patterns surface coupling signal instead of universal 0.0. Author count stored in `meta.distinct_authors`.
- **Coverage gap granularity**: `_quantize_gap` increased from 4 steps (0.0/0.25/0.5/0.75/1.0) to 20 steps (0.05 increments) for finer differentiation between partially-tested files.
- **Proximity adjustment scope**: `proximity_adjustment` now applies to any file with `coverage_gap > 0.0`, not just completely untested files. Partially-tested files imported by tested code receive proportional credit.
- **Risk reweighting threshold**: `apply_risk_reweighting()` triggers on 2+ uniform components or any zero-valued uniform component (provably absent data). Previously required 3+, meaning single-author projects with coupling=0.0 and test_instability=0.0 never got reweighted, diluting risk scores by ~40%.

## [0.8.0] - 2026-03-31

### Added

- **Variable taint tracking for JS/TS**: Regex-based tracking of `const/let/var X = './path'` assignments resolves `require(variable)` calls. Known variables upgrade to `tainted_import` (confidence=1.0); unknown variables remain `dynamic_import` (confidence=0.3). `test_mapper.py`: `_JS_VAR_ASSIGN_RE`, `_JS_SIMPLE_ASSIGN_RE`, updated `_extract_js_deps()`.
- **`shadow_graph` in `stats`**: `tool_stats()` now returns a `shadow_graph` dict with `total_edges`, `call_edges`, `import_edges`, `dynamic_import_edges`, `eval_import_edges`, `tainted_import_edges`, and `unknown_shadow_ratio`. `storage.py`: `get_edge_type_counts()`.
- **Per-file dynamic risk fields in `risk_map`**: Each entry now includes `shadow_edge_count`, `dynamic_edge_count`, `unknown_require_count` (via `new Function()` pattern scan in JS/TS files), and `hidden_risk_factor`. `impact.py`: updated `compute_risk_score()` and `get_risk_map()`.
- **`coverage_depth` in risk formula**: New 6th component — `min(distinct_covering_tests/5, 1.0)` — with weight 0.10. `test_instability` weight reduced from 0.10 to 0.05. Risk formula: `0.35*churn + 0.25*coupling + 0.15*coverage_gap + 0.10*coverage_depth + 0.10*author_concentration + 0.05*test_instability + hidden_risk_factor`.
- **`hidden_risk_factor`**: Additive uplift (0–0.15) from dynamic/eval import edge density: `min(dynamic_edge_count/20, 1.0) * 0.15`. Computed separately from the 6-component reweighting system.
- **Confidence-weighted edges**: Edge weights now blend `proximity * sqrt(confidence)` so low-confidence dynamic requires contribute proportionally less to impact scores. `test_mapper.py`: `build_test_edges()`.
- **`unknown_require_count`**: Count of `new Function(` patterns in JS/TS source files, indicating potential `eval`-based module loading. Surface-level heuristic for risk assessment.
- **3 new glossary entries**: "Dynamic require() detection", "Shadow graph", "Require confidence score" (`wiki-local/glossary.md`).

### Changed

- **`_BASE_RISK_WEIGHTS`** (`risk_meta.py`): Updated to 6-component weights reflecting new formula.
- **`docs/CUSTOM_EXTRACTORS.md`**: Completely rewritten with comprehensive JS/TS tree-sitter extractor showing scope-aware variable tracking and `tainted_import` resolution.
- **`docs/LLM_CONTRACT.md`**: Dynamic require table now includes `tainted_import`; added `risk_map dynamic-risk fields` section documenting `hidden_risk_factor`, `shadow_edge_count`, `dynamic_edge_count`.
- **`wiki-local/spec-project.md`**: Updated risk formula, test edge weighting section now mentions variable taint tracking and shadow graph.
- **`CLAUDE.md`**: Updated risk formula bullet with correct weights, `coverage_depth`, and `hidden_risk_factor`.

### Fixed

- **`risk_map` reweighting**: Now correctly handles 6 components (was 5) when 3+ are uniform across files.

## [0.6.5] - 2026-03-27

### Added

- **`CHISEL_BOOTSTRAP`**: optional dotted import path loaded at `ChiselEngine` startup (`chisel/bootstrap.py`) so users can call `register_extractor()` without forking the CLI. Tree-sitter / other parsers remain **user-installed** — Chisel stays stdlib-only.
- **`docs/CUSTOM_EXTRACTORS.md`**: full guide for `register_extractor`, bootstrap env, and optional third-party parsers.
- **`examples/chisel_bootstrap_example.py`**: commented template for copy-paste.
- `tests/test_bootstrap.py` for bootstrap loading.

### Documentation

- Cross-links from README, CONTRIBUTING, CLAUDE, `docs/ZERO_DEPS.md`, COMPLETE_PROJECT_DOCUMENTATION, ARCHITECTURE.

## [0.6.4] - 2026-03-27

### Added

- **Import-graph test impact**: `get_impacted_tests` / `suggest_tests` walk undirected static import edges to suggest tests that cover **reachable** modules (e.g. facade tests for inner modules). `storage.py`: `get_importers()`, `get_imported_files()`.
- **`tool_coupling`**: Numeric `cochange_coupling`, `import_coupling`, `effective_coupling`, plus breadth counts — import coupling stays visible in solo / low-commit repos.
- **Risk breakdown**: `coverage_fraction` alongside quantized `coverage_gap` in `compute_risk_score` and `get_risk_map`.
- **`diff_impact`**: On git failure, returns `status: "git_error"` with `message`, `project_dir`, and `hint` (never a silent empty list). CLI prints hint; `next_steps` suggests fixing project directory.

### Changed

- **Docs**: README, CLAUDE, ARCHITECTURE, COMPLETE_PROJECT_DOCUMENTATION, `wiki-local/spec-project.md`, CONTRIBUTING — agent-first, solo maintainer, multi-agent session positioning; MCP tool specs updated (22 tools, `triage`, locks, `next_steps`).
- **`schemas.py`**: Tool descriptions for `analyze`, `update`, `suggest_tests`, `coupling`, `diff_impact`.

## [0.6.3] - 2026-03-24

### Added

- **Empty-state detection**: All 11 query tools (`risk_map`, `test_gaps`, `stale_tests`, `churn`, `coupling`, `impact`, `suggest_tests`, `ownership`, `who_reviews`, `history`, `diff_impact`) now return a structured `{"status": "no_data", "message": "...", "hint": "chisel analyze"}` response when no analysis data exists, instead of silently returning `[]`. `tool_stats` includes a `hint` key when all counts are zero.
- `storage.py`: `has_analysis_data()` — cheap `SELECT 1 FROM code_units LIMIT 1` check.
- `engine.py`: `_NO_DATA_RESPONSE` constant and `_check_analysis_data()` helper. Write tools (`analyze`, `update`, `record_result`) are unaffected.
- `cli.py`: `_is_no_data()` helper — CLI prints the warning message instead of passing the dict to list formatters.
- 18 new tests: empty-state detection across engine (6), storage (2), CLI (7), limit pass-through (1), plus 2 updated existing tests.
- 567 tests pass (up from 553)

## [0.6.2] - 2026-03-22

### Fixed

- `git_analyzer.py`: Diff lines containing tabs in `git log -L` output were misidentified as numstat entries, causing `ValueError` crash on non-numeric fields. Now validates fields are digits or `-` before `int()` conversion. Found via Grafana stress test (21k files).

### Changed

- `engine.py`: Unit-level churn (`git log -L` per function) is skipped when the repo exceeds 2,000 code files (`_UNIT_CHURN_FILE_LIMIT`). Each function spawns a subprocess, making it O(n×m) — impractical for large monorepos. File-level churn is always computed.
- Stress tested on Grafana: 14,334 code files, 62,379 code units, 22,155 test edges in ~3 minutes. `risk_map` for 14k files in 0.8 seconds.
- 553 tests pass, no regressions

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
