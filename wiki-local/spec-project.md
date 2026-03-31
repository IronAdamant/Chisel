# Chisel -- Project Specification

## Overview

Chisel is a test impact analysis and code intelligence tool **for LLM agents**, aimed at **solo-maintained** repositories where **multiple agent sessions or processes** may run analysis and queries concurrently. It maps tests to code, code to git history and static imports, and answers: **what to run, what is risky, where coverage is thin**, and **git/blame context** when debugging — not org-chart or team-assignment workflows.

## Goals

1. **Targeted test selection**: Given changed files or functions, identify tests that should run to catch regressions — using direct test edges, git co-change (when history supports it), and **static import-graph reachability** (e.g. inner modules exercised only via a facade test).
2. **Risk visibility**: Surface high-risk files using churn, coupling (co-change and/or **import graph**), graduated coverage gaps, author concentration, and test instability.
3. **Git-derived lineage (not team routing)**: `ownership` (blame) and `who_reviews` (recent commit activity) expose **audit and heuristic “hot spot” signals** for agents; they do not assign human reviewers in a solo workflow.
4. **Change coupling detection**: Co-change pairs from git history **plus** import neighbors from static `import`/`require` edges (`coupling` returns both lists and numeric scores).
5. **Stale test detection**: Find tests whose edges point at code units that no longer exist.
6. **Zero dependencies**: Run anywhere Python 3.11+ is available with no pip installs beyond Chisel itself (stdlib only for core).
7. **LLM agent integration**: Expose capabilities as **MCP tools** (HTTP and stdio), with structured **`next_steps`** hints on responses where applicable.
8. **Incremental analysis**: Track file content hashes and only re-process changed files.
9. **Multi-process safety**: One project-local database (`.chisel/`) coordinated with **ProcessLock** + SQLite WAL so concurrent agents and CLI runs do not corrupt storage.

## Non-Goals

- **Test execution**: Chisel does not run tests. It tells you which tests to run.
- **Full static analysis**: Chisel uses lightweight AST extraction, not a type checker or full semantic analysis engine. Cross-file resolution depends on name matching and import paths, not type inference.
- **Language Server Protocol**: Chisel is not an LSP server. It provides batch analysis and MCP tool access.
- **Real-time file watching**: Chisel does not watch the filesystem for changes. Analysis is triggered explicitly via `chisel analyze` or the MCP `analyze` tool.
- **Multi-repo support**: Each Chisel instance operates on a single git repository.
- **Full branch comparison**: While `diff_impact` is branch-aware (auto-detects feature branch vs main for diffs), Chisel does not maintain separate analysis databases per branch.
- **Organizational reviewer assignment**: No integration with IDEs or HR systems for “who must review” — `who_reviews` is a heuristic from git activity only.

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

## Pluggable extractors (user-supplied parsers)

Built-in extraction is **stdlib-only** (regex / `ast`). For **tree-sitter**, **LSP**, or other backends, users install dependencies **outside** Chisel and call **`register_extractor(language, fn)`** from `ast_utils`. Optional env **`CHISEL_BOOTSTRAP`** names a Python module to import at `ChiselEngine` startup so registrations run before **`analyze`**. See **`docs/CUSTOM_EXTRACTORS.md`** in the repository.

## MCP Tool Specifications

**24 tools** are defined in `schemas.py`: **18 core** query/write tools (including **`start_job`** / **`job_status`** for background analyze/update) plus **6 advisory file-lock** tools for multi-agent coordination. They are reachable via CLI (where exposed), HTTP `POST /call`, and stdio MCP. Each maps to an `engine.tool_*()` method.

### start_job / job_status

- **start_job**: `kind` = `analyze` \| `update`; optional `directory`, `force` (analyze). Runs work in a **stdlib `threading`** background thread; returns `job_id` immediately. Poll **`job_status`** until `completed` or `failed`. Only one background job per engine instance at a time; otherwise returns `status: busy`.
- **job_status**: `job_id` — returns `status`, timestamps, `result` (JSON) or `error`.

### analyze

- **Input**: `directory` (optional string), `force` (optional boolean)
- **Output**: Dict with keys: `code_files_scanned`, `code_units_found`, `test_files_found`, `test_units_found`, `test_edges_built`, `commits_parsed`
- **Behavior**: Full rebuild of all data. Scans code files, extracts code units, discovers test files, parses git log, computes churn/co-changes, runs blame, builds test edges. Optionally scoped to a subdirectory for code scanning (git log and test discovery remain project-wide). Uses file content hashes to skip unchanged files unless `force=True`.

### impact

- **Input**: `files` (required array of strings), `functions` (optional array of strings)
- **Output**: List of dicts: `{test_id, file_path, name, reason, score}`
- **Behavior**: Finds tests affected by the given changes. Uses (1) **direct** test edges, (2) **co-change** coupling to other files and their tests (0.5x weight), (3) **import-graph** reachability: tests covering any file in the undirected import closure around each changed file (decayed weight by hop distance). Results sorted by score descending.

### suggest_tests

- **Input**: `file_path` (required string), `fallback_to_all` (optional boolean), `working_tree` (optional boolean)
- **Output**: List of dicts: `{test_id, file_path, name, relevance, reason}`
- **Behavior**: Uses the same impact pipeline as `impact` for a single file, then adjusts relevance with recorded test failure rates. If `fallback_to_all` and no edges match, returns stem-ranked test files. If `working_tree`, attempts stem-matching for uncommitted files with no DB edges.

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
- **Output**: Dict with `co_change_partners` (rows from storage), `import_partners` (list of `{"file": "<path>"}`), `co_change_breadth`, `import_breadth`, `cochange_coupling`, `import_coupling`, `effective_coupling` (numeric 0–1 signals for agents when co-change is empty in solo/low-commit repos).
- **Behavior**: Returns git co-change pairs meeting the adaptive threshold **and** static import neighbors (either direction). Risk scoring elsewhere combines both; import coupling is first-class when co-change is sparse.

### risk_map

- **Input**: `directory`, `exclude_tests`, `proximity_adjustment`, `coverage_mode` (`unit` | `line`) — see `schemas.py`
- **Output**: Dict: `{files: [...], _meta: {...}}` per file includes `breakdown` with `coverage_fraction` (partial coverage) and quantized `coverage_gap`, plus optional `coupling_partners` / `import_partners` inline.
- **Behavior**: Batch-computed risk scores; `_meta` documents uniform components and coupling thresholds. Sorted by risk score descending within `files`.
- **Risk formula**: `0.35 * churn + 0.25 * coupling + 0.15 * coverage_gap + 0.10 * coverage_depth + 0.10 * author_concentration + 0.05 * test_instability + hidden_risk_factor`
  - Churn: normalized to 0-1 (5.0 raw score = 1.0)
  - Coupling: `max(git co-change breadth, static import-graph breadth)` normalized to 0-1
  - Coverage gap: graduated, quantized to 0.25 steps (0.0/0.25/0.5/0.75/1.0)
  - Coverage depth: `min(distinct_covering_tests/5, 1.0)` — more distinct test files covering a file reduces risk
  - Author concentration: Herfindahl index of blame line ownership (0 = many authors, 1 = single author)
  - Test instability: average failure rate of covering tests (0 = stable, 1 = always fails)
  - Hidden risk factor: `min(dynamic_edge_count/20, 1.0) * 0.15` — additive uplift from dynamic/eval import density

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
- **Output**: List of impacted tests **or** a diagnostic dict: `no_changes` (no diff / no untracked code), **`git_error`** (git missing, wrong cwd, not a repo — includes `message`, `project_dir`, `hint`; never a silent empty list).
- **Behavior**: Merges `git diff --name-only` with untracked code files, then runs the same impact logic as `impact` (including import-graph transitive tests). Branch-aware unless `ref` is supplied.

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
- **Output**: Dict of per-table counts and diagnostic fields (e.g. `coupling_threshold`, `co_change_query_min`).
- **Behavior**: Returns summary counts for persisted tables (including import edges and related metadata) in a single query.

### triage

- **Input**: `directory`, `top_n`, `exclude_tests` (see `schemas.py`)
- **Output**: Dict combining top-risk files, filtered `test_gaps`, `stale_tests`, and a `summary` (edge counts, thresholds).
- **Behavior**: Single read-locked call for agent reconnaissance before refactors.

### Advisory file locks (`acquire_file_lock`, `release_file_lock`, `refresh_file_lock`, `check_file_lock`, `check_locks`, `list_file_locks`)

- **Purpose**: Optional coordination so multiple agents can discover conflicts before editing the same path. Advisory only — not OS-enforced locks on file content.

## Test Edge Weighting

Test edges carry a `weight` (0.4-1.0) based on confidence:

- **File proximity**: Tests in the same directory as the code get weight 1.0. Sibling directories get 0.8, shared ancestor 0.6, distant files 0.4.
- **Python import-path matching**: When a test imports `from myapp.utils import foo`, Chisel matches specifically to `myapp/utils.py:foo` rather than any `foo` in any file, then applies proximity weighting.
- **Non-Python languages**: Fall back to name-based matching with proximity weighting.
- **Dynamic `require()` detection**: For JS/TS, dynamic patterns (`require(variable)`, template literals, string concatenation, conditionals, `eval`) are recorded as `dynamic_import` / `eval_import` dep types with their own `confidence` scores (0.0–1.0). **Variable taint tracking** (`const MODULE = './foo'; require(MODULE)`) resolves known variable bindings and upgrades them to `tainted_import` (confidence=1.0). Unknown variables remain `dynamic_import` (confidence=0.3). These patterns represent the **shadow graph** — dependencies invisible to static path resolution. `coupling` and `suggest_tests` do not follow dynamic requires; agents should treat low-confidence dynamic imports as potential blind spots in risk analysis.

This reduces false positive edges in projects where multiple modules export identically-named functions.

## Server Interfaces

### HTTP MCP Server (`chisel serve`)

- Default: `http://127.0.0.1:8377`
- `GET /tools` -- returns JSON array of tool schemas
- `GET /health` -- returns `{"status": "ok"}`
- `POST /call` -- body: `{"tool": "<name>", "arguments": {<kwargs>}}`, returns `{"result": <data>, "next_steps": [...]}` (structured follow-up hints for agents)
- Uses `ThreadingMixIn` for concurrent request handling

### stdio MCP Server (`chisel-mcp` or `chisel serve-mcp`)

- Requires optional `mcp` package (`pip install chisel-test-impact[mcp]`)
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
| `suggest-tests` | `<file>`, `--fallback`, `--working-tree` | Suggest tests to run |
| `churn` | `<file>`, `--unit` | Show churn statistics |
| `ownership` | `<file>` | Show blame-based ownership |
| `who-reviews` | `<file>` | Recent commit activity (heuristic) |
| `coupling` | `<file>`, `--min-count` | Co-change + import partners |
| `risk-map` | `[directory]`, flags for tests/proximity/coverage | Risk score heatmap |
| `stale-tests` | (none) | Detect stale tests |
| `test-gaps` | `[file]`, `[--directory]`, `[--no-exclude-tests]`, `--working-tree` | Find untested code units |
| `history` | `<file>` | Commit history for a file |
| `record-result` | `<test_id>`, `--passed`\|`--failed` (required), `[--duration-ms]` | Record test outcome |
| `stats` | (none) | Database summary counts |
| `triage` | `[directory]`, `--top-n`, etc. | Combined risk + gaps + stale |
| `serve` | `--port`, `--host` | Start HTTP MCP server |
| `serve-mcp` | (none) | Start stdio MCP server |
| `acquire-lock`, `release-lock`, `refresh-lock`, `check-lock`, `check-locks`, `list-locks` | per subcommand | Advisory multi-agent file locks |

All subcommands accept `--project-dir`, `--storage-dir`, `--json`, and `--limit` flags (where applicable).
