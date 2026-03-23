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
  cli.py            — argparse CLI (17 subcommands). _run_tool() shared handler. Entry point: chisel.cli:main
  schemas.py        — JSON Schema definitions for all 15 tools + dispatch table. Shared by HTTP and stdio servers.
  mcp_server.py     — HTTP MCP server (GET /tools, /health, POST /call). ThreadedHTTPServer. dispatch_tool() shared by both servers.
  mcp_stdio.py      — stdio MCP server (requires optional 'mcp' package). _configure_server() for engine lifecycle mgmt.
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
- **Empty-state detection**: All 11 query tools return `{"status": "no_data", "message": "...", "hint": "chisel analyze"}` when the DB has no analysis data, instead of `[]`. `_check_analysis_data()` in `engine.py` calls `storage.has_analysis_data()` (`SELECT 1 FROM code_units LIMIT 1`). Write tools (`analyze`, `update`, `record_result`) and `stats` are unaffected. `stats` adds a `hint` key when all counts are zero. CLI detects this via `_is_no_data()` in `cli.py`.

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
mcp_server.py → engine.py, schemas.py
mcp_stdio.py → engine.py, mcp_server.py, schemas.py
```

## 15 MCP Tools

`analyze`, `impact`, `suggest_tests`, `churn`, `ownership`, `coupling`, `risk_map`, `stale_tests`, `history`, `who_reviews`, `diff_impact`, `update`, `test_gaps`, `record_result`, `stats`

Each wired through: engine.tool_*() → CLI subcommand, HTTP POST /call, stdio MCP.

- **`diff_impact`**: Auto-detects changed files/functions from `git diff` and returns impacted tests. Branch-aware: on feature branches diffs against main; on main diffs against HEAD.
- **`update`**: Incremental re-analysis — only re-processes changed files and new commits.
- **`test_gaps`**: Finds code units with zero test coverage, prioritized by churn risk. Excludes test files by default.
- **`record_result`**: Records test pass/fail outcomes. Feeds into `suggest_tests` (failure rate boost) and `risk_map` (test instability component).
- **`stats`**: Returns summary counts for all database tables (code units, tests, edges, commits, etc.).
- **`limit` parameter**: All list-returning tools accept `limit` to cap result size.
- **Adaptive coupling threshold**: `max(3, total_commits // 4)` — scales with project maturity.
