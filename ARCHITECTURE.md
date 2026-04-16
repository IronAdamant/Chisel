# Chisel — Architecture

**Version:** 0.9.0 | **Python:** >= 3.11 | **Dependencies:** zero (stdlib only)

Test impact analysis and code intelligence **for LLM agents**. The design target is **solo-maintained repos** where **multiple agent sessions or processes** (MCP clients, terminals, CI) may run `analyze` / read tools concurrently — hence **ProcessLock**, **WAL SQLite**, and **normalized paths** as core mechanics, not optional extras.

Questions Chisel answers for agents: **what tests matter for a change**, **where risk concentrates**, **what’s untested or stale**, and **git/blame context** when debugging — not headcount or org workflows.

## Core Data Model

A weighted graph with three edge types stored in SQLite:

```
┌─────────────┐       ┌──────────────┐       ┌──────────────┐
│  Code Unit  │──────▶│  Test Unit   │──────▶│  Git History  │
│ (function,  │ calls │ (test file,  │       │ (commits,     │
│  class,     │◀──────│  test func)  │       │  blame lines, │
│  struct...) │imports│              │       │  churn stats) │
└─────────────┘       └──────────────┘       └──────────────┘
       │                                            │
       └────────────────────────────────────────────┘
                    git log / blame
```

### Entities

| Entity | Source | Key Fields |
|--------|--------|------------|
| `CodeUnit` | AST extraction (12 languages) | file, name, type (function/class/struct/enum/impl), line range |
| `TestUnit` | Test file parsing + framework detection | file, name, framework, line range |
| `CommitRecord` | `git log --numstat` | hash, author, date, files changed, insertions, deletions |
| `BlameBlock` | `git blame --porcelain` | file, line range, commit, author, date |

### Edges

| Edge | How Built | Weight |
|------|-----------|--------|
| `test → code_unit` | Parse test imports + function calls | proximity-based (0.4-1.0): same dir=1.0, sibling=0.8, shared ancestor=0.6, distant=0.4 |
| `code_unit → commit` | `git log -L :funcname:file` or blame | recency-weighted |
| `file → file` (co-change) | Files in same commits (≥3 co-commits) | co-occurrence count |

## Module Architecture

```
ChiselEngine (engine.py) — main orchestrator
  ├── TestMapper (test_mapper.py)
  │     ├── Discover test files, detect framework
  │     ├── Extract imports + call targets (per-language)
  │     └── Build test → code_unit edges with proximity weights
  ├── GitAnalyzer (git_analyzer.py)
  │     ├── Parse `git log` output (no gitpython)
  │     ├── Parse `git blame` output
  │     └── Function-level log via `git log -L`
  ├── Metrics (metrics.py)
  │     ├── Churn scoring: sum(1 / (1 + days_since))
  │     ├── Ownership aggregation from blame blocks
  │     └── Co-change coupling detection
  ├── ImpactAnalyzer (impact.py)
  │     ├── Impacted tests (direct + co-change + import-graph reachability)
  │     ├── Risk scoring (5-component weighted formula)
  │     ├── Stale test detection (orphaned edge refs)
  │     └── Commit-activity hints (who_reviews; heuristic, not team routing)
  ├── AST Utils (ast_utils.py)
  │     ├── Multi-language extraction (12 languages)
  │     ├── Pluggable extractor registry (`register_extractor`; user deps optional)
  │     └── Brace matching with multi-line block comment tracking
  ├── Bootstrap (bootstrap.py) — `CHISEL_BOOTSTRAP` imports user plugin module
  ├── Storage (storage.py, SQLite WAL)
  │     ├── 17 tables, single persistent connection, cross-process busy-retry
  │     ├── Batch query methods for N+1 elimination
  │     └── Incremental update via content hashes
  ├── Project (project.py)
  │     ├── Project root detection (worktree-aware)
  │     ├── Path normalization (cross-platform)
  │     └── ProcessLock (fcntl on Unix, LockFileEx on Windows)
  ├── RWLock (rwlock.py) — in-process read/write lock
  ├── Schemas (schemas.py) — JSON Schema defs + dispatch table
  └── APIs
        ├── CLI (cli.py) — 28 subcommands
        ├── HTTP (mcp_server.py) — GET /tools, /health; POST /call
        └── stdio MCP (mcp_stdio.py) — for Claude Desktop/Cursor
```

## SQLite Tables (17)

```sql
code_units           — functions, classes, structs (id = file:name:type)
test_units           — test functions (id = file:name)
test_edges           — test → code links with edge_type and weight
commits              — git commit metadata
commit_files         — per-file stats per commit
blame_cache          — cached git blame, keyed by content hash
co_changes           — file pairs that change together
branch_co_changes    — branch-only co-change pairs (merge-base..HEAD)
churn_stats          — churn scores per file and per function
file_hashes          — content hashes for incremental analysis
test_results         — recorded pass/fail outcomes for prioritization
import_edges         — static import edges between source files
static_test_imports  — cached test-file import scan results
bg_jobs              — background analyze/update jobs with progress_pct
job_events           — per-phase progress events for background jobs
file_locks           — advisory file locks for multi-agent coordination
meta                 — key-value project metadata (includes project_fingerprint)
```

## 26 MCP Tools (20 functional + 6 file-lock)

| Tool | Input | Output |
|------|-------|--------|
| `analyze` | directory, force, shard | full rebuild stats |
| `update` | shard | incremental update stats |
| `impact` | files, functions | affected tests + scores |
| `diff_impact` | ref, working_tree, auto_update | affected tests from git diff |
| `suggest_tests` | file_path, fallback_to_all, working_tree, auto_update | ranked tests by relevance + failure rate |
| `churn` | file, unit_name | churn score, commits, authors |
| `ownership` | file | author breakdown (blame-based, role=original_author) |
| `who_reviews` | file | reviewer suggestions (activity-based, role=suggested_reviewer) |
| `coupling` | file, min_count | co-change + import partners |
| `risk_map` | directory, exclude_tests, working_tree, auto_update | risk scores (batch-computed) |
| `stale_tests` | — | tests pointing at removed code |
| `test_gaps` | file, directory, working_tree, limit, auto_update | untested code units by churn risk |
| `history` | file | commit timeline |
| `record_result` | test_id, passed, duration | store for future prioritization |
| `stats` | — | database summary counts |
| `triage` | directory, top_n, exclude_tests, working_tree, auto_update | combined risk_map + test_gaps + stale_tests |
| `optimize_storage` | — | `PRAGMA optimize` + conditional `VACUUM` |
| `start_job` | kind, directory, force, shard | background job id |
| `job_status` | job_id | job status + progress_pct + result |
| `cancel_job` | job_id | cancel a running background job |
| `acquire_file_lock` | file_path, agent_id, ttl, purpose | advisory lock for multi-agent coordination |
| `release_file_lock` | file_path, agent_id | release advisory lock |
| `refresh_file_lock` | file_path, agent_id, ttl | extend lock TTL |
| `check_file_lock` | file_path | lock status query |
| `check_locks` | file_paths | batch lock status query |
| `list_file_locks` | agent_id | list all active locks (optionally filtered) |

All list-returning tools accept a `limit` parameter to cap result size.

## Key Design Decisions

- **Zero deps**: stdlib only. `ast` for Python, regex for 11 other languages. `subprocess.run(["git", ...])` for git. Requires Python >= 3.11.
- **Pluggable extractors**: `register_extractor(lang, fn)` overrides built-in regex with tree-sitter/LSP. Zero-dep — just callable hooks.
- **Proximity-based edge weights**: 0.4-1.0 based on directory distance. Python import-path matching (`from myapp.utils import foo` → `myapp/utils.py:foo`) takes priority.
- **Risk formula**: `0.35*churn + 0.25*coupling + 0.15*coverage_gap + 0.10*coverage_depth + 0.10*author_concentration + 0.05*test_instability + hidden_risk_factor + new_file_boost`
- **Batch queries**: `get_risk_map()` fetches all data in ~5 queries. `_chunked()` helper stays under SQLite's 999-variable limit.
- **Working-tree support**: `risk_map`, `test_gaps`, `diff_impact`, `suggest_tests`, and `triage` can analyze uncommitted files on disk.
- **Heuristic edge backfill**: `analyze`/`update` automatically creates filename-based heuristic test edges for test files missing static edges.
- **Project fingerprint**: `meta.project_fingerprint` stores the canonical project root to warn against accidental cross-project DB reuse.
- **Background jobs**: `start_job` / `job_status` run long analyses out-of-band; `progress_pct` (0–100) is reported in status.
- **Churn formula**: `sum(1 / (1 + days_since_commit))` — recent changes weigh heavily.
- **Co-change threshold**: Adaptive `max(3, total_commits // 4)`. Commits touching >50 files skipped.
- **Blame caching**: Cached by file content hash, invalidated on change.
- **Incremental analysis**: File content hashes tracked in `file_hashes` table.
- **FK enforcement disabled**: Stale test detection relies on orphaned edge refs.
- **Cross-platform locking**: `ProcessLock` uses `fcntl.flock` (Unix) / `LockFileEx` via ctypes (Windows). Shared locks for reads, exclusive for writes.
- **Thread safety**: RWLock (in-process) + ProcessLock (cross-process). Lock order: process lock outer, RWLock inner.
- **Multi-line block comments**: `_strip_strings_and_comments` tracks `/* */` state across lines for correct brace matching.
- **Unit-churn scaling**: `_UNIT_CHURN_FILE_LIMIT = 2000` — repos exceeding this skip per-function `git log -L` (each function spawns a subprocess, O(n*m)). File-level churn always computed. Validated on Grafana (21k files, 62k units in ~3 min).
- **Numstat validation**: `_parse_log_output` validates tab-separated fields are digits or `-` before treating lines as numstat. Prevents diff lines with tabs from crashing the parser.
- **Empty-state detection**: Query tools return `{"status": "no_data", ...}` instead of `[]` when no analysis data exists. `storage.has_analysis_data()` + `engine._check_analysis_data()`. CLI handles via `_is_no_data()`.
- **Monorepo sharding**: `CHISEL_SHARDS` env var or `.chisel/shards.toml` splits data across per-directory SQLite databases. `_shard_for_path()` routes files; query tools aggregate with `_with_shard()` context manager.

## Supported Languages (12)

| Language | AST Method | Test Frameworks |
|----------|-----------|-----------------|
| Python | `ast` module (regex fallback) | pytest |
| JavaScript/TypeScript | Regex | Jest, Playwright |
| Go | Regex | Go test |
| Rust | Regex | `#[test]`, `#[cfg(test)]` |
| C# | Regex (nested generics, attributes) | xUnit, NUnit, MSTest |
| Java | Regex (annotations, nested generics) | JUnit |
| Kotlin | Regex (extension functions) | JUnit |
| C/C++ | Regex (templates, destructors) | gtest, Catch2 |
| Swift | Regex (@attributes) | XCTest |
| PHP | Regex | PHPUnit |
| Ruby | Keyword-based block detection | RSpec, Minitest |
| Dart | Regex (factory, getters/setters) | Dart test |

## File Structure

```
Chisel/
├── chisel/
│   ├── __init__.py           # version
│   ├── engine.py             # orchestrator
│   ├── storage.py            # SQLite persistence
│   ├── ast_utils.py          # multi-language AST extraction + plugin registry
│   ├── bootstrap.py          # CHISEL_BOOTSTRAP optional user module
│   ├── git_analyzer.py       # git log/blame parsing
│   ├── metrics.py            # churn, ownership, co-change computation
│   ├── test_mapper.py        # test discovery, deps, edge building
│   ├── impact.py             # impact analysis, risk scoring, reviewers
│   ├── project.py            # project root, path normalization, ProcessLock
│   ├── rwlock.py             # read-write lock
│   ├── schemas.py            # JSON Schema defs + dispatch table
│   ├── cli.py                # argparse CLI (28 subcommands)
│   ├── mcp_server.py         # HTTP MCP server
│   └── mcp_stdio.py          # stdio MCP server
├── tests/
│   ├── conftest.py           # shared fixtures (temp git repos)
│   ├── test_ast_utils.py     # AST extraction tests
│   ├── test_storage.py       # storage CRUD + batch query tests
│   ├── test_git_analyzer.py  # git parsing tests
│   ├── test_metrics.py       # churn, co-change tests
│   ├── test_test_mapper.py   # framework detection, edge building tests
│   ├── test_impact.py        # impact, risk, ownership tests
│   ├── test_engine.py        # integration tests
│   ├── test_cli.py           # CLI handler tests
│   ├── test_mcp_server.py    # HTTP server tests
│   ├── test_mcp_stdio.py     # stdio server tests
│   ├── test_rwlock.py        # concurrency tests
│   └── test_project.py       # project root, path, lock tests
├── wiki-local/               # detailed docs (spec, glossary, index)
├── pyproject.toml
├── CLAUDE.md                 # agent instructions
├── ARCHITECTURE.md           # this file
├── CHANGELOG.md
├── CONTRIBUTING.md
├── COMPLETE_PROJECT_DOCUMENTATION.md
├── LLM_Development.md
└── README.md
```
