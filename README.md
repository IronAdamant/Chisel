# Chisel

Test impact analysis and code intelligence for LLM agents.

Chisel maps tests to code, code to git history, and answers: **"what to run, what's risky, who touched it."**

## The Problem

An LLM agent changes `engine.py:store_document()`. It then either:
- Runs **all** 287 tests (slow, wasteful), or
- Guesses with `-k "test_store"` (misses regressions)

Chisel solves this with a graph connecting tests, code units, and git history.

## Install

```bash
pip install -e .
```

## Quickstart

```bash
# Analyze a project (builds all graphs)
chisel analyze .

# What tests should I run after editing engine.py?
chisel impact engine.py

# Who owns this code?
chisel ownership engine.py

# What files always change together?
chisel coupling storage.py

# Which tests are stale?
chisel stale-tests

# Risk heatmap
chisel risk-map

# Start HTTP MCP server for Claude Code
chisel serve --port 8377
```

## Features

- **Zero dependencies** — stdlib only, works everywhere Python 3.9+ runs
- **Multi-language** — Python, JavaScript/TypeScript, Go, Rust
- **10 tools**: impact, suggest-tests, churn, ownership, coupling, risk-map, stale-tests, history, who-reviews, analyze
- **Incremental** — only re-processes changed files
- **MCP servers** — HTTP and stdio for LLM agent integration
- **Framework detection** — pytest, Jest, Go test, Rust #[test], Playwright

## License

MIT
