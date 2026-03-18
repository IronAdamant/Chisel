# Chisel

Test impact analysis and code intelligence for LLM agents.

Chisel maps tests to code, code to git history, and answers: **"what to run, what's risky, who touched it."**

## The Problem

An LLM agent changes `engine.py:store_document()`. It then either:
- Runs **all** 287 tests (slow, wasteful), or
- Guesses with `-k "test_store"` (misses regressions)

When multiple agents (or agents + humans) work on the same codebase, changes in one area silently break another. Chisel gives agents the intelligence to understand the blast radius of their changes before they commit.

## Install

```bash
pip install chisel-test-impact
```

Or from source:

```bash
git clone https://github.com/IronAdamant/Chisel.git
cd Chisel
pip install -e .
```

## Use with Claude Code (MCP)

Add to your Claude Code MCP config (`~/.claude/settings.json` or project `.mcp.json`):

```json
{
  "mcpServers": {
    "chisel": {
      "command": "chisel-mcp",
      "env": {
        "CHISEL_PROJECT_DIR": "/path/to/your/project"
      }
    }
  }
}
```

Or run the HTTP server for any MCP-compatible client:

```bash
chisel serve --port 8377
```

Once connected, Claude Code can use all 15 tools directly — `analyze`, `diff_impact`, `suggest_tests`, `risk_map`, and more. Run `analyze` first to build the project graph, then `diff_impact` before each commit to know exactly which tests to run.

## Use with Cursor / Other MCP Clients

Chisel exposes a standard MCP interface. For stdio-based clients:

```bash
pip install chisel-test-impact[mcp]
chisel-mcp
```

For HTTP-based clients, point them at `http://localhost:8377` after running `chisel serve`.

## Quickstart (CLI)

```bash
# Analyze a project (builds all graphs)
chisel analyze .

# What tests are impacted by my current changes?
chisel diff-impact

# What tests should I run for this file?
chisel suggest-tests engine.py

# Who owns this code?
chisel ownership engine.py

# What files always change together?
chisel coupling storage.py

# Which tests are stale?
chisel stale-tests

# Risk heatmap across the project
chisel risk-map

# Incremental update (only re-process changed files)
chisel update

# Find code with no test coverage, sorted by risk
chisel test-gaps
```

## Try It on This Repo

```bash
git clone https://github.com/IronAdamant/Chisel.git
cd Chisel
pip install -e .

chisel analyze .
chisel risk-map
chisel diff-impact
chisel test-gaps
chisel stats
```

## 15 Tools

| Tool | What it does |
|------|-------------|
| `analyze` | Full project scan — code units, tests, git history, edges |
| `update` | Incremental re-analysis of changed files only |
| `impact` | Which tests cover these files/functions? |
| `diff_impact` | Auto-detect changes from `git diff`, return impacted tests |
| `suggest_tests` | Rank tests by relevance + historical failure rate |
| `churn` | How often does this file/function change? |
| `ownership` | Who wrote this code? (blame-based) |
| `who_reviews` | Who maintains this code? (commit-activity-based) |
| `coupling` | What files always change together? |
| `risk_map` | Risk scores for all files (churn + coupling + coverage gaps) |
| `stale_tests` | Tests pointing at code that no longer exists |
| `test_gaps` | Code units with zero test coverage, sorted by risk |
| `history` | Commit history for a specific file |
| `record_result` | Record test pass/fail for future prioritization |
| `stats` | Database summary counts |

## Features

- **Zero dependencies** — stdlib only, works everywhere Python 3.9+ runs
- **Multi-language** — Python, JavaScript/TypeScript, Go, Rust
- **Framework detection** — pytest, Jest, Go test, Rust #[test], Playwright
- **Incremental** — only re-processes changed files via content hashing
- **MCP servers** — both stdio and HTTP for LLM agent integration
- **Risk scoring** — weighted formula: churn, coupling, coverage gaps, author concentration, test instability
- **Branch-aware** — `diff_impact` auto-detects feature branch vs main

## Ecosystem

Chisel works standalone or alongside [Stele](https://github.com/IronAdamant/Stele) for multi-agent code coordination. Chisel handles test intelligence; Stele handles document-level context and conflict prevention.

## License

MIT
