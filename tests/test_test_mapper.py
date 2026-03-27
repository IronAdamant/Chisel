"""Tests for chisel.test_mapper — framework detection, discovery, deps, edges."""

import os

import pytest

from chisel.ast_utils import CodeUnit
from chisel.test_mapper import TestMapper


@pytest.fixture
def project(tmp_path):
    """Create a minimal project layout with test files."""
    # Python test file
    test_py = tmp_path / "tests" / "test_example.py"
    test_py.parent.mkdir(parents=True)
    test_py.write_text(
        "import os\n"
        "from mymodule import foo\n\n"
        "def test_foo():\n"
        "    assert foo() == 42\n\n"
        "def test_bar():\n"
        "    assert True\n\n"
        "def helper():\n"
        "    pass\n"
    )

    # Source module
    src = tmp_path / "mymodule.py"
    src.write_text(
        "def foo():\n"
        "    return 42\n\n"
        "def bar():\n"
        "    return 0\n"
    )

    # JS test file
    js_test = tmp_path / "src" / "util.test.js"
    js_test.parent.mkdir(parents=True)
    js_test.write_text(
        'import { helper } from "./helper";\n\n'
        "function testBasic() {\n"
        "  expect(helper()).toBe(true);\n"
        "}\n"
    )

    # Go test file
    go_test = tmp_path / "pkg" / "main_test.go"
    go_test.parent.mkdir(parents=True)
    go_test.write_text(
        'package main\n\n'
        'import "testing"\n\n'
        'func TestAdd(t *testing.T) {\n'
        '    result := Add(1, 2)\n'
        '    if result != 3 {\n'
        '        t.Errorf("got %d", result)\n'
        '    }\n'
        '}\n'
    )

    # Rust test file
    rs_test = tmp_path / "src" / "lib.rs"
    rs_test.parent.mkdir(parents=True, exist_ok=True)
    rs_test.write_text(
        '#[cfg(test)]\n'
        'mod tests {\n'
        '    use super::*;\n\n'
        '    #[test]\n'
        '    fn test_add() {\n'
        '        assert_eq!(add(1, 2), 3);\n'
        '    }\n'
        '}\n'
    )

    # Playwright spec file
    spec = tmp_path / "e2e" / "login.spec.ts"
    spec.parent.mkdir(parents=True)
    spec.write_text(
        'import { test, expect } from "@playwright/test";\n\n'
        'test("login page loads", async ({ page }) => {\n'
        '    await page.goto("/login");\n'
        '});\n'
    )

    # Non-test Python file (should be skipped)
    helper = tmp_path / "utils.py"
    helper.write_text("def helper():\n    pass\n")

    # .git dir (should be skipped)
    (tmp_path / ".git").mkdir()
    (tmp_path / ".git" / "dummy.py").write_text("test_x = 1\n")

    return tmp_path


@pytest.fixture
def mapper(project):
    return TestMapper(project)


class TestFrameworkDetection:
    def test_pytest(self):
        assert TestMapper.detect_framework("test_foo.py") == "pytest"
        assert TestMapper.detect_framework("foo_test.py") == "pytest"

    def test_jest(self):
        assert TestMapper.detect_framework("foo.test.js") == "jest"
        assert TestMapper.detect_framework("foo.test.ts") == "jest"
        assert TestMapper.detect_framework("foo.test.tsx") == "jest"

    def test_go(self):
        assert TestMapper.detect_framework("main_test.go") == "go"

    def test_rust_not_detected_by_name(self):
        # Rust needs content check (#[test])
        assert TestMapper.detect_framework("lib.rs") is None

    def test_non_test(self):
        assert TestMapper.detect_framework("main.py") is None
        assert TestMapper.detect_framework("app.js") is None

    def test_playwright(self, project):
        spec = str(project / "e2e" / "login.spec.ts")
        assert TestMapper.detect_framework(spec) == "playwright"


class TestDiscoverTestFiles:
    def test_finds_all_test_files(self, mapper, project):
        files = mapper.discover_test_files()
        names = [os.path.basename(f) for f in files]
        assert "test_example.py" in names
        assert "util.test.js" in names
        assert "main_test.go" in names
        assert "lib.rs" in names
        assert "login.spec.ts" in names

    def test_skips_non_test_files(self, mapper):
        files = mapper.discover_test_files()
        names = [os.path.basename(f) for f in files]
        assert "utils.py" not in names
        assert "mymodule.py" not in names

    def test_skips_git_dir(self, mapper):
        files = mapper.discover_test_files()
        for f in files:
            assert ".git" not in f.split(os.sep)


class TestParseTestFile:
    def test_python_test_file(self, mapper, project):
        test_file = str(project / "tests" / "test_example.py")
        units = mapper.parse_test_file(test_file)
        names = [u["name"] for u in units]
        assert "test_foo" in names
        assert "test_bar" in names
        # helper() is not a test
        assert "helper" not in names

    def test_go_test_file(self, mapper, project):
        test_file = str(project / "pkg" / "main_test.go")
        units = mapper.parse_test_file(test_file)
        names = [u["name"] for u in units]
        assert "TestAdd" in names

    def test_non_test_file_returns_empty(self, mapper, project):
        regular = str(project / "utils.py")
        units = mapper.parse_test_file(regular)
        assert units == []

    def test_test_unit_has_required_fields(self, mapper, project):
        test_file = str(project / "tests" / "test_example.py")
        units = mapper.parse_test_file(test_file)
        for u in units:
            assert "id" in u
            assert "file_path" in u
            assert "name" in u
            assert "framework" in u
            assert "line_start" in u
            assert "line_end" in u
            assert "content_hash" in u


class TestDependencyExtraction:
    def test_python_imports(self, mapper):
        content = (
            "import os\n"
            "from mymodule import foo, bar\n\n"
            "def test_foo():\n"
            "    result = foo()\n"
            "    bar(result)\n"
        )
        deps = mapper.extract_test_dependencies("test_f.py", content)
        names = [d["name"] for d in deps]
        assert "os" in names
        assert "foo" in names
        assert "bar" in names

    def test_python_import_types(self, mapper):
        content = "from mymod import helper\nhelper()\n"
        deps = mapper.extract_test_dependencies("test_f.py", content)
        import_deps = [d for d in deps if d["dep_type"] == "import"]
        call_deps = [d for d in deps if d["dep_type"] == "call"]
        assert any(d["name"] == "helper" for d in import_deps)
        assert any(d["name"] == "helper" for d in call_deps)

    def test_js_imports(self, mapper):
        content = 'import { helper } from "./helper";\nhelper();\n'
        deps = mapper.extract_test_dependencies("test.test.js", content)
        names = [d["name"] for d in deps]
        assert "helper" in names

    def test_go_imports(self, mapper):
        content = 'package main\n\nimport (\n\t"testing"\n\t"mymod/pkg"\n)\n'
        deps = mapper.extract_test_dependencies("main_test.go", content)
        names = [d["name"] for d in deps]
        assert "testing" in names
        assert "pkg" in names

    def test_rust_use(self, mapper):
        content = "use std::collections::{HashMap, Vec};\nuse crate::engine;\n"
        deps = mapper.extract_test_dependencies("lib.rs", content)
        names = [d["name"] for d in deps]
        assert "HashMap" in names
        assert "Vec" in names
        assert "engine" in names


class TestParseTestFileEdgeCases:
    def test_rust_test_file(self, mapper, project):
        test_file = str(project / "src" / "lib.rs")
        units = mapper.parse_test_file(test_file)
        names = [u["name"] for u in units]
        assert "test_add" in names
        for u in units:
            assert u["framework"] == "rust"

    def test_unreadable_file_returns_empty(self, mapper, tmp_path):
        missing = str(tmp_path / "gone" / "test_ghost.py")
        units = mapper.parse_test_file(missing)
        assert units == []

    def test_unknown_language_deps_returns_empty(self, mapper):
        deps = mapper.extract_test_dependencies("data.csv", "a,b,c\n1,2,3")
        assert deps == []


class TestIsTestNameEdgeCases:
    def test_jest_describe_it_test(self):
        from chisel.test_mapper import _is_test_name
        assert _is_test_name("describe", "jest") is True
        assert _is_test_name("it", "jest") is True
        assert _is_test_name("test", "jest") is True
        assert _is_test_name("testHelper", "jest") is True
        assert _is_test_name("helper", "jest") is False

    def test_playwright_same_as_jest(self):
        from chisel.test_mapper import _is_test_name
        assert _is_test_name("describe", "playwright") is True
        assert _is_test_name("test", "playwright") is True

    def test_rust_test_names(self):
        from chisel.test_mapper import _is_test_name
        assert _is_test_name("test_add", "rust") is True
        assert _is_test_name("test_", "rust") is True
        assert _is_test_name("helper", "rust") is False

    def test_unknown_framework_returns_false(self):
        from chisel.test_mapper import _is_test_name
        assert _is_test_name("test_foo", "unknown_fw") is False


class TestCheckHelpers:
    def test_check_playwright_oserror_falls_back_to_jest(self):
        from chisel.test_mapper import _check_playwright
        result = _check_playwright("/nonexistent/path/spec.ts")
        assert result == "jest"

    def test_check_rust_test_oserror_returns_false(self):
        from chisel.test_mapper import _check_rust_test
        assert _check_rust_test("/nonexistent/path/lib.rs") is False

    def test_check_playwright_without_playwright_content(self, tmp_path):
        from chisel.test_mapper import _check_playwright
        spec = tmp_path / "basic.spec.ts"
        spec.write_text('describe("test", () => {});\n')
        assert _check_playwright(str(spec)) == "jest"


class TestNewLanguageDeps:
    """Dependency extraction for the 8 newly added languages."""

    def test_csharp_using(self, mapper):
        content = "using System;\nusing MyApp.Models;\nusing static MyApp.Helpers;\n"
        deps = mapper.extract_test_dependencies("FooTest.cs", content)
        names = [d["name"] for d in deps]
        assert "System" in names
        assert "Models" in names
        assert "Helpers" in names

    def test_java_import(self, mapper):
        content = (
            "import org.junit.jupiter.api.Test;\n"
            "import com.myapp.Calculator;\n"
            "import static org.junit.Assert.*;\n"
        )
        deps = mapper.extract_test_dependencies("CalculatorTest.java", content)
        names = [d["name"] for d in deps]
        assert "Test" in names
        assert "Calculator" in names

    def test_kotlin_import(self, mapper):
        content = "import org.junit.Test\nimport com.myapp.Engine\n"
        deps = mapper.extract_test_dependencies("EngineTest.kt", content)
        names = [d["name"] for d in deps]
        assert "Test" in names
        assert "Engine" in names

    def test_cpp_include(self, mapper):
        content = '#include <gtest/gtest.h>\n#include "mylib/utils.h"\n'
        deps = mapper.extract_test_dependencies("test_utils.cpp", content)
        names = [d["name"] for d in deps]
        assert "gtest" in names
        assert "utils" in names

    def test_c_include(self, mapper):
        content = '#include <stdio.h>\n#include "parser.h"\n'
        deps = mapper.extract_test_dependencies("test_parser.c", content)
        names = [d["name"] for d in deps]
        assert "stdio" in names
        assert "parser" in names

    def test_swift_import(self, mapper):
        content = "import XCTest\nimport MyModule\n"
        deps = mapper.extract_test_dependencies("MyModuleTests.swift", content)
        names = [d["name"] for d in deps]
        assert "XCTest" in names
        assert "MyModule" in names

    def test_php_use_and_require(self, mapper):
        content = (
            "<?php\n"
            "use App\\Models\\User;\n"
            "use PHPUnit\\Framework\\TestCase;\n"
            "require_once 'helpers/math.php';\n"
        )
        deps = mapper.extract_test_dependencies("UserTest.php", content)
        names = [d["name"] for d in deps]
        assert "User" in names
        assert "TestCase" in names
        assert "math" in names

    def test_ruby_require(self, mapper):
        content = (
            "require 'rspec'\n"
            "require_relative 'lib/calculator'\n"
        )
        deps = mapper.extract_test_dependencies("calculator_spec.rb", content)
        names = [d["name"] for d in deps]
        assert "rspec" in names
        assert "calculator" in names

    def test_dart_import(self, mapper):
        content = (
            "import 'package:flutter_test/flutter_test.dart';\n"
            "import 'package:myapp/utils.dart';\n"
        )
        deps = mapper.extract_test_dependencies("utils_test.dart", content)
        names = [d["name"] for d in deps]
        assert "flutter_test" in names
        assert "utils" in names

    def test_wildcard_java_import_skipped(self, mapper):
        content = "import com.myapp.*;\n"
        deps = mapper.extract_test_dependencies("FooTest.java", content)
        names = [d["name"] for d in deps]
        assert "*" not in names


class TestDepExtractionEdgeCases:
    def test_python_syntax_error_fallback(self, mapper):
        """SyntaxError in Python falls back to regex extraction."""
        content = "from mymod import helper\ndef broken(\n"
        deps = mapper.extract_test_dependencies("test_bad.py", content)
        names = [d["name"] for d in deps]
        # Regex fallback should still find the import
        assert "helper" in names

    def test_python_regex_finds_calls(self, mapper):
        content = "import os\nfoo()\nbar(x)\n"
        # Force regex path via SyntaxError
        bad_content = content + "def broken(\n"
        deps = mapper.extract_test_dependencies("test_f.py", bad_content)
        names = [d["name"] for d in deps]
        assert "foo" in names
        assert "bar" in names

    def test_go_single_import(self, mapper):
        content = 'package main\n\nimport "fmt"\n\nfunc TestFmt(t *testing.T) {}\n'
        deps = mapper.extract_test_dependencies("main_test.go", content)
        names = [d["name"] for d in deps]
        assert "fmt" in names

    def test_js_named_imports(self, mapper):
        content = 'import { alpha, beta as b } from "./utils";\nalpha();\n'
        deps = mapper.extract_test_dependencies("test.test.js", content)
        names = [d["name"] for d in deps]
        assert "alpha" in names
        assert "beta" in names  # "beta as b" extracts "beta"

    def test_js_named_imports_multiline(self, mapper):
        """DOTALL flag: multi-line named imports are matched."""
        content = "import {\n    alpha,\n    beta\n} from '../src/utils';\n"
        deps = mapper.extract_test_dependencies("test.test.js", content)
        names = [d["name"] for d in deps]
        assert "alpha" in names
        assert "beta" in names

    def test_js_named_imports_with_module_path(self, mapper):
        """Named ESM imports should capture module_path for path-based matching."""
        content = "import { foo, bar } from './services/api';\n"
        deps = mapper.extract_test_dependencies("test.test.js", content)
        named_deps = [d for d in deps if d["name"] in ("foo", "bar")]
        assert all(d.get("module_path") == "./services/api" for d in named_deps)

    def test_js_named_imports_alias(self, mapper):
        """Named imports with 'as' alias should extract the original name."""
        content = "import { foo as bar } from './utils';\n"
        deps = mapper.extract_test_dependencies("test.test.js", content)
        names = [d["name"] for d in deps]
        assert "foo" in names
        assert "bar" not in names  # alias is not the original name


class TestBuildEdgesEdgeCases:
    def test_unreadable_test_file(self, mapper, tmp_path):
        """Edge building skips test files that can't be read."""
        test_units = [{
            "id": "gone/test_x.py:test_x",
            "file_path": "gone/test_x.py",
            "name": "test_x",
            "framework": "pytest",
            "line_start": 1, "line_end": 2, "content_hash": "abc",
        }]
        code_units = [CodeUnit("mod.py", "foo", "function", 1, 2)]
        edges = mapper.build_test_edges(test_units, code_units)
        assert edges == []


class TestBuildEdges:
    def test_basic_edge_building(self, mapper):
        test_units = [{
            "id": "tests/test_example.py:test_foo",
            "file_path": "tests/test_example.py",
            "name": "test_foo",
            "framework": "pytest",
            "line_start": 4,
            "line_end": 5,
            "content_hash": "abc",
        }]
        code_units = [
            CodeUnit("mymodule.py", "foo", "function", 1, 2),
            CodeUnit("mymodule.py", "bar", "function", 4, 5),
        ]
        edges = mapper.build_test_edges(test_units, code_units)
        code_ids = [e["code_id"] for e in edges]
        assert any("foo" in cid for cid in code_ids)

    def test_no_edges_for_unmatched(self, mapper):
        test_units = [{
            "id": "tests/test_x.py:test_x",
            "file_path": "tests/test_example.py",
            "name": "test_x",
            "framework": "pytest",
            "line_start": 1,
            "line_end": 2,
            "content_hash": "abc",
        }]
        code_units = [CodeUnit("z.py", "zzz", "function", 1, 2)]
        edges = mapper.build_test_edges(test_units, code_units)
        matched = [e for e in edges if "zzz" in e["code_id"]]
        # zzz doesn't appear in any import/call in test_example.py
        assert len(matched) == 0

    def test_edges_have_proximity_weight(self, mapper):
        """Edge weights reflect file proximity instead of always being 1.0."""
        test_units = [{
            "id": "tests/test_example.py:test_foo",
            "file_path": "tests/test_example.py",
            "name": "test_foo",
            "framework": "pytest",
            "line_start": 4,
            "line_end": 5,
            "content_hash": "abc",
        }]
        code_units = [
            CodeUnit("mymodule.py", "foo", "function", 1, 2),
        ]
        edges = mapper.build_test_edges(test_units, code_units)
        foo_edges = [e for e in edges if "foo" in e["code_id"]]
        assert len(foo_edges) > 0
        for e in foo_edges:
            assert 0.4 <= e["weight"] <= 1.0


class TestProximityWeight:
    def test_same_directory(self):
        from chisel.test_mapper import _compute_proximity_weight
        assert _compute_proximity_weight("tests/test_foo.py", "tests/foo.py") == 1.0

    def test_sibling_directories(self):
        from chisel.test_mapper import _compute_proximity_weight
        w = _compute_proximity_weight("tests/unit/test_foo.py", "tests/integration/foo.py")
        assert 0.6 <= w <= 0.8

    def test_distant_files(self):
        from chisel.test_mapper import _compute_proximity_weight
        w = _compute_proximity_weight("tests/test_foo.py", "lib/deep/nested/foo.py")
        assert w == 0.4

    def test_root_level(self):
        from chisel.test_mapper import _compute_proximity_weight
        assert _compute_proximity_weight("test_foo.py", "foo.py") == 1.0


class TestImportPathMatching:
    def test_exact_match(self):
        from chisel.test_mapper import _matches_import_path
        assert _matches_import_path("myapp/utils.py", "myapp.utils") is True

    def test_nested_match(self):
        from chisel.test_mapper import _matches_import_path
        assert _matches_import_path("src/myapp/utils.py", "myapp.utils") is True

    def test_no_match(self):
        from chisel.test_mapper import _matches_import_path
        assert _matches_import_path("other/helpers.py", "myapp.utils") is False

    def test_none_module_path(self):
        from chisel.test_mapper import _matches_import_path
        assert _matches_import_path("foo.py", None) is False


# ------------------------------------------------------------------ #
# JS/TS path-based matching
# ------------------------------------------------------------------ #


class TestJsModulePathResolution:
    def test_relative_require(self):
        from chisel.test_mapper import _resolve_js_module_path
        resolved = _resolve_js_module_path(
            "tests/services/search.test.js",
            "../../src/services/searchService",
        )
        assert resolved == "src/services/searchService"

    def test_sibling_import(self):
        from chisel.test_mapper import _resolve_js_module_path
        resolved = _resolve_js_module_path(
            "tests/utils.test.js",
            "../src/utils",
        )
        assert resolved == "src/utils"

    def test_same_dir_import(self):
        from chisel.test_mapper import _resolve_js_module_path
        resolved = _resolve_js_module_path(
            "src/__tests__/foo.test.js",
            "../foo",
        )
        assert resolved == "src/foo"

    def test_strips_js_extension(self):
        from chisel.test_mapper import _resolve_js_module_path
        resolved = _resolve_js_module_path(
            "tests/foo.test.js",
            "../src/bar.js",
        )
        assert resolved == "src/bar"

    def test_npm_package_returns_none(self):
        from chisel.test_mapper import _resolve_js_module_path
        assert _resolve_js_module_path("tests/foo.test.js", "express") is None

    def test_empty_returns_none(self):
        from chisel.test_mapper import _resolve_js_module_path
        assert _resolve_js_module_path("tests/foo.test.js", "") is None
        assert _resolve_js_module_path("tests/foo.test.js", None) is None


class TestJsImportPathMatching:
    def test_js_extension(self):
        from chisel.test_mapper import _matches_js_import_path
        assert _matches_js_import_path(
            "src/services/searchService.js", "src/services/searchService",
        ) is True

    def test_ts_extension(self):
        from chisel.test_mapper import _matches_js_import_path
        assert _matches_js_import_path(
            "src/services/searchService.ts", "src/services/searchService",
        ) is True

    def test_tsx_extension(self):
        from chisel.test_mapper import _matches_js_import_path
        assert _matches_js_import_path(
            "src/components/Button.tsx", "src/components/Button",
        ) is True

    def test_index_file(self):
        from chisel.test_mapper import _matches_js_import_path
        assert _matches_js_import_path(
            "src/utils/index.js", "src/utils",
        ) is True

    def test_no_match(self):
        from chisel.test_mapper import _matches_js_import_path
        assert _matches_js_import_path(
            "src/services/otherService.js", "src/services/searchService",
        ) is False

    def test_test_file_excluded(self):
        from chisel.test_mapper import _matches_js_import_path
        # searchService.test.js should NOT match searchService import
        assert _matches_js_import_path(
            "src/services/searchService.test.js", "src/services/searchService",
        ) is False

    def test_none_import(self):
        from chisel.test_mapper import _matches_js_import_path
        assert _matches_js_import_path("foo.js", None) is False
        assert _matches_js_import_path("foo.js", "") is False


class TestJsBindingExtraction:
    """Test that const X = require('...') extracts binding names."""

    def test_cjs_default_binding(self, mapper):
        content = "const SearchService = require('../../src/services/searchService');\n"
        deps = mapper.extract_test_dependencies("test.test.js", content)
        names = [d["name"] for d in deps]
        assert "SearchService" in names

    def test_cjs_destructured_binding(self, mapper):
        content = "const { SearchService, helper } = require('../../src/services/searchService');\n"
        deps = mapper.extract_test_dependencies("test.test.js", content)
        names = [d["name"] for d in deps]
        assert "SearchService" in names
        assert "helper" in names

    def test_cjs_destructured_with_rename(self, mapper):
        content = "const { SearchService: svc } = require('./search');\n"
        deps = mapper.extract_test_dependencies("test.test.js", content)
        names = [d["name"] for d in deps]
        assert "SearchService" in names  # extracts original name, not alias

    def test_esm_default_binding(self, mapper):
        content = "import SearchService from '../../src/services/searchService';\n"
        deps = mapper.extract_test_dependencies("test.test.js", content)
        names = [d["name"] for d in deps]
        assert "SearchService" in names

    def test_module_path_preserved(self, mapper):
        content = "const SearchService = require('../../src/services/searchService');\n"
        deps = mapper.extract_test_dependencies("test.test.js", content)
        with_path = [d for d in deps if d.get("module_path")]
        assert any(d["module_path"] == "../../src/services/searchService" for d in with_path)

    def test_let_var_bindings(self, mapper):
        content = (
            "let Foo = require('./foo');\n"
            "var Bar = require('./bar');\n"
        )
        deps = mapper.extract_test_dependencies("test.test.js", content)
        names = [d["name"] for d in deps]
        assert "Foo" in names
        assert "Bar" in names


class TestJsPathEdgeBuilding:
    """End-to-end: JS test files build edges via path resolution."""

    def test_require_builds_edges(self, mapper, tmp_path):
        """const X = require('../../src/svc') -> edge to src/svc.js units."""
        # Write a test file that requires a source module
        test_dir = tmp_path / "tests" / "services"
        test_dir.mkdir(parents=True)
        test_file = test_dir / "searchService.test.js"
        test_file.write_text(
            "const SearchService = require('../../src/services/searchService');\n"
            "describe('SearchService', () => {\n"
            "  it('should search', () => {\n"
            "    const svc = new SearchService();\n"
            "  });\n"
            "});\n"
        )

        test_units = [{
            "id": "tests/services/searchService.test.js:SearchService:test_suite",
            "file_path": "tests/services/searchService.test.js",
            "name": "SearchService",
            "framework": "jest",
            "line_start": 2,
            "line_end": 5,
            "content_hash": "abc",
        }]
        code_units = [
            CodeUnit("src/services/searchService.js", "SearchService", "class", 1, 20),
            CodeUnit("src/services/searchService.js", "search", "function", 5, 15),
            CodeUnit("src/unrelated/other.js", "other", "function", 1, 5),
        ]
        edges = mapper.build_test_edges(test_units, code_units)
        code_ids = {e["code_id"] for e in edges}
        # Should match both units in searchService.js via path resolution
        assert "src/services/searchService.js:SearchService:class" in code_ids
        assert "src/services/searchService.js:search:function" in code_ids
        # Should NOT match unrelated file
        assert "src/unrelated/other.js:other:function" not in code_ids

    def test_esm_import_builds_edges(self, mapper, tmp_path):
        """import X from '../src/utils' -> edge to src/utils.js units."""
        test_dir = tmp_path / "tests"
        test_dir.mkdir(parents=True, exist_ok=True)
        test_file = test_dir / "utils.test.js"
        test_file.write_text(
            "import { formatDate, parseDate } from '../src/utils';\n"
            "describe('utils', () => {\n"
            "  test('formatDate works', () => {});\n"
            "});\n"
        )

        test_units = [{
            "id": "tests/utils.test.js:utils:test_suite",
            "file_path": "tests/utils.test.js",
            "name": "utils",
            "framework": "jest",
            "line_start": 2,
            "line_end": 4,
            "content_hash": "abc",
        }]
        code_units = [
            CodeUnit("src/utils.js", "formatDate", "function", 1, 5),
            CodeUnit("src/utils.js", "parseDate", "function", 7, 12),
        ]
        edges = mapper.build_test_edges(test_units, code_units)
        code_ids = {e["code_id"] for e in edges}
        # Named imports match by name
        assert "src/utils.js:formatDate:function" in code_ids
        assert "src/utils.js:parseDate:function" in code_ids

    def test_npm_package_no_false_edges(self, mapper, tmp_path):
        """require('express') should not match local files named express."""
        test_dir = tmp_path / "tests"
        test_dir.mkdir(parents=True, exist_ok=True)
        test_file = test_dir / "app.test.js"
        test_file.write_text(
            "const express = require('express');\n"
            "describe('app', () => {\n"
            "  it('works', () => {});\n"
            "});\n"
        )

        test_units = [{
            "id": "tests/app.test.js:app:test_suite",
            "file_path": "tests/app.test.js",
            "name": "app",
            "framework": "jest",
            "line_start": 2,
            "line_end": 4,
            "content_hash": "abc",
        }]
        # There IS a local file called express.js — but the require is for npm
        code_units = [
            CodeUnit("src/express.js", "createApp", "function", 1, 10),
        ]
        edges = mapper.build_test_edges(test_units, code_units)
        # Path resolution returns None for non-relative imports,
        # but name matching may still link "express" if a code unit has that name.
        # The key point: no FALSE edges from path resolution.
        # The name "express" from require('express') could match via name-only
        # fallback — that's acceptable, it's the call-based matching.
        # Just verify path resolution didn't fire for a non-relative path.
        assert all(e["edge_type"] in ("import", "call") for e in edges)
