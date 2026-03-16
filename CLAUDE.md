# Chisel — CLAUDE.md

Test impact analysis and code intelligence for LLM agents. Zero external dependencies.

## Architecture

```
chisel/
  engine.py         — Orchestrator. Owns Storage, GitAnalyzer, TestMapper, ImpactAnalyzer, RWLock.
  storage.py        — SQLite persistence (WAL mode). 9 tables. Uses _fetchall/_fetchone/_execute helpers.
  ast_utils.py      — Multi-lang AST extraction (Python/JS/TS/Go/Rust). CodeUnit dataclass. _extract_brace_lang() shared by JS/TS/Go/Rust.
  git_analyzer.py   — Parses git log/blame via subprocess. Computes churn, ownership, co-change.
  test_mapper.py    — Test file discovery, framework detection, dependency extraction, edge building.
  impact.py         — Impact analysis, risk scoring, stale test detection, reviewer suggestions.
  cli.py            — argparse CLI (12 subcommands). Entry point: chisel.cli:main
  mcp_server.py     — HTTP MCP server (GET /tools, /health, POST /call). ThreadedHTTPServer. dispatch_tool() shared by both servers.
  mcp_stdio.py      — stdio MCP server (requires optional 'mcp' package). _configure_server() for engine lifecycle mgmt.
  rwlock.py         — Read-write lock for concurrent access.
```

## Key Design Decisions

- **Zero deps**: stdlib only. `ast` for Python, regex for JS/TS/Go/Rust. `subprocess.run(["git", ...])` for git.
- **FK enforcement disabled** in SQLite: stale test detection relies on orphaned edge refs; re-analysis deletes/recreates code_units freely.
- **Churn formula**: `sum(1 / (1 + days_since_commit))` — recent changes weigh heavily.
- **Risk formula**: `0.4*churn + 0.3*coupling_breadth + 0.2*(1-test_coverage) + 0.1*author_concentration`
- **Co-change threshold**: Only pairs with >= 3 co-commits stored.
- **Blame caching**: Cached by file content hash, invalidated on change.
- **Incremental updates**: File content hashes tracked in `file_hashes` table.
- **Persistent connection**: Storage uses a single SQLite connection (`check_same_thread=False`) with RWLock for thread safety.
- **Ownership vs Reviewers**: `ownership` = blame-based (who wrote the code, `role: "original_author"`). `who_reviews` = commit-activity-based (who maintains it, `role: "suggested_reviewer"`).
- **Shared constants**: `_SKIP_DIRS` and `_EXTENSION_MAP` live in `ast_utils.py`. `_CODE_EXTENSIONS` in `engine.py` is derived from `_EXTENSION_MAP`.
- **Shared dispatch**: `dispatch_tool()` in `mcp_server.py` is used by both HTTP and stdio servers to avoid duplicated dispatch logic.

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
engine.py → storage.py, ast_utils.py, git_analyzer.py, test_mapper.py, impact.py, rwlock.py
test_mapper.py → ast_utils.py
impact.py → storage.py, git_analyzer.py
cli.py → engine.py, mcp_server.py, mcp_stdio.py
mcp_server.py → engine.py
mcp_stdio.py → engine.py, mcp_server.py
```

## 13 MCP Tools

`analyze`, `impact`, `suggest_tests`, `churn`, `ownership`, `coupling`, `risk_map`, `stale_tests`, `history`, `who_reviews`, `diff_impact`, `update`, `test_gaps`

Each wired through: engine.tool_*() → CLI subcommand, HTTP POST /call, stdio MCP.

- **`diff_impact`**: Auto-detects changed files/functions from `git diff` and returns impacted tests. No manual file list needed.
- **`update`**: Incremental re-analysis — only re-processes changed files and new commits.
- **`test_gaps`**: Finds code units with zero test coverage, prioritized by churn risk. Use after `analyze` to see what tests need writing.
