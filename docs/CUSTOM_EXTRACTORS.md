# Custom extractors

The built-in scanners live in `src/extract`. They are part of the binary. There is no plugin loader and no pip package.

`CHISEL_EXTRACTOR`, when set, is an executable. It reads `{"path","language"}` on stdin and writes a JSON list of units. A non-zero exit or bad JSON falls back to the built-in scanner and records a warning.

An agent can also write `.chisel/extensions.toml` in the project:

```
extractor = "tools/my-extractor"
```

A relative path is from the project root and must stay inside the project after symlinks are resolved. Chisel does not run that file unless the process was started with `--allow-repo-extractor` or `CHISEL_ALLOW_REPO_EXTRACTOR=1`. A tool argument cannot turn it on. `CHISEL_EXTRACTOR` overrides the file and does not need the flag. Chisel only executes that program. Tree-sitter, a Postgres client, or any other library stays in the external program. This repository does not take those dependencies, and the snapshot stays `.chisel/store`.

Unset, with no file, uses the built-in scanner.

`CHISEL_DEP_EXTRACTOR`, or `dep_extractor` in the same toml file, is a second executable. It reads `{"path","language","content"}` on stdin and writes a JSON list of dependencies: `name`, `dep_type` (`import` or `call`), optional `module_path`, optional `confidence`. A bad run falls back to the built-in dependency scan. The same `--allow-repo-extractor` gate applies to the toml path. `CHISEL_DEP_EXTRACTOR` overrides the file. This is the replacement for Python `register_dep_extractor` and for `CHISEL_BOOTSTRAP`. Chisel does not import a Python module. If `CHISEL_BOOTSTRAP` is a path to an executable, that program is the dependency extractor.

Storage is still `.chisel/store`. `chisel migrate` is the one-shot copy from an old `chisel.db`. Do not use SQLite as the live store.

Status values: `no_data`, `no_changes`, `no_edges`, `git_error`, `stale_db`, `error`, `unsupported`, `auto_queued`.
