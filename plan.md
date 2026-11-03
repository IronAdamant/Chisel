# Chisel

The product is the Rust crate in `src/`. `chisel` and `chisel-mcp` are two binaries over that crate. Storage is the snapshot `.chisel/store`. There is no Python package in this tree.

`PLAN_NEXT.md` is not part of this repository. It was a Python-era backlog name. Do not look for it, and do not treat a missing file as unfinished work. Cross-shard test edges and `flakiness_trend` are implemented in the Rust engine. `AGENTS.md` is the call contract.

Checked-in result checks are the JSON files under `tests/fixtures/oracle`. They were frozen from the old Python tool. New behaviour is covered by the Rust tests, including a Cargo-layout fixture at `tests/fixtures/rust_crate`.

## What the binary does

Chisel links tests to code units, mixes in git churn and ownership, and answers `suggest-tests`, `diff-impact`, `test-gaps`, and `risk-map`. Scores are heuristics. Read `_meta.uniform_components` before trusting a composite risk score.

Status values: `no_data`, `no_changes`, `no_edges`, `git_error`, `stale_db`, `error`, `unsupported`, `auto_queued`. `no_data` and `stale_db` exit 0. `error` and `git_error` exit 1.

## Layout

| Path | Role |
|---|---|
| `src/main.rs` | `chisel` binary. Debug-only durability helpers stay here. |
| `src/bin/chisel-mcp.rs` | `chisel-mcp` binary. Same entry as `chisel serve-mcp`. |
| `src/cli.rs` | Argv parser and command dispatch. |
| `src/engine` | Analyze, update, queries, advisory locks. |
| `src/extract` | Built-in scanners. One test-file classifier lives in `src/engine/mod.rs` (`looks_like_test`). |
| `src/store.rs` | Snapshot read and write. Temp file, fsync, rename. |
| `src/git.rs` | `git` subprocess. |
| `src/impact.rs` | Risk math. |
| `src/json.rs` | JSON parser and serializer. |
| `src/http.rs` | `GET /health`, `GET /tools`, `POST /call`. No `/mcp` path. |
| `src/mcp.rs` | Stdio MCP: newline-delimited JSON, plus Content-Length. |
| `src/fswatch.rs` | `inotify`, `kqueue`, or `ReadDirectoryChangesW`, then the caller checks git. |
| `src/lock.rs` | `flock` on Unix, `LockFileEx` on Windows. |
| `src/schemas.rs` | The 26 tool names and input schemas. |

## Test discovery

`looks_like_test` is the only path classifier. The hint printed when analyze finds no tests uses that same pattern list.

Cargo integration tests are `.rs` files under the top-level `tests/` directory, including `tests/a.rs`. Rust functions marked `#[test]`, `#[tokio::test]`, `#[rstest]`, or `#[test_case]` are test units even when the surrounding file is source. A source file that also contains `#[test]` stays a code file for `risk-map` and `test-gaps`. The test functions themselves are not reported as untested code.

## Dependency rule

`Cargo.toml` keeps `[dependencies]` and `[dev-dependencies]` empty. There is no `build.rs`. CI fails the build if either table is non-empty. The toolchain is pinned in `rust-toolchain.toml` and in `.github/workflows/ci.yml`.

Allowed: the Rust standard library, first-party modules in `src/`, `extern "C"` for the two lock calls, system `git`, and an optional extractor program named by `CHISEL_EXTRACTOR` or by `.chisel/extensions.toml` when the process was started with `--allow-repo-extractor`.

Not allowed: crates.io crates, a vendored SQLite amalgamation or tree-sitter grammar, `build.rs`, or an in-process plugin loader.

## CI

`.github/workflows/ci.yml` runs `cargo test` on Linux, macOS, and Windows. Release builds still cover Linux musl, macOS arm64, macOS x64, and Windows. `chisel --version` prints the package version from `Cargo.toml`.

## Out of scope

- Reading `chisel.db`, `:memory:`, or `file:` storage paths.
- A general regex engine, a general SQL engine, or a full language compiler.
- Tree-sitter, LSP, TLS, or HTTP/2.
- Replacing `git`.
- Changing risk weights in the same change as a discovery fix.
