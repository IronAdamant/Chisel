# Zero dependencies

`Cargo.toml` has empty `[dependencies]` and `[dev-dependencies]`. There is no `build.rs`. The binary does not pip-install anything and does not link SQLite.

Persistence is the snapshot `.chisel/store`. `chisel migrate` is the only reader of a legacy `chisel.db`, and it copies test results into the snapshot. `:memory:` and `file:` storage paths are rejected.

Status values: `no_data`, `no_changes`, `no_edges`, `git_error`, `stale_db`, `error`, `unsupported`, `auto_queued`.
