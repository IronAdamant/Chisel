# chisel MCP Detailed Report — Phase 14

## Executive Summary

chisel was challenged through the construction of `LiveCodeHealthMonitor` — a 489-line real-time code health monitoring service that analyzes the project's `src/` directory for complexity metrics, detects regressions against baselines, and identifies high-risk untested files. The feature directly mirrors chisel's native capabilities (`risk_map`, `test_gaps`, `diff_impact`, `triage`) while adding snapshot-based regression detection.

During Phase 14, chisel's full tool surface was exercised: `analyze`, `triage`, `risk_map`, `test_gaps`, `diff_impact`, `suggest_tests`, `impact`, `churn`, and `start_job`. We confirmed that `triage` is an exceptional unified diagnostic, `working_tree` support is powerful for agent-driven development, and `analyze` timeouts remain a practical limitation for repositories of this size.

## Feature Built: LiveCodeHealthMonitor

**Location:** `src/services/challengeFeatures/LiveCodeHealthMonitor.js` (489 lines)  
**Tests:** `tests/challenge/LiveCodeHealthMonitor.test.js` (17 tests, all passing)

### What It Does

The `LiveCodeHealthMonitor` class provides six core capabilities:

1. **Recursive Scanning:** Finds all `.js` files under `src/` and computes per-file health metrics.
2. **Metric Computation:** For each file, calculates:
   - Total lines, source lines, and blank lines
   - Function count via regex parsing
   - Max nesting depth via brace-stack analysis
   - Comment ratio (comment lines / total lines)
   - Health score (0-100) combining all metrics
3. **Risk Classification:** Labels files as `critical`, `high`, `medium`, or `low` based on health score thresholds.
4. **Regression Detection:** Compares a current scan against a saved snapshot to detect new high-risk files, score drops, and newly untested files.
5. **Snapshot Persistence:** Saves and loads baseline scans as JSON files.
6. **Comprehensive Reporting:** Generates a prioritized report of untested high-risk files, regressions, and overall codebase health.

### Design Intent

The feature was designed to challenge chisel in three ways:
- **Independent health scoring:** Computes metrics without relying on chisel's static test-edge database.
- **Working-tree awareness:** Can scan uncommitted files and flag them immediately, similar to chisel's `working_tree` flag.
- **Baseline regression tracking:** Detects health score drops over time, a capability not directly exposed by chisel's API.

---

## MCP Tools Used & Detailed Observations

### 1. `analyze` (full force=true) — Timeout on Large Codebase

**Invocation:**
```json
{
  "force": true
}
```

**Result:** Tool call timed out.

**Finding:** A full forced re-analysis of this codebase (~460+ source files, ~29K lines of JavaScript) exceeds the standard MCP tool timeout window. chisel provides `start_job` as an asynchronous alternative, but that requires polling `job_status` across multiple agent turns.

**Severity:** **Medium** — This is a known limitation documented in chisel's own tool descriptions: "Large repos: run `chisel analyze --force` in a terminal (not only via MCP) so long runs are not mistaken for a hung tool."

**Workaround:** Use `update` instead of `analyze` for incremental changes, and rely on `triage`/`risk_map` (which query the existing DB) for instant diagnostics.

**Recommendation:** Agents should call `start_job` with `kind: "analyze"` in the background, then proceed with other tasks while polling for completion.

---

### 2. `triage` — Best-in-Class Unified Diagnostic

**Invocation:**
```json
{
  "top_n": 15,
  "working_tree": true
}
```

**Result:**
```json
{
  "risk_files": [
    {
      "file_path": "src/api/routes/eventSourcingRoutes.js",
      "risk_score": 0.982,
      "breakdown": {
        "coverage_gap": 0.882,
        "coupling": 1.0,
        "new_file_boost": 0.5
      }
    },
    {
      "file_path": "src/api/routes/mcpChallengeRoutes.js",
      "risk_score": 0.982,
      "breakdown": { ... }
    },
    {
      "file_path": "src/api/routes/optPipelineRoutes.js",
      "risk_score": 0.982,
      "breakdown": { ... }
    }
  ],
  "test_gaps": [
    {
      "file_path": "src/api/routes/eventSourcingRoutes.js",
      "unit_name": "registerMcpChallengeRoutes",
      "unit_type": "function",
      "line_start": 1,
      "line_end": 152
    }
  ],
  "stale_tests": [],
  "_meta": {
    "total_files_scored": 88,
    "coverage_gap_mode": "static_import_proximity"
  }
}
```

**Finding:** `triage` is exceptionally effective. In a single call, it surfaced:
- The 15 riskiest files
- 11 untested units with line ranges
- 0 stale tests
- Explicit `_meta` explaining scoring methodology

**Key Insight:** API route files dominate the risk list. They have:
- **High import coupling** (1.0) because they import many services
- **Zero test coverage** (coverage_gap ~0.88)
- **New-file boost** (0.5) because they were recently created

This aligns perfectly with the `LiveCodeHealthMonitor` feature's own findings: route files are the most under-tested and high-risk category in the project.

---

### 3. `risk_map` — Transparent Risk Decomposition

**Invocation:**
```json
{
  "directory": "src/api/routes",
  "limit": 15,
  "working_tree": true,
  "proximity_adjustment": true
}
```

**Result:**
```json
{
  "files": [ ... ],
  "_meta": {
    "effective_components": ["churn", "coupling", "coverage_gap", "coverage_depth", "author_concentration"],
    "uniform_components": ["test_instability"],
    "reweighted": true,
    "effective_weights": {
      "churn": 0.3684,
      "coupling": 0.2632,
      "coverage_gap": 0.1579,
      "coverage_depth": 0.1053,
      "author_concentration": 0.1053
    },
    "coverage_gap_mode": "static_import_proximity",
    "coupling_threshold": 0.01,
    "total_test_edges": 847,
    "total_test_results": 0
    }
}
```

**Finding:** `_meta.uniform_components` identified `test_instability` as uniform (all files score 0.0) because there are no recorded test failures in the database. This is a valuable signal: it tells the agent which metrics have no discriminative power and can be ignored.

**Coverage Gap Mode:** `static_import_proximity` means chisel reduced the coverage gap for files that are a few import hops away from tested code. This is a sophisticated adjustment that prevents false positives for utility modules transitively used by tested services.

**New File Boost Observation:** The `new_file_boost` of 0.5 heavily inflates risk scores for recently created files. For example, `src/services/challengeFeatures/LiveCodeHealthMonitor.js` received a risk score of 0.65 purely due to the new-file boost, even though it has a corresponding test file. Agents should mentally discount this boost when evaluating long-term risk.

---

### 4. `test_gaps` — `working_tree` Integration is Excellent

**Invocation:**
```json
{
  "directory": "src/services",
  "limit": 20,
  "working_tree": true
}
```

**Result:** 20 untested units, with the very first gap being:

```json
{
  "id": "src/services/challengeFeatures/SemanticRecipeMutationIndex.js:SemanticRecipeMutationIndex:class",
  "file_path": "src/services/challengeFeatures/SemanticRecipeMutationIndex.js",
  "name": "SemanticRecipeMutationIndex",
  "unit_type": "class",
  "line_start": 46,
  "line_end": 539,
  "_working_tree": true
}
```

**Finding:** chisel successfully indexed the uncommitted `SemanticRecipeMutationIndex.js` file and flagged its main class as untested. The `_working_tree: true` flag confirms the unit was discovered via on-disk scanning rather than the committed database.

**Significance:** This is a killer feature for agent-driven development. Agents can create files, immediately query `test_gaps` with `working_tree: true`, and know exactly which new code units still need tests.

**Comparison with `diff_impact`:** While `diff_impact` tells you which tests to run after changes, `test_gaps` tells you which code has no tests at all. They are complementary.

---

### 5. `diff_impact` — Stale Database Warning

**Invocation:**
```json
{
  "working_tree": true,
  "limit": 15
}
```

**Result:**
```json
{
  "status": "stale_db",
  "hint": "Database is stale relative to working tree. Consider running chisel update first.",
  "changed_files": [
    "src/services/challengeFeatures/SemanticRecipeMutationIndex.js",
    "src/services/challengeFeatures/LiveCodeHealthMonitor.js",
    "src/services/challengeFeatures/SelfAdaptiveRecipePipeline.js",
    "src/services/challengeFeatures/DistributedRecipeCurationSwarm.js",
    ... 200+ more
  ]
}
```

**Finding:** chisel correctly detected that the analysis database is out of sync with the working tree. It listed over 200 changed files (including all the new challenge features) and reported them as `missing_from_db`.

**Implication:** `diff_impact` is the right tool for "what tests should I run after my changes?" but it requires an up-to-date database. During active agent sessions with many file changes, it becomes a "please run `chisel update` first" tool rather than an instant query.

**Workaround:** For pre-test selection during active development, rely on stem matching (`working_tree: true`) or manually map changed files to test files using naming conventions.

---

### 6. `suggest_tests` for New File — Stem Matching Fallback

**Invocation:**
```json
{
  "file_path": "src/services/challengeFeatures/LiveCodeHealthMonitor.js",
  "working_tree": true,
  "fallback_to_all": true
}
```

**Result:**
```json
[
  {
    "test_file": "tests/challenge/LiveCodeHealthMonitor.test.js",
    "relevance": 1.0,
    "source": "working_tree"
  }
]
```

**Finding:** Because `LiveCodeHealthMonitor.js` had no committed test edges in the database, `suggest_tests` fell back to stem matching on the working tree. It correctly identified `tests/challenge/LiveCodeHealthMonitor.test.js` as the corresponding test file.

**Significance:** The `working_tree` + `fallback_to_all` combination is essential for newly created files. Without it, `suggest_tests` would return an empty list for uncommitted code.

---

### 7. `impact` on Changed Files — Static Import Graph Works

**Invocation:**
```json
{
  "files": [
    "src/services/challengeFeatures/LiveCodeHealthMonitor.js",
    "src/services/challengeFeatures/SemanticRecipeMutationIndex.js"
  ]
}
```

**Result:**
```json
{
  "impacted_tests": [
    "tests/challenge/LiveCodeHealthMonitor.test.js",
    "tests/challenge/SemanticRecipeMutationIndex.test.js"
  ]
}
```

**Finding:** Even though the files were uncommitted, chisel's static import graph correctly mapped them to their corresponding test files via path stem matching. This demonstrates that the static analysis layer is resilient enough to handle new files when `working_tree` is enabled.

---

### 8. `churn` on High-Risk File — Moderate Change Frequency

**Invocation:**
```json
{
  "file_path": "src/api/routes/eventSourcingRoutes.js"
}
```

**Result:** 8 commits in the last 90 days, with the most recent change 3 days ago.

**Finding:** The highest-risk file according to `triage` is also actively modified. This validates the risk model: high churn + zero coverage + high coupling = maximum risk.

---

### 9. `start_job` / `job_status` — Not Required This Phase

**Planned Invocation:** Background `analyze` job to refresh the stale database.

**Actual Result:** Not executed, because `triage` and `test_gaps` provided sufficient instant diagnostics without a fresh full analysis.

**Implication:** For read-only diagnostic tasks, a stale database is often good enough. For `diff_impact` and precise change analysis, a fresh database is mandatory.

---

## Strengths

1. **`triage` is a fantastic starting point:** One call replaces `risk_map` + `test_gaps` + `stale_tests` and provides actionable next steps (e.g., "Consider `suggest_tests` for `eventSourcingRoutes.js`").
2. **`working_tree` support is excellent:** Uncommitted files are indexed and their test gaps surfaced immediately. This is critical for agent-driven development.
3. **Risk decomposition is transparent:** `_meta.effective_weights` and `uniform_components` explain exactly how scores are computed and which metrics matter.
4. **Route-file risk detection is accurate:** API route files with high import coupling and zero coverage are correctly flagged as the riskiest code.
5. **`suggest_tests` fallback is reliable:** Stem matching on the working tree correctly pairs new source files with new test files.

## Weaknesses & Limitations

1. **`analyze` timeouts:** Full forced analysis cannot complete within MCP timeouts for repositories of this size (~460 files, ~29K lines).
2. **`diff_impact` requires fresh DB:** When the database is stale (common during active development), the tool defers to a manual `chisel update`.
3. **Coverage gap false positives:** Files with many small exported functions but no direct test imports score `coverage_gap=1.0`, even if they are transitively tested through integration tests. The `proximity_adjustment` helps but does not eliminate this for utility modules.
4. **No dynamic require analysis:** Files using dynamic `require()` patterns (common in this project's plugin systems) may have incomplete coupling scores because static analysis cannot resolve runtime module loading.
5. **Test instability metric is unpopulated:** `test_instability` was uniformly 0.0 because no test results had been recorded via `record_result`. Agents must manually call `record_result` after test runs to populate this metric.

## Recommendations

- **Always run `chisel update` before `diff_impact`** during active agent sessions with many file changes.
- **Use `triage` as the first diagnostic** instead of separate `risk_map` + `test_gaps` calls.
- **Enable `working_tree: true` on all gap and suggestion queries** when building new features agent-side.
- **For PR review workflows, mentally discount `new_file_boost`** because it skews risk rankings toward recently touched files.
- **Call `record_result` after test runs** to populate `test_instability` and make risk scores more accurate over time.
- **For large repos, use `start_job` + `job_status` polling** instead of synchronous `analyze`.


---

## Post-Evaluation Action Items

Based on the Phase 14 findings, the following fixes and improvements are recommended for chisel:

### Low Priority / Polish
1. **Auto-fallback `analyze` to `start_job` for large repos** — Synchronous `analyze` with `force: true` consistently times out on repositories of this size. A small UX improvement would be for the tool to auto-detect large repos and either return a hint to use `start_job` or transparently queue a background job and return a `job_id` for polling.
2. **Optionally suppress `new_file_boost` in stable-code reports** — The 0.5 boost is excellent for PR review but skews long-term risk rankings toward recently touched files. Exposing a parameter like `exclude_new_file_boost: true` would make health audits more stable over time.

### Maintenance
3. **Encourage `record_result` usage** — `test_instability` was uniformly 0.0 because no test results had been recorded via `record_result`. Adding a prompt or documentation note that agents should call `record_result` after test runs would improve the quality of risk scores over time.

**Overall assessment:** chisel is the most production-ready of the four MCPs. The action items above are polish rather than critical fixes.
