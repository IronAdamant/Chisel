"""Tests for chisel.import_graph — static import edges."""

import pytest

from chisel.import_graph import build_import_edges
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
