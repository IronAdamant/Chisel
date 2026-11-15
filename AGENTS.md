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

`no_data` and `stale_db` exit 0. `error` and `git_error` exit 1. A usage error (unknown command or flag) exits 2. `chisel <command> --help` works for every command. In the CLI, `analyze --force` runs inline and `start-job` waits for its job.

MCP is `chisel serve-mcp` or the `chisel-mcp` binary. `CHISEL_TOOLS` and `CHISEL_DISABLE` hide MCP tool groups (`core` always on; `risk`, `history`, `maintenance`, `locks`); a hidden tool's call returns `isError` naming the variable to change; an unknown name, or core in `CHISEL_DISABLE`, exits 2 at startup. See README "Tool groups". Stdio messages are newline-delimited JSON. Content-Length framing is still accepted and answered in the same framing. HTTP `chisel serve` answers `GET /health`, `GET /tools`, and `POST /call` only. There is no `/mcp` path. `suggest_tests` on a JS or TS file that calls `eval` or `new Function` adds a `warn_eval_used` next step.

First call `analyze` when you see `no_data`. After edits, call `diff_impact` to pick which tests to run. Its rows are ranked in tiers: direct hits (`source: direct`: the test calls a changed function), then `call_graph` rows (the test calls a function that calls the changed one, e.g. `calls Ui::new, which reaches the changed GlyphAtlas::new`), then weak evidence: `sibling` (no test calls the changed function; tests of other methods of its type) and `co_change` (only for a changed file no test calls into). `depth` 1 keeps direct hits only, 3 goes one caller further (default 2).

Above those tiers comes `changed_test`: a hunk inside a test's own lines names that test first, and only that test (a hunk in a shared helper names the tests that call the helper). Guard tests are not rows: `guards` lists them per test file, `{file, tests, source, rule | reads, changed}` (`changed` only when the diff has more than one file), at most 10 (`guards_total` counts the rest). `source: always_run` is a project rule; `source: reads_source` is a test that reads a changed path by a path written in the test (a guard over `crates/a/src/` built from `env!("CARGO_MANIFEST_DIR")`, an `include_str!`). A rule is a line in the `agent-tools` block: `chisel: always-run crates/a/src/**, scripts/*.py -> crates/a/tests/guard.rs, crates/b/tests/x.rs:name` (paths with `*` and `**`; tests as a file, a test id or a test name). A detected scope is dropped when it is the project root or a folder holding two or more Cargo packages (`crates/**`); a crate's own folders (`crates/a/src/**`) are kept. A test that reads wider needs a rule. Within a tier, a test that reaches more of the changed units, and rarer ones, scores higher (`units` says how many when two or more); a diff with one changed function keeps the scores and order it had. The reply is `{"items": [...], "run": [...]}` (plus `guards` when any apply): `run` holds narrow commands: per package the targets the rows live in and their names (`cargo test -p a --lib --test it -- x y`), guard files whose tests are all named as `cargo test -p a --test guard`, pytest node ids. Past 30 names a command keeps the targets and drops the names, and `run_note` says so. Past `limit` (default 50) the reply stops before a tie that crosses it (`tied` counts it) and summarises the rest in `groups` (`{group, tests, top, run}` per Cargo package or test folder), with `truncated` and `total`.

Rust names a test binds itself (a closure in a `let`, a parameter) are not calls to project functions. A module-qualified call through a std module (`std::fs::write`) reaches no project function of that name. A bare call nothing imports matches functions of the caller's own crate by name only (weight ×0.5), and only those visible from the caller (`pub`, or in its module).

What `diff_impact` diffs: on main or master, uncommitted work against HEAD; on any other branch or a detached HEAD, everything since the merge-base with main (or master). `since: "<ref>"` (CLI `--since`) scores every change from the merge-base of that ref and HEAD to the working tree, committed or not, and the reply becomes `{"items": [...], "since", "base"}`. `ref` is a plain diff against that ref; pass one of the two. A file deleted since the base is never reported as missing.

Worktrees: read tools (`analyze`, `update`, `diff_impact`, `impact`, `suggest_tests`, `risk_map`, `test_gaps`, `triage`, `stale_tests`, `stats`, `churn`, `ownership`, `coupling`, `history`, `who_reviews`) accept `worktree_root`, the absolute path of a git worktree of this project (CLI `--worktree`). The answer is for that tree: main's snapshot is the base and is never written; an overlay under `.chisel/overlays/<hash>/` re-indexes only files whose content differs, is re-checked by mtime and size on each call, is rebuilt when main's snapshot changes, and is removed once its worktree is gone. Files git reports unchanged since main's analyzed commit are not read, and the worktree's index is never written (git runs on a private copy). Replies add `worktree` and `overlay_files` (a list reply becomes `{"items": [...]}`). Started inside a linked worktree with no `--project-dir`/`--storage-dir` (and no store in the worktree itself), the CLI and `serve-mcp` use that worktree automatically. A path that does not exist or belongs to another repository is a one-line `status: "error"`. Results, locks, jobs and captured issues always go to main's snapshot. With shards configured, `worktree_root` answers `unsupported`.

Rust calls resolve by type and path: `Type::f`, `Self::f`, `self.f()` inside `impl Type`, `self.field.f()` through the field's type, a local or parameter whose type is written (`let a = Type::new()`, `fn t(a: &Type)`), and `use` imports. A call through a std trait or type (`Default::default()`, `Vec::new()`) and a std method name on an unknown receiver (`x.clear()`, `x.len()`) link to nothing. A rare method name on an unknown receiver links at a lower weight. Edge rows carry the resolved `owner` type; same-named methods of one file (`GlyphKey::new`, `GlyphAtlas::new`) are told apart by it. In a Cargo workspace a Rust test links only to its own crate and the crates it depends on, and issue suspects follow the same rule. Use `suggest_tests` with `file_path`. Use `triage` for a bounded risk pass. `impact` requires `files`. A missing required argument returns `status: "error"`, not an empty list.

A refused `acquire_file_lock` returns the holder, its `purpose`, `ttl_remaining` and a `reason`. A failed `release_file_lock` or `refresh_file_lock` returns a `reason`. An unknown tool name returns the list of valid names.

Test failures are logged automatically when the `chisel capture --hook` PostToolUse hook is installed (`chisel hook-install --write`). Call `issues` to see open failures, `issue <id>` for suspects (covered code changed since the test last passed) and the fix once it passes, and `trace` for a file's history of breakage. Issues close themselves when the test passes again.

`start_job` returns `queued` and a real `job_id` before the work finishes. `job_status` reads that job. `cancel_job` is observed between files.

## Choosing groups

Agents pick Chisel's MCP tool groups per project with an `agent-tools` block in the project's root `AGENTS.md`. Add one line for Chisel; other tools read their own lines:

````markdown
```agent-tools
chisel: disable locks, history
```
````

| Group | What it is for | Turn it off when |
|---|---|---|
| `core` | analyze, update, diff_impact, impact, suggest_tests, record_result, stats | never (always on) |
| `risk` | risk_map, test_gaps, triage, stale_tests: where to look and what to test next | you only need "which tests to run" |
| `history` | churn, ownership, coupling, history, who_reviews | you read `git log`/`git blame` yourself, or another tool covers it |
| `maintenance` | start_job, job_status, cancel_job, optimize_storage | analyze finishes well within your client's timeout |
| `locks` | advisory file locks with a `purpose` note | a single agent works on the project, or CoordinationHub or Trammel manages claims |
| `issues` | issues, issue, trace: failures captured from test runs, their suspects and fixes | no capture hook runs here and you do not use `chisel run` |

`chisel: tools risk` keeps only `core` and `risk`. Lists take group or tool names, separated by commas or spaces. Chisel re-reads the file on each request and sends `notifications/tools/list_changed` when the visible tools change, so no restart is needed. A bad name is ignored and reported as a warning in the `initialize` instructions and in `stats`. `CHISEL_TOOLS` / `CHISEL_DISABLE` in the client's `env` override the `tools` / `disable` lines respectively.

## Scale

Per-function churn and blame come from one batched `git log -p` history stream, replayed in-process with git's own `-L` and blame rules (`src/history.rs`). A file it cannot replay exactly (dirty or untracked, a diff-driver attribute, a binary diff, a rename from a non-code path) falls back to `git log -L` per function and `git blame` per file. A no-op `update` sets `edge_rebuild_skipped` true and reads no history. One changed file recomputes that file plus newer commits. Above 2000 code files, file-level churn still runs and per-function churn does not (`unit_churn_skipped`). `auto_update` refreshes inline at or under 50 changed files (`auto_update_limit`, or `CHISEL_AUTO_UPDATE_LIMIT`). Above that, the read answers from the snapshot as it is, starts an `update` job and sets `auto_update_skip_reason: "too_many_changed_files"` and `auto_update_job_id`; poll `job_status` with that id, then call again. While a job is running the same fields name that job (`background_job_running`). The CLI has no client timeout and refreshes inline whatever the count.

`working_tree` reads the worktree only when set. `stats` returns counts, not the full result history. List tools apply a default limit and set `truncated` when capped.

Shards are one snapshot each under `.chisel/shards/<shard>/`, configured by `CHISEL_SHARDS` or `.chisel/shards.toml`. Writes use the longest path prefix. Unscoped reads aggregate. An unknown shard is `status: "error"`. Cross-shard test edges are written to the parent `.chisel/store` and overlaid onto the code shard for `impact`, `suggest_tests`, `diff_impact`, risk coverage, and `stale_tests`. Aggregated stats report `cross_shard_test_edges` as a count. A file that matches no shard prefix is analyzed under `.chisel/shards/_unassigned/`. `stale_tests` with `working_tree` also reports a snapshot name that the current file no longer defines.

`chisel run -- pytest …` and Jest record results in one snapshot flush and forward the test process exit code. `chisel watch` waits on an inotify, kqueue, or Windows change notification, then runs `update` when `git status` changed. It does not use a file-watcher crate. Python units come from the built-in syntax tree. Locks are advisory records. They do not lock source files.

Scores are heuristics. Read `_meta.uniform_components` before trusting a composite risk score. `risk_map` and `triage` rows omit fields that are zero, null, false or empty; `file_path`, `risk_score` and `flakiness_trend` always appear. With `exclude_tests` (the default), `risk_map` and `test_gaps` also leave out code under test-support directories (`tests/`, `test/`, `__tests__/`, `testdata/`, `fixtures/`, `__mocks__/`, `spec/`). `flakiness_trend` is present when a covering test has dated results in the current week and in an older week of the last 28 days. It does not change risk weights. `record_result` stores `recorded_at`, the `platform` (default: this OS), and a failure's `message` (300 characters). A pass also records the commit it passed on. A failure after passes answers `first_failure_after: N`; captured issues show it too. `stats` with `not_passed_since: "<ref>"` lists tests with no recorded pass on a commit containing that ref (those that passed before it first), and with `not_run_on: "linux"` tests with no result recorded on that OS.

Test filenames that produce edges include `test_*.py`, `*_test.py`, `*.test.ts`, `*.spec.ts`, `*_test.go`, `*_test.rs`, and any `.rs` file under a crate's `tests/` directory (the root crate's or `crates/a/tests/` in a workspace; not `src/tests/`). Rust functions marked `#[test]`, `#[tokio::test]`, `#[rstest]`, or `#[test_case]` are test units even when the file is not named like a test.

`CHISEL_EXTRACTOR` or `extractor` in `.chisel/extensions.toml` names an executable for code units. `CHISEL_DEP_EXTRACTOR` or `dep_extractor` in that file names an executable for test dependencies. It reads `{"path","language","content"}` and writes a JSON list of `{name,dep_type,module_path,confidence}`. That program may use its own libraries. Chisel does not link them. The toml file is ignored unless the process was started with `--allow-repo-extractor` or `CHISEL_ALLOW_REPO_EXTRACTOR=1`. A tool argument cannot turn it on. The path must stay inside the project. The environment variables override the file and do not need the flag. A bad run falls back to the built-in scanner. `CHISEL_BOOTSTRAP` is not imported as Python. If it is a path to an executable it is the dependency extractor. A git ref that starts with `-` is rejected.
