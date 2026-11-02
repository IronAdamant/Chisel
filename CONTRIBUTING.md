# Contributing

The product is the Rust crate in `src/`. `AGENTS.md` is the contract.

```bash
cargo build --release
cargo test -- --test-threads=8
```

The binary is `target/release/chisel`. `chisel-mcp` is the same program.

Storage is `.chisel/store`. Do not open `chisel.db`. Do not pass `:memory:` or `file:` as a storage path. Do not pip-install Chisel and do not add a SQLite database.

`Cargo.toml` `[dependencies]` and `[dev-dependencies]` stay empty. There is no `build.rs`.

Status values the tools return: `no_data`, `no_changes`, `no_edges`, `git_error`, `stale_db`, `error`, `unsupported`, `auto_queued`.

`no_data` and `stale_db` exit 0. `error` and `git_error` exit 1.
