# Chisel -- Glossary

Definitions of terms and concepts specific to the Chisel project.

---

**Author concentration**
A measure of how many authors have contributed to a file, expressed as a Herfindahl index (sum of squared ownership fractions). Ranges from 0 to 1. A value of 1.0 means a single author wrote 100% of the file. A value near 0 means many authors contributed roughly equally. Used as a component of the risk score (weight: 0.1).

**Blame cache**
Cached output from `git blame --porcelain`, stored in the `blame_cache` SQLite table. Keyed by file path and content hash. When a file's content hash changes, the old blame data is invalidated and re-fetched. Blame parsing is expensive, so caching avoids repeated calls for unchanged files.

**Churn score**
A numeric measure of how actively a file or function is being changed. Computed as `sum(1 / (1 + days_since_commit))` across all commits that touched the file or function. Recent changes contribute much more than older ones. A churn score of 5.0 is treated as the normalization ceiling (mapped to 1.0) in risk calculations.

**Churn stats**
The full set of metrics stored per file or function in the `churn_stats` table: `commit_count`, `distinct_authors`, `total_insertions`, `total_deletions`, `last_changed`, and `churn_score`. When `unit_name` is empty, the row represents file-level stats. When `unit_name` is set, it represents function-level stats obtained via `git log -L`.

**Co-change coupling**
A relationship between two files that frequently appear in the same commits. Stored in the `co_changes` table with a `co_commit_count` and `last_co_commit` date. Only pairs with >= 3 co-commits are stored (configurable via `min_count`). Used by impact analysis to find transitive test relationships and as a component of the risk score (weight: 0.25).

**Code unit**
The fundamental unit of code that Chisel tracks. Represented by the `CodeUnit` dataclass in `ast_utils.py` and stored in the `code_units` SQLite table. A code unit has a `file_path`, `name`, `unit_type`, `line_start`, and `line_end`. Types include `function`, `async_function`, `class`, `struct`, `enum`, `interface`, and `impl`. The primary key is `file:name:type`. Methods inside classes are qualified as `ClassName.method_name`.

**Content hash**
SHA-256 hex digest of a file's contents, computed by `compute_file_hash()` in `ast_utils.py`. Used in the `file_hashes` table for incremental analysis (skip unchanged files) and in the `blame_cache` table for blame invalidation.

**Edge type**
The type of relationship in a test edge. Possible values: `"import"` (the test file imports a module containing the code unit) or `"call"` (the test file calls a function matching the code unit name).

**File hash table**
The `file_hashes` SQLite table that stores the most recent content hash for every scanned file. During `analyze()` and `update()`, the engine compares the current hash against the stored hash to determine which files need re-processing.

**Framework detection**
The process by which `TestMapper.detect_framework()` identifies which test framework a file belongs to, based on filename patterns (`test_*.py` for pytest, `*.test.js` for Jest, `*_test.go` for Go, `*.spec.ts` for Playwright) and file content (`#[test]` for Rust, `playwright` import for Playwright vs. Jest disambiguation).

**Impact analysis**
The process of determining which tests are affected by a set of file or function changes. Combines direct test edges (a test imports/calls a changed code unit) with transitive hits via co-change coupling (the changed file frequently co-changes with another file that has test edges). Transitive hits are scored at 0.5x the direct edge weight.

**MCP (Model Context Protocol)**
The protocol used to expose Chisel's tools to LLM agents. Chisel provides two MCP servers: an HTTP server (`mcp_server.py`) and a stdio server (`mcp_stdio.py`). The HTTP server uses a simple JSON-over-HTTP approach. The stdio server uses the `mcp` Python package for standard MCP compliance.

**Original author**
The role assigned to entries returned by the `ownership` tool. Determined by `git blame` data -- shows who wrote which lines of the current version of a file. Contrast with "suggested reviewer."

**Orphaned edge reference**
A test edge whose `code_id` points to a code unit that no longer exists in the `code_units` table. This is possible because SQLite foreign key enforcement is intentionally disabled. Orphaned references are the mechanism by which stale test detection works.

**Risk score**
A composite metric indicating how risky it is to change a file. Formula: `0.35 * churn + 0.25 * coupling + 0.15 * coverage_gap + 0.10 * coverage_depth + 0.10 * author_concentration + 0.05 * test_instability + hidden_risk_factor`. Each of the first 6 components is normalized to the 0-1 range. `hidden_risk_factor` (0–0.15) is added separately from dynamic/eval import edge density. Higher values indicate higher risk. Computed by `ImpactAnalyzer.compute_risk_score()`.

**RWLock (read-write lock)**
A concurrency primitive in `rwlock.py` that allows multiple concurrent readers or one exclusive writer. Used by `ChiselEngine` to protect storage access: `tool_*()` read methods acquire a read lock, while `analyze()` and `update()` acquire a write lock.

**Skip directories**
The set of directory names that are always excluded when walking the project tree. Defined as `_SKIP_DIRS` in `ast_utils.py` and imported by `engine.py` and `test_mapper.py`. Includes `.git`, `node_modules`, `__pycache__`, `.tox`, `.venv`, `venv`, `env`, `.mypy_cache`, `.pytest_cache`, `.ruff_cache`, `dist`, `build`, `.eggs`, `target`, `vendor`, and `Pods`.

**Stale test**
A test whose edges point to code units that have been removed or renamed. Detected by `ImpactAnalyzer.detect_stale_tests()`, which scans all test edges and checks whether the referenced code unit still exists in the `code_units` table.

**Suggested reviewer**
The role assigned to entries returned by the `who_reviews` tool. Determined by recent commit activity -- shows who has been actively modifying/maintaining a file. Activity score uses the same recency-weighted formula as churn. Contrast with "original author."

**Test edge**
A directed relationship from a test unit to a code unit, stored in the `test_edges` SQLite table. Each edge has a `test_id`, `code_id`, `edge_type` (import or call), and `weight`. Edges are built by `TestMapper.build_test_edges()` by matching test file dependencies against known code units by name. The composite primary key is `(test_id, code_id, edge_type)`.

**Test unit**
A single test function or method discovered by the test mapper. Stored in the `test_units` SQLite table with `id`, `file_path`, `name`, `framework`, `line_start`, `line_end`, and `content_hash`. The ID format is `relative_path:function_name`. Test names are identified by framework conventions (e.g., `test_*` for pytest, `Test*` for Go).

**Tool dispatch table**
The `_TOOL_DISPATCH` dict in `schemas.py` that maps each tool name to its engine method name and list of accepted argument names. Used by both the HTTP server and the stdio server (which imports it) to route tool calls to the correct `engine.tool_*()` method.

**WAL mode (Write-Ahead Logging)**
The SQLite journal mode used by Chisel's storage. Enables concurrent readers alongside a single writer without blocking. Set once at connection creation with `PRAGMA journal_mode=WAL`. Combined with `PRAGMA synchronous=NORMAL` for a balance of durability and performance.

**Dynamic require() detection**
A set of regex patterns in `test_mapper.py` that detect `require()` calls invisible to standard static analysis. Categories include: `variable` (e.g., `require(variableName)`), `template` (e.g., `` require(`./${module}`) ``), `concat` (e.g., `require('./' + name)`), `conditional` (e.g., `require(cond ? './prod' : './dev')`), and `eval` (e.g., `new Function('require', code)`). Each pattern returns a `confidence` score; the `dep_type` is `dynamic_import` or `eval_import`. This enables Chisel to surface shadow graph dependencies that would otherwise be invisible to `coupling`, `suggest_tests`, and `risk_map`.

**Shadow graph**
The subgraph of module dependencies that exist at runtime but are invisible to Chisel's static analysis. Includes dynamically loaded modules via `require(variable)`, template literals, string concatenation, conditionals, and `eval`/`new Function`. The shadow graph is not stored separately but emerges from the `dynamic_import` / `eval_import` dep types returned by `_extract_js_deps()`. Risk scoring can account for hidden dependencies by treating low-confidence requires as potential blind spots.

**Require confidence score**
A numeric value (0.0–1.0) indicating how reliably Chisel can determine the actual module path from a `require()` call. `static` (1.0): literal string path. `tainted` (1.0): variable reference where the variable was assigned a path via `const M = './foo'; require(M)` — variable taint tracking resolves the actual path. `template` (0.4): template literal with interpolation. `variable` (0.3): variable reference with unknown binding. `concat` (0.2): string concatenation. `conditional` (0.3): ternary branch. `eval` (0.0): loaded via `eval()` or `new Function()` — completely invisible to static analysis. Confidence is blended into edge weights as `proximity * sqrt(confidence)`. Used by risk analysis to account for unknown dependencies in dynamically loaded modules.
