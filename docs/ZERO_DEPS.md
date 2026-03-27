# Zero-dependency policy

Chisel’s **runtime** is **stdlib only** (`pyproject.toml` has `dependencies = []`). This is intentional: install anywhere Python 3.11+ runs, no supply-chain surface for core behavior, and predictable behavior in sandboxes.

## What is allowed

| Layer | Allowed |
|-------|--------|
| Core package `chisel/` | Python standard library only |
| CLI / HTTP MCP / stdio MCP | Same — `http.server`, `sqlite3`, `subprocess` for `git`, etc. |
| **Optional** extras in `pyproject.toml` | Declared separately; not imported at runtime unless the extra is installed |

## Optional extras (not part of “core zero-dep”)

- **`[mcp]`** — pulls in the `mcp` PyPI package **only** for the stdio MCP entrypoint (`chisel-mcp`). The HTTP server (`chisel serve`) does not need it.
- **`[dev]`** — pytest, ruff, etc., for contributors only.

Installing `chisel-test-impact` with **no extras** gives a fully functional CLI and HTTP MCP server.

## Tree-sitter, LSP, or “better parsers” (item 7)

**They are not in the default package.** Higher-accuracy parsing would normally add **native** or **heavy** dependencies (tree-sitter bindings, language servers). That would **violate** the strict zero-dep constraint for the default install.

The supported extension path — without changing the default wheel — is:

1. **`register_extractor()`** in `ast_utils.py` — your code (or a **separate** optional package you publish) can register a tree-sitter-backed extractor **at runtime** if the user has installed those deps in **their** environment.
2. **`CHISEL_BOOTSTRAP`** — set to a dotted import path; Chisel loads that module when `ChiselEngine` starts (see `chisel/bootstrap.py`). Use it to call `register_extractor()` without wrapping the CLI yourself.
3. **Fork / vendor** — advanced users can maintain a fork with optional accelerators; the upstream project stays stdlib-only.

Full guide: **[CUSTOM_EXTRACTORS.md](CUSTOM_EXTRACTORS.md)** · example: **`examples/chisel_bootstrap_example.py`**.

So: **strict zero-dep** applies to **what we ship as `chisel-test-impact` on PyPI** with default installs. Optional plug-ins and user-registered extractors live **outside** that guarantee unless we add a clearly named optional extra (which would document non-zero deps for that path only).

## CI

- `scripts/check_version.py` — version strings stay aligned.
- `scripts/benchmark_chisel.py` — smoke timing on a tiny repo (guards egregious regressions).
