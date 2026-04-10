# Chisel

Test impact analysis and code intelligence built for AI coding agents. Zero external dependencies, open source, MIT licensed.

Chisel maps tests to code, code to git history, and answers: **what to run, what’s risky, and who touched it.** It runs as an MCP server alongside your agent — Claude Code, Cursor, Windsurf, Cline, or any MCP-compatible client.

![Chisel analyzing a real project — risk map, churn, ownership, test gaps, and agent interpretation](docs/chisel-demo.png)

## What it does

Chisel builds a graph connecting your code, tests, and git history, then answers three questions:

### 1. What to run

You change `engine.py:store_document()`. Instead of running all 287 tests or guessing with `-k “test_store”`, Chisel tells the agent exactly which tests are impacted — through direct edges and transitive import-chain coupling.

### 2. What’s risky

Risk scores per file based on churn rate, coupling breadth, test coverage gaps, author concentration, and test instability. A file that changes often, has one author, and no tests? That’s your highest risk.

### 3. Who touched it

Blame-based ownership (who wrote it) and commit-activity-based reviewer suggestions (who maintains it). Useful when multiple agents or developers work on the same codebase and you need to understand lineage.

## Why it exists

When multiple LLM agents (or agents + humans) work on the same codebase, changes in one area can silently break another.

Chisel gives AI coding assistants the intelligence to understand the blast radius of their changes before they commit. One agent’s refactor doesn’t silently regress another agent’s work — automated code quality checks that work at the speed of your agent.

## Install

Available on [PyPI](https://pypi.org/project/chisel-test-impact/):

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
  “mcpServers”: {
    “chisel”: {
      “command”: “chisel-mcp”,
      “env”: {
        “CHISEL_PROJECT_DIR”: “/path/to/your/project”
      }
    }
  }
}
```

Run `analyze` first to build the project graph, then `diff_impact` after edits to see which tests to run. For large repos, run `chisel analyze` in a terminal instead of through MCP to avoid timeouts.

## Use with Cursor, Windsurf, Cline, or other MCP clients

Chisel exposes a standard MCP interface. For stdio-based clients:

```bash
pip install chisel-test-impact[mcp]
chisel-mcp
```

For HTTP-based clients:

```bash
chisel serve --port 8377
```

## Quickstart (CLI)

```bash
# Analyze a project (builds the graph)
chisel analyze .

# What tests are impacted by my current changes?
chisel diff-impact

# What tests should I run for this file?
chisel suggest-tests engine.py

# Risk heatmap across the project
chisel risk-map

# Find code with no test coverage, sorted by risk
chisel test-gaps

# Who owns this code?
chisel ownership engine.py

# Incremental update (only re-process changed files)
chisel update
```

## Try it on this repo

```bash
git clone https://github.com/IronAdamant/Chisel.git
cd Chisel
pip install -e .

chisel analyze .
chisel risk-map
chisel diff-impact
chisel test-gaps
```

## MCP Tools

18 core tools plus 6 advisory file-lock helpers for multi-agent coordination.

| Tool | What it does |
|------|-------------|
| `analyze` | Full project scan — builds the code/test/git graph |
| `update` | Incremental re-analysis of changed files only |
| `diff_impact` | Detects your changes from `git diff` and returns impacted tests |
| `suggest_tests` | Ranks tests by relevance for a given file |
| `impact` | Which tests cover these files or functions? |
| `risk_map` | Risk scores for all files (churn + coupling + coverage gaps) |
| `test_gaps` | Code with zero test coverage, sorted by risk |
| `triage` | Top risks + gaps + stale tests in one call |
| `churn` | How often does this file or function change? |
| `coupling` | Files that change together or import each other |
| `ownership` | Blame-based — who wrote this code? |
| `who_reviews` | Commit-activity-based — who maintains this code? |
| `stale_tests` | Tests pointing at code that no longer exists |
| `history` | Commit history for a file |
| `record_result` | Log test pass/fail outcomes for future prioritization |
| `stats` | Database summary and diagnostic counts |
| `start_job` | Run analyze/update in background (avoids MCP timeouts) |
| `job_status` | Poll a background job until complete |

## Features

- **Zero dependencies** — stdlib only, Python 3.11+, works anywhere
- **Multi-language** — Python, JavaScript/TypeScript, Go, Rust, C#, Java, Kotlin, C/C++, Swift, PHP, Ruby, Dart
- **Framework-aware** — pytest, Jest, Go test, Rust #[test], Playwright, xUnit/NUnit/MSTest, JUnit, XCTest, PHPUnit, RSpec, Minitest, gtest, Dart test
- **Incremental** — only re-processes changed files, not the whole repo
- **Branch-aware** — `diff_impact` auto-detects feature branch vs main
- **Multi-agent safe** — cross-process locks so parallel agents don’t corrupt the graph
- **MCP + CLI** — stdio and HTTP MCP servers, plus a full CLI with 18 subcommands
- **Custom extractors** — plug in tree-sitter or LSP via `register_extractor()` if you need it

## Ecosystem

Chisel sits in the agent loop: impact -> tests -> record results -> refresh analysis. It works standalone or alongside [Stele](https://github.com/IronAdamant/Stele) for semantic code context.

**Docs:** [Agent playbook](docs/AGENT_PLAYBOOK.md) | [Zero-dependency policy](docs/ZERO_DEPS.md) | [Custom extractors](docs/CUSTOM_EXTRACTORS.md)

## License

MIT
