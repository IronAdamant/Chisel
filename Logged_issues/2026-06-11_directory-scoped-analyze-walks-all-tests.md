# Directory-scoped analyze does not scope test discovery

**Severity:** moderate
**Date:** 2026-06-11
**Found by:** Claude Code v0.12.0 modernization pass (dogfooding on this repo)

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
