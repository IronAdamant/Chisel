# Chisel — Detailed Validation Report (Phase MCP Challenge Features)

**Date:** 2026-03-27  
**Project:** RecipeLab (`/home/aron/Documents/coding_projects/RecipeLab_alt`)  
**Scope:** New zero-dependency features built to stress Chisel: **import-chain probe** (`chiselProbe`), **coverage gap** via tests only on the facade, and **impact** expectations for `diff_impact` / `suggest_tests`.

---

## North star (authoritative)

**Chisel exists to give LLM agents reliable test impact analysis and code intelligence**—not decorative metrics. An agent must be able to answer: *what changed, what tests matter, where risk concentrates,* without hand-maintaining a mental model of the whole repo.

RecipeLab validation confirms the **core graph is real** after a full analysis (`analyze --force`): thousands of **test edges** and hundreds of **import edges** are exactly the substrate agents need. Anything that blocks that pipeline—opaque failures, empty suggestions where the graph could apply, or coupling that only works in multi-author git history—is a **product defect**, not a user limitation.

---

## Re-orientation mandate

Treat the items below as **ship blockers** until addressed. Soft language does not apply.

1. **`coupling` based only on co-change is insufficient** for solo and low-commit workflows. **Import-graph and static dependency signals must be first-class** in coupling and related scores. Co-change can remain a supplement; it must not be the only story when history is thin.
2. **Working tree and untracked files must be visible** to test suggestion and impact tools. Agents edit before commit; Chisel must meet them there.
3. **`diff_impact` and git-backed tools must fail loud and usefully** when `git` is missing or the repo root is wrong—clear error, suggested `--project-dir`, never silent empties that look like “no impact.”
4. **MCP `update` timeouts** are unacceptable without guidance: document terminal **`chisel update` / `chisel analyze`**, or stream progress; agents cannot guess whether the tool hung or succeeded.
5. **`coverage_gap` must graduate beyond binary** where the codebase supports it—weighted or proximity-based scoring is required for honest agent decisions.

**Verdict:** Chisel is **on mission** for test impact once the DB is built; the **mandate** is to close the gaps above so “code intelligence for agents” is **true in every workflow**, not only in ideal git histories.

---

## 1. Features that target Chisel

| Feature | Location | Chisel stress mechanism |
|--------|----------|-------------------------|
| **Chisel probe chain** | `src/services/chiselProbe/chainCore.js` → `chainMiddle.js` → `chainInner.js` → `chiselProbeFacade.js` | **Deep import chain** with **no direct tests** on inner modules; only `tests/services/chiselProbeFacade.test.js` imports the facade. |
| **ChiselFacade test** | `tests/services/chiselProbeFacade.test.js` | Asserts `runProbe(4) === 12` (end-to-end chain), exercising **transitive** behavior without unit tests on `chainCore`/`chainMiddle`/`chainInner`. |
| **MCP Triangulation** | `src/services/mcpTriangulationService.js` | Local **import closure** + **heuristic test mapping** — analog to `diff_impact` / `suggest_tests` for comparison. |
| **Unified Readiness Gate** | `src/services/unifiedReadinessGate.js` | Warns when **no** heuristic test mapping exists; pushes workflow toward Chisel-style impact tools. |

---

## 2. Tools invoked (this session)

| Tool | Arguments | Result (short) |
|------|-----------|----------------|
| `suggest_tests` | `file_path: .../chainCore.js` | **[]** (empty) |
| `suggest_tests` | `file_path: .../chiselProbeFacade.js` | **[]** (empty) |
| `diff_impact` | `limit: 25` | **Error:** `git diff --name-only HEAD failed` — environment reported **not a git repository** (MCP sandbox / cwd issue). |
| `risk_map` | `directory: "src/services/chiselProbe"` | **[]** (empty) |

---

## 3. Interpretation

### 3.1 Empty `suggest_tests`

**Expected reasons (per project documentation):**

- **No git history edges** for brand-new files, or files not committed — Chisel’s test graph is often **history-based**.
- **Uncommitted** changes invisible to the graph.

**What the probe chain proves:** Even when a file is **only** reachable through a **facade** test, **risk** and **test suggestions** are hard to infer without **import-graph static analysis**. This directly supports the **Phase 7+ target**: import-graph coupling and working-tree awareness.

### 3.2 `diff_impact` failure

The tool depends on **git diff**. In this environment the MCP server could not run `git` against a valid repo root (rc=129, “not a git repository”).  

**Implication:** The **feature** is still valid for **local** validation (`McpTriangulationService.buildImportClosure`), but **Chisel could not be scored** on this run.

### 3.3 Empty `risk_map` for scoped directory

Possible causes: empty result when the server’s cwd does not match the project, or no data for the scope. Treat as **inconclusive** for this session.

---

## 4. Intended behavior vs. Chisel gaps (mapping)

| Intended signal | Code location | Chisel gap |
|-----------------|---------------|------------|
| Inner module **untested** directly | `chainCore.js`, `chainMiddle.js`, `chainInner.js` | `suggest_tests` returned nothing — aligns with **untracked / no edges** limitation. |
| **Facade** tests cover chain | `chiselProbeFacade.test.js` | `suggest_tests` on facade also empty — **history** likely missing in MCP workspace. |
| **Transitive impact** | `mcpTriangulationService.js` | `diff_impact` unavailable — **git** environment issue. |

---

## 5. Required actions for Chisel maintainers

These are **not optional recommendations**. They are the minimum to align the product with its stated purpose.

1. **Ship import-graph static analysis** for `suggest_tests` and coupling-adjacent signals. Co-change alone is **not** sufficient.
2. **Implement working-tree and untracked-file analysis** for test suggestions and impact; treat “committed only” as a bug for agent workflows.
3. **When `git` is unavailable**, return explicit failure with remediation—**never** empty results that read as “nothing to do.”

---

## 6. Local API / behavior reference

- **Probe math:** `runProbe(4)` → inner chain → `12` (verified in unit test).  
- **Triangulation:** `GET /api/mcp-triangulate?goal=...` and `POST /api/mcp-readiness/evaluate` expose **import closure** and **test heuristics** without third-party deps.

---

## 7. Verdict for this batch

| Chisel capability | Verdict |
|-------------------|--------|
| `suggest_tests` | **Empty** for probe files — **consistent with known limitations** for new/untracked files. |
| `diff_impact` | **Failed** — **environment/git root** issue, not necessarily Chisel logic. |
| `risk_map` | **Empty** — inconclusive. |
| **Design intent** | **Achieved:** Code encodes a **deep chain** + **single test surface** for future Chisel import-graph validation. |

---

## 8. Full re-analysis audit (2026-03-27, local CLI)

**Command run (host):**

```text
chisel analyze --force --project-dir /home/aron/Documents/coding_projects/RecipeLab_alt
```

**Reported output:**

| Metric | Value |
|--------|------:|
| Code files scanned | 202 |
| Code units found | 1,058 |
| Test files found | 45 |
| Test units found | 846 |
| Test edges built | 8,966 |
| Commits parsed | 3 |
| Orphaned results cleaned | 8 |

**Follow-up `chisel stats` (earlier agent run, pre/post may differ slightly):** code units ~1,043–1,058, test units ~839–846, test edges ~8,892–8,966, **import edges 270**, **co-changes 0**, churn stats ~281.

### Verdict: is Chisel “good enough” for this project?

**Yes, for what it does best here:** after `analyze --force`, the **test graph is dense** (thousands of test edges, hundreds of import edges). That supports **`diff_impact`**, **`suggest_tests`**, **`test_gaps`**, **`stale_tests`**, and **`churn`** as long as the DB is kept fresh (`chisel update` after meaningful edits) and git history is visible to Chisel.

**Still weak (unchanged):** **`coupling`** (co-change) will stay near **empty** in a **low-commit / single-author** repo — stats showed **Co Changes: 0**. Use **import-graph** tools in-repo or Chisel’s **import edges** as the practical coupling signal, not co-change alone.

**Operational note:** MCP `update` can **time out**; prefer **`chisel update` / `chisel analyze --force`** in a terminal for long runs, then use MCP for **`stats`**, **`suggest_tests`**, **`diff_impact`**.

---

## Related findings (root `MCP_Findings/`)

- `stele-context.md` — memory, symbols, search re-orientation  
- `trammel.md` — plan fidelity and multi-agent execution  

---

## Closure — Fixes Applied (2026-04-14)

The following gaps identified in this report have been addressed in Chisel `main`:

1. **Import-graph coupling is now first-class.** The coupling formula changed from `max(cochange, cochange + 0.25 * import)` to `max(cochange, import, 0.5*cochange + 0.5*import)` in `tool_coupling`, `compute_risk_score`, and `get_risk_map`. In single-author/low-commit repos, import coupling now dominates instead of being a minor boost.
2. **Working-tree visibility** was already partially present and has been deepened:
   - `risk_map` now supports `working_tree=true` to include untracked files in scoring, with a `new_file_boost` of 0.5 for zero-churn + zero-coverage files.
   - `test_gaps` elevates working-tree gaps to the top of the list.
   - `diff_impact` static import scan and stem-match fallback now apply to ALL changed files (staged + untracked).
3. **`diff_impact` structured git errors** were already present (`status: "git_error"`, `error: "not_a_git_repo"`, `cwd`, `project_dir`, `hint`).
4. **MCP timeout guidance** is now surfaced directly in return values: `tool_analyze` and `tool_update` include a `hint` suggesting `start_job` for large repos.
5. **Graduated coverage gap** is now the default: `risk_map` defaults to `coverage_mode="line"` and `proximity_adjustment=True`, so coverage gaps are weighted by line count and proximity to tested code instead of binary per-unit.
6. **`analyze`/`update` git warnings**: When git is unavailable, the returned stats dict includes `git_warning` explaining that churn, blame, and coupling will be missing.

This file is closed.
