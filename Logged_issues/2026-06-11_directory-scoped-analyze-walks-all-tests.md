# Directory-scoped analyze does not scope test discovery

**Severity:** moderate
**Date:** 2026-06-11
**Found by:** Claude Code v0.12.0 modernization pass (dogfooding on this repo)
**Status:** RESOLVED in v0.13.0 (same day)

## Resolution

Root cause was gitignore-blind scanning plus an O(units × deps × all_units)
edge-matching loop, not the directory scoping itself (tests legitimately
live outside the scoped directory, so scoping discovery would break the
main use case). Three fixes:

1. **gitignore-aware scanning** (`git_visible_paths` in `project.py`): both
   the engine code scan and `TestMapper.discover_test_files` filter through
   `git ls-files --cached --others --exclude-standard` and never traverse
   ignored trees. `CHISEL_INCLUDE_IGNORED=1` opts out; non-git projects are
   unfiltered. Test files discovered here: 3,895 → 26.
2. **`build_test_edges` memoization**: matching depends only on
   (module_path, code file), so match results are cached per module path
   and deps per file. 17.6s → 0.23s on identical input, byte-identical
   output (A/B verified against the pre-change algorithm).
3. **`update()` gating**: edge rebuild skipped entirely when no files
   changed and no new commits (`edge_rebuild_skipped: true`); per-function
   `git log -L` churn restricted to changed files.

Net effect on this repo: full analyze 19.4s → 2.3s; no-op update ~18s →
0.14s; single-file update ~19s → 1.4s. Regression tests:
`tests/test_gitignore_filter.py`, `TestUpdateEdgeRebuildGating`.

## Symptom

`chisel analyze chisel/` scanned 20 code files but discovered **3,895 test
files / 33,086 test units**, because `TestMapper` walks the entire
`project_dir` regardless of the `directory` argument passed to `analyze`.
On this repo that includes `lang_any_sample_project_for_testing_10k_monorep/`
(gitignored, ~10k files), making scoped analyzes and every subsequent
`update` minutes-long: an incremental `chisel update` ran 18+ minutes at
~100% CPU rebuilding edges for ~31k test units.

## Expected

When `analyze` is scoped to a directory, test discovery (and edge building)
should either be scoped the same way or made explicitly configurable
(e.g. respect the scope and warn that cross-directory edges are skipped).

## Workarounds

- Add bulk fixture directories to `_SKIP_DIRS` candidates, or
- Consider honoring `.gitignore` during file scans (the sample monorepo is
  gitignored but still scanned), or
- Document that fixture trees must live outside the analyzed project root.

## Pointers

- `chisel/engine.py` — `analyze(directory=...)` scopes `_scan_code_files`
  but constructs `TestMapper(self.project_dir)` and discovers tests
  project-wide.
- `chisel/test_mapper.py` — `discover_test_files()` walk root.
