# Chisel MCP Report - Review Twelve

## Date: 2026-04-10

## Challenge Feature: TestAffinityAnalyzer (Feature 2)

### Purpose
Built a test-source affinity scoring system (532 LOC) that analyzes import graphs between source and test files, computes affinity scores, identifies uncovered sources, redundant tests, and hotspots. This was designed to challenge Chisel's own test analysis capabilities - our service computes metrics that Chisel should validate.

### Additional Coverage
RecipeComplianceEngine (Feature 6, ~450 LOC) challenged Chisel with a 5-dependency import graph (Recipe + NutritionService + DietaryComplianceService + CostEstimationService + db).

---

## Tool Results

### update - PASS
- **Result**: 18 files updated, 141 code units found, 0 new commits
- **Performance**: Fast, no errors
- **Assessment**: Reliable incremental analysis. Correctly detected all 18 new files.

### diff_impact - NOTABLE FINDING
- **Result**: 20 impacted tests returned (all from testPyramid.test.js)
- **Reason**: `direct edge to createTempTestDir (call)` - testPyramid creates temp directories and scans for .js files, so new files affect its classification results
- **Gap**: diff_impact did NOT list the 6 new test files themselves as impacted by the new services. This is because the new files are **untracked** (not yet committed). Chisel's diff_impact compares against git refs, not working tree.
- **Known limitation**: This was documented in Phase 7. Chisel needs `working_tree: true` support for diff_impact (currently only available on suggest_tests).
- **Assessment**: Correct for what it can see (committed files), but blind to untracked new files.

### suggest_tests - PASS
- **Target**: `src/services/recipeComplianceEngine.js`
- **Result**: 10 test functions identified, all from `recipeComplianceEngine.test.js`
- **Source**: `hybrid` (direct DB edge + static import → source path)
- **Relevance score**: 0.512 for all
- **Assessment**: Correctly identified the right test file with hybrid source verification. For a newly created, uncommitted file, this is good performance. The `working_tree: true` flag enabled detection of untracked test files.
- **Comparison to prior**: Consistent with Review Eight results. Hybrid source confirmation is reliable.

### test_gaps - PASS
- **Scope**: `src/services/` directory
- **Result**: 10 untested functions/classes identified
- **Notable gaps found**:
  - `collectionService.js:CollectionService` (deliberate gap)
  - `costEstimationService.js:CostEstimationService` (deliberate gap)
  - `dietaryComplianceService.js:DietaryComplianceService` (deliberate gap)
  - Various small utility functions (chiselProbe, mcpTriangulation)
- **Assessment**: None of the 6 new services appear in gaps - confirming our tests provide coverage. The deliberate gaps from Phase 6 are still correctly identified.
- **Binary coverage issue persists**: test_gaps correctly shows CostEstimationService as untested, but RecipeComplianceEngine (which uses CostEstimationService internally) doesn't give CostEstimation partial credit. Coverage is still binary: directly tested (0.0 gap) or not (1.0 gap).

### coupling - CO-CHANGE STILL 0.0
- **Target**: `src/services/recipeComplianceEngine.js`
- **Result**:
  - `co_change_partners`: [] (empty - 0.0)
  - `import_partners`: 5 files (complianceRoutes, Recipe, CostEstimation, DietaryCompliance, NutritionService)
  - `import_coupling`: 0.625
  - `effective_coupling`: 0.625
- **Assessment**: Co-change coupling remains fundamentally broken for single-author projects. Import coupling works correctly and identifies all 5 dependencies. The effective_coupling falls back to import_coupling when co-change is 0.0.
- **Known limitation**: This has been confirmed across all 12 reviews. Requires multi-author commit history.

### risk_map - ORIENTATION DATA
- **Total files**: 285 (pre-commit, includes new files via working_tree)
- **Total test edges**: 35,944
- **Uniform components**: `test_instability: 0.0` (all tests passing)
- **Co-change coupling**: 0.0 globally (single-author)
- **Highest risk**: `src/cli/index.js` (0.534) - partial coverage, high import coupling
- **Assessment**: Risk map provides useful orientation but the uniform co-change coupling component means 1 of 5 risk dimensions is always noise.

### record_result - PASS (6/6)
- All 6 new test files recorded as passing
- Duration captured for future test instability tracking
- Assessment: Reliable. Always works.

---

## Gaps Confirmed

| Gap | Status | Severity |
|-----|--------|----------|
| Co-change coupling 0.0 for single-author | CONFIRMED (12th time) | HIGH |
| diff_impact blind to untracked files | CONFIRMED | HIGH |
| coverage_gap is binary (0/1) | CONFIRMED | MEDIUM |
| risk_map uniform co-change misleads composite | CONFIRMED | MEDIUM |

## New Findings

1. **TestAffinityAnalyzer vs Chisel comparison**: Our custom affinity analyzer computes per-file affinity scores (0.0-1.0) with 4 weighted dimensions (direct import 0.4, naming 0.2, transitive 0.2, shared deps 0.2). Chisel's suggest_tests returns a single relevance score (0.512) with source type. Our analyzer provides more granular insight per file-pair; Chisel provides broader coverage across the whole project. They're complementary.

2. **suggest_tests working_tree improvement**: The `working_tree: true` parameter is now essential for active development. Without it, new files are invisible. This should be the default.

3. **Import coupling is the reliable signal**: With co-change at 0.0, import_coupling (0.625 for compliance engine) is the only useful coupling metric. Chisel should weight import_coupling higher when co-change data is absent.

## Recommendations

1. **Add working_tree support to diff_impact**: Currently only suggest_tests supports it.
2. **Graduated coverage_gap scoring**: Files imported by tested code should get partial credit (0.3-0.7 instead of binary 1.0/0.0).
3. **Auto-detect single-author and suppress co-change**: When all commits are single-author, suppress co-change from risk_map to avoid uniform noise.

---

## Root Cause Analysis: Chisel Source Code Trace

Source located at `/home/aron/Documents/coding_projects/Chisel/`.

### Issue 1: Co-change Coupling Returns 0.0 (Single-Author)

**Code path**: `metrics.py:160-202` → `engine.py:379-381,905` → `git_analyzer.py:75-134`

**Execution trace**:

1. `compute_co_changes()` at `metrics.py:160` counts file pairs appearing in the same commit
2. Each commit includes an `author` field from `git_analyzer.py:75-134`
3. An adaptive threshold filters pairs at `engine.py:905`:
   ```python
   def coupling_threshold(commit_count):
       """Adaptive co-change threshold with half-log scaling."""
       if commit_count <= 0:
           return 2
       return max(2, int(math.log2(commit_count) / 2) + 1)
   ```
   For 10 commits → threshold 2, for 50 commits → threshold 3.
4. The query filters at `engine.py:379-381`:
   ```python
   query_min = self.storage.get_co_change_query_min()
   effective = max(min_count, query_min)
   co_change_partners = self.storage.get_co_changes(file_path, min_count=effective)
   ```

**Why it fails**: In single-author projects with bulk commits (entire features committed at once), individual file pairs rarely co-occur >= threshold times across *separate* commits. A file pair needs to appear together in 3+ distinct commits to register. Single-author workflows typically produce fewer, larger commits rather than many small ones, so pairs don't accumulate enough co-occurrence counts.

**The design assumption**: Co-change was designed to detect **collaboration patterns** — files that different people need to modify together. Single-author repos have no collaboration signal, so the metric legitimately returns zero. But this ignores that a single author's *own* commit patterns still carry coupling signal (files modified together repeatedly are logically coupled).

**Fix**: Detect `distinct_authors == 1` and either lower the threshold to 1, or use branch-based co-change (partially implemented at `engine.py:691,930-935`), or weight `import_coupling` to 100% when co-change data is absent.

### Issue 2: diff_impact Blind to Untracked Files

**Code path**: `engine.py:514-573`

**Execution trace**:

1. `engine.py:514-521` — diff_impact DOES detect untracked files:
   ```python
   diff_files = self.git.get_changed_files(ref)
   untracked_raw = self.git.get_untracked_files()
   untracked_code = {
       p for p in untracked_raw
       if path_has_code_extension(p)
   }
   changed_files = sorted(set(diff_files) | untracked_code)
   ```

2. `engine.py:534-541` — But function extraction is skipped for untracked:
   ```python
   for fp in changed_files:
       if fp in untracked_code:
           continue  # Cannot git diff untracked files
       functions.extend(self.git.get_changed_functions(fp, ref))
   ```

3. `engine.py:549-573` — Untracked files are passed to impact analysis with stem-match fallback:
   ```python
   result = self.impact.get_impacted_tests(
       changed_files,
       functions or None,
       untracked_files=untracked_code,
   )
   ```

**Why the issue manifests**: diff_impact does include untracked files, but since they have no git history, the DB has zero test edges for them. The stem-match fallback (`_working_tree_suggest()`) kicks in with a fixed 0.5 relevance threshold at line 561. However, this only finds tests whose filenames match the source filename — it misses tests that import the file by path. The result is that new files either get weak stem-match results (source=`working_tree`) or nothing.

**The real gap**: diff_impact lacks a `working_tree: bool` parameter like `suggest_tests` has (`engine.py:318-319`). Users cannot control whether the working-tree scan includes untracked files' static imports. suggest_tests with `working_tree=true` performs a full static require scan of untracked test files; diff_impact does not.

**Fix**: Add `working_tree: bool` parameter to `tool_diff_impact()` that enables full static import scanning for untracked files, matching what suggest_tests already does.

### Issue 3: coverage_gap is Binary (0.0 or 1.0)

**Code path**: `impact.py:74-76,786-790`

**Execution trace**:

1. Quantization function at `impact.py:74-76`:
   ```python
   def _quantize_gap(value, steps=4):
       """Quantize coverage_gap to fixed steps for graduated risk levels."""
       return round(value * steps) / steps
   ```
   With `steps=4`, possible outputs are `[0.0, 0.25, 0.5, 0.75, 1.0]`.

2. Coverage calculation at `impact.py:786-790`:
   ```python
   if coverage_mode == "line" and total_lines > 0:
       coverage = tested_lines / total_lines
   else:
       coverage = tested_count / max(len(code_units), 1)
   coverage_gap = _quantize_gap(1.0 - coverage)
   ```

3. Proximity adjustment at `impact.py:800-803`:
   ```python
   if proximity_adjustment and fp not in tested_files:
       mh = hop_dist.get(fp)
       if mh is not None and mh > 0:
           coverage_gap = _apply_coverage_proximity(coverage_gap, mh)
   ```
   Only applies to **completely untested** files when `proximity_adjustment=true`.

**Why it's binary in practice**: The quantization itself supports 5 levels, but the coverage input is already binary for most files. A file either has test edges (all code units → `tested_count = len(code_units)` → coverage = 1.0 → gap = 0.0) or it doesn't (tested_count = 0 → coverage = 0.0 → gap = 1.0). There's no concept of "this file is partially covered" because test edges are file-to-file, not file-to-function. If ANY test imports a file, ALL code units in that file are marked as "covered."

**The design flaw**: Coverage is measured by "does at least one test import this file" rather than "what percentage of this file's functions are exercised." The `coverage_mode: "line"` option weights by line count but still uses the same binary test-edge detection — it just makes bigger untested functions contribute more gap.

**Fix**: Increase `_QUANTIZE_STEPS` from 4 to 20 for finer granularity. More importantly, implement **function-level test edges** — track which specific functions within a file are called by tests, not just whether the file is imported. Also, the proximity adjustment should apply to partially-tested files (files imported by tested code), not just completely untested ones.

### Issue 4: risk_map Doesn't Suppress Uniform Components

**Code path**: `risk_meta.py:82-159`

**Execution trace**:

1. Uniform detection at `risk_meta.py:82-110` correctly identifies uniform components:
   ```python
   for comp in _COMPONENTS:
       values = {f["breakdown"][comp] for f in files}
       if len(values) <= 1:
           uniform[comp] = {
               "value": val,
               "reason": _diagnose_uniform(comp, val, stats),
           }
       else:
           effective.append(comp)
   ```
   Returns `{"test_instability": {"value": 0.0, "reason": "all covering tests passing"}}` — correct.

2. Reweighting gate at `risk_meta.py:131`:
   ```python
   if len(uniform) < 3:
       return risk_map, {"reweighted": False}
   ```
   **Only reweights if 3 or more components are uniform.**

3. In single-author projects:
   - `cochange_coupling`: 0.0 everywhere (uniform) — 1 component
   - `test_instability`: 0.0 everywhere (uniform) — 2 components
   - `churn`, `coverage_gap`, `author_concentration`: vary — NOT uniform
   - Total uniform = 2 → **below threshold of 3 → no reweighting**

**Why the composite score is misleading**: With 2 uniform-zero components still weighted in the composite, every file gets ~40% of its risk score from components that carry zero information. A file with high churn and low coverage might score 0.45 instead of 0.75, because coupling (0.0) and test_instability (0.0) dilute the signal.

**The design assumption**: The threshold of 3 was chosen to avoid reweighting when only 1-2 components are noisy (conservative approach). But the issue is that uniform-at-zero components (coupling=0.0, instability=0.0) aren't just noisy — they're **provably absent**. Noisy data and absent data should be treated differently.

**Fix**: The threshold at line 131:
```python
if len(uniform) < 3:
    return risk_map, {"reweighted": False}
```
Should be changed to either:
- **Option A**: Lower threshold to 2: `if len(uniform) < 2`
- **Option B**: Special-case zero-value uniforms: always exclude components that are uniform AND have value 0.0, regardless of count
- **Option C**: Add `noise_adjusted_risk_score` field that strips uniform-zero components from the composite, keeping the original `risk_score` for backward compatibility

---

## Summary: Source-Level Root Causes

| Issue | Source File | Key Lines | Design Assumption That Fails |
|-------|-----------|-----------|------------------------------|
| Co-change 0.0 | `metrics.py`, `engine.py` | 160-202, 379-381, 905 | Co-change = collaboration signal; single-author = no signal. But single-author commit patterns DO carry coupling information. |
| diff_impact untracked | `engine.py` | 514-573 | Untracked files are detected but only get stem-match fallback. No `working_tree` parameter for full static import scanning. |
| coverage_gap binary | `impact.py` | 74-76, 786-790 | Test edges are file-level, not function-level. Any import = fully covered. Quantization to 4 steps can't help when input is already 0 or 1. |
| risk_map uniform | `risk_meta.py` | 82-159, line 131 | Reweighting threshold of 3 is too conservative. Uniform-at-zero components (provably absent data) should be excluded regardless of count. |
