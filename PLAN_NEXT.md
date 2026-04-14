# Chisel Next-Phase Plan — Scale, Observability, Coverage

> Goal: Take Chisel from "production-grade for mid-size repos" to "trusted at very-large monorepo scale" while maintaining the **zero-dependency** constraint.

---

## 1. Monorepo Scale — Incremental Import Graph

**Problem:** `_rebuild_import_edges()` calls `clear_import_edges()` and rebuilds the entire static import graph for **all** code files on every `update()` and `analyze()`. On a 10k+ file monorepo this becomes a major bottleneck.

**Approach (zero-dep):**
- Change `_rebuild_import_edges` to accept `changed_files` instead of `code_files`.
- Delete import edges only for files that have actually changed (or been deleted).
- Build new edges only for the changed files and upsert them.
- This preserves existing edges for untouched files and turns an `O(all_files)` operation into `O(changed_files)`.

- [ ] Add `storage.delete_import_edges_for_files(file_paths)` batch helper
- [ ] Refactor `_rebuild_import_edges(changed_files)` to be truly incremental
- [ ] Update `analyze()` and `update()` callers to pass the correct file set
- [ ] Add benchmark/regression test with 1k+ synthetic files to prove the win

---

## 2. Monorepo Scale — Directory-Scoped `suggest_tests`

**Problem:** The `kimi_review_2` audit flagged that there is no bulk `suggest_tests` API. Calling the tool file-by-file is impractical for large fuzz suites or monorepo modules.

**Approach (zero-dep):**
- Add an optional `directory` parameter to `tool_suggest_tests`.
- When `directory` is provided, scan all code files under that path (respecting `working_tree`) and aggregate suggestions.
- Return a map keyed by `code_file_path` with a limited number of top suggestions per file to keep payloads bounded.

- [ ] Add `directory` param to `tool_suggest_tests` and engine method
- [ ] Implement `_suggest_tests_for_directory()` with per-file top-N limiting
- [ ] Add test: directory-scoped suggestions return results for multiple files

---

## 3. Background Job Observability

**Problem:** `start_job` + `job_status` works, but there is no cancellation, no progress streaming, and no event history for long-running analyses.

**Approach (zero-dep):**
- **Cancellation:** Add `cancel_job(job_id)` tool that sets a `cancel_requested_at` timestamp on the `bg_jobs` row. Worker threads check this flag at key phase boundaries and raise a graceful `JobCancelledError`.
- **Progress events:** Create a `job_events` table (`job_id`, `event_type`, `payload`, `created_at`). The engine writes phase-start/phase-end events as it progresses. `job_status` can optionally return the last N events so callers can see "parsing commits…" instead of just a percentage.
- **Timeout hardening:** Add a `started_at` column to `bg_jobs` and a sweeper that auto-fails jobs exceeding a configured max duration (e.g., 30 minutes).

- [ ] Add `cancel_requested_at` and `started_at` columns to `bg_jobs` schema
- [ ] Add `cancel_job` MCP tool + engine method
- [ ] Add `job_events` table and `record_job_event()` storage helper
- [ ] Instrument `analyze()` / `update()` phases to write progress events
- [ ] Update `job_status` to return recent events
- [ ] Add tests for cancellation and event recording

---

## 4. Test Coverage — Framework Fixture Suite

**Problem:** The new regex-based framework detectors for Rust, C#, Java, Swift, and Go module resolution work in practice but have limited dedicated test coverage. A refactor of `test_mapper.py` could silently break them.

**Approach (zero-dep):**
- Create a `tests/fixtures/languages/` tree with minimal, realistic source files for each framework.
- Write data-driven tests that assert `TestMapper.parse_test_file()` finds the expected test units and `build_test_edges()` resolves the right imports.
- Coverage targets:
  - Rust: `#[test]`, `#[tokio::test]`, `#[rstest]`
  - C#: `[Fact]`, `[Theory]`, `[TestMethod]`
  - Java: `@Test`, `@ParameterizedTest`
  - Swift: `@Test`
  - Go: `go.mod` module stripping + package-dir edge matching

- [ ] Create fixture files under `tests/fixtures/languages/{rust,csharp,java,swift,go}/`
- [ ] Add `test_language_frameworks.py` with parameterized cases
- [ ] Verify Go module resolution with nested package paths

---

## 5. Monorepo Scale — Query Pagination & Bounded Payloads

**Problem:** Tools like `risk_map`, `test_gaps`, and `triage` can return enormous JSON payloads on monorepos because they build full Python lists before truncating.

**Approach (zero-dep):**
- Push `limit` enforcement into the SQL layer where possible (e.g., `risk_map` top-N ordering, `test_gaps` capped queries).
- Add hard upper bounds (`max_limit=1000`) on server-side for any tool accepting `limit`.
- Document payload limits in `LLM_CONTRACT.md`.

- [ ] Audit all tools that accept `limit` and enforce `min(limit, 1000)` at the tool boundary
- [ ] Move `risk_map` top-N slicing closer to the DB query where feasible
- [ ] Move `test_gaps` limit into the storage query

---

## 6. Maintenance — DB Optimize & Vacuum

**Problem:** After months of incremental updates, SQLite can accumulate fragmentation and stale query plans.

**Approach (zero-dep):**
- Add `storage.optimize()` that runs `PRAGMA optimize;` and, if WAL size exceeds a threshold, `VACUUM`.
- Expose an optional `optimize` parameter on `analyze`/`update` (default `False`) or a dedicated `optimize_storage` MCP tool.

- [ ] Add `optimize()` to `storage.py`
- [ ] Expose via MCP tool `optimize_storage`
- [ ] Add test verifying `optimize()` does not corrupt the DB

---

## Deferred / Future

These are intentionally **not** in this plan because they either violate zero-dep or require deeper architectural research:

- **Native file watcher / auto-update** — Would require `watchdog` or inotify bindings. Better left to CI/CD calling `start_job(kind='update')`.
- **Tree-sitter extractors** — Would add a compiled dependency. The `register_extractor` / `register_dep_extractor` hooks already allow users to plug tree-sitter in at the application layer without forcing it on Chisel.
- **Distributed storage** — SQLite is the right call for zero-dep. If monorepo scale outgrows a single SQLite file, the next step is sharding by sub-directory, not switching to Postgres.

---

## Acceptance Criteria

- [ ] All new features have tests; total suite ≥ 750 passing.
- [ ] `_rebuild_import_edges` on `update()` with zero changed files completes in < 50 ms on the Chisel repo itself.
- [ ] No new runtime dependencies introduced.
- [ ] `PLAN_NEXT.md` is fully checked off before the next major release.
