# Chisel

The product is the Rust crate in `src/`. Read `AGENTS.md` for how to call it.

Build with `cargo build --release`. The binary is `chisel`. `chisel-mcp` is the same program. Storage is the snapshot `.chisel/store`. Chisel does not open `chisel.db`. `:memory:` and `file:` storage paths are rejected.

Do not pip-install Chisel. Do not use SQLite as the store.

Status values: `no_data`, `no_changes`, `no_edges`, `git_error`, `stale_db`, `error`, `unsupported`, `auto_queued`. `no_data` and `stale_db` exit 0. `error` and `git_error` exit 1.
