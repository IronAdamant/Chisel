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

## Dynamic `require()` and confidence scoring

Chisel's JS/TS extractor detects `require()` patterns that cannot be statically resolved:

| `dep_type` | `require_type` | Confidence | Example |
|-------------|----------------|------------|---------|
| `import` | (static) | 1.0 | `require('./foo')` |
| `tainted_import` | `variable` | 1.0 | `const M = './foo'; require(M)` — path was statically traced |
| `dynamic_import` | `variable` | 0.3 | `require(variable)` — unknown variable |
| `dynamic_import` | `template` | 0.4 | `` require(`./${x}`) `` |
| `dynamic_import` | `concat` | 0.2 | `require('./' + name)` |
| `dynamic_import` | `conditional` | 0.3 | `require(cond ? './a' : './b')` |
| `eval_import` | `eval` | 0.0 | `new Function('require', code)` |

`confidence` indicates how reliably the actual module path is known. Edge weights blend `proximity * sqrt(confidence)` so low-confidence requires contribute less to impact scores. `tainted_import` is a special case where variable assignment was tracked (`const MODULE = './plugins/auth'`) so the path is known — it gets full confidence.

## `risk_map` dynamic-risk fields

Each per-file entry in `risk_map` output includes:

| Field | Type | Description |
|-------|------|-------------|
| `hidden_risk_factor` | float | Additive risk component (0–0.15) from `dynamic_import`/`eval_import` edge density: `min(dynamic_edge_count/20, 1.0) * 0.15` |
| `new_file_boost` | float | Additive 0.5 for files with zero churn and zero test coverage, so new/untracked files surface in rankings |
| `shadow_edge_count` | int | Edges that are not `call`-type (import + dynamic types) |
| `dynamic_edge_count` | int | Edges with `edge_type` of `dynamic_import` or `eval_import` |
| `breakdown.hidden_risk` | float | Same as `hidden_risk_factor` (for breakdown consistency) |
| `breakdown.new_file_boost` | float | Same as `new_file_boost` (for breakdown consistency) |

`hidden_risk_factor` and `new_file_boost` are added to the risk score after the 6-component formula. Files with 20+ dynamic edges reach the maximum `0.15` hidden-risk uplift. Use `dynamic_edge_count` to assess how many deps are actually unknown. Use `working_tree=true` to include untracked files in `risk_map` scoring.

## MCP vs CLI

- **MCP** wraps results with **`next_steps`** suggestions when using HTTP/stdio servers—use them for follow-up tool calls.
- Long **`analyze`** runs should use **`start_job`** + **`job_status`** or a terminal to avoid client timeouts.

## Cursor / host integration (out of band)

Tighter integration (custom transports, UI, or bundled system tools) is a **host** concern. This repo stays **MCP + CLI**-friendly and **stdlib-first** as above.
