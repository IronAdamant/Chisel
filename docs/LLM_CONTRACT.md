# LLM / MCP contract for Chisel

This document defines how **agents** should interpret Chisel responses: security posture, status codes, trust ordering, and heuristic limits. It complements **`AGENT_PLAYBOOK.md`** (workflows) and **`ZERO_DEPS.md`** (dependency policy).

## Security model (why native / stdlib)

- **Default Chisel** is **stdlib-only** for core analysis—no bundled third-party Python dependencies in the core install. That reduces **supply-chain** risk for the analysis runtime.
- **Git** is used via **`subprocess`** (the `git` binary), not a vendored library.
- **Parsing** uses **`ast`** (Python) and **regex** extractors (other languages), not tree-sitter or LSP in the default package. Accuracy trades off against install weight and audit surface.
- **Optional** tree-sitter/LSP-style extractors can be registered **in the user's environment** (`register_extractor`, `CHISEL_BOOTSTRAP`) without changing the core wheel—see **`CUSTOM_EXTRACTORS.md`**.

Agents should **not** assume semantic parity with IDE-grade analysis; treat results as **strong heuristics** plus **git facts** where applicable.

## Response shape: what to read first

When a tool returns a **dict**, check keys in this order before trusting long lists or scores:

1. **`status`** — `no_data` | `no_changes` | `no_edges` | `git_error` (and tool-specific)
2. **`error`** — e.g. `not_a_git_repo` on `diff_impact` / git failures
3. **`message`**, **`hint`**
4. **`_meta`** — on `risk_map` / triage (uniform components, thresholds)
5. List payloads (`files`, `test_gaps`, etc.)

Empty **`[]`** without a **`status`** usually means “nothing matched,” not “error.” **`git_error`** is never an empty list.

## `suggest_tests` item `source`

| Value | Meaning |
|-------|--------|
| `direct` | Test–code edge in DB |
| `co_change` | Via git co-change partner |
| `import_graph` | Via static import graph reachability |
| `static_require` | Test file import/require resolved to this source path (no DB edge) |
| `hybrid` | Both DB impact and static scan agree on the same test |
| `fallback` | Stem/name similarity only |
| `working_tree` | Untracked file stem match |

**Trust:** `direct` > `hybrid` > `import_graph` / `co_change` > `static_require` > `fallback` / `working_tree`. Always run tests in the real environment before release.

## MCP vs CLI

- **MCP** wraps results with **`next_steps`** suggestions when using HTTP/stdio servers—use them for follow-up tool calls.
- Long **`analyze`** runs should use **`start_job`** + **`job_status`** or a terminal to avoid client timeouts.

## Cursor / host integration (out of band)

Tighter integration (custom transports, UI, or bundled system tools) is a **host** concern. This repo stays **MCP + CLI**-friendly and **stdlib-first** as above.
