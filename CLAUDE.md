# Chisel — CLAUDE.md

Test impact analysis and code intelligence for LLM agents. Zero external dependencies.

## Audience (how to read this project)

- **Primary user**: autonomous and semi-autonomous **coding agents** (MCP, CLI invoked by agents), not a stand-alone dashboard for engineering managers.
- **Typical human**: **solo developer** orchestrating multiple agent sessions, parallel tasks, or long-running analyses — *not* “hand off to another engineer” workflows. Tools like `ownership` / `who_reviews` expose **git-derived context** (blame, commit activity) for debugging and risk heuristics; they are not org-chart features.
- **Multi-agent safety** (see `project.py`, `ProcessLock`, `RWLock`) exists so **several processes** (agents, terminals, CI) can share one `.chisel/` database without corrupting reads/writes — treat this as a **first-class product requirement**, not an edge case.

When editing behavior or docs, prefer: **structured tool results**, **explicit statuses** (`no_data`, `no_changes`, `git_error`), **import-graph + test edges** over relying on git co-change alone, and **clear hints** when MCP would otherwise time out or mis-resolve `project_dir`.

## Architecture

```
chisel/
  engine.py         — Orchestrator. Owns Storage, GitAnalyzer, TestMapper, ImpactAnalyzer, RWLock, ProcessLock.
  project.py        — Multi-agent safety: project root detection, path normalization, storage resolution, cross-process file lock.
  storage.py        — SQLite persistence (WAL mode, 30s busy timeout, write retry). 13 tables (incl. meta, import_edges, branch_co_changes).
  ast_utils.py      — Multi-lang AST extraction (12 languages). CodeUnit dataclass. _extract_brace_lang() shared by brace-delimited langs.
  git_analyzer.py   — Parses git log/blame via subprocess. Branch/diff queries.
  metrics.py        — Pure computation: churn scoring, ownership aggregation, co-change detection, coupling_threshold(). _parse_iso_date shared utility.
  import_graph.py   — Static import_edges between source files (structural coupling).
  test_mapper.py    — Test file discovery, framework detection, dependency extraction, edge building.
  impact.py         — Impact analysis, risk scoring, stale test detection, reviewer suggestions.
  risk_meta.py      — Risk-map _meta diagnostics and dynamic reweighting when components are uniform.
  cli.py            — argparse CLI (18 subcommands). _run_tool() shared handler. Entry point: chisel.cli:main
  schemas.py        — JSON Schema definitions for all 22 tools + dispatch table. Shared by HTTP and stdio servers.
  mcp_server.py     — HTTP MCP server (GET /tools, /health, POST /call). ThreadedHTTPServer. dispatch_tool() shared by both servers.
  mcp_stdio.py      — stdio MCP server (requires optional 'mcp' package). _configure_server() for engine lifecycle mgmt.
  next_steps.py     — Contextual next-step suggestions for MCP tool responses. compute_next_steps() dispatched per tool.
  llm_contract.py   — Agent vocabulary: trust note, status/source constants; MCP tool descriptions reference this (see docs/LLM_CONTRACT.md).
  rwlock.py         — Read-write lock for in-process concurrent access.
```

## Key Design Decisions

- **Zero deps**: stdlib only. `ast` for Python, regex for JS/TS/Go/Rust. `subprocess.run(["git", ...])` for git. Requires Python >= 3.11.
- **FK enforcement disabled** in SQLite: stale test detection relies on orphaned edge refs; re-analysis deletes/recreates code_units freely.
- **Churn formula**: `sum(1 / (1 + days_since_commit))` — recent changes weigh heavily.
- **Risk formula**: `0.35*churn + 0.25*coupling + 0.15*coverage_gap + 0.10*coverage_depth + 0.10*author_concentration + 0.05*test_instability + hidden_risk_factor`. The first 6 components are reweighted when 3+ are uniform. `hidden_risk_factor` (0–0.15) is added separately from dynamic/eval import edge density. Coupling uses `max(git co-change, static import-graph)` breadth. `coverage_gap` is graduated (quantized to 0.25 steps: 0.0/0.25/0.5/0.75/1.0). `coverage_depth = min(distinct_covering_tests/5, 1.0)`. `get_risk_map` may reweight the composite when 3+ components are uniform across files. `proximity_adjustment` optionally reduces `coverage_gap` by import distance to tested code.
- **Co-change ingest**: `compute_co_changes` uses adaptive `min_count` from `coupling_threshold()`; queries use `meta.co_change_query_min` so stored pairs are visible. Branch-only pairs stored in `branch_co_changes` from `merge-base..HEAD`. Commits touching >50 files are skipped (bulk operations).
- **Blame caching**: Cached by file content hash, invalidated on change.
- **Incremental updates**: File content hashes tracked in `file_hashes` table.
- **Persistent connection**: Storage uses a single SQLite connection (`check_same_thread=False`) with RWLock for thread safety.
- **Multi-agent / multi-process safety** (solo dev + parallel agents): `project.py` provides: (1) `detect_project_root()` canonicalizes via git common dir so worktrees share identity, (2) `normalize_path()` ensures consistent relative paths, (3) `resolve_storage_dir()` defaults to project-local `.chisel/` (priority: explicit > env > project-local > ~/.chisel/), (4) `ProcessLock` for cross-process coordination — shared locks for reads, exclusive for writes — so concurrent agent runs and CLI `analyze`/`update` do not interleave destructive storage operations. Cross-platform: `fcntl.flock` on Unix, `LockFileEx` on Windows.
- **SQLite concurrency**: 30s `busy_timeout` + exponential-backoff retry on `_execute` for cross-process SQLITE_BUSY.
- **Ownership vs Reviewers**: `ownership` = blame-based (`role: "original_author"`). `who_reviews` = commit-activity-based (`role: "suggested_reviewer"`). Both are **git-derived signals** for agents (lineage, hot spots); they are not substitutes for team assignment in a solo workflow.
- **Shared constants**: `_SKIP_DIRS` and `_EXTENSION_MAP` live in `ast_utils.py`. `_CODE_EXTENSIONS` in `engine.py` is derived from `_EXTENSION_MAP`. `_SKIP_DIRS` includes `coverage`, `.next`, `.nuxt` to exclude build/test output artifacts.
- **Shared dispatch**: `dispatch_tool()` in `mcp_server.py` is used by both HTTP and stdio servers. Tool schemas and dispatch tables live in `schemas.py`.
- **Edge weighting**: Test edges carry a weight (0.4-1.0) based on file proximity, blended with `sqrt(confidence)` for dynamic requires: `weight = proximity * sqrt(confidence)`. `_compute_proximity_weight()` in `test_mapper.py`.
- **Three-tier edge matching** in `build_test_edges()`: (1) Python import-path matching (`from myapp.utils import foo` → `myapp/utils.py:foo`, requires both path and name match), (2) JS/TS path-based matching (`require('../../src/services/searchService')` → resolves relative path, matches ALL code units in the resolved file), (3) name-only matching (universal fallback). Priority chain ensures precise matching where possible with file-level fallback for JS.
- **JS/TS import binding extraction**: `_extract_js_deps()` extracts binding names from `const X = require('...')` (`_JS_CJS_DEFAULT_RE`), destructured requires `const { X, Y } = require('...')` (`_JS_CJS_DESTRUCTURED_RE`), and ESM defaults `import X from '...'` (`_JS_ESM_DEFAULT_RE`). All include `module_path` for path-based matching. Combined with `_JS_IMPORT_RE` (file-stem name) and `_JS_NAMED_IMPORT_RE` (ESM named imports), this covers CommonJS and ESM patterns.
- **Dynamic require() detection (DynamicRequireChainTracer)**: Chisel detects `require()` patterns invisible to naive static analysis: variable refs (`require(variable)`), template literals, string concatenation, conditionals, and eval-based loading. Variable taint tracking (`const MODULE = './foo'; require(MODULE)`) resolves known variables and upgrades them to `tainted_import` (confidence=1.0). Unknown variables produce `dynamic_import` (confidence=0.3). Confidence is blended into edge weights via `proximity * sqrt(confidence)`. Files with `dynamic_import`/`eval_import` edges accumulate `hidden_risk_factor` in risk scoring: `min(dynamic_edge_count/20, 1.0) * 0.15` added to the 5-component risk formula. `shadow_edge_count` and `dynamic_edge_count` are exposed in `risk_map` output.
- **JS path resolution**: `_resolve_js_module_path(test_file, module_path)` resolves relative imports against the test file's directory. `_matches_js_import_path(code_file, resolved)` strips JS/TS extensions and handles `index.js` barrel imports. `_strip_js_ext()` shared helper. `_JS_EXTENSIONS` frozenset in `test_mapper.py`.
- **AST regex improvements**: C#/Java support nested generics `<A<B>>` and annotations/attributes `@Override`/`[Test]`. Kotlin supports extension functions `fun String.foo()`. C++ supports template functions and destructors `~Foo()`. Swift supports `@objc`-style attributes. Dart supports factory constructors and getters/setters.
- **Jest/Mocha/Vitest test block extraction**: `_JS_JEST_BLOCK_RE` in `ast_utils.py` matches `describe('name', ...)`, `it('name', ...)`, `test('name', ...)` (plus `.only`/`.skip`/`.todo` modifiers) as code units with `unit_type` "test_suite" or "test_case". `_TEST_UNIT_TYPES` in `test_mapper.py` ensures these are recognized as test units regardless of `_is_test_name()`. This enables test edge building for JS/TS projects — the `require()`/`import` dep extraction already worked but was unreachable without test units.
- **Pluggable extractors**: `register_extractor(lang, fn)` in `ast_utils.py` lets users override built-in regex extractors with tree-sitter or LSP-backed ones (installed **outside** Chisel). `_custom_extractors` checked before `_EXTRACTORS` in `extract_code_units()`. **`CHISEL_BOOTSTRAP`** (see `chisel/bootstrap.py`) imports a user module at `ChiselEngine` startup so agents can load registrations without forking CLI. Docs: `docs/CUSTOM_EXTRACTORS.md`.
- **Batch SQL queries**: `storage.py` provides `get_*_batch()` methods for edges, code units, co-changes, churn, and blame. `impact.get_risk_map()` uses these to compute all risk scores in ~5 queries total instead of N*5. `_chunked()` helper splits large batches to stay under SQLite's variable limit.
- **Process-level read locks**: All read tool methods in `engine.py` acquire `_process_lock.shared()` (outer) + `lock.read_lock()` (inner). Writes acquire `_process_lock.exclusive()` + `lock.write_lock()`. This allows concurrent reads from multiple processes while blocking during writes.
- **Unit-churn scaling**: `_UNIT_CHURN_FILE_LIMIT = 2000` in `engine.py`. Repos with more than 2000 code files skip per-function `git log -L` churn (each function spawns a subprocess). File-level churn is always computed. Validated on Grafana (21k files, 62k units in ~3 min).
- **Numstat validation**: `_parse_log_output` in `git_analyzer.py` validates tab-separated fields are digits or `-` before treating them as numstat. Diff lines with tabs were being misidentified as numstat entries in `git log -L` output.
- **Encoding safety**: All `subprocess.run()` calls use `encoding="utf-8", errors="replace"`. Git history may contain non-UTF-8 bytes (Latin-1 commit messages, binary diff fragments); these are replaced with `�` instead of crashing. File reads in `engine.py` and `test_mapper.py` already used `errors="replace"`.
- **Empty-state detection**: All 12 query tools return `{"status": "no_data", "message": "...", "hint": "chisel analyze"}` when the DB has no analysis data, instead of `[]`. `_check_analysis_data()` in `engine.py` calls `storage.has_analysis_data()` (`SELECT 1 FROM code_units LIMIT 1`). Write tools (`analyze`, `update`, `record_result`) and `stats` are unaffected. `stats` adds a `hint` key when all counts are zero. CLI detects this via `_is_no_data()` in `cli.py`.
- **Next-step suggestions**: `next_steps.py` provides `compute_next_steps(tool_name, result)` which returns structured suggestions per tool. Each hint is a dict: `{"tool": "...", "args": {...}, "reason": "..."}` for tool invocations, or `{"action": "...", "reason": "..."}` for non-tool guidance. LLM agents can directly invoke suggested tools without parsing prose. Integrated at the dispatch level in `mcp_server.py` — HTTP responses include `"next_steps": [...]` as a sibling to `"result"`, stdio wraps both in a `{"result": ..., "next_steps": [...]}` envelope. CLI is unaffected. All 16 tools now have registered hint functions — `churn`, `ownership`, `coupling`, `who_reviews`, `history`, `stats`, and `record_result` were added in v0.7.
- **Diagnostic empty responses**: `diff_impact` returns `{"status": "no_changes", ...}` when no diff found. `stale_tests` returns `{"status": "no_edges", "stale_tests": [], ...}` when no test edges exist (so agents can distinguish "no stale tests" from "nothing to evaluate"). CLI `_is_no_data()` handles `"no_data"`, `"no_changes"`, and `"no_edges"` status values.
- **LLM-prescriptive schema descriptions**: Tool descriptions in `schemas.py` use prescriptive language ("Use when...", "Use after...") to help LLM agents decide which tool to call. Cross-references between related tools (ownership↔who_reviews, analyze↔update, impact↔diff_impact). Coupling description references `stats` for threshold visibility.
- **Inline coupling partners**: `risk_map` includes `"coupling_partners"` (top 3 git co-change) and `"import_partners"` (top 3 static import neighbors) per file. The `coupling` tool exposes the same data at the single-file level.
- **Triage tool**: Composite `triage` runs `risk_map` (top-N) + `test_gaps` (filtered to top-N files) + `stale_tests` in a single read lock. Returns a dict, not a list, so `limit` is not injected. Summary includes `test_edge_count`, `test_result_count`, and `coupling_threshold` for data quality visibility.
- **Risk-map `_meta` envelope**: `tool_risk_map()` returns `{"files": [...], "_meta": {...}}`. `_meta` includes `effective_components`, `uniform_components`, `reweighted`, `effective_weights`, `coverage_gap_mode`, `coupling_threshold`, `total_test_edges`, `total_test_results`. Built by `build_risk_meta()` / `apply_risk_reweighting()` in `risk_meta.py`. `dispatch_tool()` applies `limit` to `result["files"]` for dict-wrapped responses.
- **Risk-map test-file exclusion**: `risk_map` and `triage` exclude test files by default (`exclude_tests=True`). Test files always score `coverage_gap=1.0` because edges go FROM test units, never TO test-file code units — including them adds noise and masks real coverage differences between source files. `storage.get_test_file_paths()` fetches distinct test file paths from `test_units`. CLI flag: `--no-exclude-tests`. Aligns with `test_gaps` which already excludes test files by default.

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
engine.py → project.py, storage.py, ast_utils.py, git_analyzer.py, metrics.py, import_graph.py, test_mapper.py, impact.py, risk_meta.py, rwlock.py
project.py → (no internal deps, uses subprocess for git)
import_graph.py → test_mapper.py
test_mapper.py → ast_utils.py, project.py
impact.py → metrics.py
risk_meta.py → metrics.py
metrics.py → (no internal deps)
cli.py → engine.py, mcp_server.py, mcp_stdio.py
schemas.py → (no internal deps)
mcp_server.py → engine.py, next_steps.py, schemas.py
mcp_stdio.py → engine.py, mcp_server.py, schemas.py
next_steps.py → (no internal deps)
```

## 24 MCP Tools

`analyze`, `impact`, `suggest_tests`, `churn`, `ownership`, `coupling`, `risk_map`, `stale_tests`, `history`, `who_reviews`, `diff_impact`, `update`, `test_gaps`, `record_result`, `stats`, `triage`, `start_job`, `job_status`, plus 6 advisory file lock tools

Each wired through: engine.tool_*() → CLI subcommand, HTTP POST /call, stdio MCP.

- **`diff_impact`**: Combines `git diff` changed files with untracked code files (`ls-files --others`) and returns impacted tests. Branch-aware: on feature branches diffs against main; on main diffs against HEAD. Untracked paths use whole-file impact (no function-level diff); untracked files with no DB edges fall back to stem-matching (`source: "working_tree"`). Returns diagnostic dict (`status: "no_changes"`) when neither diff nor untracked code files exist.
- **`update`**: Incremental re-analysis — only re-processes changed files and new commits.
- **`test_gaps`**: Finds code units that have no test coverage, prioritized by churn risk. Excludes test files by default. Accepts `working_tree=True` to also scan untracked (uncommitted) files from disk and include their code units as gaps with churn=0.
- **`suggest_tests`**: Returns impacted tests ranked by relevance. Accepts `fallback_to_all=True` to return all known test files ranked by stem-match similarity when the target file has no test edges (useful for new/unanalyzed files). Accepts `working_tree=True` to also scan untracked files on disk. Output is capped at `_WORKING_TREE_SUGGEST_LIMIT` (30) entries when `working_tree=True` to prevent output explosion in large projects.
- **`coupling`**: Returns both `co_change_partners` (git co-change pairs with commit counts) and `import_partners` (static import neighbors) for a file. Uses adaptive hybrid scoring: co-change dominates when present; import coupling is the sole source in single-author projects; additive boost when both are non-zero. Note: co-change coupling requires multi-author commit history with many small commits — solo projects or bulk-commit workflows will show 0.0 co-change; rely on `import_partners` instead.
- **`record_result`**: Records test pass/fail outcomes. Feeds into `suggest_tests` (failure rate boost) and `risk_map` (test instability component). Also attempts heuristic edge creation via filename matching when no edges exist for the test file — `_create_heuristic_edges()` in `engine.py` uses `_test_to_source_stem()` and `storage.get_code_units_by_file_stem()`.
- **`stats`**: Returns summary counts for all database tables plus `coupling_threshold`, `co_change_query_min`, and `branch_coupling_commits` (when present) for diagnostics.
- **`triage`**: Combined risk_map + test_gaps + stale_tests for top-N riskiest files. Single command for pre-audit/refactor prioritization. Returns `{top_risk_files, test_gaps, stale_tests, summary}`.
- **`limit` parameter**: All list-returning tools accept `limit` to cap result size. Also applies to dict-wrapped responses with a `files` key (e.g. `risk_map`).
- **Adaptive coupling threshold**: `coupling_threshold()` in `metrics.py` — half-log scaling: 10→2, 50→3, 100→4, 200→4, 1000→5, 10000→7.
- **Coverage gap modes**: `risk_map` accepts `coverage_mode="unit"` (default, equal weight per code unit) or `"line"` (weighted by line count so large untested units have proportionally higher gap).
- **Working-tree mode**: `suggest_tests`, `test_gaps`, and `diff_impact` (automatic) analyze untracked files directly from disk, enabling coverage insights during active development before files are committed. `diff_impact` uses stem-matching fallback for untracked files with no DB edges.
