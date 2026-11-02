---
name: chisel
description: Run Chisel test-impact analysis on any git project — which tests to run after a change, what's risky, what's untested. Use when the user asks to run chisel, find impacted tests, check risk or coverage gaps, or after a batch of code edits.
---

# Chisel — Test Impact Analysis for Agents

The binary is `chisel`, built from `src/` with `cargo build --release`. `chisel-mcp` is the same binary and starts stdio MCP. Oracle JSON is in `tests/fixtures/oracle`. Do not pip-install a package to get this binary.

Storage is the snapshot `.chisel/store`. Chisel does not open `chisel.db`.

## Core loop

```bash
chisel --json analyze .
chisel --json diff-impact
chisel --json suggest-tests src/calc.py
chisel --json triage
chisel --json update
chisel run -- pytest tests/
```

Prefer MCP when `chisel-mcp` is connected (26 tools). HTTP `chisel serve` is `GET /health`, `GET /tools`, and `POST /call` only. It is not an MCP URL and it has no `/mcp` path.

Always pass `--json` on the CLI and branch on `status` before treating a payload as a list. `no_data`, `stale_db`, and `no_changes` exit 0. `error` and `git_error` exit 1.

## Status values

| Status | What to do |
|---|---|
| `no_data` | `analyze` |
| `stale_db` | `update`, or call again with `auto_update` true |
| `no_changes` | Nothing in the diff to impact |
| `no_edges` | No test edges yet. Check that test filenames match the discovery patterns |
| `git_error` | Git failed. Read `error`, `cwd`, and `project_dir` |
| `error` | Missing required argument or unknown shard. Read `message` |
| `auto_queued` | `force` analyze on more than 300 code files. Poll `job_status` with the returned `job_id` |
| `unsupported` | That call is not available. Read `message` |

`auto_update` runs inline only when at most 50 files changed and no job is running. Otherwise the tool still returns and sets `auto_update_skip_reason` to `too_many_changed_files` or `background_job_running`.

## Flags that change results

- `working_tree` true re-reads the worktree. Leave it false to use the snapshot.
- `suggest_tests` requires `file_path`. `impact` requires `files`. A missing required argument is `status: "error"`, not an empty list.
- `CHISEL_PROJECT_DIR` and `CHISEL_STORAGE_DIR` apply when the CLI flags are absent.
- `CHISEL_INCLUDE_IGNORED=1` scans ignored files. Non-git projects are unfiltered.
- Shards: `CHISEL_SHARDS` or `.chisel/shards.toml`. One snapshot per shard. Unscoped reads aggregate. Unknown shard is `status: "error"`. Cross-shard test edges are unsupported.
- Above 2000 code files, file-level churn still runs and per-function `git log -L` does not (`unit_churn_skipped`).
- A no-op `update` sets `edge_rebuild_skipped` true and does not run `git blame` or `git log -L`.
- `chisel run` records pytest and Jest and forwards the test process exit code. One snapshot flush for the whole run.
- `stats` returns counts. It does not embed every historical result.
- List tools apply a default limit and set `truncated` when the cap cuts the list.
- Suggestion `source` is one of `direct`, `co_change`, `import_graph`, `static_require`, `hybrid`, `fallback`, `working_tree`.
- Locks are advisory rows in the snapshot.

## When a repo is large

`start_job` with `kind` `analyze` or `update` returns `queued` and a real `job_id` before the work finishes. Poll `job_status`. `cancel_job` is observed between files. Do not wait on a synchronous `analyze` of a monorepo inside an MCP timeout.
