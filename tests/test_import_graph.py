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

    def test_scan_subset_only_builds_edges_for_scanned_files(self, tmp_project):
        """When scan_rel_paths is provided, only those files are re-scanned,
        but resolution still sees all source_rel_paths."""
        mapper = TestMapper(str(tmp_project))
        rels = ["provider.py", "consumer.py"]
        # Scan only consumer.py; provider.py should still resolve
        edges = build_import_edges(
            mapper, str(tmp_project), rels, set(), scan_rel_paths={"consumer.py"},
        )
        pairs = {(e["importer_file"], e["imported_file"]) for e in edges}
        assert ("consumer.py", "provider.py") in pairs
        assert len(edges) == 1

    def test_empty_scan_subset_returns_no_new_edges(self, tmp_project):
        """When scan_rel_paths is empty, no edges are built."""
        mapper = TestMapper(str(tmp_project))
        rels = ["provider.py", "consumer.py"]
        edges = build_import_edges(
            mapper, str(tmp_project), rels, set(), scan_rel_paths=set(),
        )
        assert edges == []

    def test_static_import_has_full_confidence(self, tmp_project):
        """Hard static imports default to confidence=1.0."""
        mapper = TestMapper(str(tmp_project))
        rels = ["provider.py", "consumer.py"]
        edges = build_import_edges(mapper, str(tmp_project), rels, set())
        for e in edges:
            assert e.get("confidence", 1.0) == 1.0


class TestDynamicRequireEdges:
    """Soft (dynamic require) edges from path.join / template / concat patterns."""

    def test_path_join_resolves_directory_to_all_plugins(self, tmp_path):
        """require(path.join(__dirname, 'plugins', name)) emits soft edges
        to every JS file in the plugins directory.
        """
        root = tmp_path / "proj"
        (root / "src" / "plugins").mkdir(parents=True)
        (root / "src" / "dispatcher.js").write_text(
            "const path = require('path');\n"
            "function dispatch(name) {\n"
            "  return require(path.join(__dirname, 'plugins', name));\n"
            "}\n"
            "module.exports = { dispatch };\n"
        )
        (root / "src" / "plugins" / "plugA.js").write_text(
            "module.exports = { compute: () => 'A' };\n"
        )
        (root / "src" / "plugins" / "plugB.js").write_text(
            "module.exports = { compute: () => 'B' };\n"
        )
        mapper = TestMapper(str(root))
        rels = [
            "src/dispatcher.js",
            "src/plugins/plugA.js",
            "src/plugins/plugB.js",
        ]
        edges = build_import_edges(mapper, str(root), rels, set())
        soft_targets = {
            e["imported_file"] for e in edges
            if e["importer_file"] == "src/dispatcher.js"
            and e.get("confidence", 1.0) < 1.0
        }
        assert "src/plugins/plugA.js" in soft_targets
        assert "src/plugins/plugB.js" in soft_targets

    def test_template_literal_resolves_directory(self, tmp_path):
        """require(`./plugins/${name}`) emits soft edges to plugin files."""
        root = tmp_path / "proj"
        (root / "src" / "plugins").mkdir(parents=True)
        (root / "src" / "loader.js").write_text(
            "function load(name) { return require(`./plugins/${name}`); }\n"
        )
        (root / "src" / "plugins" / "alpha.js").write_text(
            "module.exports = { run: () => null };\n"
        )
        mapper = TestMapper(str(root))
        rels = ["src/loader.js", "src/plugins/alpha.js"]
        edges = build_import_edges(mapper, str(root), rels, set())
        pairs = {
            (e["importer_file"], e["imported_file"]) for e in edges
            if e.get("confidence", 1.0) < 1.0
        }
        assert ("src/loader.js", "src/plugins/alpha.js") in pairs

    def test_string_concat_resolves_directory(self, tmp_path):
        """require('./plugins/' + name) emits soft edges to plugin files."""
        root = tmp_path / "proj"
        (root / "src" / "plugins").mkdir(parents=True)
        (root / "src" / "loader.js").write_text(
            "function load(name) { return require('./plugins/' + name); }\n"
        )
        (root / "src" / "plugins" / "beta.js").write_text(
            "module.exports = { run: () => null };\n"
        )
        mapper = TestMapper(str(root))
        rels = ["src/loader.js", "src/plugins/beta.js"]
        edges = build_import_edges(mapper, str(root), rels, set())
        pairs = {
            (e["importer_file"], e["imported_file"]) for e in edges
            if e.get("confidence", 1.0) < 1.0
        }
        assert ("src/loader.js", "src/plugins/beta.js") in pairs

    def test_static_import_supersedes_soft_edge(self, tmp_path):
        """If the same pair has both static and dynamic edges, confidence
        sticks at 1.0 (the higher value wins).
        """
        root = tmp_path / "proj"
        (root / "src" / "plugins").mkdir(parents=True)
        (root / "src" / "loader.js").write_text(
            "const fixed = require('./plugins/fixed');\n"
            "function load(n) { return require('./plugins/' + n); }\n"
        )
        (root / "src" / "plugins" / "fixed.js").write_text(
            "module.exports = {};\n"
        )
        mapper = TestMapper(str(root))
        rels = ["src/loader.js", "src/plugins/fixed.js"]
        edges = build_import_edges(mapper, str(root), rels, set())
        for e in edges:
            if (
                e["importer_file"] == "src/loader.js"
                and e["imported_file"] == "src/plugins/fixed.js"
            ):
                assert e["confidence"] == 1.0

    def test_fanout_cap_suppresses_huge_directories(self, tmp_path):
        """When a dynamic-require directory has more than the fan-out cap
        of files, no soft edges are emitted — the heuristic would be too
        diffuse to be useful and would flood the import graph.
        """
        from chisel.import_graph import _DYNAMIC_REQUIRE_FANOUT_CAP

        root = tmp_path / "proj"
        plugin_dir = root / "src" / "plugins"
        plugin_dir.mkdir(parents=True)
        # Create cap+5 plugin files — over the threshold.
        rels = ["src/loader.js"]
        for i in range(_DYNAMIC_REQUIRE_FANOUT_CAP + 5):
            name = f"plug{i:03d}.js"
            (plugin_dir / name).write_text(
                f"module.exports = {{ id: {i} }};\n",
            )
            rels.append(f"src/plugins/{name}")
        (root / "src" / "loader.js").write_text(
            "function load(name) { return require('./plugins/' + name); }\n"
        )
        mapper = TestMapper(str(root))
        edges = build_import_edges(mapper, str(root), rels, set())
        soft_edges = [
            e for e in edges if e.get("confidence", 1.0) < 1.0
        ]
        # No soft edges should be emitted — the directory is too large.
        assert soft_edges == [], (
            f"Fan-out cap not enforced: emitted {len(soft_edges)} soft edges "
            f"into a {_DYNAMIC_REQUIRE_FANOUT_CAP + 5}-file directory"
        )

    def test_fanout_cap_allows_small_directories(self, tmp_path):
        """Fan-out cap allows normal-size plugin directories through."""
        from chisel.import_graph import _DYNAMIC_REQUIRE_FANOUT_CAP

        root = tmp_path / "proj"
        plugin_dir = root / "src" / "plugins"
        plugin_dir.mkdir(parents=True)
        small_count = min(10, _DYNAMIC_REQUIRE_FANOUT_CAP)
        rels = ["src/loader.js"]
        for i in range(small_count):
            name = f"plug{i:02d}.js"
            (plugin_dir / name).write_text("module.exports = {};\n")
            rels.append(f"src/plugins/{name}")
        (root / "src" / "loader.js").write_text(
            "function load(n) { return require('./plugins/' + n); }\n"
        )
        mapper = TestMapper(str(root))
        edges = build_import_edges(mapper, str(root), rels, set())
        soft_edges = [
            e for e in edges if e.get("confidence", 1.0) < 1.0
        ]
        assert len(soft_edges) == small_count

    def test_tainted_variable_assignment_emits_full_confidence_edge(self, tmp_path):
        """const PATH = './foo'; require(PATH) → real (1.0) edge to foo.js.

        Variable taint is statically resolvable, so it should NOT be a
        soft edge.
        """
        root = tmp_path / "proj"
        root.mkdir()
        (root / "main.js").write_text(
            "const PATH = './foo';\n"
            "const m = require(PATH);\n"
        )
        (root / "foo.js").write_text("module.exports = {};\n")
        mapper = TestMapper(str(root))
        rels = ["main.js", "foo.js"]
        edges = build_import_edges(mapper, str(root), rels, set())
        pair_conf = {
            (e["importer_file"], e["imported_file"]): e.get("confidence", 1.0)
            for e in edges
        }
        assert pair_conf.get(("main.js", "foo.js")) == 1.0
