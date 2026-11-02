# Architecture

Chisel is a zero-dependency Rust program. `AGENTS.md` is the agent contract.

`src/engine` analyzes the worktree, answers queries, and records advisory locks. `src/extract` scans source files. `src/store.rs` reads and writes one snapshot, `.chisel/store`, with a temp file, fsync, and rename. It does not open `chisel.db` and it rejects `:memory:` and `file:` paths.

Git is a subprocess. HTTP is `GET /health`, `GET /tools`, and `POST /call` on `127.0.0.1:8377`. There is no `/mcp` path. MCP is stdio Content-Length JSON-RPC from `chisel serve-mcp` or the `chisel-mcp` name of the same binary.

Status values: `no_data`, `no_changes`, `no_edges`, `git_error`, `stale_db`, `error`, `unsupported`, `auto_queued`.

Do not pip-install this project. Do not store analysis in SQLite.
