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
