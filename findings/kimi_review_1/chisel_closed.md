# Review Nineteen: Chisel Challenge Report

## Feature: ChiselWorkingTreeGapInjector

**Purpose**: Generates a simulated temporary project with graduated test coverage (full, partial, indirect, none) and deliberately marks some files as untracked, specifically to challenge Chisel's `diff_impact`, `test_gaps`, `risk_map`, and working-tree awareness.

---

## Implementation Summary

`ChiselWorkingTreeGapInjector` (plus `CoverageModule`) creates a temp directory structure with:

- **Layer 0 (Base)**: `nutritionBase`, `allergenBase`, `seasonalBase` — `full` coverage
- **Layer 1 (Analysis)**: `nutritionAnalyzer`, `allergenChecker`, `seasonalMatcher` — `partial` coverage
- **Layer 2 (Services)**: `recipeValidator`, `mealPlanOptimizer`, `costEstimator` — `indirect` coverage
- **Layer 3 (Top)**: `recommendationEngine`, `trendAnalyzer`, `socialRecipeAdapter` — `none` coverage

Dependency wiring creates a 12-module graph with 11 dependency edges.

**Untracked files**: `trendAnalyzer.js`, `socialRecipeAdapter.js`, `allergenChecker.js` (src), plus `allergenChecker.test.js`.

Risk scoring formula:
- `none` → +0.4
- `indirect` → +0.3
- `partial` → +0.15
- +0.05 per dependent (max +0.3)
- +0.3 if untracked
- Capped at 1.0

Tests: **5 passing** (coverage types, complex scenario stats, risk profile ordering, untracked tracking, cleanup).

---

## Chisel Tools Tested

### 1. `diff_impact` — Post-Change Test Impact

**Challenge**: We ran `diff_impact(working_tree: true)` after creating all 6 new feature files + 6 new test files.

**Result**: The top 20 results were **100% from `tests/services/testPyramid.test.js`**, returning tests like `TestClassifier`, `classifyFile returns unit for model test`, `Recipe`, etc. All reasons were `"direct edge to createTempTestDir (call)"`.

**Analysis**: `diff_impact` completely **ignored the 12 new files** we added. It did not associate the new `steleSemanticMutationEngine.js`, `chiselWorkingTreeGapInjector.js`, or their tests with any impact. Instead, it returned impacts from an existing file (`testPyramid.test.js`) that happens to call `fs.mkdtempSync` — a completely unrelated temp-directory operation.

**Critical Gap**: `diff_impact` with `working_tree: true` does **not** actually detect impacts from newly added uncommitted source files. It only sees changes in already-tracked files.

---

### 2. `test_gaps` — Untested Code Discovery

**Challenge**: We ran `test_gaps(limit: 20, working_tree: true)`.

**Result**: The top results were the usual high-churn untested units:
- `src/api/routeLoader.js:loadRoutes` (churn 0.885)
- `src/services/importFanout/leafCalculator.js:computeYieldFactor` (churn 0.885)
- `src/services/importFanout/midTransformer.js:scaleIngredient` (churn 0.885)
- `src/services/namespaceCollision/collisionRegistry.js:getContext` (churn 0.885)
- `src/services/queryPlanner/plannerAST.js:SelectStatement` (churn 0.885)

**Analysis**: Again, **none of the 12 new modules** appeared in the top 20 test gaps. The algorithm is still driven by git churn history. Because the new files have zero commits, their churn score is 0.0, so they are invisible to `test_gaps`.

This directly confirms the long-standing gap documented in `CLAUDE.md`:
> "`suggest_tests` / `test_gaps` / `risk_map` cannot see untracked/uncommitted files. Fix needed: analyze working tree, not just git history."

**Gap**: `working_tree: true` does not actually elevate newly created files with zero coverage into the gap rankings.

---

### 3. `risk_map` — Composite Risk Scoring

**Challenge**: We ran `risk_map(limit: 20)`.

**Result**: Top risky files:
1. `src/api/routeLoader.js` — risk 0.5873 (churn 0.177, coverage_gap 1.0)
2. `src/api/routes/optimizerRoutes.js` — risk 0.5798
3. `src/cli/index.js` — risk 0.5611
4. `src/services/deltaMerge/index.js` — risk 0.5389
5. `src/services/optimizer/varietyScorer.js` — risk 0.5387

**Analysis**: The new files (`steleSemanticMutationEngine.js`, `chiselWorkingTreeGapInjector.js`, etc.) were **completely absent** from the top 20. This is because `risk_map` relies heavily on:
- `churn` (requires git history)
- `coupling` (requires import edges or co-change history)
- `coverage_gap` (requires test edges)

New files score 0.0 on churn and have not yet accumulated test edges in Chisel's database.

**Meta-observation**: The `_meta` block reported:
```json
{
  "uniform_components": {
    "test_instability": { "value": 0.0, "reason": "all covering tests passing" }
  },
  "effective_components": ["churn", "coupling", "coverage_gap", "coverage_depth", "author_concentration"]
}
```

This is helpful — Chisel correctly identified that `test_instability` provides no signal. However, there is **no warning** that the new files are missing from the analysis entirely.

---

### 4. `suggest_tests` — Test Recommendations for New Files

**Challenge**: We ran `suggest_tests` on `mcpResilientDocumentationPipeline.js` with `working_tree: true`.

**Result**: It returned `tests/cli/index.test.js` with relevance 0.1, reason `"working-tree: stem-matched test (file not yet committed)"`.

**Analysis**: The stem-match fallback is active, but it matched `src/services/mcpResilientDocumentationPipeline.js` to `tests/cli/index.test.js` — a completely unrelated test file. There is a perfectly good `tests/services/mcpResilientDocumentationPipeline.test.js` sitting in the working tree, but it was not found.

**Gap**: The stem-matching heuristic is too coarse (matches on `index` vs `pipeline`??) and misses obvious same-directory test files.

---

## Refactoring Exercise: `validatePositiveInt` → `validatePositiveInteger`

As a real-world stress test, we performed a simple cross-file rename of `validatePositiveInt` to `validatePositiveInteger` in `src/utils/validation.js` and updated all call sites across 3 model files and the validation test suite.

**Chisel findings:**

- `suggest_tests` on `src/utils/validation.js` returned `tests/services/validationShards.test.js` with a low relevance score, instead of the correct `tests/utils/validation.test.js`.
- `diff_impact` was not directly consulted for this specific change, but prior experiments showed it ignores uncommitted modifications.
- Running the actual tests (`node tests/utils/validation.test.js`) passed with **14/14**.

This reinforces the finding that Chisel's stem-matching heuristic for test suggestion is inaccurate, and working-tree changes are not well-integrated into diff-based impact analysis.

---

## Gaps Identified

| Gap | Severity | Evidence |
|-----|----------|----------|
| `diff_impact` ignores new uncommitted source files | **Critical** | Only returned existing `testPyramid.test.js` impacts |
| `test_gaps` rankings exclude zero-churn new files | **Critical** | 12 new modules with `none` coverage invisible in top 20 |
| `risk_map` silently omits new files | High | New feature files absent from top 20 risk list |
| `suggest_tests` stem-match is inaccurate | Medium | Matched pipeline file to `cli/index.test.js` |
| No "new file coverage gap" metric | High | Files with 0 commits + 0 tests are invisible |

---

## Recommendations

1. **New-File Risk Boost**: Add a `new_file_coverage_gap` component to `risk_map` that scores files with 0 commits and 0 tests as high risk (e.g., 0.8) rather than 0.0.
2. **Working-Tree `test_gaps`**: When `working_tree: true` is set, explicitly include uncommitted files in the gap ranking, ordered by their coverage type (`none` > `indirect` > `partial`).
3. **Improve `suggest_tests` Stem Matching**: Prefer tests in the same directory (`tests/services/X.test.js` for `src/services/X.js`) over fuzzy substring matches.
4. **Diff Impact for New Files**: When `working_tree: true`, detect newly added files and return their directly associated tests as impacted.
5. **Missing-File Warning**: `risk_map` should emit a warning when files in the working tree are excluded from scoring due to lack of git history.

---

## Conclusion

`ChiselWorkingTreeGapInjector` proves that Chisel's tools remain **git-history-centric** despite the `working_tree` flag. The most important gap is that new files with zero coverage are completely invisible to `test_gaps` and `risk_map`. Until Chisel treats the working tree as a first-class source of truth — not just a fallback — it cannot effectively guide development on feature branches or in rapid-iteration workflows.
