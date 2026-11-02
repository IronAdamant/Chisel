# Custom extractors

The built-in scanners live in `src/extract`. They are part of the binary. There is no plugin loader and no pip package.

`CHISEL_EXTRACTOR`, when set, is an executable. It reads `{"path","language"}` on stdin and writes a JSON list of units. A non-zero exit or bad JSON falls back to the built-in scanner and records a warning.

An agent can also write `.chisel/extensions.toml` in the project:

```
extractor = "tools/my-extractor"
```

A relative path is from the project root. `CHISEL_EXTRACTOR` overrides the file. Chisel only executes that program. Tree-sitter, a Postgres client, or any other library stays in the external program. This repository does not take those dependencies, and the snapshot stays `.chisel/store`.

Unset, with no file, uses the built-in scanner.

Storage is still `.chisel/store`. Do not open `chisel.db`. Do not use SQLite.

Status values: `no_data`, `no_changes`, `no_edges`, `git_error`, `stale_db`, `error`, `unsupported`, `auto_queued`.
