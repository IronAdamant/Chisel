# Extractors

Chisel scans source with the code in `src/extract`. Optional replacement is the `CHISEL_EXTRACTOR` subprocess described in `CUSTOM_EXTRACTORS.md`.

The product does not pip-install parsers and does not store results in SQLite. The snapshot is `.chisel/store`. `chisel.db` is not opened.

Status values: `no_data`, `no_changes`, `no_edges`, `git_error`, `stale_db`, `error`, `unsupported`, `auto_queued`.
