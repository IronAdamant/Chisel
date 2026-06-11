---
name: chisel
description: Run Chisel test-impact analysis on any git project — which tests to run after a change, what's risky, what's untested. Use when the user asks to run chisel, find impacted tests, check risk or coverage gaps, or after a batch of code edits in a project with a .chisel/ DB (or to bootstrap one with `chisel analyze`).
---

# Chisel — Test Impact Analysis for Agents

Chisel (source: `~/Documents/coding_projects/Chisel`, CLI: `chisel`, MCP server: `chisel`, PyPI: `chisel-test-impact`) maps tests to code and code to git history, answering: **what to run, what's risky, who touched it.** Zero dependencies, per-project `.chisel/` SQLite DB.

## Core loop (day-to-day)

```bash
chisel analyze .                  # first time, or after big structural changes
# ... edit code ...
chisel diff-impact                # which tests are impacted by my changes?
chisel run -- pytest tests/      # run tests AND auto-record pass/fail results
chisel triage                     # top risks + coverage gaps + stale tests in one call
chisel update                     # incremental refresh (near-instant when nothing changed)
```

Prefer the MCP tools when the `chisel` server is connected (26 tools): `diff_impact`, `suggest_tests`, `triage`, `risk_map`, `test_gaps`, `record_result`, `stats`; `start_job`/`job_status`/`cancel_job` for long analyses. Pass `auto_update=true` on query tools to refresh a stale DB inline. The CLI is equivalent and scripting-safe.

## Key facts (v0.13+)

- **gitignore-aware**: ignored trees (vendored deps, build output, fixture dirs) are never scanned or traversed; untracked-but-not-ignored files ARE visible, so working-tree analysis still works. `CHISEL_INCLUDE_IGNORED=1` overrides; non-git projects are unfiltered.
- **Real CLI exit codes** since v0.13: `chisel analyze && pytest` scripting works (`status: error|git_error` → 1).
- **No-op `update` is near-instant** (`edge_rebuild_skipped: true`); a one-file update rebuilds in seconds, not minutes.
- **Uncommitted files**: pass `working_tree=true` to `risk_map` / `test_gaps` / `suggest_tests` / `diff_impact`.
- **Monorepos**: `CHISEL_SHARDS=pkg1,pkg2` (or `.chisel/shards.toml`); `shard` param on `analyze`/`update`/`start_job` (MCP and CLI `--shard`). Query tools aggregate across shards automatically.
- **`source` trust order** on suggestions: `hybrid > direct > import_graph > co_change > static_require > working_tree > fallback`.
- **Solo-author repos**: co-change coupling is naturally sparse — rely on `import_partners` / `import_coupling`.
- **Risk scores**: read `_meta.uniform_components` (and `reweighted`) before trusting the composite; `exclude_new_file_boost=true` for stable long-term rankings.

## When tools return empty

| Symptom | Action |
|---------|--------|
| `no_data` | Run `analyze` on the repo root. |
| `stale_db` | `auto_update=true`, or run `analyze`/`update`. |
| New/untracked file invisible | `working_tree=true`. |
| `suggest_tests` empty | `fallback_to_all=true` or `working_tree=true`. |
| MCP timeout on analyze | `start_job` + poll `job_status`, or run `chisel analyze` in a terminal. |

Full protocol: `docs/AGENT_PLAYBOOK.md` and `docs/LLM_CONTRACT.md` in the Chisel repo.
