# Chisel — agent playbook

Short guide for **LLM agents** using Chisel (MCP or CLI). Solo maintainer, multiple sessions — one `.chisel/` database per project; use **`--project-dir`** when the cwd is not the repo root.

**Security / trust:** Chisel defaults to **stdlib-only** analysis (minimal supply-chain surface). See **`LLM_CONTRACT.md`** for status codes, how to read `_meta`, and **`source`** trust ordering on `suggest_tests`.

## 1. First-time or after big structural changes

1. Run **`analyze`** (full scan) — or **`start_job`** with `kind: "analyze"` and poll **`job_status`** if the MCP client might time out on large repos.
2. Confirm with **`stats`** (non-zero `code_units`, `test_edges`).

## 2. Day-to-day loop

1. Edit code.
2. **`diff_impact`** — which tests to run (fix **`git_error`** if cwd/project wrong).
3. Run tests in the terminal or CI.
4. **`record_result`** for failures/passes to feed ranking and risk.
5. **`update`** after small edits — or **`start_job`** / `update` for heavy jobs.

## 3. When tools return empty or sparse

| Symptom | Action |
|--------|--------|
| `no_data` | Run **`analyze`** on the repo root. |
| `stale_db` | The file isn't in the Chisel DB yet — run **`analyze`** or use **`working_tree=true`** for uncommitted files. |
| `suggest_tests` empty | Try **`fallback_to_all`** or **`working_tree`** for untracked files; ensure **analyze** has run. |
| `risk_map` misses new files | Use **`working_tree=true`** to include untracked files in risk scoring. |
| `coupling` co-change empty | Normal in solo history — use **`import_coupling`** / **`import_partners`** and **`risk_map`**. |
| MCP timeout on analyze | Use **`start_job`** + **`job_status`**, or run **`chisel analyze`** in a terminal. |

## 4. `source` on impact / suggest_tests

Each suggestion may include **`source`**: `direct` | `co_change` | `import_graph` | `static_require` | `hybrid` | `fallback` | `working_tree`. Prefer **`direct`** or **`hybrid`**; **`import_graph`** means coverage via another module (e.g. facade); **`static_require`** is from import/require resolution without a DB edge; **`fallback`** / **`working_tree`** are stem heuristics—verify in the repo.

Full table: **`LLM_CONTRACT.md`**.

## 5. Risk and triage

- **`triage`** — one call for top-N risk + gaps + stale tests.
- **`risk_map`** — read **`_meta.uniform_components`** before trusting the composite score.
- **`exclude_new_file_boost=true`** — use in `risk_map` or `triage` when you want stable long-term rankings without the temporary 0.5 uplift for brand-new files.

## 6. Copy-paste CI (GitHub)

See **`examples/github-actions/chisel.yml`** in this repo — run analyze (and optionally diff) on pushes/PRs.

## 7. Optional: your own parser (tree-sitter, etc.)

Chisel ships **stdlib-only** extractors. To use a heavier parser, install it **in your** environment and register via **`register_extractor`** — see **`docs/CUSTOM_EXTRACTORS.md`** and **`CHISEL_BOOTSTRAP`**.
