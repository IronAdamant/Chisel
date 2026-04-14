"""Framework-specific fixture tests for test_mapper detection."""

import os

import pytest

from chisel.ast_utils import extract_code_units
from chisel.test_mapper import TestMapper


@pytest.fixture
def mapper(tmp_path):
    return TestMapper(str(tmp_path))


class TestRustFrameworkDetection:
    def test_detects_test_tokio_test_rstest(self, mapper, tmp_path):
        src = tmp_path / "basic.rs"
        src.write_text(
            '#[test]\n'
            'fn test_addition() {\n'
            '    assert_eq!(2 + 2, 4);\n'
            '}\n\n'
            '#[tokio::test]\n'
            'async fn test_async_fetch() {\n'
            '    assert!(true);\n'
            '}\n\n'
            '#[rstest]\n'
            'fn test_with_fixture() {\n'
            '    assert!(true);\n'
            '}\n\n'
            'fn plain_helper() -> i32 {\n'
            '    42\n'
            '}\n'
        )
        units = mapper.parse_test_file(str(src))
        names = {u["name"] for u in units}
        assert "test_addition" in names
        assert "test_async_fetch" in names
        assert "test_with_fixture" in names
        assert "plain_helper" not in names


class TestCSharpFrameworkDetection:
    def test_detects_fact_theory_not_plain(self, mapper, tmp_path):
        src = tmp_path / "BasicTests.cs"
        src.write_text(
            'using Xunit;\n\n'
            'public class BasicTests\n'
            '{\n'
            '    [Fact]\n'
            '    public void TestAddition()\n'
            '    {\n'
            '        Assert.Equal(4, 2 + 2);\n'
            '    }\n\n'
            '    [Theory]\n'
            '    [InlineData(1)]\n'
            '    public void TestTheory(int x)\n'
            '    {\n'
            '        Assert.True(x > 0);\n'
            '    }\n\n'
            '    public void PlainHelper()\n'
            '    {\n'
            '    }\n'
            '}\n'
        )
        units = mapper.parse_test_file(str(src))
        names = {u["name"] for u in units}
        assert "TestAddition" in names
        assert "TestTheory" in names
        assert "PlainHelper" not in names


class TestJavaFrameworkDetection:
    def test_detects_test_parameterized_test(self, mapper, tmp_path):
        src = tmp_path / "BasicTest.java"
        src.write_text(
            'import org.junit.jupiter.api.Test;\n'
            'import org.junit.jupiter.params.ParameterizedTest;\n'
            'import org.junit.jupiter.params.provider.ValueSource;\n\n'
            'public class BasicTest {\n'
            '    @Test\n'
            '    public void testAddition() {\n'
            '        assertEquals(4, 2 + 2);\n'
            '    }\n\n'
            '    @ParameterizedTest\n'
            '    @ValueSource(strings = {"hello"})\n'
            '    public void testParameterized(String s) {\n'
            '        assertNotNull(s);\n'
            '    }\n\n'
            '    public void plainHelper() {\n'
            '    }\n'
            '}\n'
        )
        units = mapper.parse_test_file(str(src))
        names = {u["name"] for u in units}
        assert "testAddition" in names
        assert "testParameterized" in names
        assert "plainHelper" not in names


class TestSwiftFrameworkDetection:
    def test_detects_test_not_plain(self, mapper, tmp_path):
        src = tmp_path / "BasicTests.swift"
        src.write_text(
            'import Testing\n\n'
            '@Test\n'
            'func testAddition() {\n'
            '    #expect(2 + 2 == 4)\n'
            '}\n\n'
            '@Test\n'
            'func testStrings() {\n'
            '    #expect("hello".isEmpty == false)\n'
            '}\n\n'
            'func plainHelper() -> Int {\n'
            '    return 42\n'
            '}\n'
        )
        units = mapper.parse_test_file(str(src))
        names = {u["name"] for u in units}
        assert "testAddition" in names
        assert "testStrings" in names
        assert "plainHelper" not in names


class TestGoModuleResolution:
    def test_go_import_path_resolves_to_directory(self, mapper, tmp_path):
        go_mod = tmp_path / "go.mod"
        go_mod.write_text("module example.com/demo\n\ngo 1.21\n")
        pkg = tmp_path / "pkg" / "utils"
        pkg.mkdir(parents=True)
        (pkg / "utils.go").write_text("package utils\n\nfunc Add(a, b int) int { return a + b }\n")
        test_file = tmp_path / "pkg" / "utils" / "utils_test.go"
        test_file.write_text(
            'package utils\n\n'
            'import "testing"\n'
            'import "example.com/demo/pkg/utils"\n\n'
            'func TestAdd(t *testing.T) {\n'
            '    if utils.Add(2, 2) != 4 {\n'
            '        t.Fail()\n'
            '    }\n'
            '}\n'
        )
        # Parse the test file
        test_units = mapper.parse_test_file(str(test_file))
        assert any(u["name"] == "TestAdd" for u in test_units)

        # Build edges: should resolve import to utils.go in same package
        # Use relative path to match engine behavior
        rel_code_file = "pkg/utils/utils.go"
        content = (pkg / "utils.go").read_text()
        code_units = extract_code_units(rel_code_file, content)
        edges = mapper.build_test_edges(test_units, code_units)
        imported = {e["code_id"] for e in edges}
        assert any("utils.go" in cid for cid in imported)
