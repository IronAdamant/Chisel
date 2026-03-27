"""Tests for chisel.import_graph — static import edges."""

import pytest

from chisel.import_graph import _resolve_import_targets, build_import_edges
from chisel.test_mapper import TestMapper


@pytest.fixture
def tmp_project(tmp_path):
    """Two Python modules: consumer imports provider."""
    root = tmp_path / "proj"
    root.mkdir()
    (root / "provider.py").write_text("def value():\n    return 1\n")
    (root / "consumer.py").write_text("from provider import value\n\n\ndef use():\n    return value()\n")
    return root


class TestBuildImportEdges:
    def test_python_import_creates_edge(self, tmp_project):
        mapper = TestMapper(str(tmp_project))
        rels = ["provider.py", "consumer.py"]
        edges = build_import_edges(mapper, str(tmp_project), rels, set())
        pairs = {(e["importer_file"], e["imported_file"]) for e in edges}
        assert ("consumer.py", "provider.py") in pairs

    def test_js_non_relative_bare_import(self, tmp_path):
        """Path-style bare imports (require('src/utils')) are not skipped.

        Previously, any import path containing '/' was skipped as an npm package.
        After removing that skip, the fallback stem-based matching handles them.
        """
        root = tmp_path / "proj"
        root.mkdir()
        # src/utils.js matches require('utils') via stem (basename without ext)
        (root / "utils.js").write_text("export function value() { return 1; }\n")
        (root / "main.js").write_text("const utils = require('utils');\n")
        mapper = TestMapper(str(root))
        rels = ["utils.js", "main.js"]
        edges = build_import_edges(mapper, str(root), rels, set())
        pairs = {(e["importer_file"], e["imported_file"]) for e in edges}
        assert ("main.js", "utils.js") in pairs

    def test_go_import_resolve_targets(self, tmp_path):
        root = tmp_path / "goproj"
        root.mkdir()
        (root / "widget.go").write_text("package widget\nfunc W() {}\n")
        dep = {"name": "widget", "dep_type": "import", "module_path": "x/y/widget"}
        all_paths = {"widget.go"}
        hits = list(
            _resolve_import_targets("widget_test.go", dep, "x/y/widget", all_paths),
        )
        assert "widget.go" in hits
