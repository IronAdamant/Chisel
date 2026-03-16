# Chisel — Test Impact & Code Intelligence for LLM Agents

Zero-dependency companion to [Stele](../Stele/). Maps tests to code, code to git history, and answers: "what to run, what's risky, who touched it."

## Problem

An LLM agent changes `engine.py:store_document()`. It then either:
- Runs **all** 287 tests (slow, wasteful), or
- Guesses with `-k "test_store"` (misses regressions)

It also has no idea whether this function is stable (untouched for months) or a hotspot (changed 15 times this week).

## Core Data Model

A single weighted graph with three edge types layered on Stele's symbol graph:

```
┌─────────────┐       ┌──────────────┐       ┌──────────────┐
│  Code Unit  │──────▶│  Test Unit   │──────▶│  Git History  │
│ (function,  │ calls │ (test file,  │       │ (commits,     │
│  class,     │◀──────│  test func)  │       │  blame lines, │
│  module)    │imports│              │       │  churn stats) │
└─────────────┘       └──────────────┘       └──────────────┘
       │                                            │
       └────────────────────────────────────────────┘
                    git log / blame
```

### Entities

| Entity | Source | Key Fields |
|--------|--------|------------|
| `CodeUnit` | Stele symbol graph + AST | file, name, type (func/class/module), line range |
| `TestUnit` | Test file AST parsing | file, name, framework (pytest/jest/go test), line range |
| `CommitRecord` | `git log --numstat` | hash, author, date, files changed, insertions, deletions |
| `BlameBlock` | `git blame --porcelain` | file, line range, commit, author, date |

### Edges

| Edge | How Built | Weight |
|------|-----------|--------|
| `test → code_unit` | Parse test imports + function calls via AST | call count |
| `code_unit → commit` | `git log -L :funcname:file` or blame | recency-weighted |
| `file → file` (co-change) | Files that appear in same commits | co-occurrence count |

## Architecture

```
Chisel (engine.py) — main orchestrator
  ├── TestMapper (test_mapper.py)
  │     ├── Parse test files, detect framework (pytest/jest/go/rust)
  │     ├── Extract imports + call targets
  │     └── Build test → code_unit edges
  ├── GitAnalyzer (git_analyzer.py)
  │     ├── Parse `git log` output (no gitpython dep)
  │     ├── Parse `git blame` output
  │     ├── Compute churn scores, ownership, co-change coupling
  │     └── Build code_unit → commit edges
  ├── ImpactAnalyzer (impact.py)
  │     ├── Given changed files/functions → affected tests (via test edges)
  │     ├── Risk score = f(churn, recency, coupling breadth)
  │     ├── Ownership query ("who last touched this, how often")
  │     └── Stale test detection (tests that cover dead/removed code)
  ├── Storage (storage.py, SQLite)
  │     ├── Cached graph edges
  │     ├── Git history snapshots
  │     └── Incremental update (only re-parse changed files)
  └── APIs
        ├── CLI (cli.py)
        ├── MCP stdio (mcp_stdio.py) — for Claude Desktop
        └── HTTP (mcp_server.py) — for Claude Code

Stele integration:
  └── Reads Stele's symbol graph (optional, falls back to own AST parsing)
```

## SQLite Tables

```sql
-- Code units (functions, classes, modules)
CREATE TABLE code_units (
    id TEXT PRIMARY KEY,          -- file:name:type
    file_path TEXT NOT NULL,
    name TEXT NOT NULL,
    unit_type TEXT NOT NULL,       -- func, class, module
    line_start INTEGER,
    line_end INTEGER,
    content_hash TEXT,
    updated_at TEXT
);

-- Test units
CREATE TABLE test_units (
    id TEXT PRIMARY KEY,
    file_path TEXT NOT NULL,
    name TEXT NOT NULL,
    framework TEXT,               -- pytest, jest, go, rust, playwright
    line_start INTEGER,
    line_end INTEGER,
    content_hash TEXT,
    updated_at TEXT
);

-- Test → code_unit edges
CREATE TABLE test_edges (
    test_id TEXT REFERENCES test_units(id),
    code_id TEXT REFERENCES code_units(id),
    edge_type TEXT,               -- import, call, fixture
    weight REAL DEFAULT 1.0,
    PRIMARY KEY (test_id, code_id, edge_type)
);

-- Git commit records
CREATE TABLE commits (
    hash TEXT PRIMARY KEY,
    author TEXT,
    author_email TEXT,
    date TEXT,
    message TEXT
);

-- Commit → file changes
CREATE TABLE commit_files (
    commit_hash TEXT REFERENCES commits(hash),
    file_path TEXT,
    insertions INTEGER,
    deletions INTEGER,
    PRIMARY KEY (commit_hash, file_path)
);

-- Blame cache (per file, invalidated by content hash)
CREATE TABLE blame_cache (
    file_path TEXT,
    line_start INTEGER,
    line_end INTEGER,
    commit_hash TEXT,
    author TEXT,
    author_email TEXT,
    date TEXT,
    content_hash TEXT,            -- of the file when blame was run
    PRIMARY KEY (file_path, line_start)
);

-- Co-change coupling
CREATE TABLE co_changes (
    file_a TEXT,
    file_b TEXT,
    co_commit_count INTEGER,
    last_co_commit TEXT,
    PRIMARY KEY (file_a, file_b)
);

-- Churn summary (materialized view, rebuilt on analyze)
CREATE TABLE churn_stats (
    file_path TEXT,
    unit_name TEXT,               -- nullable (file-level if null)
    commit_count INTEGER,
    distinct_authors INTEGER,
    total_insertions INTEGER,
    total_deletions INTEGER,
    last_changed TEXT,
    churn_score REAL,             -- weighted: recent changes count more
    PRIMARY KEY (file_path, unit_name)
);
```

## Key Queries (MCP Tools)

| Tool | Input | Output |
|------|-------|--------|
| `impact` | changed files/functions | affected test list + risk score |
| `suggest_tests` | file path or diff | ordered test list to run |
| `churn` | file or function | churn score, commit count, last changed |
| `ownership` | file or function | author breakdown (% of blame lines) |
| `coupling` | file path | co-change partners + strength |
| `risk_map` | directory | heatmap of risk scores |
| `stale_tests` | — | tests covering removed/renamed code |
| `history` | function name | commit timeline with diffs |
| `who_reviews` | file or diff | suggested reviewers by ownership |
| `analyze` | directory | full rebuild of git + test graph |

## Design Decisions

- **Zero deps**: stdlib only. `ast` for Python, regex for JS/TS/Go/Rust. `subprocess.run(["git", ...])` for git.
- **Git is the source of truth**: No gitpython. Parse `git log --format=...` and `git blame --porcelain` text output.
- **Incremental**: Track file content hashes. Only re-parse test files and re-blame files that changed since last run.
- **Stele optional**: Can read Stele's SQLite DB directly for symbol graph if available. Falls back to own lightweight AST extraction.
- **Framework detection**: Auto-detect test framework from file patterns (`test_*.py`, `*.test.js`, `*_test.go`) and imports (`import pytest`, `describe(`).
- **Churn score formula**: `sum(1 / (1 + days_since_commit))` — recent changes weigh heavily, old changes decay.
- **Co-change threshold**: Only store pairs with >= 3 co-commits to avoid noise.
- **Risk score**: `0.4 * churn_norm + 0.3 * coupling_breadth_norm + 0.2 * (1 - test_coverage) + 0.1 * author_concentration`. Higher = riskier to change.
- **Blame caching**: Blame is expensive. Cache by file content hash, invalidate on change.
- **Thread safety**: Same RWLock pattern as Stele for concurrent MCP access.

## CLI Examples

```bash
# Analyze a project (builds all graphs)
chisel analyze .

# What tests should I run after editing engine.py?
chisel impact engine.py
# → test_engine.py (direct import, 23 calls)
# → test_integration.py (transitive via storage.py)
# → Risk: HIGH (churn=0.82, 15 commits in 7 days)

# Who owns this code?
chisel ownership stele/engine.py
# → IronAdamant: 94% (blame lines)
# → Last changed: 2026-03-16

# What files always change together?
chisel coupling stele/storage.py
# → stele/engine.py (18 co-commits)
# → stele/session_storage.py (12 co-commits)

# Which tests are stale?
chisel stale-tests
# → test_old_feature.py:test_removed_api — calls `old_function` (removed in abc123)
```

## File Structure

```
Chisel/
├── chisel/
│   ├── __init__.py           # version
│   ├── engine.py             # orchestrator
│   ├── test_mapper.py        # test file parsing, edge building
│   ├── git_analyzer.py       # git log/blame parsing, churn/ownership
│   ├── impact.py             # impact analysis, risk scoring
│   ├── storage.py            # SQLite persistence
│   ├── ast_utils.py          # lightweight AST helpers (multi-lang)
│   ├── cli.py                # CLI entry point
│   ├── mcp_server.py         # HTTP MCP server
│   └── mcp_stdio.py          # stdio MCP server
├── tests/
│   ├── test_test_mapper.py
│   ├── test_git_analyzer.py
│   ├── test_impact.py
│   └── test_storage.py
├── pyproject.toml
├── CLAUDE.md
├── ARCHITECTURE.md
└── README.md
```

## Integration with Stele

Chisel can optionally connect to a Stele instance for richer analysis:

```python
# If Stele DB exists, read symbol graph directly
stele_db = Path(".stele/index.db")
if stele_db.exists():
    symbols = read_stele_symbols(stele_db)  # direct SQLite read
    # Enriches test→code edges with Stele's cross-file symbol resolution
```

This avoids duplicating Stele's symbol extraction while adding the test/git layer on top.
