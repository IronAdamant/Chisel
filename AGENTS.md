# Chisel

The product is the Rust crate in `src/`. Build it with `cargo build --release`. The binary is `chisel`. `chisel-mcp` is the same program and starts the stdio MCP server.

There is no Python package in this tree. Result checks use the JSON under `tests/fixtures/oracle`. Do not pip-install a package to work on this tree. Do not follow `AGENTS.md` inside `lang_any_sample_project_for_testing_10k_monorep/`. That tree is a fixture.

## Storage

`.chisel/store` is the snapshot. `chisel migrate` is the only command that reads a legacy SQLite `chisel.db`, and it only copies `test_results` into the snapshot. `CHISEL_PROJECT_DIR` and `CHISEL_STORAGE_DIR` are honoured when CLI flags are absent. `:memory:` and `file:` storage paths are rejected.

## How to call it

CLI agents pass `--json` and branch on `status`. Exit code is not enough.

| Status | Meaning |
|---|---|
| `no_data` | No snapshot yet. Call `analyze`. |
| `no_changes` | Diff is empty. |
| `no_edges` | Nothing to evaluate for stale tests. |
| `git_error` | Git failed. Read `error`, `cwd`, `project_dir`. |
| `stale_db` | Snapshot is behind the worktree. `update`, or `auto_update`. |
| `error` | Bad arguments or unknown shard. |
| `unsupported` | That operation is not available. |
| `auto_queued` | `force` analyze on more than 300 code files. Poll `job_status`. |

`no_data` and `stale_db` exit 0. `error` and `git_error` exit 1.

MCP is `chisel serve-mcp` or the `chisel-mcp` binary. Stdio messages are newline-delimited JSON. Content-Length framing is still accepted and answered in the same framing. HTTP `chisel serve` answers `GET /health`, `GET /tools`, and `POST /call` only. There is no `/mcp` path. `suggest_tests` on a JS or TS file that calls `eval` or `new Function` adds a `warn_eval_used` next step.

First call `analyze` when you see `no_data`. After edits, call `diff_impact`. Use `suggest_tests` with `file_path`. Use `triage` for a bounded risk pass. `impact` requires `files`. A missing required argument returns `status: "error"`, not an empty list.

`start_job` returns `queued` and a real `job_id` before the work finishes. `job_status` reads that job. `cancel_job` is observed between files.

## Scale

A no-op `update` sets `edge_rebuild_skipped` true and does not spawn `git blame` or per-function `git log -L`. One changed file recomputes that file plus newer commits. Above 2000 code files, file-level churn still runs and per-function `git log -L` does not (`unit_churn_skipped`). `auto_update` refreshes inline at or under 50 changed files. Above that, or while a job is running, the read still returns and sets `auto_update_skip_reason`.

`working_tree` reads the worktree only when set. `stats` returns counts, not the full result history. List tools apply a default limit and set `truncated` when capped.

Shards are one snapshot each under `.chisel/shards/<shard>/`, configured by `CHISEL_SHARDS` or `.chisel/shards.toml`. Writes use the longest path prefix. Unscoped reads aggregate. An unknown shard is `status: "error"`. Cross-shard test edges are written to the parent `.chisel/store` and overlaid onto the code shard for `impact`, `suggest_tests`, `diff_impact`, risk coverage, and `stale_tests`. Aggregated stats report `cross_shard_test_edges` as a count. A file that matches no shard prefix is analyzed under `.chisel/shards/_unassigned/`. `stale_tests` with `working_tree` also reports a snapshot name that the current file no longer defines.

`chisel run -- pytest …` and Jest record results in one snapshot flush and forward the test process exit code. `chisel watch` waits on an inotify, kqueue, or Windows change notification, then runs `update` when `git status` changed. It does not use a file-watcher crate. Python units come from the built-in syntax tree. Locks are advisory records. They do not lock source files.

Scores are heuristics. Read `_meta.uniform_components` before trusting a composite risk score. `flakiness_trend` is present when a covering test has dated results in the current week and in an older week of the last 28 days. It does not change risk weights. `record_result` stores `recorded_at`.

Test filenames that produce edges include `test_*.py`, `*_test.py`, `*.test.ts`, `*.spec.ts`, `*_test.go`, `*_test.rs`, and any `.rs` file under the top-level `tests/` directory. Rust functions marked `#[test]`, `#[tokio::test]`, `#[rstest]`, or `#[test_case]` are test units even when the file is not named like a test.

`CHISEL_EXTRACTOR` or `extractor` in `.chisel/extensions.toml` names an executable for code units. `CHISEL_DEP_EXTRACTOR` or `dep_extractor` in that file names an executable for test dependencies. It reads `{"path","language","content"}` and writes a JSON list of `{name,dep_type,module_path,confidence}`. That program may use its own libraries. Chisel does not link them. The toml file is ignored unless the process was started with `--allow-repo-extractor` or `CHISEL_ALLOW_REPO_EXTRACTOR=1`. A tool argument cannot turn it on. The path must stay inside the project. The environment variables override the file and do not need the flag. A bad run falls back to the built-in scanner. `CHISEL_BOOTSTRAP` is not imported as Python. If it is a path to an executable it is the dependency extractor. A git ref that starts with `-` is rejected.
