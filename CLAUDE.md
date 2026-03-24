# Chisel — CLAUDE.md

Test impact analysis and code intelligence for LLM agents. Zero external dependencies.

## Architecture

```
chisel/
  engine.py         — Orchestrator. Owns Storage, GitAnalyzer, TestMapper, ImpactAnalyzer, RWLock, ProcessLock.
  project.py        — Multi-agent safety: project root detection, path normalization, storage resolution, cross-process file lock.
  storage.py        — SQLite persistence (WAL mode, 30s busy timeout, write retry). 10 tables.
  ast_utils.py      — Multi-lang AST extraction (12 languages). CodeUnit dataclass. _extract_brace_lang() shared by brace-delimited langs.
  git_analyzer.py   — Parses git log/blame via subprocess. Branch/diff queries.
  metrics.py        — Pure computation: churn scoring, ownership aggregation, co-change detection. _parse_iso_date shared utility.
  test_mapper.py    — Test file discovery, framework detection, dependency extraction, edge building.
  impact.py         — Impact analysis, risk scoring, stale test detection, reviewer suggestions. Caches failure rates.
  cli.py            — argparse CLI (18 subcommands). _run_tool() shared handler. Entry point: chisel.cli:main
  schemas.py        — JSON Schema definitions for all 16 tools + dispatch table. Shared by HTTP and stdio servers.
  mcp_server.py     — HTTP MCP server (GET /tools, /health, POST /call). ThreadedHTTPServer. dispatch_tool() shared by both servers.
  mcp_stdio.py      — stdio MCP server (requires optional 'mcp' package). _configure_server() for engine lifecycle mgmt.
  next_steps.py     — Contextual next-step suggestions for MCP tool responses. compute_next_steps() dispatched per tool.
  rwlock.py         — Read-write lock for in-process concurrent access.
```

## Key Design Decisions

- **Zero deps**: stdlib only. `ast` for Python, regex for JS/TS/Go/Rust. `subprocess.run(["git", ...])` for git. Requires Python >= 3.11.
- **FK enforcement disabled** in SQLite: stale test detection relies on orphaned edge refs; re-analysis deletes/recreates code_units freely.
- **Churn formula**: `sum(1 / (1 + days_since_commit))` — recent changes weigh heavily.
- **Risk formula**: `0.35*churn + 0.25*coupling + 0.2*coverage_gap + 0.1*author_concentration + 0.1*test_instability`
- **Co-change threshold**: Only pairs with >= 3 co-commits stored. Commits touching >50 files are skipped (bulk operations).
- **Blame caching**: Cached by file content hash, invalidated on change.
- **Incremental updates**: File content hashes tracked in `file_hashes` table.
- **Persistent connection**: Storage uses a single SQLite connection (`check_same_thread=False`) with RWLock for thread safety.
- **Multi-agent safety**: `project.py` provides: (1) `detect_project_root()` canonicalizes via git common dir so worktrees share identity, (2) `normalize_path()` ensures consistent relative paths, (3) `resolve_storage_dir()` defaults to project-local `.chisel/` (priority: explicit > env > project-local > ~/.chisel/), (4) `ProcessLock` for cross-process coordination — shared locks for reads, exclusive for writes. Cross-platform: `fcntl.flock` on Unix, `LockFileEx` on Windows.
- **SQLite concurrency**: 30s `busy_timeout` + exponential-backoff retry on `_execute` for cross-process SQLITE_BUSY.
- **Ownership vs Reviewers**: `ownership` = blame-based (who wrote the code, `role: "original_author"`). `who_reviews` = commit-activity-based (who maintains it, `role: "suggested_reviewer"`).
- **Shared constants**: `_SKIP_DIRS` and `_EXTENSION_MAP` live in `ast_utils.py`. `_CODE_EXTENSIONS` in `engine.py` is derived from `_EXTENSION_MAP`.
- **Shared dispatch**: `dispatch_tool()` in `mcp_server.py` is used by both HTTP and stdio servers. Tool schemas and dispatch tables live in `schemas.py`.
- **Edge weighting**: Test edges carry a weight (0.4-1.0) based on file proximity. Python import-path matching (`from myapp.utils import foo` → `myapp/utils.py:foo`) takes priority over name-only matching. `_compute_proximity_weight()` and `_matches_import_path()` in `test_mapper.py`.
- **AST regex improvements**: C#/Java support nested generics `<A<B>>` and annotations/attributes `@Override`/`[Test]`. Kotlin supports extension functions `fun String.foo()`. C++ supports template functions and destructors `~Foo()`. Swift supports `@objc`-style attributes. Dart supports factory constructors and getters/setters.
- **Pluggable extractors**: `register_extractor(lang, fn)` in `ast_utils.py` lets users override built-in regex extractors with tree-sitter or LSP-backed ones. `_custom_extractors` checked before `_EXTRACTORS` in `extract_code_units()`. Zero-dep — the registry is just hooks.
- **Batch SQL queries**: `storage.py` provides `get_*_batch()` methods for edges, code units, co-changes, churn, and blame. `impact.get_risk_map()` uses these to compute all risk scores in ~5 queries total instead of N*5. `_chunked()` helper splits large batches to stay under SQLite's variable limit.
- **Process-level read locks**: All read tool methods in `engine.py` acquire `_process_lock.shared()` (outer) + `lock.read_lock()` (inner). Writes acquire `_process_lock.exclusive()` + `lock.write_lock()`. This allows concurrent reads from multiple processes while blocking during writes.
- **Unit-churn scaling**: `_UNIT_CHURN_FILE_LIMIT = 2000` in `engine.py`. Repos with more than 2000 code files skip per-function `git log -L` churn (each function spawns a subprocess). File-level churn is always computed. Validated on Grafana (21k files, 62k units in ~3 min).
- **Numstat validation**: `_parse_log_output` in `git_analyzer.py` validates tab-separated fields are digits or `-` before treating them as numstat. Diff lines with tabs were being misidentified as numstat entries in `git log -L` output.
- **Encoding safety**: All `subprocess.run()` calls use `encoding="utf-8", errors="replace"`. Git history may contain non-UTF-8 bytes (Latin-1 commit messages, binary diff fragments); these are replaced with `�` instead of crashing. File reads in `engine.py` and `test_mapper.py` already used `errors="replace"`.
- **Empty-state detection**: All 12 query tools return `{"status": "no_data", "message": "...", "hint": "chisel analyze"}` when the DB has no analysis data, instead of `[]`. `_check_analysis_data()` in `engine.py` calls `storage.has_analysis_data()` (`SELECT 1 FROM code_units LIMIT 1`). Write tools (`analyze`, `update`, `record_result`) and `stats` are unaffected. `stats` adds a `hint` key when all counts are zero. CLI detects this via `_is_no_data()` in `cli.py`.
- **Next-step suggestions**: `next_steps.py` provides `compute_next_steps(tool_name, result)` which returns structured suggestions per tool. Each hint is a dict: `{"tool": "...", "args": {...}, "reason": "..."}` for tool invocations, or `{"action": "...", "reason": "..."}` for non-tool guidance. LLM agents can directly invoke suggested tools without parsing prose. Integrated at the dispatch level in `mcp_server.py` — HTTP responses include `"next_steps": [...]` as a sibling to `"result"`, stdio wraps both in a `{"result": ..., "next_steps": [...]}` envelope. CLI is unaffected. All 16 tools now have registered hint functions — `churn`, `ownership`, `coupling`, `who_reviews`, `history`, `stats`, and `record_result` were added in v0.7.
- **Diagnostic empty responses**: `diff_impact` returns `{"status": "no_changes", "ref": ..., "branch": ..., "message": ...}` instead of bare `[]` when no diff is found. CLI `_is_no_data()` handles both `"no_data"` and `"no_changes"` status values. `_hints_diff_impact` in `next_steps.py` handles the diagnostic dict case, suggesting `diff_impact` with `HEAD~1` or `update`.
- **LLM-prescriptive schema descriptions**: Tool descriptions in `schemas.py` use prescriptive language ("Use when...", "Use after...") to help LLM agents decide which tool to call. Cross-references between related tools (ownership↔who_reviews, analyze↔update, impact↔diff_impact). Coupling description references `stats` for threshold visibility.
- **Inline coupling partners**: `risk_map` includes `"coupling_partners"` (top 3 by co-commit count) in each file entry alongside the breakdown. Data is already fetched in the batch query — no extra DB calls.
- **Triage tool**: Composite `triage` runs `risk_map` (top-N) + `test_gaps` (filtered to top-N files) + `stale_tests` in a single read lock. Returns a dict, not a list, so `limit` is not injected. Summary includes `test_edge_count`, `test_result_count`, and `coupling_threshold` for data quality visibility.
- **Risk-map `_meta` envelope**: `tool_risk_map()` returns `{"files": [...], "_meta": {...}}` instead of a bare list. `_meta` contains: `total_files`, `coupling_threshold`, `total_test_edges`, `total_test_results`, `effective_components` (list of components that vary across files), `uniform_components` (dict of components with identical values + diagnostic reason). This tells LLM agents which risk components are providing real signal vs noise. `_build_risk_meta()` and `_diagnose_uniform()` in `engine.py`. `dispatch_tool()` in `mcp_server.py` applies `limit` to `result["files"]` for dict-wrapped responses. CLI `_limit()` handles both formats.

## Dev Commands

```bash
pip install -e ".[dev]" --break-system-packages   # Arch Linux
pytest tests/ -v --tb=short                       # full suite
chisel analyze .                                  # analyze current project
chisel analyze src/                               # analyze subdirectory only
chisel serve --port 8377                          # HTTP MCP server
```

## Module Dependency Graph

```
engine.py → project.py, storage.py, ast_utils.py, git_analyzer.py, metrics.py, test_mapper.py, impact.py, rwlock.py
project.py → (no internal deps, uses subprocess for git)
test_mapper.py → ast_utils.py, project.py
impact.py → metrics.py
metrics.py → (no internal deps)
cli.py → engine.py, mcp_server.py, mcp_stdio.py
schemas.py → (no internal deps)
mcp_server.py → engine.py, next_steps.py, schemas.py
mcp_stdio.py → engine.py, mcp_server.py, schemas.py
next_steps.py → (no internal deps)
```

## 16 MCP Tools

`analyze`, `impact`, `suggest_tests`, `churn`, `ownership`, `coupling`, `risk_map`, `stale_tests`, `history`, `who_reviews`, `diff_impact`, `update`, `test_gaps`, `record_result`, `stats`, `triage`

Each wired through: engine.tool_*() → CLI subcommand, HTTP POST /call, stdio MCP.

- **`diff_impact`**: Auto-detects changed files/functions from `git diff` and returns impacted tests. Branch-aware: on feature branches diffs against main; on main diffs against HEAD. Returns diagnostic dict (`status: "no_changes"`) with `ref`, `branch`, `message` when no diff is found, instead of bare `[]`.
- **`update`**: Incremental re-analysis — only re-processes changed files and new commits.
- **`test_gaps`**: Finds code units with zero test coverage, prioritized by churn risk. Excludes test files by default.
- **`record_result`**: Records test pass/fail outcomes. Feeds into `suggest_tests` (failure rate boost) and `risk_map` (test instability component).
- **`stats`**: Returns summary counts for all database tables plus `coupling_threshold` (when commits > 0) so LLM agents can diagnose coupling=0.0 results.
- **`triage`**: Combined risk_map + test_gaps + stale_tests for top-N riskiest files. Single command for pre-audit/refactor prioritization. Returns `{top_risk_files, test_gaps, stale_tests, summary}`.
- **`limit` parameter**: All list-returning tools accept `limit` to cap result size. Also applies to dict-wrapped responses with a `files` key (e.g. `risk_map`).
- **Adaptive coupling threshold**: `max(3, int(log2(commits)) + 1)` — logarithmic scaling. Previous `commits // 4` was too aggressive (400 commits → 100 threshold, killing all signal). New formula: 10→4, 50→6, 200→8, 1000→11, 10000→14. Defined in `_coupling_threshold()` in `engine.py`.
