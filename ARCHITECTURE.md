# Architecture

Chisel is a zero-dependency Rust program. `AGENTS.md` is the agent contract. `plan.md` records the layout and the rules that stay in force.

The crate exposes two binaries. `chisel` is `src/main.rs`. `chisel-mcp` is `src/bin/chisel-mcp.rs`. Both call `cli::entry`. When the program name is `chisel-mcp`, the entry inserts `serve-mcp` and speaks stdio MCP. `chisel serve` is HTTP only: `GET /health`, `GET /tools`, and `POST /call` on `127.0.0.1:8377`. There is no `/mcp` path.

## Flow

1. `analyze` lists code files through git (or a walk when ignored files are included), extracts units, and writes one snapshot.
2. Test files come from `looks_like_test` in `src/engine/mod.rs`. That is the only filename classifier. Rust files under the top-level `tests/` directory are test files. Functions with a `#[test]`-family attribute are test units even inside a source file.
3. Edges are calls and imports from those test units to code units. `suggest-tests`, `diff-impact`, `test-gaps`, and `risk-map` read the snapshot.
4. `update` recomputes changed files. A no-op update sets `edge_rebuild_skipped` and reads no history. Function churn and blame for the files in scope come from one `git log -p` stream (`src/history.rs`, driven by `src/engine/history_churn.rs`); files it cannot replay exactly fall back to `git log -L` per function and `git blame` per file.
5. `risk-map` and `test-gaps` skip paths that `looks_like_test` accepts. A source file that also holds `#[test]` functions stays in those answers. The test functions themselves are not listed as untested code.

## Modules

| Module | Responsibility |
|---|---|
| `src/cli.rs` | Flags, subcommands, `chisel run`, `chisel watch`. |
| `src/engine/analyze.rs` | Full and incremental scans, test units, edges. |
| `src/engine/query.rs` | Read tools: impact, suggestions, risk, gaps, triage. |
| `src/engine/callers.rs` | Callers of changed functions, a few calls up, read from source at query time, for `diff_impact` `depth`. |
| `src/engine/issues.rs` | Issue capture, suspects, hunk-to-unit mapping, fix summaries. |
| `src/cargo.rs` | Cargo manifests: which crates a Rust test can reach (dependencies, dev-dependencies, workspace deps). |
| `src/engine/locks.rs` | Advisory lock rows in the snapshot. |
| `src/extract` | Python syntax units, brace-language units, import and call deps. `calls.rs` reads how a name is called (`Type::f`, `x.f()`, `f()`, outside strings and comments) and `use` imports. |
| `src/syntax.rs` | Built-in Python and JavaScript syntax tree used by the scanners. |
| `src/store.rs` | One snapshot file. Write to a temp file, fsync, rename. |
| `src/git.rs` | Porcelain via `std::process`. A ref that starts with `-` is rejected. |
| `src/history.rs` | Parses one `git log -p -m -U0` stream and replays git's `-L` line ranges and blame over it. |
| `src/engine/history_churn.rs` | Runs the batched git calls for analyze and update; spawns `-L`/`blame` only for files the stream cannot replay. |
| `src/impact.rs` | Churn, coupling, coverage gap, risk score. |
| `src/json.rs` | Parser, serializer, Python-compatible rounding for scores. |
| `src/hash.rs` | SHA-256 for file content. |
| `src/http.rs` | The small JSON HTTP server. |
| `src/mcp.rs` | Stdio MCP. Newline-delimited JSON, and Content-Length when the client sends that. |
| `src/fswatch.rs` | OS change notification. `collect_files` is only compiled for the kqueue path. |
| `src/lock.rs` | Cross-process shared and exclusive locks. |
| `src/schemas.rs` | Tool list. Missing required arguments are `status: "error"`. |
| `src/next_steps.rs` | Hint rows attached to tool results. |

Shards are one snapshot each under `.chisel/shards/<shard>/`. Writes use the longest path prefix. Unscoped reads aggregate. Cross-shard test edges are written to the parent snapshot and overlaid for impact, suggestions, diff, risk coverage, and stale tests.

## Storage and git

`.chisel/store` is the snapshot. `chisel migrate` reads a legacy `chisel.db` once and copies `test_results`. Other commands do not open it. `:memory:` and `file:` storage paths are rejected. `CHISEL_PROJECT_DIR` and `CHISEL_STORAGE_DIR` apply when the CLI flags are absent.

Git is a subprocess. Locks are advisory records. They do not lock source files.

## Status

`no_data`, `no_changes`, `no_edges`, `git_error`, `stale_db`, `error`, `unsupported`, `auto_queued`. `no_data` and `stale_db` exit 0. `error` and `git_error` exit 1.

Do not pip-install this project. Do not store analysis in SQLite. Do not add a crates.io dependency.
