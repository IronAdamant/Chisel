"""Tests for chisel.ast_utils -- multi-language AST extraction."""

import hashlib
import textwrap

import pytest

from chisel.ast_utils import (
    CodeUnit,
    _find_block_end,
    _strip_strings_and_comments,
    compute_file_hash,
    detect_language,
    extract_code_units,
)


# =========================================================================
# Helpers
# =========================================================================


def _units_by_name(units, name):
    """Return units whose name matches *name*."""
    return [u for u in units if u.name == name]


# =========================================================================
# detect_language
# =========================================================================


class TestDetectLanguage:
    """Tests for extension -> language mapping."""

    @pytest.mark.parametrize(
        "path, expected",
        [
            ("foo.py", "python"),
            ("foo.pyw", "python"),
            ("bar.js", "javascript"),
            ("baz.jsx", "javascript"),
            ("mod.mjs", "javascript"),
            ("mod.cjs", "javascript"),
            ("qux.ts", "typescript"),
            ("quux.tsx", "typescript"),
            ("main.go", "go"),
            ("lib.rs", "rust"),
            # Case insensitivity
            ("FOO.PY", "python"),
            ("Bar.JS", "javascript"),
        ],
    )
    def test_supported_extensions(self, path, expected):
        assert detect_language(path) == expected

    @pytest.mark.parametrize(
        "path",
        [
            "readme.md",
            "Makefile",
            "data.json",
            "style.css",
            "image.png",
            "noext",
        ],
    )
    def test_unsupported_extensions(self, path):
        assert detect_language(path) is None


# =========================================================================
# compute_file_hash
# =========================================================================


class TestComputeFileHash:
    """Tests for SHA-256 hashing."""

    def test_consistency(self, tmp_path):
        p = tmp_path / "sample.txt"
        p.write_text("hello world\n")
        h1 = compute_file_hash(str(p))
        h2 = compute_file_hash(str(p))
        assert h1 == h2

    def test_correct_value(self, tmp_path):
        p = tmp_path / "sample.txt"
        content = b"hello world\n"
        p.write_bytes(content)
        expected = hashlib.sha256(content).hexdigest()
        assert compute_file_hash(str(p)) == expected

    def test_different_content_different_hash(self, tmp_path):
        a = tmp_path / "a.txt"
        b = tmp_path / "b.txt"
        a.write_text("aaa")
        b.write_text("bbb")
        assert compute_file_hash(str(a)) != compute_file_hash(str(b))


# =========================================================================
# _find_block_end
# =========================================================================


class TestFindBlockEnd:
    """Tests for brace-depth-based block-end detection."""

    def test_simple_block(self):
        lines = [
            "func foo() {",
            "    return 1",
            "}",
        ]
        assert _find_block_end(lines, 0) == 3

    def test_nested_braces(self):
        lines = [
            "func foo() {",
            "    if true {",
            "        x()",
            "    }",
            "}",
        ]
        assert _find_block_end(lines, 0) == 5

    def test_brace_on_next_line(self):
        lines = [
            "func foo()",
            "{",
            "    return 1",
            "}",
        ]
        assert _find_block_end(lines, 0) == 4

    def test_no_braces(self):
        lines = [
            "x = 1",
            "y = 2",
        ]
        # No opening brace -> returns start_idx + 1 (1-based)
        assert _find_block_end(lines, 0) == 1

    def test_single_line_block(self):
        lines = ["fn foo() { 42 }"]
        assert _find_block_end(lines, 0) == 1

    def test_deeply_nested(self):
        lines = [
            "func a() {",
            "  {",
            "    {",
            "      x()",
            "    }",
            "  }",
            "}",
        ]
        assert _find_block_end(lines, 0) == 7

    def test_braces_in_string_literal(self):
        """Braces inside string literals should be ignored."""
        lines = [
            'func foo() {',
            '    x := "{"',
            '    y := "}"',
            '    return x',
            '}',
        ]
        assert _find_block_end(lines, 0) == 5

    def test_braces_in_comment(self):
        """Braces inside // comments should be ignored."""
        lines = [
            "func bar() {",
            "    // this } is a comment",
            "    x()",
            "}",
        ]
        assert _find_block_end(lines, 0) == 4

    def test_unmatched_braces(self):
        """If braces never close, return the last line index + 1."""
        lines = [
            "func broken() {",
            "    open {",
        ]
        assert _find_block_end(lines, 0) == 2

    def test_start_idx_midway(self):
        """Starting from a non-zero index should still work."""
        lines = [
            "// preamble",
            "func second() {",
            "    body()",
            "}",
        ]
        assert _find_block_end(lines, 1) == 4


class TestStripStringsAndComments:
    """Tests for the helper that removes strings and comments."""

    def test_double_quoted_string(self):
        assert _strip_strings_and_comments('x = "{" + y') == "x =  + y"

    def test_single_quoted_string(self):
        assert _strip_strings_and_comments("x = '{' + y") == "x =  + y"

    def test_line_comment_slash(self):
        result = _strip_strings_and_comments("code() // } comment")
        assert "}" not in result

    def test_hash_not_treated_as_comment(self):
        # '#' is only a comment in Python, which uses _py_block_end instead.
        # For JS/TS/Go/Rust (which use _strip_strings_and_comments), '#' is not a comment.
        result = _strip_strings_and_comments("code() # } comment")
        assert "}" in result

    def test_escaped_quote(self):
        result = _strip_strings_and_comments(r'x = "\"{" + y')
        assert "{" not in result

    def test_no_special_chars(self):
        assert _strip_strings_and_comments("plain line") == "plain line"


# =========================================================================
# Python extraction
# =========================================================================


class TestPythonExtraction:
    """Tests for Python AST-based extraction."""

    def test_function(self):
        src = textwrap.dedent("""\
            def greet(name):
                return f"Hello, {name}"
        """)
        units = extract_code_units("mod.py", src)
        fns = _units_by_name(units, "greet")
        assert len(fns) == 1
        assert fns[0].unit_type == "function"
        assert fns[0].line_start == 1

    def test_async_function(self):
        src = textwrap.dedent("""\
            async def fetch(url):
                pass
        """)
        units = extract_code_units("mod.py", src)
        fns = _units_by_name(units, "fetch")
        assert len(fns) == 1
        assert fns[0].unit_type == "async_function"

    def test_class(self):
        src = textwrap.dedent("""\
            class Foo:
                x = 1
        """)
        units = extract_code_units("mod.py", src)
        classes = _units_by_name(units, "Foo")
        assert len(classes) == 1
        assert classes[0].unit_type == "class"

    def test_nested_method(self):
        src = textwrap.dedent("""\
            class MyClass:
                def my_method(self):
                    pass
        """)
        units = extract_code_units("mod.py", src)
        methods = _units_by_name(units, "MyClass.my_method")
        assert len(methods) == 1
        assert methods[0].unit_type == "function"
        # The class itself should also be extracted
        classes = _units_by_name(units, "MyClass")
        assert len(classes) == 1

    def test_multiple_methods(self):
        src = textwrap.dedent("""\
            class Calculator:
                def add(self, a, b):
                    return a + b

                def subtract(self, a, b):
                    return a - b
        """)
        units = extract_code_units("calc.py", src)
        assert any(u.name == "Calculator.add" for u in units)
        assert any(u.name == "Calculator.subtract" for u in units)

    def test_syntax_error_fallback(self):
        # Invalid Python that should trigger the regex fallback
        src = textwrap.dedent("""\
            def valid_func():
                pass

            def another_func(
                x, y
            ):
                return x + y

            class MyClass:
                def method(self):
                    pass

            this is not valid python !!@#$
        """)
        units = extract_code_units("broken.py", src)
        # Regex fallback should still find the functions/classes
        names = [u.name for u in units]
        assert "valid_func" in names
        assert "another_func" in names
        assert "MyClass" in names
        assert "MyClass.method" in names

    def test_line_numbers(self):
        src = textwrap.dedent("""\
            x = 1

            def foo():
                return 42

            class Bar:
                pass
        """)
        units = extract_code_units("mod.py", src)
        foo = _units_by_name(units, "foo")[0]
        assert foo.line_start == 3
        bar = _units_by_name(units, "Bar")[0]
        assert bar.line_start == 6

    def test_empty_file(self):
        units = extract_code_units("empty.py", "")
        assert units == []

    def test_end_lineno_accuracy(self):
        """Verify that end_lineno from the AST is used for accurate ranges."""
        src = textwrap.dedent("""\
            def short():
                return 1

            def longer():
                x = 1
                y = 2
                return x + y

            def last():
                pass
        """)
        units = extract_code_units("multi.py", src)
        short = _units_by_name(units, "short")[0]
        longer = _units_by_name(units, "longer")[0]
        last = _units_by_name(units, "last")[0]
        assert short.line_start == 1
        assert short.line_end == 2
        assert longer.line_start == 4
        assert longer.line_end == 7
        assert last.line_start == 9
        assert last.line_end == 10

    def test_decorators_not_in_name(self):
        """Decorated functions should still be extracted correctly."""
        src = textwrap.dedent("""\
            @property
            def value(self):
                return self._v
        """)
        units = extract_code_units("dec.py", src)
        assert len(_units_by_name(units, "value")) == 1

    def test_pyw_extension(self):
        """Python Windows extension should be recognized."""
        src = "def win_func():\n    pass\n"
        units = extract_code_units("script.pyw", src)
        assert len(_units_by_name(units, "win_func")) == 1


# =========================================================================
# JavaScript / TypeScript extraction
# =========================================================================


class TestJsTsExtraction:
    """Tests for JS/TS regex-based extraction."""

    def test_named_function(self):
        src = textwrap.dedent("""\
            function hello(name) {
                console.log(name);
            }
        """)
        units = extract_code_units("app.js", src)
        fns = _units_by_name(units, "hello")
        assert len(fns) == 1
        assert fns[0].unit_type == "function"
        assert fns[0].line_start == 1
        assert fns[0].line_end == 3

    def test_exported_function(self):
        src = textwrap.dedent("""\
            export function process(data) {
                return data;
            }
        """)
        units = extract_code_units("mod.ts", src)
        assert len(_units_by_name(units, "process")) == 1

    def test_async_function(self):
        src = textwrap.dedent("""\
            async function fetchData(url) {
                const res = await fetch(url);
                return res.json();
            }
        """)
        units = extract_code_units("api.js", src)
        assert len(_units_by_name(units, "fetchData")) == 1

    def test_arrow_function(self):
        src = textwrap.dedent("""\
            const add = (a, b) => {
                return a + b;
            }
        """)
        units = extract_code_units("util.js", src)
        fns = _units_by_name(units, "add")
        assert len(fns) == 1
        assert fns[0].unit_type == "function"

    def test_let_arrow(self):
        src = textwrap.dedent("""\
            let multiply = (a, b) => {
                return a * b;
            }
        """)
        units = extract_code_units("math.ts", src)
        assert len(_units_by_name(units, "multiply")) == 1

    def test_class(self):
        src = textwrap.dedent("""\
            class Animal {
                constructor(name) {
                    this.name = name;
                }
            }
        """)
        units = extract_code_units("animal.js", src)
        cls = _units_by_name(units, "Animal")
        assert len(cls) == 1
        assert cls[0].unit_type == "class"
        assert cls[0].line_end == 5

    def test_exported_class(self):
        src = textwrap.dedent("""\
            export class Widget {
                render() {}
            }
        """)
        units = extract_code_units("widget.tsx", src)
        assert len(_units_by_name(units, "Widget")) == 1

    def test_jsx_extension(self):
        src = textwrap.dedent("""\
            function App() {
                return <div />;
            }
        """)
        units = extract_code_units("App.jsx", src)
        assert len(_units_by_name(units, "App")) == 1

    def test_empty_js(self):
        units = extract_code_units("empty.js", "")
        assert units == []

    def test_mjs_extension(self):
        """ES module .mjs files should be recognized."""
        src = textwrap.dedent("""\
            export function handler(req) {
                return req;
            }
        """)
        units = extract_code_units("api.mjs", src)
        assert len(_units_by_name(units, "handler")) == 1

    def test_cjs_extension(self):
        """CommonJS .cjs files should be recognized."""
        src = textwrap.dedent("""\
            function setup() {
                return {};
            }
        """)
        units = extract_code_units("setup.cjs", src)
        assert len(_units_by_name(units, "setup")) == 1

    def test_var_arrow(self):
        src = textwrap.dedent("""\
            var legacy = (x) => {
                return x;
            }
        """)
        units = extract_code_units("old.js", src)
        assert len(_units_by_name(units, "legacy")) == 1

    def test_class_with_nested_braces(self):
        """Class containing methods with inner blocks."""
        src = textwrap.dedent("""\
            class Router {
                handle(req) {
                    if (req.ok) {
                        return true;
                    }
                }
            }
        """)
        units = extract_code_units("router.js", src)
        cls = _units_by_name(units, "Router")
        assert len(cls) == 1
        assert cls[0].line_start == 1
        assert cls[0].line_end == 7


# =========================================================================
# Go extraction
# =========================================================================


class TestGoExtraction:
    """Tests for Go regex-based extraction."""

    def test_function(self):
        src = textwrap.dedent("""\
            func main() {
                fmt.Println("hello")
            }
        """)
        units = extract_code_units("main.go", src)
        fns = _units_by_name(units, "main")
        assert len(fns) == 1
        assert fns[0].unit_type == "function"
        assert fns[0].line_start == 1
        assert fns[0].line_end == 3

    def test_method_receiver(self):
        src = textwrap.dedent("""\
            func (s *Server) Start() {
                s.listen()
            }
        """)
        units = extract_code_units("server.go", src)
        fns = _units_by_name(units, "Start")
        assert len(fns) == 1

    def test_struct(self):
        src = textwrap.dedent("""\
            type Config struct {
                Host string
                Port int
            }
        """)
        units = extract_code_units("config.go", src)
        structs = _units_by_name(units, "Config")
        assert len(structs) == 1
        assert structs[0].unit_type == "struct"
        assert structs[0].line_end == 4

    def test_interface(self):
        src = textwrap.dedent("""\
            type Reader interface {
                Read(p []byte) (n int, err error)
            }
        """)
        units = extract_code_units("io.go", src)
        ifaces = _units_by_name(units, "Reader")
        assert len(ifaces) == 1
        assert ifaces[0].unit_type == "interface"

    def test_multiple_declarations(self):
        src = textwrap.dedent("""\
            func init() {
            }

            type Pair struct {
                A int
                B int
            }

            func (p Pair) Sum() {
                return p.A + p.B
            }
        """)
        units = extract_code_units("pair.go", src)
        names = [u.name for u in units]
        assert "init" in names
        assert "Pair" in names
        assert "Sum" in names

    def test_empty_go(self):
        units = extract_code_units("empty.go", "")
        assert units == []

    def test_value_receiver(self):
        """Method with value (non-pointer) receiver."""
        src = textwrap.dedent("""\
            func (p Point) String() string {
                return fmt.Sprintf("(%f, %f)", p.X, p.Y)
            }
        """)
        units = extract_code_units("point.go", src)
        assert len(_units_by_name(units, "String")) == 1

    def test_struct_with_nested_braces(self):
        """Ensure struct body with default values doesn't confuse matching."""
        src = textwrap.dedent("""\
            type Config struct {
                Tags map[string]string
            }
        """)
        units = extract_code_units("cfg.go", src)
        s = _units_by_name(units, "Config")
        assert len(s) == 1
        assert s[0].line_start == 1
        assert s[0].line_end == 3


# =========================================================================
# Rust extraction
# =========================================================================


class TestRustExtraction:
    """Tests for Rust regex-based extraction."""

    def test_fn(self):
        src = textwrap.dedent("""\
            fn main() {
                println!("hello");
            }
        """)
        units = extract_code_units("main.rs", src)
        fns = _units_by_name(units, "main")
        assert len(fns) == 1
        assert fns[0].unit_type == "function"
        assert fns[0].line_start == 1
        assert fns[0].line_end == 3

    def test_pub_fn(self):
        src = textwrap.dedent("""\
            pub fn new(x: i32) -> Self {
                Self { x }
            }
        """)
        units = extract_code_units("lib.rs", src)
        assert len(_units_by_name(units, "new")) == 1

    def test_async_fn(self):
        src = textwrap.dedent("""\
            pub async fn serve(addr: &str) {
                loop {}
            }
        """)
        units = extract_code_units("server.rs", src)
        assert len(_units_by_name(units, "serve")) == 1

    def test_struct(self):
        src = textwrap.dedent("""\
            pub struct Point {
                x: f64,
                y: f64,
            }
        """)
        units = extract_code_units("geom.rs", src)
        s = _units_by_name(units, "Point")
        assert len(s) == 1
        assert s[0].unit_type == "struct"

    def test_enum(self):
        src = textwrap.dedent("""\
            enum Color {
                Red,
                Green,
                Blue,
            }
        """)
        units = extract_code_units("color.rs", src)
        e = _units_by_name(units, "Color")
        assert len(e) == 1
        assert e[0].unit_type == "enum"

    def test_impl(self):
        src = textwrap.dedent("""\
            impl Point {
                fn distance(&self) -> f64 {
                    (self.x * self.x + self.y * self.y).sqrt()
                }
            }
        """)
        units = extract_code_units("geom.rs", src)
        impls = [u for u in units if u.unit_type == "impl"]
        assert len(impls) == 1
        assert impls[0].name == "Point"
        assert impls[0].line_end == 5

    def test_generic_impl(self):
        src = textwrap.dedent("""\
            impl<T> Vec<T> {
                fn len(&self) -> usize {
                    self.len
                }
            }
        """)
        units = extract_code_units("vec.rs", src)
        impls = [u for u in units if u.unit_type == "impl"]
        assert len(impls) == 1
        # The name should start with Vec
        assert impls[0].name.startswith("Vec")

    def test_multiple_rust_items(self):
        src = textwrap.dedent("""\
            struct Foo {
                x: i32,
            }

            enum Bar {
                A,
                B,
            }

            impl Foo {
                fn new(x: i32) -> Self {
                    Foo { x }
                }
            }

            fn free_func() {
                println!("free");
            }
        """)
        units = extract_code_units("multi.rs", src)
        types = {u.unit_type for u in units}
        assert "struct" in types
        assert "enum" in types
        assert "impl" in types
        assert "function" in types

    def test_empty_rust(self):
        units = extract_code_units("empty.rs", "")
        assert units == []

    def test_pub_crate_fn(self):
        """pub(crate) visibility should be handled."""
        src = textwrap.dedent("""\
            pub(crate) fn internal() {
                todo!()
            }
        """)
        units = extract_code_units("lib.rs", src)
        assert len(_units_by_name(units, "internal")) == 1

    def test_unsafe_fn(self):
        src = textwrap.dedent("""\
            pub unsafe fn raw_ptr(p: *const u8) {
                std::ptr::read(p);
            }
        """)
        units = extract_code_units("ffi.rs", src)
        fns = _units_by_name(units, "raw_ptr")
        assert len(fns) == 1

    def test_unit_struct(self):
        """Unit struct (no braces) should still be found."""
        src = "pub struct Marker;\n"
        units = extract_code_units("types.rs", src)
        s = _units_by_name(units, "Marker")
        assert len(s) == 1
        assert s[0].unit_type == "struct"

    def test_fn_with_generics(self):
        src = textwrap.dedent("""\
            fn process<T: Clone>(item: T) {
                let _ = item.clone();
            }
        """)
        units = extract_code_units("gen.rs", src)
        assert len(_units_by_name(units, "process")) == 1


# =========================================================================
# C# extraction
# =========================================================================


class TestCSharpExtraction:
    """Tests for C# regex-based extraction."""

    def test_basic_class(self):
        src = textwrap.dedent("""\
            public class Foo {
            }
        """)
        units = extract_code_units("app.cs", src)
        cls = _units_by_name(units, "Foo")
        assert len(cls) == 1
        assert cls[0].unit_type == "class"

    def test_method_generic_return(self):
        src = textwrap.dedent("""\
            public class Svc {
                public List<string> GetItems() {
                    return new List<string>();
                }
            }
        """)
        units = extract_code_units("svc.cs", src)
        fns = _units_by_name(units, "GetItems")
        assert len(fns) == 1
        assert fns[0].unit_type == "function"

    def test_method_nested_generics(self):
        src = textwrap.dedent("""\
            public class Builder {
                Dictionary<string, List<int>> Build() {
                    return null;
                }
            }
        """)
        units = extract_code_units("builder.cs", src)
        fns = _units_by_name(units, "Build")
        assert len(fns) == 1
        assert fns[0].unit_type == "function"

    def test_attributed_method(self):
        src = textwrap.dedent("""\
            public class Tests {
                [Test] public void RunTest() {
                    Assert.Pass();
                }
            }
        """)
        units = extract_code_units("tests.cs", src)
        fns = _units_by_name(units, "RunTest")
        assert len(fns) == 1
        assert fns[0].unit_type == "function"

    def test_attributed_class(self):
        src = textwrap.dedent("""\
            [Serializable] public class Data {
                public int Id;
            }
        """)
        units = extract_code_units("data.cs", src)
        cls = _units_by_name(units, "Data")
        assert len(cls) == 1
        assert cls[0].unit_type == "class"

    def test_static_method(self):
        src = textwrap.dedent("""\
            public class Program {
                public static void Main() {
                    Console.WriteLine("Hello");
                }
            }
        """)
        units = extract_code_units("program.cs", src)
        fns = _units_by_name(units, "Main")
        assert len(fns) == 1
        assert fns[0].unit_type == "function"

    def test_interface(self):
        src = textwrap.dedent("""\
            public interface IFoo {
                void DoWork();
            }
        """)
        units = extract_code_units("ifoo.cs", src)
        ifaces = _units_by_name(units, "IFoo")
        assert len(ifaces) == 1
        assert ifaces[0].unit_type == "interface"

    def test_async_method(self):
        src = textwrap.dedent("""\
            public class Client {
                public async Task<int> FetchAsync() {
                    return await GetValueAsync();
                }
            }
        """)
        units = extract_code_units("client.cs", src)
        fns = _units_by_name(units, "FetchAsync")
        assert len(fns) == 1
        assert fns[0].unit_type == "function"

    def test_empty_csharp(self):
        units = extract_code_units("empty.cs", "")
        assert units == []


# =========================================================================
# Java extraction
# =========================================================================


class TestJavaExtraction:
    """Tests for Java regex-based extraction."""

    def test_basic_class(self):
        src = textwrap.dedent("""\
            public class Foo {
            }
        """)
        units = extract_code_units("Foo.java", src)
        cls = _units_by_name(units, "Foo")
        assert len(cls) == 1
        assert cls[0].unit_type == "class"

    def test_annotated_method(self):
        src = textwrap.dedent("""\
            public class Svc {
                @Override public void process() {
                    // impl
                }
            }
        """)
        units = extract_code_units("Svc.java", src)
        fns = _units_by_name(units, "process")
        assert len(fns) == 1
        assert fns[0].unit_type == "function"

    def test_generic_return_type(self):
        src = textwrap.dedent("""\
            public class Repo {
                public List<String> getItems() {
                    return Collections.emptyList();
                }
            }
        """)
        units = extract_code_units("Repo.java", src)
        fns = _units_by_name(units, "getItems")
        assert len(fns) == 1
        assert fns[0].unit_type == "function"

    def test_nested_generics_package_private(self):
        src = textwrap.dedent("""\
            class Builder {
                Map<String, List<Integer>> build() {
                    return new HashMap<>();
                }
            }
        """)
        units = extract_code_units("Builder.java", src)
        fns = _units_by_name(units, "build")
        assert len(fns) == 1
        assert fns[0].unit_type == "function"

    def test_record(self):
        src = textwrap.dedent("""\
            public record Point(int x, int y) {
            }
        """)
        units = extract_code_units("Point.java", src)
        recs = _units_by_name(units, "Point")
        assert len(recs) == 1
        assert recs[0].unit_type == "record"

    def test_interface(self):
        src = textwrap.dedent("""\
            public interface Foo {
                void doWork();
            }
        """)
        units = extract_code_units("Foo.java", src)
        ifaces = _units_by_name(units, "Foo")
        assert len(ifaces) == 1
        assert ifaces[0].unit_type == "interface"

    def test_annotated_class(self):
        src = textwrap.dedent("""\
            @Entity public class User {
                private String name;
            }
        """)
        units = extract_code_units("User.java", src)
        cls = _units_by_name(units, "User")
        assert len(cls) == 1
        assert cls[0].unit_type == "class"

    def test_empty_java(self):
        units = extract_code_units("Empty.java", "")
        assert units == []


# =========================================================================
# Kotlin extraction
# =========================================================================


class TestKotlinExtraction:
    """Tests for Kotlin regex-based extraction."""

    def test_basic_class(self):
        src = textwrap.dedent("""\
            class Foo {
            }
        """)
        units = extract_code_units("Foo.kt", src)
        cls = _units_by_name(units, "Foo")
        assert len(cls) == 1
        assert cls[0].unit_type == "class"

    def test_data_class(self):
        src = textwrap.dedent("""\
            data class User(val name: String) {
            }
        """)
        units = extract_code_units("User.kt", src)
        cls = _units_by_name(units, "User")
        assert len(cls) == 1
        assert cls[0].unit_type == "class"

    def test_extension_function(self):
        src = textwrap.dedent("""\
            fun String.toSnake() {
                // impl
            }
        """)
        units = extract_code_units("ext.kt", src)
        fns = _units_by_name(units, "toSnake")
        assert len(fns) == 1
        assert fns[0].unit_type == "function"

    def test_suspend_function(self):
        src = textwrap.dedent("""\
            suspend fun fetch() {
                // coroutine
            }
        """)
        units = extract_code_units("async.kt", src)
        fns = _units_by_name(units, "fetch")
        assert len(fns) == 1
        assert fns[0].unit_type == "function"

    def test_object(self):
        src = textwrap.dedent("""\
            object Singleton {
                val x = 1
            }
        """)
        units = extract_code_units("single.kt", src)
        objs = _units_by_name(units, "Singleton")
        assert len(objs) == 1
        assert objs[0].unit_type == "object"

    def test_sealed_class(self):
        src = textwrap.dedent("""\
            sealed class Result {
            }
        """)
        units = extract_code_units("result.kt", src)
        cls = _units_by_name(units, "Result")
        assert len(cls) == 1
        assert cls[0].unit_type == "class"

    def test_inline_class(self):
        src = textwrap.dedent("""\
            inline class Password(val value: String) {
            }
        """)
        units = extract_code_units("password.kt", src)
        cls = _units_by_name(units, "Password")
        assert len(cls) == 1
        assert cls[0].unit_type == "class"

    def test_empty_kotlin(self):
        units = extract_code_units("empty.kt", "")
        assert units == []


# =========================================================================
# C / C++ extraction
# =========================================================================


class TestCppExtraction:
    """Tests for C/C++ regex-based extraction."""

    def test_basic_class(self):
        src = textwrap.dedent("""\
            class Foo {
            };
        """)
        units = extract_code_units("foo.cpp", src)
        cls = _units_by_name(units, "Foo")
        assert len(cls) == 1
        assert cls[0].unit_type == "class"

    def test_template_class(self):
        src = textwrap.dedent("""\
            template<typename T> class Container {
            };
        """)
        units = extract_code_units("container.hpp", src)
        cls = _units_by_name(units, "Container")
        assert len(cls) == 1
        assert cls[0].unit_type == "class"

    def test_namespace(self):
        src = textwrap.dedent("""\
            namespace MyLib {
                int x = 1;
            }
        """)
        units = extract_code_units("lib.cpp", src)
        ns = _units_by_name(units, "MyLib")
        assert len(ns) == 1
        assert ns[0].unit_type == "namespace"

    def test_enum_class(self):
        src = textwrap.dedent("""\
            enum class Color {
                Red,
                Green,
                Blue,
            };
        """)
        units = extract_code_units("color.cpp", src)
        enums = _units_by_name(units, "Color")
        assert len(enums) == 1
        assert enums[0].unit_type == "enum"

    def test_template_function(self):
        src = textwrap.dedent("""\
            template<typename T> void process(T item) {
                // impl
            }
        """)
        units = extract_code_units("util.cpp", src)
        fns = _units_by_name(units, "process")
        assert len(fns) == 1
        assert fns[0].unit_type == "function"

    def test_namespace_qualified_return(self):
        src = textwrap.dedent("""\
            std::string getName() {
                return "hello";
            }
        """)
        units = extract_code_units("name.cpp", src)
        fns = _units_by_name(units, "getName")
        assert len(fns) == 1
        assert fns[0].unit_type == "function"

    def test_c_header_struct(self):
        """C header with struct should be recognized via .h extension."""
        src = textwrap.dedent("""\
            struct Point {
                int x;
                int y;
            };
        """)
        units = extract_code_units("point.h", src)
        structs = _units_by_name(units, "Point")
        assert len(structs) == 1
        assert structs[0].unit_type == "struct"

    def test_empty_cpp(self):
        units = extract_code_units("empty.cpp", "")
        assert units == []


# =========================================================================
# Swift extraction
# =========================================================================


class TestSwiftExtraction:
    """Tests for Swift regex-based extraction."""

    def test_basic_class(self):
        src = textwrap.dedent("""\
            class Foo {
            }
        """)
        units = extract_code_units("Foo.swift", src)
        cls = _units_by_name(units, "Foo")
        assert len(cls) == 1
        assert cls[0].unit_type == "class"

    def test_protocol(self):
        src = textwrap.dedent("""\
            protocol Drawable {
                func draw()
            }
        """)
        units = extract_code_units("draw.swift", src)
        protos = _units_by_name(units, "Drawable")
        assert len(protos) == 1
        assert protos[0].unit_type == "protocol"

    def test_attributed_func(self):
        src = textwrap.dedent("""\
            @objc func setup() {
                // bridge
            }
        """)
        units = extract_code_units("bridge.swift", src)
        fns = _units_by_name(units, "setup")
        assert len(fns) == 1
        assert fns[0].unit_type == "function"

    def test_attributed_class(self):
        src = textwrap.dedent("""\
            @objc class Bridge {
                var x = 0
            }
        """)
        units = extract_code_units("bridge.swift", src)
        cls = _units_by_name(units, "Bridge")
        assert len(cls) == 1
        assert cls[0].unit_type == "class"

    def test_static_func(self):
        src = textwrap.dedent("""\
            class Factory {
                static func create() {
                    // factory
                }
            }
        """)
        units = extract_code_units("factory.swift", src)
        fns = _units_by_name(units, "create")
        assert len(fns) == 1
        assert fns[0].unit_type == "function"

    def test_mutating_func(self):
        src = textwrap.dedent("""\
            struct Toggle {
                mutating func toggle() {
                    self.on = !self.on
                }
            }
        """)
        units = extract_code_units("toggle.swift", src)
        fns = _units_by_name(units, "toggle")
        assert len(fns) == 1
        assert fns[0].unit_type == "function"

    def test_empty_swift(self):
        units = extract_code_units("empty.swift", "")
        assert units == []


# =========================================================================
# PHP extraction
# =========================================================================


class TestPhpExtraction:
    """Tests for PHP regex-based extraction."""

    def test_basic_class(self):
        src = textwrap.dedent("""\
            class Foo {
            }
        """)
        units = extract_code_units("Foo.php", src)
        cls = _units_by_name(units, "Foo")
        assert len(cls) == 1
        assert cls[0].unit_type == "class"

    def test_trait(self):
        src = textwrap.dedent("""\
            trait Loggable {
                public function log() {}
            }
        """)
        units = extract_code_units("loggable.php", src)
        traits = _units_by_name(units, "Loggable")
        assert len(traits) == 1
        assert traits[0].unit_type == "trait"

    def test_public_method(self):
        src = textwrap.dedent("""\
            class Svc {
                public function process() {
                    // impl
                }
            }
        """)
        units = extract_code_units("svc.php", src)
        fns = _units_by_name(units, "process")
        assert len(fns) == 1
        assert fns[0].unit_type == "function"

    def test_static_method(self):
        src = textwrap.dedent("""\
            class Factory {
                public static function create() {
                    return new self();
                }
            }
        """)
        units = extract_code_units("factory.php", src)
        fns = _units_by_name(units, "create")
        assert len(fns) == 1
        assert fns[0].unit_type == "function"

    def test_abstract_class(self):
        src = textwrap.dedent("""\
            abstract class Base {
                abstract public function run();
            }
        """)
        units = extract_code_units("base.php", src)
        cls = _units_by_name(units, "Base")
        assert len(cls) == 1
        assert cls[0].unit_type == "class"

    def test_empty_php(self):
        units = extract_code_units("empty.php", "")
        assert units == []


# =========================================================================
# Ruby extraction
# =========================================================================


class TestRubyExtraction:
    """Tests for Ruby end-delimited extraction."""

    def test_basic_class(self):
        src = textwrap.dedent("""\
            class Foo
              def bar
                1
              end
            end
        """)
        units = extract_code_units("foo.rb", src)
        cls = _units_by_name(units, "Foo")
        assert len(cls) == 1
        assert cls[0].unit_type == "class"

    def test_module(self):
        src = textwrap.dedent("""\
            module MyApp
              VERSION = "1.0"
            end
        """)
        units = extract_code_units("app.rb", src)
        mods = _units_by_name(units, "MyApp")
        assert len(mods) == 1
        assert mods[0].unit_type == "module"

    def test_method(self):
        src = textwrap.dedent("""\
            def process
              true
            end
        """)
        units = extract_code_units("util.rb", src)
        fns = _units_by_name(units, "process")
        assert len(fns) == 1
        assert fns[0].unit_type == "function"

    def test_self_method(self):
        src = textwrap.dedent("""\
            class Maker
              def self.build
                new
              end
            end
        """)
        units = extract_code_units("maker.rb", src)
        fns = _units_by_name(units, "build")
        assert len(fns) == 1
        assert fns[0].unit_type == "function"

    def test_bang_method(self):
        src = textwrap.dedent("""\
            def save!
              true
            end
        """)
        units = extract_code_units("model.rb", src)
        fns = _units_by_name(units, "save!")
        assert len(fns) == 1
        assert fns[0].unit_type == "function"

    def test_predicate_method(self):
        src = textwrap.dedent("""\
            def empty?
              false
            end
        """)
        units = extract_code_units("check.rb", src)
        fns = _units_by_name(units, "empty?")
        assert len(fns) == 1
        assert fns[0].unit_type == "function"

    def test_namespaced_class(self):
        src = textwrap.dedent("""\
            class Foo::Bar
              def hello
                puts "hi"
              end
            end
        """)
        units = extract_code_units("bar.rb", src)
        cls = _units_by_name(units, "Foo::Bar")
        assert len(cls) == 1
        assert cls[0].unit_type == "class"

    def test_empty_ruby(self):
        units = extract_code_units("empty.rb", "")
        assert units == []


# =========================================================================
# Dart extraction
# =========================================================================


class TestDartExtraction:
    """Tests for Dart regex-based extraction."""

    def test_basic_class(self):
        src = textwrap.dedent("""\
            class Foo {
            }
        """)
        units = extract_code_units("foo.dart", src)
        cls = _units_by_name(units, "Foo")
        assert len(cls) == 1
        assert cls[0].unit_type == "class"

    def test_mixin(self):
        src = textwrap.dedent("""\
            mixin Printable {
                void printSelf() {}
            }
        """)
        units = extract_code_units("printable.dart", src)
        mixins = _units_by_name(units, "Printable")
        assert len(mixins) == 1
        assert mixins[0].unit_type == "mixin"

    def test_method_with_return_type(self):
        src = textwrap.dedent("""\
            class Person {
                String getName() {
                    return name;
                }
            }
        """)
        units = extract_code_units("person.dart", src)
        fns = _units_by_name(units, "getName")
        assert len(fns) == 1
        assert fns[0].unit_type == "function"

    def test_factory_constructor(self):
        """Factory constructor captures the class name."""
        src = textwrap.dedent("""\
            class Foo {
                factory Foo() {
                    return _instance;
                }
            }
        """)
        units = extract_code_units("foo.dart", src)
        # The factory regex captures "Foo" as the name
        factories = [u for u in units if u.name == "Foo" and u.unit_type == "function"]
        assert len(factories) == 1

    def test_getter(self):
        src = textwrap.dedent("""\
            class Config {
                String get name {
                    return _name;
                }
            }
        """)
        units = extract_code_units("config.dart", src)
        fns = _units_by_name(units, "name")
        assert len(fns) == 1
        assert fns[0].unit_type == "function"

    def test_static_method(self):
        src = textwrap.dedent("""\
            class Utils {
                static void process() {
                    // impl
                }
            }
        """)
        units = extract_code_units("utils.dart", src)
        fns = _units_by_name(units, "process")
        assert len(fns) == 1
        assert fns[0].unit_type == "function"

    def test_abstract_class(self):
        src = textwrap.dedent("""\
            abstract class Base {
                void run();
            }
        """)
        units = extract_code_units("base.dart", src)
        cls = _units_by_name(units, "Base")
        assert len(cls) == 1
        assert cls[0].unit_type == "class"

    def test_extension(self):
        src = textwrap.dedent("""\
            extension StringExt on String {
                bool get isBlank => trim().isEmpty;
            }
        """)
        units = extract_code_units("ext.dart", src)
        exts = _units_by_name(units, "StringExt")
        assert len(exts) == 1
        assert exts[0].unit_type == "extension"

    def test_empty_dart(self):
        units = extract_code_units("empty.dart", "")
        assert units == []


# =========================================================================
# Unsupported language
# =========================================================================


class TestUnsupportedLanguage:
    """Extraction for unsupported file types returns empty list."""

    def test_markdown(self):
        assert extract_code_units("readme.md", "# Hello") == []

    def test_json(self):
        assert extract_code_units("data.json", '{"key": 1}') == []


# =========================================================================
# CodeUnit dataclass
# =========================================================================


class TestCodeUnit:
    """Basic sanity checks on the dataclass."""

    def test_fields(self):
        cu = CodeUnit("f.py", "foo", "function", 1, 5)
        assert cu.file_path == "f.py"
        assert cu.name == "foo"
        assert cu.unit_type == "function"
        assert cu.line_start == 1
        assert cu.line_end == 5

    def test_equality(self):
        a = CodeUnit("f.py", "foo", "function", 1, 5)
        b = CodeUnit("f.py", "foo", "function", 1, 5)
        assert a == b


# =========================================================================
# Extractor plugin registry
# =========================================================================


class TestExtractorRegistry:
    """Tests for register_extractor / unregister_extractor / get_registered_extractors."""

    def test_register_and_use_custom_extractor(self):
        """Register a custom extractor for 'python' and verify it is invoked."""
        from chisel.ast_utils import register_extractor, unregister_extractor

        sentinel = CodeUnit("custom.py", "custom_fn", "function", 1, 1)
        calls = []

        def fake_extractor(file_path, content):
            calls.append((file_path, content))
            return [sentinel]

        try:
            register_extractor("python", fake_extractor)
            result = extract_code_units("test.py", "def real(): pass")
            assert result == [sentinel]
            assert len(calls) == 1
            assert calls[0] == ("test.py", "def real(): pass")
        finally:
            unregister_extractor("python")

    def test_custom_overrides_builtin(self):
        """A custom extractor must take priority over the built-in one."""
        from chisel.ast_utils import register_extractor, unregister_extractor

        def override_extractor(file_path, content):
            return [CodeUnit(file_path, "overridden", "function", 1, 1)]

        try:
            register_extractor("python", override_extractor)
            units = extract_code_units("mod.py", "def real_func():\n    pass\n")
            # Built-in would produce "real_func"; custom produces "overridden"
            assert len(units) == 1
            assert units[0].name == "overridden"
        finally:
            unregister_extractor("python")

    def test_unregister_restores_builtin(self):
        """After unregistering, the built-in extractor should be used again."""
        from chisel.ast_utils import register_extractor, unregister_extractor

        def dummy_extractor(file_path, content):
            return [CodeUnit(file_path, "dummy", "function", 1, 1)]

        try:
            register_extractor("python", dummy_extractor)
            # Confirm custom is active
            assert extract_code_units("m.py", "def foo(): pass")[0].name == "dummy"
        finally:
            unregister_extractor("python")

        # After unregister, built-in should handle it
        units = extract_code_units("m.py", "def foo():\n    pass\n")
        names = [u.name for u in units]
        assert "foo" in names

    def test_unregister_nonexistent_raises(self):
        """Unregistering a language with no custom extractor must raise KeyError."""
        from chisel.ast_utils import unregister_extractor

        with pytest.raises(KeyError):
            unregister_extractor("nonexistent_language_xyz")

    def test_register_non_callable_raises(self):
        """Registering a non-callable must raise TypeError."""
        from chisel.ast_utils import register_extractor

        with pytest.raises(TypeError):
            register_extractor("python", "not_a_callable")

        with pytest.raises(TypeError):
            register_extractor("python", 42)

        with pytest.raises(TypeError):
            register_extractor("python", None)

    def test_get_registered_extractors_returns_copy(self):
        """Mutating the returned dict must not affect the internal registry."""
        from chisel.ast_utils import (
            get_registered_extractors,
            register_extractor,
            unregister_extractor,
        )

        def fake(file_path, content):
            return []

        try:
            register_extractor("python", fake)
            snapshot = get_registered_extractors()
            assert "python" in snapshot

            # Mutate the returned copy
            snapshot["python"] = None
            snapshot["bogus"] = lambda fp, c: []

            # Internal state must be unchanged
            actual = get_registered_extractors()
            assert actual["python"] is fake
            assert "bogus" not in actual
        finally:
            unregister_extractor("python")
