# Agent playbook

Follow `AGENTS.md`. The product is the Rust binary `chisel`.

Storage is `.chisel/store`. Do not open `chisel.db`. Do not pass `:memory:` or `file:`. Do not pip-install Chisel. Do not use SQLite.

Call `analyze` on `no_data`. Call `update` or pass `auto_update` on `stale_db`. Pass `--json` and branch on `status`.

Status values: `no_data`, `no_changes`, `no_edges`, `git_error`, `stale_db`, `error`, `unsupported`, `auto_queued`.
