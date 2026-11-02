# Custom extractors

The built-in scanners live in `src/extract`. They are part of the binary. There is no plugin loader and no pip package.

`CHISEL_EXTRACTOR`, when set, is an executable. It reads `{"path","language"}` on stdin and writes a JSON list of units. A non-zero exit or bad JSON falls back to the built-in scanner and records a warning. Unset uses the built-in scanner.

Storage is still `.chisel/store`. Do not open `chisel.db`. Do not use SQLite.

Status values: `no_data`, `no_changes`, `no_edges`, `git_error`, `stale_db`, `error`, `unsupported`, `auto_queued`.
