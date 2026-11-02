# Chisel

The program you run is the Rust binary from `src/`. Read `AGENTS.md`.

Storage is the snapshot `.chisel/store`. Chisel does not open `chisel.db`. `:memory:` and `file:` paths are rejected. Do not pip-install Chisel and do not use SQLite as the store.

Status values: `no_data`, `no_changes`, `no_edges`, `git_error`, `stale_db`, `error`, `unsupported`, `auto_queued`.

`no_data` and `stale_db` exit 0. `error` and `git_error` exit 1.
