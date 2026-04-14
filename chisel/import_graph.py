"""Static import edges between source files (for structural coupling)."""

from __future__ import annotations

import os

from chisel.test_mapper import (
    TestMapper,
    _matches_import_path,
    _matches_js_import_path,
    _read_file,
    _resolve_js_module_path,
)


def build_import_edges(
    mapper: TestMapper,
    project_dir: str,
    source_rel_paths: list[str],
    test_rel_paths: set[str],
    scan_rel_paths: set[str] | None = None,
) -> list[dict]:
    """Build file-level import edges for non-test source files.

    Each edge is ``{"importer_file": str, "imported_file": str}`` with paths
    relative to *project_dir*. Only ``dep_type == "import"`` dependencies are
    used (not dynamic calls).

    *source_rel_paths* should list all analyzed code file paths; resolution
    only links to paths present in that set.

    *scan_rel_paths* is an optional subset of *source_rel_paths* to actually
    re-scan. When omitted, all source paths are scanned. This supports
    incremental rebuilds: old edges for changed files are removed first, then
    only changed files are re-scanned while resolution still sees all paths.
    """
    all_paths = set(source_rel_paths)
    scan_set = set(scan_rel_paths) if scan_rel_paths is not None else all_paths
    candidates = [p for p in scan_set if p not in test_rel_paths]
    edges: list[dict] = []
    seen: set[tuple[str, str]] = set()

    for importer in candidates:
        abs_path = os.path.join(project_dir, importer)
        content = _read_file(abs_path)
        if content is None:
            continue
        deps = mapper.extract_test_dependencies(importer, content)
        for dep in deps:
            if dep.get("dep_type") != "import":
                continue
            module_path = dep.get("module_path")
            for tgt in _resolve_import_targets(
                importer, dep, module_path, all_paths,
            ):
                if tgt == importer or tgt not in all_paths:
                    continue
                key = (importer, tgt)
                if key not in seen:
                    seen.add(key)
                    edges.append({
                        "importer_file": importer,
                        "imported_file": tgt,
                    })
    return edges


def _resolve_import_targets(importer, dep, module_path, all_paths: set[str]):
    """Yield project-relative paths in *all_paths* that *dep* resolves to."""
    # Heuristic: use path logic for Python and JS/TS
    if importer.endswith(".py"):
        if module_path:
            for p in all_paths:
                if p.endswith(".py") and _matches_import_path(p, module_path):
                    yield p
        return

    if importer.endswith((".js", ".jsx", ".mjs", ".cjs", ".ts", ".tsx")):
        if module_path and module_path.startswith("."):
            resolved = _resolve_js_module_path(importer, module_path)
            if resolved:
                for p in all_paths:
                    if _matches_js_import_path(p, resolved):
                        yield p
            return
        # Non-relative bare module name (e.g. require('SimilarityService'))
        # falls through to name-based matching below.
        # Path-style non-relative imports (e.g. 'src/utils', 'lib/foo') also
        # fall through — the stem-based fallback at the bottom handles them.

    if importer.endswith(".go") and module_path:
        # import "example.com/foo/bar" → match bar.go by final path segment
        base = module_path.rstrip("/").rsplit("/", 1)[-1]
        for p in all_paths:
            if p.endswith(".go") and os.path.basename(p).split(".")[0] == base:
                yield p
        return

    # Fallback: unique name match (last resort)
    name = dep.get("name")
    if not name:
        return
    matches = [p for p in all_paths if os.path.basename(p).split(".")[0] == name]
    if len(matches) == 1:
        yield matches[0]
