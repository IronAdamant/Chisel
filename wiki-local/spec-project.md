# Chisel -- Project Specification

## Overview

Chisel is a test impact analysis and code intelligence tool designed for LLM agents. It maps tests to code, code to git history, and answers: "what to run, what's risky, who touched it."

## Goals

1. **Targeted test selection**: Given a set of changed files or functions, identify the minimal set of tests that need to run to catch regressions.
2. **Risk visibility**: Surface which files are high-risk based on churn, coupling breadth, test coverage gaps, and author concentration.
3. **Ownership and review intelligence**: Identify who wrote the code (blame-based ownership) and who is best positioned to review changes (commit-activity-based).
4. **Change coupling detection**: Identify files that frequently change together, revealing hidden architectural dependencies.
5. **Stale test detection**: Find tests that reference code units that no longer exist.
6. **Zero dependencies**: Run anywhere Python 3.9+ is available with no pip installs beyond Chisel itself.
7. **LLM agent integration**: Expose all capabilities as MCP tools via HTTP and stdio servers.
8. **Incremental analysis**: Track file content hashes and only re-process changed files.

## Non-Goals

- **Test execution**: Chisel does not run tests. It tells you which tests to run.
- **Full static analysis**: Chisel uses lightweight AST extraction, not a type checker or full semantic analysis engine. Cross-file resolution depends on name matching, not type inference.
- **Language Server Protocol**: Chisel is not an LSP server. It provides batch analysis and MCP tool access.
- **Real-time file watching**: Chisel does not watch the filesystem for changes. Analysis is triggered explicitly via `chisel analyze` or the MCP `analyze` tool.
- **Multi-repo support**: Each Chisel instance operates on a single git repository.
- **Branch-aware analysis**: Chisel analyzes the current working tree and git history. It does not compare across branches.

## Supported Languages

| Language | AST Method | Extensions | Test Frameworks |
|----------|-----------|------------|-----------------|
| Python | `ast` module (regex fallback on SyntaxError) | `.py`, `.pyw` | pytest |
| JavaScript | Regex patterns | `.js`, `.jsx`, `.mjs`, `.cjs` | Jest |
| TypeScript | Regex patterns (same as JS) | `.ts`, `.tsx` | Jest, Playwright |
| Go | Regex patterns | `.go` | Go test (`Test*`, `Benchmark*`) |
| Rust | Regex patterns | `.rs` | `#[test]`, `#[cfg(test)]` |

### Code Unit Types Extracted

- **Python**: `function`, `async_function`, `class` (methods qualified as `ClassName.method_name`)
- **JavaScript/TypeScript**: `function` (named functions, arrow functions), `class`
- **Go**: `function` (including methods with receivers), `struct`, `interface`
- **Rust**: `function`, `struct`, `enum`, `impl`

## MCP Tool Specifications

All 10 tools are accessible through three interfaces: CLI subcommands, HTTP POST /call, and stdio MCP. Each tool maps to an `engine.tool_*()` method.

### analyze

- **Input**: `directory` (optional string), `force` (optional boolean)
- **Output**: Dict with keys: `code_files_scanned`, `code_units_found`, `test_files_found`, `test_units_found`, `test_edges_built`, `commits_parsed`
- **Behavior**: Full rebuild of all data. Scans code files, extracts code units, discovers test files, parses git log, computes churn/co-changes, runs blame, builds test edges. Optionally scoped to a subdirectory for code scanning (git log and test discovery remain project-wide). Uses file content hashes to skip unchanged files unless `force=True`.

### impact

- **Input**: `files` (required array of strings), `functions` (optional array of strings)
- **Output**: List of dicts: `{test_id, file_path, name, reason, score}`
- **Behavior**: Finds tests affected by the given changes. Uses direct test edges (test imports/calls the changed code) and transitive co-change coupling (files that frequently change together). Transitive hits are scored at 0.5x weight. Results sorted by score descending.

### suggest_tests

- **Input**: `file_path` (required string), `diff` (optional string, reserved for future use)
- **Output**: List of dicts: `{test_id, file_path, name, relevance, reason}`
- **Behavior**: Wrapper around `impact` for a single file. Returns tests ordered by relevance.

### churn

- **Input**: `file_path` (required string), `unit_name` (optional string)
- **Output**: Dict or list of dicts with: `file_path`, `unit_name`, `commit_count`, `distinct_authors`, `total_insertions`, `total_deletions`, `last_changed`, `churn_score`
- **Behavior**: Returns churn statistics. If `unit_name` is provided, returns stats for that specific function. Otherwise returns file-level stats (or all units for the file if no file-level entry exists).
- **Churn formula**: `sum(1 / (1 + days_since_commit))` -- recent changes weigh exponentially more.

### ownership

- **Input**: `file_path` (required string)
- **Output**: List of dicts: `{author, author_email, line_count, percentage, role}`
- **Behavior**: Returns blame-based code ownership. Each entry has `role: "original_author"`. Percentage represents the fraction of blame lines attributed to each author.

### coupling

- **Input**: `file_path` (required string), `min_count` (optional integer, default 3)
- **Output**: List of dicts: `{file_a, file_b, co_commit_count, last_co_commit}`
- **Behavior**: Returns files that frequently appear in the same commits as the given file. Pairs are sorted by co-commit count descending. Only pairs meeting the minimum threshold are returned.

### risk_map

- **Input**: `directory` (optional string)
- **Output**: List of dicts: `{file_path, unit_name, risk_score, breakdown}`
- **Behavior**: Computes risk scores for all tracked files, optionally scoped to a directory. Sorted by risk score descending.
- **Risk formula**: `0.4 * churn + 0.3 * coupling_breadth + 0.2 * (1 - test_coverage) + 0.1 * author_concentration`
  - Churn: normalized to 0-1 (5.0 raw score = 1.0)
  - Coupling breadth: normalized to 0-1 (10+ coupled files = 1.0)
  - Test coverage: fraction of code units with at least one test edge
  - Author concentration: Herfindahl index of blame line ownership (0 = many authors, 1 = single author)

### stale_tests

- **Input**: None
- **Output**: List of dicts: `{test_id, test_name, missing_code_id, edge_type}`
- **Behavior**: Finds tests whose edges point to code units that no longer exist in the database. This works because FK enforcement is disabled in SQLite, so orphaned edge references survive code unit deletion.

### history

- **Input**: `file_path` (required string)
- **Output**: List of dicts: `{hash, author, author_email, date, message, insertions, deletions}`
- **Behavior**: Returns commit history for a file, sorted by date descending.

### who_reviews

- **Input**: `file_path` (required string)
- **Output**: List of dicts: `{author, author_email, recent_commits, last_commit_date, days_since_last_commit, insertions, deletions, percentage, role}`
- **Behavior**: Suggests reviewers based on recent commit activity. Each entry has `role: "suggested_reviewer"`. Unlike `ownership` (which shows who wrote the code), this shows who has been actively maintaining the file. Activity score uses the same recency formula as churn: `1 / (1 + days_since_commit)`.

## Server Interfaces

### HTTP MCP Server (`chisel serve`)

- Default: `http://127.0.0.1:8377`
- `GET /tools` -- returns JSON array of tool schemas
- `GET /health` -- returns `{"status": "ok"}`
- `POST /call` -- body: `{"tool": "<name>", "arguments": {<kwargs>}}`, returns `{"result": <data>}`
- Uses `ThreadingMixIn` for concurrent request handling

### stdio MCP Server (`chisel-mcp` or `chisel serve-mcp`)

- Requires optional `mcp` package (`pip install chisel[mcp]`)
- Communicates over stdin/stdout per MCP protocol specification
- Environment variables: `CHISEL_PROJECT_DIR`, `CHISEL_STORAGE_DIR`
- Runs synchronous engine methods in a thread executor to avoid blocking the async event loop

## CLI Subcommands

| Command | Arguments | Description |
|---------|-----------|-------------|
| `analyze` | `[directory]`, `--force` | Full project analysis |
| `impact` | `<files...>` | Show impacted tests |
| `suggest-tests` | `<file>` | Suggest tests to run |
| `churn` | `<file>`, `--unit` | Show churn statistics |
| `ownership` | `<file>` | Show blame-based ownership |
| `coupling` | `<file>`, `--min-count` | Show co-change partners |
| `risk-map` | `[directory]` | Risk score heatmap |
| `stale-tests` | (none) | Detect stale tests |
| `history` | `<file>` | Commit history for a file |
| `who-reviews` | `<file>` | Suggest reviewers |
| `serve` | `--port`, `--host` | Start HTTP MCP server |
| `serve-mcp` | (none) | Start stdio MCP server |

All subcommands accept `--project-dir`, `--storage-dir`, and `--json` flags.
