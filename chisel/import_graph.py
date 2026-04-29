"""Static import edges between source files (for structural coupling)."""

from __future__ import annotations

import os
import re

from chisel.test_mapper import (
    TestMapper,
    _JS_EXTENSIONS,
    _matches_import_path,
    _matches_js_import_path,
    _read_file,
    _resolve_js_module_path,
    _strip_js_ext,
)

# Edge types that produce import_edges. ``import`` is hard-static (full
# confidence). ``tainted_import`` is variable-taint resolved (also full
# confidence — the variable was assigned a literal path the parser can
# resolve). ``dynamic_import`` is a heuristic resolution (template literal,
# string concat, path.join with a directory) and is stored at reduced
# confidence so callers can distinguish guaranteed imports from soft hints.
_HARD_DEP_TYPES = frozenset({"import", "tainted_import"})
_SOFT_DEP_TYPES = frozenset({"dynamic_import"})

# Strip ``${...}`` template placeholders before treating a path as a glob.
_TEMPLATE_PLACEHOLDER_RE = re.compile(r"\$\{[^}]*\}")

# Cap fan-out per dynamic-require resolution. Above this threshold the
# directory is large enough that the heuristic ("this require loads ONE of
# the files in this dir") provides almost no information per edge. Better
# to suppress entirely than flood the import graph with low-signal edges.
# The dynamic-require call is still surfaced via the source-level
# ``unknown_require_count`` and ``hidden_risk_factor`` in risk scoring.
_DYNAMIC_REQUIRE_FANOUT_CAP = 50


def build_import_edges(
    mapper: TestMapper,
    project_dir: str,
    source_rel_paths: list[str],
    test_rel_paths: set[str],
    scan_rel_paths: set[str] | None = None,
) -> list[dict]:
    """Build file-level import edges for non-test source files.

    Each edge is ``{"importer_file": str, "imported_file": str,
    "confidence": float}`` with paths relative to *project_dir*. Three
    kinds of edges are produced:

    * Hard static imports (``import``) — confidence 1.0.
    * Tainted imports (``tainted_import`` from variable-taint analysis,
      e.g. ``const PATH = './foo'; require(PATH)``) — confidence 1.0
      because the parser fully resolved the variable to a literal path.
    * Soft dynamic imports (``dynamic_import`` template-literal, string
      concat, or ``path.join(__dirname, '<dir>', var)``) — emitted as
      one edge per candidate file inside the implicated directory, at
      the dep's reported confidence (typically 0.2–0.4).

    Soft edges let downstream tools surface tests for plugin files that
    are loaded only at runtime via dynamic require(). They are stored
    with confidence < 1.0 so cycle detection and exact-import queries
    can ignore them.

    *source_rel_paths* should list all analyzed code file paths; resolution
    only links to paths present in that set.

    *scan_rel_paths* is an optional subset of *source_rel_paths* to actually
    re-scan. When omitted, all source paths are scanned.
    """
    all_paths = set(source_rel_paths)
    scan_set = set(scan_rel_paths) if scan_rel_paths is not None else all_paths
    candidates = [p for p in scan_set if p not in test_rel_paths]
    # Pre-bucket code files by their containing directory for fast
    # directory-pattern resolution of soft (dynamic-require) edges.
    dir_index: dict[str, list[str]] = {}
    for p in all_paths:
        d = os.path.dirname(p.replace("\\", "/"))
        dir_index.setdefault(d, []).append(p)

    edges: list[dict] = []
    seen: dict[tuple[str, str], float] = {}

    def _emit(importer: str, tgt: str, confidence: float) -> None:
        if tgt == importer or tgt not in all_paths:
            return
        key = (importer, tgt)
        prev = seen.get(key)
        if prev is None or confidence > prev:
            seen[key] = confidence

    for importer in candidates:
        abs_path = os.path.join(project_dir, importer)
        content = _read_file(abs_path)
        if content is None:
            continue
        deps = mapper.extract_test_dependencies(importer, content)
        for dep in deps:
            dep_type = dep.get("dep_type")
            module_path = dep.get("module_path")
            if dep_type in _HARD_DEP_TYPES:
                for tgt in _resolve_import_targets(
                    importer, dep, module_path, all_paths,
                ):
                    _emit(importer, tgt, 1.0)
            elif dep_type in _SOFT_DEP_TYPES and module_path:
                soft_conf = float(dep.get("confidence", 0.3))
                for tgt in _resolve_dynamic_targets(
                    importer, module_path, dir_index,
                ):
                    _emit(importer, tgt, soft_conf)

    for (importer, tgt), conf in seen.items():
        edges.append({
            "importer_file": importer,
            "imported_file": tgt,
            "confidence": conf,
        })
    return edges


def _resolve_dynamic_targets(
    importer: str, module_path: str, dir_index: dict[str, list[str]],
):
    """Yield candidate code files for a dynamic-require ``module_path``.

    The path comes from a heuristic require — template literal, string
    concatenation, or ``path.join(__dirname, '<dir>', var)`` — so the exact
    target is unknown. The resolution treats *module_path* as a directory
    pattern and yields every code file in that directory.

    Examples (importer = ``src/services/dispatcher.js``)::

        './plugins/'                → src/services/plugins/*.js
        './plugins/${name}'         → src/services/plugins/*.js
        'plugins'                   → src/services/plugins/*.js  (path.join form)
        './plugins/foo'             → src/services/plugins/foo.js (single file)

    Only relative directory patterns are resolved; bare module names are
    skipped to avoid creating noisy edges to npm package shadows.
    """
    cleaned = _TEMPLATE_PLACEHOLDER_RE.sub("", module_path or "").strip()
    if not cleaned:
        return
    # path.join(__dirname, 'plugins', name) emits module_path = 'plugins'
    # (the directory token, no leading './'). Treat that the same as './plugins'.
    if cleaned.startswith(".") or "/" in cleaned or _is_directoryish(cleaned):
        # Resolve against the importer's directory.
        importer_norm = importer.replace("\\", "/")
        importer_dir = os.path.dirname(importer_norm)
        joined = os.path.normpath(os.path.join(importer_dir, cleaned))
        joined = joined.replace("\\", "/")
        # Trailing-slash or no-extension → directory glob. If the cleaned
        # path resolves to an existing file (single dynamic ref to one
        # module), match just that file.
        candidates = dir_index.get(joined, [])
        if len(candidates) <= _DYNAMIC_REQUIRE_FANOUT_CAP:
            for cand in candidates:
                yield cand
        # Above the cap, the heuristic is too diffuse — suppress emission.
        # The dynamic-require pattern still surfaces through eval/unknown
        # require counts at the source-file level, so risk is not lost.

        # Also try the path itself as a file stem (e.g. './plugins/foo'
        # without extension when foo.js exists alongside). This is the
        # explicit-single-file case and is not subject to the fan-out cap.
        if "." not in os.path.basename(joined):
            stripped = _strip_js_ext(joined)
            for ext in _JS_EXTENSIONS:
                p = stripped + ext
                if p in dir_index.get(os.path.dirname(p), []):
                    yield p


def _is_directoryish(token: str) -> bool:
    """Heuristic: a single path token with no extension and no special chars
    likely names a sibling directory (matches the path.join form).
    """
    if not token:
        return False
    if token.startswith(("./", "../", "/")):
        return False
    if "/" in token:
        return False
    return "." not in token


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
