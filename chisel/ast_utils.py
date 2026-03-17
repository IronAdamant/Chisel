"""Multi-language AST extraction for Chisel.

Extracts code units (functions, classes, structs, etc.) from source files
across Python, JavaScript/TypeScript, Go, and Rust. Fully self-contained
with zero external dependencies beyond the Python standard library.
"""

import ast
import hashlib
import re
from dataclasses import dataclass
from pathlib import Path
from typing import List, Optional

# Directories to always skip when walking the project tree.
_SKIP_DIRS = {
    ".git", "node_modules", "__pycache__", ".tox", ".venv", "venv",
    "env", ".mypy_cache", ".pytest_cache", ".ruff_cache", "dist",
    "build", ".eggs", "target",
}


@dataclass
class CodeUnit:
    """Represents a single extractable unit of code."""

    file_path: str
    name: str
    unit_type: str  # "function", "async_function", "class", "struct", "enum", "impl", etc.
    line_start: int
    line_end: int


# ---------------------------------------------------------------------------
# Language detection
# ---------------------------------------------------------------------------

_EXTENSION_MAP = {
    ".py": "python",
    ".pyw": "python",
    ".js": "javascript",
    ".jsx": "javascript",
    ".mjs": "javascript",
    ".cjs": "javascript",
    ".ts": "typescript",
    ".tsx": "typescript",
    ".go": "go",
    ".rs": "rust",
}


def detect_language(file_path: str) -> Optional[str]:
    """Return the language string for a file path based on its extension."""
    ext = Path(file_path).suffix.lower()
    return _EXTENSION_MAP.get(ext)


# ---------------------------------------------------------------------------
# File hashing
# ---------------------------------------------------------------------------


def compute_file_hash(file_path: str) -> str:
    """Return the SHA-256 hex digest of a file's contents."""
    h = hashlib.sha256()
    with open(file_path, "rb") as f:
        for chunk in iter(lambda: f.read(8192), b""):
            h.update(chunk)
    return h.hexdigest()


# ---------------------------------------------------------------------------
# Brace-matching helper
# ---------------------------------------------------------------------------


def _find_block_end(lines: List[str], start_idx: int) -> int:
    """Find the line number (1-based) of the closing brace for a block.

    Scans forward from *start_idx* (0-based index into *lines*) looking for
    the first ``{``.  Once found, tracks brace depth and returns the 1-based
    line number where depth returns to zero.  If no opening brace is found,
    returns ``start_idx + 1`` (the 1-based line of the start line itself).

    String literals and single-line comments are stripped before counting
    braces so that ``"{"`` or ``// }`` do not cause false matches.
    """
    depth = 0
    found_open = False

    for i in range(start_idx, len(lines)):
        cleaned = _strip_strings_and_comments(lines[i])
        for ch in cleaned:
            if ch == "{":
                depth += 1
                found_open = True
            elif ch == "}":
                depth -= 1
                if found_open and depth == 0:
                    return i + 1  # 1-based

    # Fallback: if an opening brace was found but never closed, return
    # the last line number.  If no brace was found at all, return the
    # start line (1-based).
    if found_open:
        return len(lines)
    return start_idx + 1


def _strip_strings_and_comments(line: str) -> str:
    """Remove string literals and trailing ``//`` comments from a single line.

    Handles ``"``, ``'``, and backtick-quoted strings with backslash
    escaping.  ``#`` is **not** treated as a comment (Python files use
    ``_py_block_end`` instead).  Multi-line strings and block comments
    (``/* */``) are **not** handled -- this is a best-effort helper to
    avoid miscounting braces.
    """
    result: list = []
    i = 0
    length = len(line)
    while i < length:
        ch = line[i]
        # Single-line comment markers
        if ch == "/" and i + 1 < length and line[i + 1] == "/":
            break
        # Note: '#' is only a comment in Python, which uses _py_block_end
        # instead of _find_block_end, so we do not treat '#' as a comment here.
        # Quoted strings
        if ch in ('"', "'", "`"):
            quote = ch
            i += 1
            while i < length and line[i] != quote:
                if line[i] == "\\" and i + 1 < length:
                    i += 2
                    continue
                i += 1
            i += 1  # skip closing quote
            continue
        result.append(ch)
        i += 1
    return "".join(result)


# ---------------------------------------------------------------------------
# Python extraction
# ---------------------------------------------------------------------------

_PY_FUNC_RE = re.compile(
    r"^(?P<indent>\s*)(?:async\s+)?def\s+(?P<name>[A-Za-z_]\w*)\s*\(",
)
_PY_CLASS_RE = re.compile(
    r"^(?P<indent>\s*)class\s+(?P<name>[A-Za-z_]\w*)\s*[\(:]",
)


def _extract_python_ast(file_path: str, content: str) -> List[CodeUnit]:
    """Extract code units from Python source using the ``ast`` module."""
    try:
        tree = ast.parse(content, filename=file_path)
    except SyntaxError:
        return _extract_python_regex(file_path, content)

    units: List[CodeUnit] = []

    # Build parent map in a single pass to avoid O(n²) class lookups
    parent_map: dict = {}
    for cls_node in ast.walk(tree):
        if isinstance(cls_node, ast.ClassDef):
            for child in ast.iter_child_nodes(cls_node):
                parent_map[id(child)] = cls_node.name

    for node in ast.walk(tree):
        if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)):
            parent_class = parent_map.get(id(node))
            name = f"{parent_class}.{node.name}" if parent_class else node.name
            unit_type = (
                "async_function"
                if isinstance(node, ast.AsyncFunctionDef)
                else "function"
            )
            end = getattr(node, "end_lineno", None) or node.lineno
            units.append(CodeUnit(file_path, name, unit_type, node.lineno, end))

        elif isinstance(node, ast.ClassDef):
            end = getattr(node, "end_lineno", None) or node.lineno
            units.append(CodeUnit(file_path, node.name, "class", node.lineno, end))

    return units


def _extract_python_regex(file_path: str, content: str) -> List[CodeUnit]:
    """Regex fallback for Python files that fail ``ast.parse``."""
    units: List[CodeUnit] = []
    lines = content.splitlines()
    current_class: Optional[str] = None
    current_class_indent: int = -1

    for idx, line in enumerate(lines):
        lineno = idx + 1

        cls_m = _PY_CLASS_RE.match(line)
        if cls_m:
            indent_len = len(cls_m.group("indent"))
            name = cls_m.group("name")
            current_class = name
            current_class_indent = indent_len
            # Estimate end: scan forward for next line at same or lesser indent
            end = _py_block_end(lines, idx, indent_len)
            units.append(CodeUnit(file_path, name, "class", lineno, end))
            continue

        fn_m = _PY_FUNC_RE.match(line)
        if fn_m:
            indent_len = len(fn_m.group("indent"))
            name = fn_m.group("name")
            is_async = "async" in line.split("def")[0]

            # Check if this function is nested inside the current class
            if current_class and indent_len > current_class_indent:
                name = f"{current_class}.{name}"
            else:
                current_class = None
                current_class_indent = -1

            unit_type = "async_function" if is_async else "function"
            end = _py_block_end(lines, idx, indent_len)
            units.append(CodeUnit(file_path, name, unit_type, lineno, end))

    return units


def _py_block_end(lines: List[str], start_idx: int, indent: int) -> int:
    """Estimate the end line of a Python block starting at *start_idx*."""
    for i in range(start_idx + 1, len(lines)):
        stripped = lines[i].strip()
        if not stripped or stripped.startswith("#"):
            continue
        line_indent = len(lines[i]) - len(lines[i].lstrip())
        if line_indent <= indent:
            return i  # 1-based line number of last line in block (may include trailing blank)
    return len(lines)


# ---------------------------------------------------------------------------
# JavaScript / TypeScript extraction
# ---------------------------------------------------------------------------

_JS_NAMED_FUNC_RE = re.compile(
    r"^\s*(?:export\s+)?(?:async\s+)?function\s+(?P<name>[A-Za-z_$]\w*)\s*\(",
)
_JS_CLASS_RE = re.compile(
    r"^\s*(?:export\s+)?class\s+(?P<name>[A-Za-z_$]\w*)",
)
_JS_ARROW_RE = re.compile(
    r"^\s*(?:export\s+)?(?:const|let|var)\s+(?P<name>[A-Za-z_$]\w*)"
    r"\s*=\s*(?:async\s+)?(?:\([^)]*\)|[A-Za-z_$]\w*)\s*=>",
)


def _extract_brace_lang(
    file_path: str, content: str, patterns: list,
) -> List[CodeUnit]:
    """Extract code units from a brace-delimited language.

    Args:
        patterns: list of (compiled_regex, unit_type) tuples.
                  unit_type is a string, OR a callable(match) -> (name, type).
    """
    units: List[CodeUnit] = []
    lines = content.splitlines()

    for idx, line in enumerate(lines):
        lineno = idx + 1
        for regex, unit_type in patterns:
            m = regex.match(line)
            if m:
                end = _find_block_end(lines, idx)
                if callable(unit_type):
                    name, utype = unit_type(m)
                else:
                    name = m.group("name")
                    utype = unit_type
                units.append(CodeUnit(file_path, name, utype, lineno, end))
                break

    return units


# ---------------------------------------------------------------------------
# Go extraction
# ---------------------------------------------------------------------------

_GO_FUNC_RE = re.compile(
    r"^\s*func\s+(?:\(\s*\w+\s+\*?\w+\s*\)\s+)?(?P<name>[A-Za-z_]\w*)\s*\(",
)
_GO_TYPE_RE = re.compile(
    r"^\s*type\s+(?P<name>[A-Za-z_]\w*)\s+(?P<kind>struct|interface)\b",
)

# ---------------------------------------------------------------------------
# Rust extraction
# ---------------------------------------------------------------------------

_RS_FN_RE = re.compile(
    r"^\s*(?:pub(?:\s*\(\s*\w+\s*\))?\s+)?(?:async\s+)?(?:unsafe\s+)?fn\s+(?P<name>[A-Za-z_]\w*)\s*[<(]",
)
_RS_STRUCT_RE = re.compile(
    r"^\s*(?:pub(?:\s*\(\s*\w+\s*\))?\s+)?struct\s+(?P<name>[A-Za-z_]\w*)",
)
_RS_ENUM_RE = re.compile(
    r"^\s*(?:pub(?:\s*\(\s*\w+\s*\))?\s+)?enum\s+(?P<name>[A-Za-z_]\w*)",
)
_RS_IMPL_RE = re.compile(
    r"^\s*impl(?:\s*<[^>]*>)?\s+"
    r"(?:[A-Za-z_]\w*(?:\s*<[^>]*>)?\s+for\s+)?"
    r"(?P<name>[A-Za-z_]\w*(?:\s*<[^>]*>)?)",
)

# ---------------------------------------------------------------------------
# Per-language pattern tables
# ---------------------------------------------------------------------------

_JS_TS_PATTERNS = [
    (_JS_NAMED_FUNC_RE, "function"),
    (_JS_CLASS_RE, "class"),
    (_JS_ARROW_RE, "function"),
]

_GO_PATTERNS = [
    (_GO_FUNC_RE, "function"),
    (_GO_TYPE_RE, lambda m: (m.group("name"), m.group("kind"))),
]

_RS_PATTERNS = [
    (_RS_FN_RE, "function"),
    (_RS_STRUCT_RE, "struct"),
    (_RS_ENUM_RE, "enum"),
    (_RS_IMPL_RE, lambda m: (m.group("name"), "impl")),
]


def _extract_js_ts(file_path: str, content: str) -> List[CodeUnit]:
    """Extract code units from JavaScript or TypeScript source."""
    return _extract_brace_lang(file_path, content, _JS_TS_PATTERNS)


def _extract_go(file_path: str, content: str) -> List[CodeUnit]:
    """Extract code units from Go source."""
    return _extract_brace_lang(file_path, content, _GO_PATTERNS)


def _extract_rust(file_path: str, content: str) -> List[CodeUnit]:
    """Extract code units from Rust source."""
    return _extract_brace_lang(file_path, content, _RS_PATTERNS)


# ---------------------------------------------------------------------------
# Dispatcher
# ---------------------------------------------------------------------------

_EXTRACTORS = {
    "python": _extract_python_ast,
    "javascript": _extract_js_ts,
    "typescript": _extract_js_ts,
    "go": _extract_go,
    "rust": _extract_rust,
}


def extract_code_units(file_path: str, content: str) -> List[CodeUnit]:
    """Extract code units from *content* using the appropriate language parser.

    Dispatches to a language-specific extractor based on the file extension.
    Returns an empty list for unsupported languages.
    """
    lang = detect_language(file_path)
    if lang is None:
        return []
    return _EXTRACTORS[lang](file_path, content)
