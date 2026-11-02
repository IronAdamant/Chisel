# Chisel Rust rewrite

Status: specification. Execution is `plan/rust_finish.md`. The port is not finished until that file's scale bar and the cutover check below are both true.

This document is the long-term rewrite of Chisel into Rust. The reason for the rewrite is faster checks and faster tool calls for agents. Calendar time for the port is unlimited. Build pace stays slow.

`PLAN_NEXT.md` stays the Python feature backlog (cross-shard edges, flakiness trends). Those items are not part of the parity port. A speed improvement may land only after the feature it touches already matches Python.

The Python tree remains the oracle until a feature matches. Nothing in `chisel/` is deleted as part of standing this plan up.

## Goal

Agents spend less time waiting on Chisel. A check (`diff_impact`, `suggest_tests`, `risk_map`, `triage`, `test_gaps`, and the other read tools) and a refresh (`analyze`, `update`) should return sooner than the Python process does, with the same answers.

One static binary, `chisel`, plus the `chisel-mcp` name for the same binary. Same 26 MCP tools, same 28 CLI subcommands, same status envelopes, same risk math. The crate list is empty. Persistence, JSON, HTTP, stdio MCP, and source extraction are first-party modules.

## How each feature is built

Every feature goes through two gates, in order. The second gate does not start until the first has passed.

1. **Match.** Output matches Python 0.15.1 on the fixtures: same ids, scores, statuses, labels, and exit codes. Key order in JSON may differ.
2. **Improve.** Make that feature faster for an agent without changing the matched output. Record what got faster and how it was measured. If an improvement would change a result, it waits.

Improvements that serve the goal, once the matching feature is green:

- Process startup. A static binary instead of a Python interpreter, for one-shot CLI calls agents spawn themselves.
- Warm MCP calls. The stdio and HTTP servers stay up. Time is the tool body, not import time.
- The snapshot store. Typed in-memory maps instead of parsing SQL on every read.
- Fewer `git` process spawns for the same porcelain result: batch where the output is identical, reuse a parse, skip work `update` already knows is unchanged.
- Scanners and edge building. Same units and same edges, less CPU between files.
- Parallel `git blame` and other subprocess work only where the merged result matches the serial one.

`git log -L` and `git blame` still shell out to system `git`. Their runtime is a floor for cold history work. The improvement there is calling them less often and overlapping independent calls, not reimplementing git. Read-tool latency on an already-built snapshot is where agents feel the port first.

## Zero-dependency rule

This rewrite is compliant with a complete zero-dep rule. The equivalents of `sqlite3`, `ast`, `http.server`, and `json` are part of Chisel, not libraries Chisel depends on. Zero-dep limits what the program pulls in. It does not limit how much first-party code we write.

`Cargo.toml` has `dependencies = []` and `dev-dependencies = []` for the life of the port. CI fails the build if either table is non-empty. There is no `build.rs`. There is no vendored third-party C or Rust.

### Allowed

| Piece | Why it is still zero-dep |
|---|---|
| Rust standard library | Same role as the Python standard library. `std::fs`, `std::net`, `std::process`, `std::thread`, `std::io`, `std::env`. |
| First-party JSON module | Replaces `json`. Written and reviewed in this repo. |
| First-party HTTP/1.1 server | Replaces `http.server`. Uses `std::net::TcpListener` only. |
| First-party stdio MCP framing | Replaces the optional `mcp` package and any Rust MCP SDK. The MCP spec is a document, not a dependency. |
| First-party snapshot store | Replaces `sqlite3` for the queries Chisel actually runs. Not a copy of SQLite. |
| First-party language scanners | Replace `ast` for Python and the regex extractors for the other eleven languages. |
| First-party argv parser | Replaces argparse. Twenty-eight subcommands, no `clap`. |
| `extern "C"` for `flock` and `LockFileEx` | The OS call, declared in our code. Rust `std` on Unix already links the system libc, and on Windows already links `kernel32`. We do not add the `libc` crate or the `windows-sys` crate. |
| System `git` via `std::process` | Same runtime requirement as today. Not compiled in. Not a crate. |
| Optional extractor program named by an environment variable | User-supplied process speaking JSON over stdio. Not linked, not shipped. Same posture as `CHISEL_BOOTSTRAP` living outside the wheel. |

### Forbidden

These would break the rule even if they are convenient or "small":

- Any crates.io crate, including dev-only crates (`serde`, `regex`, `libc`, `nix`, `clap`, `tokio`, `rmcp`, `rusqlite`, `tempfile`, `proptest`, `criterion`).
- A vendored SQLite amalgamation, tree-sitter grammar, or any other third-party source dropped into the tree.
- `build.rs` that compiles C or fetches code.
- PyO3, an embedded CPython, or calling Python `ast` at runtime.
- In-process dynamic libraries as the plugin mechanism.
- A PyPI runtime dependency for the stdio server. Stdio MCP moves into the core binary, which is stricter than today's optional `[mcp]` extra.

### Trusted base

What remains trusted, on purpose:

- The pinned Rust compiler and its standard library (the toolchain, analogous to installing CPython).
- The operating system C library that `std` already links, used only for the two lock calls above.
- The `git` executable the user already has.

A snapshot store we can read in one sitting is the supply-chain trade. We own its crash behavior, and the tests for that behavior are part of the store phase, not a follow-up.

## What stays the same

- Tool names and JSON shapes from `chisel/schemas.py`, including `next_steps`.
- Status values: `no_data`, `no_changes`, `no_edges`, `git_error`, `stale_db`, plus the existing tool-specific statuses.
- CLI subcommands and exit codes. `error` and `git_error` exit 1. `chisel run` forwards the test runner's exit code.
- Binary names: `chisel` and `chisel-mcp` (the second is the same binary, or a symlink to it).
- Risk weights in `chisel/risk_meta.py` (`churn` 0.35, `coupling` 0.25, `coverage_gap` 0.15, `coverage_depth` 0.10, `author_concentration` 0.10, `test_instability` 0.05), plus `hidden_risk_factor` (max 0.15) and `new_file_boost` (0.5). One function applies them.
- Banker's rounding wherever Python calls `round`. Rust `f64::round` is not a substitute. Scores keep today's 4-place and 6-place rounding.
- Code-unit ids: `{relative_path}:{name}:{unit_type}`.
- Orphan test edges survive code-unit deletion, because `stale_tests` reads them.
- Environment variables: `CHISEL_PROJECT_DIR`, `CHISEL_STORAGE_DIR` (reject `:memory:` and `file:` URIs), `CHISEL_SHARDS`, `CHISEL_INCLUDE_IGNORED`.
- Git invocations: `log --numstat`, `blame --porcelain`, `log -L`, `merge-base`, `diff --name-only`, `ls-files`. Numstat fields accept digits or `-`. Non-UTF-8 output is replaced, not fatal.
- Thresholds: inline `auto_update` skips above 50 changed files. `analyze` with `force` on more than 300 code files queues a background job.
- Advisory file locks (the six tools) stay advisory records in the store. They do not lock file contents on disk.
- Shards: one store per shard, queries aggregate, writes route by longest path prefix. Cross-shard test edges stay a known gap.

## What changes

- The on-disk database. Rust writes a snapshot file. It does not open `chisel.db`. Existing projects migrate by running `chisel analyze` again. The Python file is left in place and ignored.
- `CHISEL_BOOTSTRAP` as a Python import path. The replacement is `CHISEL_EXTRACTOR`, an executable. The core never imports a language runtime.
- Thread-local "current shard" mutation. A shard is an explicit handle.
- `optimize_storage` rewrites the snapshot. There is no WAL to checkpoint. The tool stays, so agents keep a stable name.
- Stdio MCP no longer needs a separate install extra.
- Agent-facing latency, after parity, as described under "How each feature is built."

## Repository layout

One crate, added beside the Python package. No workspace members.

```text
rust-toolchain.toml          pinned compiler
Cargo.toml                   [dependencies] empty, [dev-dependencies] empty
src/lib.rs
src/main.rs                  argv dispatch
src/json.rs
src/http.rs
src/mcp.rs                   stdio Content-Length JSON-RPC
src/lock.rs                  flock / LockFileEx
src/store.rs                 snapshot, 17 record sets, indexes
src/extract/mod.rs
src/extract/python.rs
src/extract/scanners.rs      the other eleven languages
src/git.rs
src/impact.rs                pure formulas and ranking
src/engine.rs                26 tools, jobs, shards
src/cli.rs
src/bin/chisel.rs
tests/                       cargo tests only, std temp dirs
```

`src/` here means the crate root once the crate exists. The Python package `chisel/` is untouched until cutover.

## Store

Chisel's storage is 17 record sets and a fixed list of lookups (by id, by file path, by pair, plus joins from edges to units). The access rule is already `ProcessLock`: readers hold a shared lock, a writer holds an exclusive lock, and those modes do not overlap. The store relies on that rule. It does not implement SQL, a page cache, or a write-ahead log.

### Files

Inside the storage directory (project `.chisel/` by default, or `CHISEL_STORAGE_DIR`):

- `chisel.lock`. Same flock protocol as `project.py`. Shared for reads, exclusive for writes. A Python process and a Rust process exclude each other if they share a storage directory.
- `store`. The published snapshot.
- `store.tmp`. The file being written. Deleted on the next open if it is still present.

Shard data lives under `shards/<shard>/` with its own `store` and `chisel.lock`.

### Publish

1. Mutate in-memory maps for the duration of the tool call.
2. Serialize the live records to `store.tmp`.
3. `fsync` the temp file.
4. Atomically rename `store.tmp` onto `store`.
5. `fsync` the directory.
6. Release the exclusive lock.

A crash before the rename leaves the previous `store` in place. One flush per tool call, including `chisel run`, so recording many test results does not rewrite once per row.

Readers load `store` under the shared lock. Indexes are hash maps for the primary key and the secondary keys `storage.py` already uses (`file_path`, `code_id`, `test_id`, import target, co-change partner). Joins are lookups in those maps.

Deleted code units do not delete edges that point at them.

`mtime` on `file_hashes` is informational. Content change is decided by SHA-256. A mismatch with a Python-written float must not, by itself, force a re-extract. Rust does not read the Python database, so this only matters if a future import path copies those rows.

### Record sets

Match the current tables, without SQL types:

`code_units`, `test_units`, `test_edges`, `commits`, `commit_files`, `blame_cache`, `co_changes`, `churn_stats`, `file_hashes`, `import_edges` (with `confidence`), `branch_co_changes`, `test_results`, `meta`, `file_locks`, `bg_jobs`, `job_events`, `static_test_imports`.

The snapshot header carries a magic string, a format version, and a generation counter. Format version 1 is the 0.15.1 record set. Later changes bump the version and migrate inside this module.

### Tests that define "durable"

- Two processes: a writer blocks until the reader's shared lock drops, and the reader never observes a torn file.
- `kill -9` of a writer after `store.tmp` exists and before the rename. The next open still loads the previous snapshot and removes the temp file.
- Round-trip every record set, including an edge whose code id has been deleted.
- Reject `:memory:` and `file:` storage directories.

## JSON

A module for the values this program reads and writes.

- Objects, arrays, strings, numbers, booleans, null.
- Duplicate object keys: last value wins, matching Python `json`.
- Integers that fit in `i64` stay integers. Other numbers are `f64`.
- Reject `NaN`, `Infinity`, and `-Infinity` on parse and on write.
- Standard string escapes, explicit UTF-8, a nesting depth cap, a byte-size cap.
- Emit UTF-8. Stdio tool payloads use the same pretty-print the Python server uses (`indent=2`). HTTP responses may be compact, matching `json.dumps` without indent in `mcp_server.py`.

Acceptance: fixtures taken from real tool results and from `schemas.py` parse and re-serialize without changing meaning. A corpus of hostile inputs (deep nesting, truncated strings, bad UTF-8) returns errors and does not panic.

## HTTP

`std::net::TcpListener`, default `127.0.0.1:8377`, bounded thread pool (32 workers, queue depth in line with today's 128).

- `GET /tools` returns the tool schema list.
- `GET /health` returns `{"status":"ok"}`.
- `POST /call` reads a `Content-Length` body, dispatches, and writes JSON.
- Unknown paths return 404 JSON. Bad JSON and bad arguments return 400. Unknown tool returns 404. Uncaught tool failure returns 500.
- One request per connection, matching `BaseHTTPRequestHandler`'s default HTTP/1.0 behavior.
- Body size cap. No TLS, no HTTP/2, no chunked transfer in version 1 of this server. Clients that speak to `chisel serve` today send `Content-Length`.

## Stdio MCP

Frame messages the way a language server does:

```text
Content-Length: <byte length>\r\n
\r\n
<json-rpc body>
```

Implement `initialize`, the initialized notification, `tools/list`, and `tools/call`. `tools/call` runs the engine on a worker thread and returns one text content item whose text is `{"result": ..., "next_steps": ...}` when hints exist. Log to stderr only. Stdout is the protocol stream.

Do not implement resources, prompts, sampling, OAuth, or server tasks. Unknown methods return a JSON-RPC method-not-found error.

Acceptance: a small stdio script drives `initialize`, `tools/list` (26 tools), and one `tools/call`, and the payload matches the Python server for the same engine state.

## Extraction

Output type is the current `CodeUnit`: file, name, unit type, line start, line end.

### Python

Target the `ast` walk in `_extract_python_ast`, not a full Python compiler.

- Track strings (including triple-quoted and f-strings), comments, and bracket depth.
- Recognize `def`, `async def`, and `class` only at bracket depth zero, at the start of a logical line.
- Methods are named `ClassName.method` when the function is a direct member of the class. A function nested inside a method keeps its own name. That matches `parent_map` in the Python extractor.
- Start line is the `def` or `class` line, not the decorator line.
- End line is the last line of the block.
- On a scan failure, use the indent fallback that `_extract_python_regex` already implements.

Acceptance: the cases in `tests/test_ast_utils.py` and the language fixtures produce the same units as Python 0.15.1.

### Other languages

Hand-written scanners for JavaScript, TypeScript, Go, Rust, C#, Java, Kotlin, C, C++, Swift, PHP, Ruby, and Dart, covering the same unit kinds the current extractors emit. Share one brace matcher that ignores braces inside strings and comments.

The JS/TS dependency scanner covers static `import` / `require`, destructuring, ESM defaults, and the dynamic forms (`variable`, template, concat, conditional, `eval`) with the current confidence values. The lookbehind in the assignment scanner is written as "previous character is not an identifier character." No regex crate.

Test-framework detection stays as it is: pytest, Jest/Mocha/Vitest, Playwright, Go test, Rust `#[test]`, xUnit/NUnit/MSTest, JUnit including parameterized forms, XCTest and Swift Testing, PHPUnit, RSpec, Minitest, gtest, Dart test.

`CHISEL_EXTRACTOR`, when set, is run as a subprocess for a file. It receives the path and language on stdin as JSON and returns a JSON list of units. A non-zero exit or bad JSON falls back to the built-in scanner and records a warning on the analyze result. The core binary does not load plugins.

## Git

`git.rs` shells out with `std::process::Command`. Parsers cover the porcelain listed above. Lossy UTF-8 matches today's `errors="replace"`. Missing `git`, a non-repo, and a failed diff return `git_error` with `cwd`, `project_dir`, and a hint. They do not return an empty list.

## Engine behavior worth pinning

- Read tools take the shared process lock. Writes, `analyze`, `update`, and `record_result` take the exclusive lock.
- `update` reprocesses files whose SHA-256 changed and skips the edge rebuild when nothing changed (`edge_rebuild_skipped: true`).
- `analyze` and test discovery skip gitignored trees unless `CHISEL_INCLUDE_IGNORED=1`. Non-git projects are unfiltered.
- Per-function `git log -L` runs only for changed files, and is skipped entirely above 2000 code files. File-level churn still runs.
- Commits that touch more than 50 files are skipped for co-change.
- Single-author history halves the adaptive co-change threshold, with a floor of 1.
- `diff_impact` is branch-aware unless `ref` is set. Untracked code files are included. `working_tree` uses the static import overlay.
- `suggest_tests` source labels stay `direct`, `co_change`, `import_graph`, `static_require`, `hybrid`, `fallback`, `working_tree`.
- Background jobs are one cooperative thread per process. Cancel is a flag checked between files. `job_status` reads the store.
- `auto_update` runs an inline `update` when the store is stale, and records `auto_update_skip_reason` when a job is running or more than 50 files changed.

## Phases

Each phase ends when its match checks pass against the Python oracle on the language fixtures and on a small git fixture. The speed pass for that phase comes after the match, and it must keep the same output. Do not start the next phase by deleting Python. Do not skip a match to chase a timing number.

1. **Toolchain and guard.** Crate with an empty dependency table, pinned toolchain, CI check that both dependency tables are empty, `cargo test` that builds. No behavior yet.
2. **JSON.** Parser, serializer, hostile-input tests, fixtures from real payloads.
3. **Lock and store.** Snapshot publish, the record sets, the durability tests, storage-dir rejection.
4. **Extractors.** Python scanner plus the other languages, diffed against current unit lists. Dynamic-require confidence included.
5. **Git parsers.** Numstat, blame, `log -L`, encoding replacement, `git_error` shape.
6. **Pure impact.** Churn, coupling threshold, quantize boundaries, `compose_risk_score`, proximity weights. Golden JSON for the banker's-rounding edges (values at `n + 0.5` in the last kept digit).
7. **Engine writes.** `analyze`, `update`, edges, `record_result`, incremental hashes, `chisel run` for pytest and Jest output.
8. **Engine reads.** The remaining tools, including `triage`, `working_tree`, and `auto_update` skip reasons.
9. **CLI.** All 28 subcommands and exit codes.
10. **HTTP, then stdio MCP.** Schema list parity and one live tool call on each transport.
11. **Jobs, shards, advisory locks.** Thread-pool behavior, unknown shard error, two shards aggregated on an unscoped read.
12. **Cutover.** Release binaries (Linux musl, macOS arm64, macOS x64, Windows). `chisel-mcp` on `PATH` is enough for current MCP configs. Document that 0.15 databases are not read and that `chisel analyze` rebuilds the snapshot. Only after this phase may the Python package become a thin launcher, and only if we still want a `pip` install path. The launcher must exec the binary and must not reimplement tools.

## Out of scope

- Cross-shard test-edge routing.
- Flakiness trends over weeks.
- A general regex engine, a general SQL engine, or a full Python compiler.
- Tree-sitter, LSP, TLS, HTTP/2.
- Replacing `git`.
- Reading `chisel.db`.
- Changing risk weights, tool names, or id format in the same effort as the match.
- A speed change that alters a matched result. That is a later, separate decision.

## Cutover check

The port is ready to replace the Python entry points when all of the following are true:

- `cargo metadata` shows no dependencies.
- The durability tests pass.
- Extractor output matches on the current fixtures.
- Risk golden files match, including banker's rounding.
- `tools/list` advertises the same 26 tools as `schemas.py`.
- A fixture repo produces the same `diff_impact`, `suggest_tests`, and `risk_map` JSON as Python 0.15.1, ignoring key order.
- `chisel run -- pytest` records results and forwards the pytest exit code.
