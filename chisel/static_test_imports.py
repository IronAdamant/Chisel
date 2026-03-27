"""Scan test files for require/import paths when DB test edges are missing."""

from __future__ import annotations

import os
from pathlib import Path

from chisel.ast_utils import detect_language
from chisel.import_graph import _resolve_import_targets
from chisel.test_mapper import TestMapper, _compute_proximity_weight


class StaticImportIndex:
    """One-time scan of test files (DB + optional disk-only) to map code paths → tests."""

    __slots__ = ("_project_dir", "_edges")

    def __init__(
        self,
        project_dir: str,
        storage,
        disk_test_files: dict[str, list[str]] | None = None,
        extra_code_paths: set[str] | None = None,
    ):
        self._project_dir = str(project_dir)
        self._edges: list[dict] = []
        self._build(storage, disk_test_files or {}, extra_code_paths or set())

    def _build(
        self,
        storage,
        disk_test_files: dict[str, list[str]],
        extra_code_paths: set[str],
    ) -> None:
        all_paths = set(storage.get_resolvable_code_file_paths()) | set(extra_code_paths)
        combined: dict[str, list[str]] = dict(storage.get_all_test_files())
        for rel, names in disk_test_files.items():
            if rel not in combined:
                combined[rel] = names
            else:
                merged = list(dict.fromkeys(combined[rel] + names))
                combined[rel] = merged

        for test_fp, unit_names in combined.items():
            abs_path = os.path.join(self._project_dir, test_fp)
            try:
                content = Path(abs_path).read_text(encoding="utf-8", errors="replace")
            except OSError:
                continue
            lang = detect_language(test_fp)
            py_imp = test_fp.endswith(".py")
            deps = TestMapper.extract_test_dependencies(test_fp, content)
            for dep in deps:
                if dep.get("dep_type") != "import":
                    continue
                mp = dep.get("module_path")
                if py_imp:
                    gap_eligible = False
                elif lang in ("javascript", "typescript") and mp and mp.startswith("."):
                    gap_eligible = True
                elif test_fp.endswith(".go") and mp:
                    gap_eligible = True
                else:
                    gap_eligible = False
                for tgt in _resolve_import_targets(
                    test_fp, dep, mp, all_paths,
                ):
                    tgt_n = tgt.replace("\\", "/")
                    for name in unit_names:
                        self._edges.append({
                            "tgt": tgt_n,
                            "test_fp": test_fp,
                            "name": name,
                            "gap_eligible": gap_eligible,
                            "py_imp": py_imp,
                        })

    def find_tests(
        self,
        code_file_path: str,
        *,
        include_python: bool = True,
        gap_eligible_only: bool = False,
    ) -> list[dict]:
        """Tests whose static imports resolve to *code_file_path* (project-relative)."""
        norm = code_file_path.replace("\\", "/")
        by_id: dict[str, dict] = {}
        for e in self._edges:
            if e["tgt"] != norm:
                continue
            if gap_eligible_only and not e["gap_eligible"]:
                continue
            if not include_python and e["py_imp"]:
                continue
            tid = f"{e['test_fp']}:{e['name']}"
            score = _compute_proximity_weight(e["test_fp"], norm)
            reason = f"static import → {e['tgt']}"
            prev = by_id.get(tid)
            if prev is None or score > prev["score"]:
                by_id[tid] = {
                    "test_id": tid,
                    "file_path": e["test_fp"],
                    "name": e["name"],
                    "score": score,
                    "reason": reason,
                    "source": "static_require",
                }
        out = list(by_id.values())
        out.sort(key=lambda x: x["score"], reverse=True)
        return out
