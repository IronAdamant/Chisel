# Chisel

The product is the Rust crate in `src/`. Read `AGENTS.md` for how to call it and `ARCHITECTURE.md` for the module map.

Build with `cargo build --release`. The `chisel` binary is `src/main.rs`. The `chisel-mcp` binary is `src/bin/chisel-mcp.rs`. Both call the same entry point. Storage is the snapshot `.chisel/store`. Chisel does not open `chisel.db`. `:memory:` and `file:` storage paths are rejected.

Do not pip-install Chisel. Do not use SQLite as the store. `Cargo.toml` dependency tables stay empty. There is no `build.rs`.

`looks_like_test` in `src/engine/mod.rs` is the only test-file classifier. Rust files under `tests/` are tests. `#[test]` functions are test units.

Status values: `no_data`, `no_changes`, `no_edges`, `git_error`, `stale_db`, `error`, `unsupported`, `auto_queued`. `no_data` and `stale_db` exit 0. `error` and `git_error` exit 1. Usage errors exit 2.

Rust call resolution is `src/engine/resolve.rs` (with `src/extract/rust_items.rs`); worktree overlays are `src/engine/overlay.rs`.
