# Chisel

Test impact analysis and code intelligence built for AI coding agents. Zero external dependencies, open source, MIT licensed.

Chisel maps tests to code, code to git history, and answers: **what to run, what's risky, and who touched it.** It runs as an MCP server alongside your agent — Claude Code, Cursor, Windsurf, Cline, or any MCP-compatible client.

![Chisel analyzing a real project — risk map, churn, ownership, test gaps, and agent interpretation](docs/chisel-demo.png)

## What it does

Chisel builds a graph connecting your code, tests, and git history, then answers three questions:

### 1. What to run

You change a function in `src/engine/analyze.rs`. Chisel tells the agent which tests are impacted, through direct edges and the import graph, instead of running the whole suite or guessing from the filename.

### 2. What's risky

Risk scores per file based on churn rate, coupling breadth, test coverage gaps, author concentration, and test instability. A file that changes often, has one author, and no tests? That's your highest risk.

### 3. Who touched it

Blame-based ownership (who wrote it) and commit-activity-based reviewer suggestions (who maintains it). Useful when multiple agents or developers work on the same codebase and you need to understand lineage.

## Why it exists

When multiple LLM agents (or agents + humans) work on the same codebase, changes in one area can silently break another.

Chisel gives AI coding assistants the intelligence to understand the blast radius of their changes before they commit. One agent's refactor doesn't silently regress another agent's work — automated code quality checks that work at the speed of your agent.

## Install

The tool you run is the Rust binary. Build it from this repo. Result checks use the JSON under `tests/fixtures/oracle`. There is no Python package to install.

```bash
git clone https://github.com/IronAdamant/Chisel.git
cd Chisel
cargo build --release
```

The binary is `target/release/chisel`. `chisel serve-mcp` starts the stdio MCP server. A local build also writes `chisel-mcp`, which is the same program.

Storage is a snapshot file at `.chisel/store`. Chisel does not open `chisel.db`. Pass `--json` and branch on the `status` field. `no_data` means run `analyze`. `stale_db` means run `update` or pass `auto_update`.

## Use with Claude Code (MCP)

Add to your Claude Code MCP config (`~/.claude/settings.json` or project `.mcp.json`):

```json
{
  "mcpServers": {
    "chisel": {
      "command": "chisel",
      "args": ["serve-mcp"],
      "env": {
        "CHISEL_PROJECT_DIR": "/path/to/your/project",
        "CHISEL_STORAGE_DIR": "/path/to/your/project/.chisel"
      }
    }
  }
}
```

`chisel serve-mcp` speaks stdio MCP (Content-Length JSON-RPC). The release binary is `chisel`. A copy named `chisel-mcp` or `chisel-mcp.exe` starts the same server. `CHISEL_PROJECT_DIR` and `CHISEL_STORAGE_DIR` are honored when the CLI flags are absent.

Run `analyze` first to build the project graph, then `diff_impact` after edits to see which tests to run. `analyze` with `force` on more than 300 code files returns `auto_queued` and a `job_id`. Poll `job_status`. `--working-tree` reads the worktree. Leave it off to answer from the snapshot.

**Monorepo sharding:** `CHISEL_SHARDS` or `.chisel/shards.toml` gives each shard its own snapshot under `.chisel/shards/<shard>/`. Unscoped reads aggregate. Writes use the longest path prefix. A test in one shard that imports code in another is stored on the parent snapshot and included in `impact` and `suggest_tests`. An unknown shard returns `status: "error"`.

After running tests, call `record_result` so Chisel can track failure rates and test instability over time. Or use `chisel run -- pytest tests/` to run tests and record results automatically.

There is also an installable Claude Code **skill** with the distilled agent protocol: copy [`skills/SKILL.md`](skills/SKILL.md) to `~/.claude/skills/chisel/SKILL.md`.

## Use with Cursor, Windsurf, Cline, or other MCP clients

Stdio MCP for local clients:

```bash
chisel serve-mcp
```

HTTP is a separate JSON API, not an MCP URL. `chisel serve --port 8377` answers `GET /health`, `GET /tools`, and `POST /call`. There is no `/mcp` path.

## Quickstart (CLI)

```bash
# Analyze a project (builds the graph)
chisel analyze .

# What tests are impacted by my current changes?
chisel diff-impact

# What tests should I run for this file?
chisel suggest-tests src/engine/analyze.rs

# Risk heatmap (defaults: line-weighted coverage + proximity adjustment)
chisel risk-map
chisel risk-map --working-tree --auto-update
chisel risk-map --no-proximity --coverage-mode unit   # override defaults

# Find code with no test coverage, sorted by risk
chisel test-gaps --working-tree

# Who owns this code?
chisel ownership src/engine/analyze.rs

# Run tests AND record pass/fail results in one step
chisel run -- pytest tests/

# Incremental update (near-instant when nothing changed)
chisel update
```

## Try it on this repo

```bash
git clone https://github.com/IronAdamant/Chisel.git
cd Chisel
cargo build --release
./target/release/chisel analyze .
chisel risk-map
chisel diff-impact
chisel test-gaps
```

The same binary has been run on flask, requests, cJSON, quixbugs, and the Probatio Separate language trees. Those snapshots were written outside the repos.

## MCP Tools

20 functional tools plus 6 advisory file-lock helpers for multi-agent coordination.

| Tool | What it does |
|------|-------------|
| `analyze` | Full project scan — builds the code/test/git graph. Optional `shard` param for sharded monorepos |
| `update` | Incremental re-analysis of changed files only. Optional `shard` param for sharded monorepos |
| `diff_impact` | Detects your changes from `git diff` and returns impacted tests. `working_tree=true` reads untracked files. `auto_update=true` refreshes a stale snapshot inline |
| `suggest_tests` | Ranks tests by relevance for a given file. Requires `file_path`. `auto_update=true` refreshes a stale snapshot inline |
| `impact` | Which tests cover these files or functions? |
| `risk_map` | Risk scores for all files (churn + coupling + coverage gaps). Defaults: line-weighted coverage + proximity. `working_tree=true` includes untracked files. `exclude_new_file_boost=true` suppresses the new-file boost. `auto_update=true` refreshes a stale snapshot inline |
| `test_gaps` | Code with zero test coverage, sorted by risk. `working_tree=true` includes uncommitted files. `auto_update=true` refreshes a stale snapshot inline |
| `triage` | Top risks + gaps + stale tests in one call. Supports `exclude_new_file_boost` and `auto_update` |
| `churn` | How often does this file or function change? |
| `coupling` | Files that change together or import each other |
| `ownership` | Blame-based — who wrote this code? |
| `who_reviews` | Commit-activity-based — who maintains this code? |
| `stale_tests` | Tests whose edges name code ids that are no longer in the snapshot |
| `history` | Commit history for a file |
| `record_result` | Log test pass/fail outcomes for future prioritization |
| `run` | CLI-only: run tests and auto-record results (pytest, Jest) |
| `stats` | Snapshot counts. It does not return the full result history |
| `start_job` | Run analyze/update in background (avoids MCP timeouts). Optional `shard` param |
| `job_status` | Poll a background job until complete |
| `cancel_job` | Request cooperative cancellation of a running background job |
| `optimize_storage` | Rewrite the snapshot file at `.chisel/store` |

## Features

- **Zero dependencies** — the Rust binary links only the standard library. No crates.io crates. System `git` is required at runtime.
- **Multi-language** — Python, JavaScript/TypeScript, Go, Rust, C#, Java, Kotlin, C/C++, Swift, PHP, Ruby, Dart
- **Framework-aware** — pytest, Jest, Go test, Rust #[test], Playwright, xUnit/NUnit/MSTest, JUnit (incl. parameterized), XCTest + Swift Testing, PHPUnit, RSpec, Minitest, gtest, Dart test
- **Incremental** — `update` recomputes changed files and newer commits. A no-op update sets `edge_rebuild_skipped` and does not run `git blame` or per-function `git log -L`. Above 2000 code files, file-level churn still runs and per-function `git log -L` does not (`unit_churn_skipped`).
- **gitignore-aware** — ignored trees (vendored deps, build output, fixtures) are never scanned; untracked files still are (`CHISEL_INCLUDE_IGNORED=1` to override)
- **Branch-aware** — `diff_impact` auto-detects feature branch vs main. Feature-branch co-change comes from `merge-base..HEAD`. `main` and `master` keep that list empty.
- **Multi-agent safe** — cross-process locks so parallel agents don't corrupt the snapshot
- **MCP + CLI** — stdio MCP (`chisel serve-mcp`) and a JSON HTTP API (`GET /health`, `GET /tools`, `POST /call`), plus a CLI with 28 subcommands
- **CI-friendly** — `error` and `git_error` exit 1. Other statuses, including `no_data` and `stale_db`, exit 0. `examples/github-actions/chisel.yml` builds this binary with Cargo. Do not pip-install Chisel.
- **Monorepo sharding** — one snapshot per shard under `.chisel/shards/<shard>/` (`CHISEL_SHARDS` or `.chisel/shards.toml`). Cross-shard test edges are overlaid from the parent snapshot. `flakiness_trend` appears when dated test results span two weeks.
- **Custom extractors** — set `CHISEL_EXTRACTOR` to an executable. It receives the path and language on stdin and returns a JSON list of units. A non-zero exit or bad JSON falls back to the built-in scanner and records a warning. Unset uses the built-in scanner. The binary does not load tree-sitter or plugins.

## Ecosystem

Chisel sits in the agent loop: impact -> tests -> record results -> refresh analysis. It works standalone or alongside [Stele](https://github.com/IronAdamant/Stele) for semantic code context.

**Docs for this binary:** [AGENTS.md](AGENTS.md) | [Claude Code skill](skills/SKILL.md)

Checked-in oracle JSON lives in `tests/fixtures/oracle`. There is no Python package to install.

## License

MIT
