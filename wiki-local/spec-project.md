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
| C# | Regex patterns (supports attributes, nested generics) | `.cs` | xUnit, NUnit, MSTest |
| Java | Regex patterns (supports annotations, nested generics) | `.java` | JUnit |
| Kotlin | Regex patterns (supports extension functions) | `.kt`, `.kts` | JUnit |
| C/C++ | Regex patterns (supports templates, destructors) | `.c`, `.h`, `.cpp`, `.cc`, `.cxx`, `.hpp`, `.hxx` | gtest, Catch2 |
| Swift | Regex patterns (supports @attributes) | `.swift` | XCTest |
| PHP | Regex patterns | `.php` | PHPUnit |
| Ruby | Keyword-based block detection (`end`) | `.rb` | RSpec, Minitest |
| Dart | Regex patterns (supports factory, getters/setters) | `.dart` | Dart test |

### Code Unit Types Extracted

- **Python**: `function`, `async_function`, `class` (methods qualified as `ClassName.method_name`)
- **JavaScript/TypeScript**: `function` (named functions, arrow functions), `class`
- **Go**: `function` (including methods with receivers), `struct`, `interface`
- **Rust**: `function`, `struct`, `enum`, `impl`
- **C#**: `function` (methods with generics/attributes), `class`, `struct`, `interface`, `enum`, `record`
- **Java**: `function` (methods with annotations/generics), `class`, `interface`, `enum`, `record`
- **Kotlin**: `function` (extension functions, suspend), `class`, `object`, `interface`
- **C/C++**: `function` (templates, destructors), `class`, `struct`, `namespace`, `enum`
- **Swift**: `function` (with @attributes), `class`, `struct`, `enum`, `protocol`, `actor`
- **PHP**: `function`, `class`, `interface`, `trait`, `enum`
- **Ruby**: `function`, `class`, `module`
- **Dart**: `function` (factory, getters/setters), `class`, `mixin`, `extension`

## MCP Tool Specifications

All 15 tools are accessible through three interfaces: CLI subcommands, HTTP POST /call, and stdio MCP. Each tool maps to an `engine.tool_*()` method.

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
- **Risk formula**: `0.35 * churn + 0.25 * coupling_breadth + 0.2 * (1 - test_coverage) + 0.1 * author_concentration + 0.1 * test_instability`
  - Churn: normalized to 0-1 (5.0 raw score = 1.0)
  - Coupling breadth: normalized to 0-1 (10+ coupled files = 1.0)
  - Test coverage: fraction of code units with at least one test edge
  - Author concentration: Herfindahl index of blame line ownership (0 = many authors, 1 = single author)
  - Test instability: average failure rate of covering tests (0 = stable, 1 = always fails)

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

### diff_impact

- **Input**: `ref` (optional string)
- **Output**: List of dicts: `{test_id, file_path, name, reason, score}`
- **Behavior**: Auto-detects changed files and functions from `git diff` and returns impacted tests. Branch-aware: on a feature branch diffs against main/master; on main diffs against HEAD (unstaged changes). Optionally accepts a custom git ref to diff against.

### update

- **Input**: None
- **Output**: Dict with keys: `files_updated`, `code_units_found`, `new_commits`, `orphaned_results_cleaned`
- **Behavior**: Incremental re-analysis. Only re-processes files whose content hash has changed and stores new commits not yet in the database. Recomputes churn and coupling if any changes are detected.

### test_gaps

- **Input**: `file_path` (optional string), `directory` (optional string), `exclude_tests` (optional boolean, default true)
- **Output**: List of dicts: `{id, file_path, name, unit_type, line_start, line_end, churn_score, commit_count}`
- **Behavior**: Finds code units with zero test edges, prioritized by churn score descending. By default excludes units from test files. Can be scoped to a single file or directory.

### record_result

- **Input**: `test_id` (required string), `passed` (required boolean), `duration_ms` (optional integer)
- **Output**: Dict: `{test_id, passed, recorded: true}`
- **Behavior**: Records a test pass/fail outcome in the `test_results` table. Feeds into `suggest_tests` (failure rate boost) and `risk_map` (test instability component).

### stats

- **Input**: None
- **Output**: Dict: `{code_units, test_units, test_edges, commits, commit_files, blame_cache, co_changes, churn_stats, file_hashes, test_results}`
- **Behavior**: Returns summary counts for all 10 database tables in a single query.

## Test Edge Weighting

Test edges carry a `weight` (0.4-1.0) based on confidence:

- **File proximity**: Tests in the same directory as the code get weight 1.0. Sibling directories get 0.8, shared ancestor 0.6, distant files 0.4.
- **Python import-path matching**: When a test imports `from myapp.utils import foo`, Chisel matches specifically to `myapp/utils.py:foo` rather than any `foo` in any file, then applies proximity weighting.
- **Non-Python languages**: Fall back to name-based matching with proximity weighting.

This reduces false positive edges in projects where multiple modules export identically-named functions.

## Server Interfaces

### HTTP MCP Server (`chisel serve`)

- Default: `http://127.0.0.1:8377`
- `GET /tools` -- returns JSON array of tool schemas
- `GET /health` -- returns `{"status": "ok"}`
- `POST /call` -- body: `{"tool": "<name>", "arguments": {<kwargs>}}`, returns `{"result": <data>}`
- Uses `ThreadingMixIn` for concurrent request handling

### stdio MCP Server (`chisel-mcp` or `chisel serve-mcp`)

- Requires optional `mcp` package (`pip install chisel-ai[mcp]`)
- Communicates over stdin/stdout per MCP protocol specification
- Environment variables: `CHISEL_PROJECT_DIR`, `CHISEL_STORAGE_DIR`
- Runs synchronous engine methods in a thread executor to avoid blocking the async event loop

## CLI Subcommands

| Command | Arguments | Description |
|---------|-----------|-------------|
| `analyze` | `[directory]`, `--force` | Full project analysis |
| `update` | (none) | Incremental re-analysis of changed files |
| `impact` | `<files...>`, `--functions` | Show impacted tests |
| `diff-impact` | `[--ref]` | Auto-detect changes, show impacted tests |
| `suggest-tests` | `<file>` | Suggest tests to run |
| `churn` | `<file>`, `--unit` | Show churn statistics |
| `ownership` | `<file>` | Show blame-based ownership |
| `who-reviews` | `<file>` | Suggest reviewers |
| `coupling` | `<file>`, `--min-count` | Show co-change partners |
| `risk-map` | `[directory]` | Risk score heatmap |
| `stale-tests` | (none) | Detect stale tests |
| `test-gaps` | `[--file]`, `[--directory]`, `[--no-exclude-tests]` | Find untested code units |
| `history` | `<file>` | Commit history for a file |
| `record-result` | `<test_id>`, `--passed`/`--failed`, `[--duration-ms]` | Record test outcome |
| `stats` | (none) | Database summary counts |
| `serve` | `--port`, `--host` | Start HTTP MCP server |
| `serve-mcp` | (none) | Start stdio MCP server |

All subcommands accept `--project-dir`, `--storage-dir`, `--json`, and `--limit` flags.
