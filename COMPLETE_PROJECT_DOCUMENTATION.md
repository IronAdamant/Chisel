# Chisel

Updated: 2026-11-03.

Purpose: inventory of this repository. The program you run is the Rust binary from `src/`. Read `AGENTS.md` for calls and `ARCHITECTURE.md` for how the pieces fit.

Storage is the snapshot `.chisel/store`. Chisel does not open `chisel.db`. `:memory:` and `file:` paths are rejected. Do not pip-install Chisel and do not use SQLite as the store.

Status values: `no_data`, `no_changes`, `no_edges`, `git_error`, `stale_db`, `error`, `unsupported`, `auto_queued`.

`no_data` and `stale_db` exit 0. `error` and `git_error` exit 1.

`library.md`, `file_health.md`, and `file_health.json` were removed. They were generated maps of the deleted Python package. Nothing in this tree regenerates them. `monitored_paths.txt` was removed because it held one machine's absolute path and the binary does not read it.

## Root

| Path | Purpose |
|---|---|
| `README.md` | How to build and call the binary. |
| `AGENTS.md` | Agent contract: status values, flags, scale limits. |
| `CLAUDE.md` | Short orientation for agents that load this file first. |
| `ARCHITECTURE.md` | Module map and the analyze-to-query flow. |
| `plan.md` | Current layout and the dependency rule. Not a Python port schedule. |
| `CONTRIBUTING.md` | Build and test commands. |
| `CHANGELOG.md` | Release notes. Older entries mention the Python package. |
| `LLM_Development.md` | Historical session log. |
| `Cargo.toml` | Package `chisel`. Empty dependency tables. Two binaries. |
| `Cargo.lock` | Lockfile for the empty dependency graph. |
| `rust-toolchain.toml` | Pinned Rust channel. |
| `LICENSE` | MIT. |
| `.gitignore` | Build output, `.chisel/`, and the old `plan/` directory. |
| `exclude_patterns.txt` | Paths a watcher should skip. Not read by the Rust binary. |
| `GROK_CONNECTORS.md` | How to point an MCP client at `chisel-mcp`. |
| `skills/SKILL.md` | Skill text for calling the binary. |
| `.grok/skills/chisel/SKILL.md` | Same skill, installed for this workspace. |

## `src/`

| Path | Purpose |
|---|---|
| `src/lib.rs` | Crate root. |
| `src/main.rs` | `chisel` binary. |
| `src/bin/chisel-mcp.rs` | `chisel-mcp` binary. |
| `src/cli.rs` | Argv and dispatch, including `--version`. |
| `src/engine/mod.rs` | Engine type and `looks_like_test`. |
| `src/engine/analyze.rs` | `analyze` and `update`. |
| `src/engine/query.rs` | Read tools. |
| `src/engine/locks.rs` | Advisory locks. |
| `src/extract/mod.rs` | Scanner entry. |
| `src/extract/python.rs` | Python units. |
| `src/extract/brace.rs` | Brace languages, including Rust `#[test]`. |
| `src/extract/deps.rs` | Imports and calls. |
| `src/syntax.rs` | Python and JavaScript syntax tree. |
| `src/store.rs` | Snapshot file. |
| `src/git.rs` | Git subprocess. |
| `src/impact.rs` | Risk math. |
| `src/json.rs` | JSON. |
| `src/hash.rs` | SHA-256. |
| `src/http.rs` | HTTP server. |
| `src/mcp.rs` | Stdio MCP. |
| `src/fswatch.rs` | OS directory watch. |
| `src/lock.rs` | Process locks. |
| `src/schemas.rs` | Tool schemas. |
| `src/schemas_data.json` | Schema payload compiled into `schemas.rs`. |
| `src/next_steps.rs` | Follow-up hints. |

## `tests/`

| Path | Purpose |
|---|---|
| `tests/scale.rs` | Behaviour checks against temp git repos and the binary. |
| `tests/oracle_parity.rs` | Binary output against frozen oracle JSON. |
| `tests/surface_contract.rs` | Tool list shape. |
| `tests/extract_parity.rs` | Language fixtures, including Rust `#[test]` unit types. |
| `tests/durability.rs` | Snapshot publish under a killed writer. |
| `tests/rust_layout.rs` | Cargo layout fixture and a dogfood run on this repo. |
| `tests/fixtures/oracle/` | Frozen JSON for the Python-era oracle. |
| `tests/fixtures/languages/` | Small samples per language. |
| `tests/fixtures/rust_crate/` | `src/lib.rs`, `src/a.rs`, `tests/a.rs`, and a `Cargo.toml`. |

## Other trees

| Path | Purpose |
|---|---|
| `.github/workflows/ci.yml` | Test on Linux, macOS, and Windows. Release builds for the publish targets. |
| `.github/workflows/publish.yml` | Tagged release builds. |
| `docs/AGENT_PLAYBOOK.md` | Short pointer at `AGENTS.md`. |
| `docs/CUSTOM_EXTRACTORS.md` | External extractor program. |
| `docs/EXTRACTOR_ECOSYSTEM.md` | Where an external extractor fits. |
| `docs/LLM_CONTRACT.md` | Pointer at the status contract. |
| `docs/ZERO_DEPS.md` | Empty dependency tables. |
| `docs/chisel-demo.png` | Screenshot used by the README. |
| `examples/github-actions/chisel.yml` | Example workflow that builds this binary with Cargo. |
| `findings/` | Closed review notes. |
| `journal/2026/` | Dated notes from June 2026. |
| `Logged_issues/` | One closed issue note. |
| `wiki-local/` | Local notes. They still mention `schemas.py` in places. The Rust schemas are `src/schemas.rs`. |

## Removed in this pass

| Path | Why |
|---|---|
| `library.md` | Wikifier map of `chisel/*.py`. Those modules are gone, and this repo cannot regenerate the file. |
| `file_health.md` | Same age, same deleted tree. |
| `file_health.json` | Machine-readable copy of that matrix. |
| `pending_updates.md` | Empty. |
| `monitored_paths.txt` | Held `/home/aron/Documents/coding_projects/Chisel`. The binary does not read it. |
