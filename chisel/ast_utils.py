"""Multi-language AST extraction for Chisel.

Extracts code units (functions, classes, structs, etc.) from source files
across Python, JavaScript/TypeScript, Go, Rust, C#, Java, C/C++, Kotlin,
Swift, PHP, Ruby, and Dart. Fully self-contained with zero external
dependencies beyond the Python standard library.
"""

import ast
import hashlib
import re
from dataclasses import dataclass
from functools import partial
from pathlib import Path
from typing import List, Optional

# Directories to always skip when walking the project tree.
_SKIP_DIRS = {
    ".git", "node_modules", "__pycache__", ".tox", ".venv", "venv",
    "env", ".mypy_cache", ".pytest_cache", ".ruff_cache", "dist",
    "build", ".eggs", "target", "vendor", "Pods",
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
    # Python
    ".py": "python", ".pyw": "python",
    # JavaScript / TypeScript
    ".js": "javascript", ".jsx": "javascript", ".mjs": "javascript", ".cjs": "javascript",
    ".ts": "typescript", ".tsx": "typescript",
    # Go
    ".go": "go",
    # Rust
    ".rs": "rust",
    # C#
    ".cs": "csharp",
    # Java
    ".java": "java",
    # C / C++
    ".c": "c", ".h": "c",
    ".cc": "cpp", ".cpp": "cpp", ".cxx": "cpp", ".hpp": "cpp", ".hxx": "cpp",
    # Kotlin
    ".kt": "kotlin", ".kts": "kotlin",
    # Swift
    ".swift": "swift",
    # PHP
    ".php": "php",
    # Ruby
    ".rb": "ruby",
    # Dart
    ".dart": "dart",
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
# Brace-matching helper (shared by all brace-delimited languages)
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

    if found_open:
        return len(lines)
    return start_idx + 1


def _strip_strings_and_comments(line: str) -> str:
    """Remove string literals and trailing ``//`` comments from a single line."""
    result: list = []
    i = 0
    length = len(line)
    while i < length:
        ch = line[i]
        if ch == "/" and i + 1 < length and line[i + 1] == "/":
            break
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
            end = _py_block_end(lines, idx, indent_len)
            units.append(CodeUnit(file_path, name, "class", lineno, end))
            continue

        fn_m = _PY_FUNC_RE.match(line)
        if fn_m:
            indent_len = len(fn_m.group("indent"))
            name = fn_m.group("name")
            is_async = "async" in line.split("def")[0]

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
            return i
    return len(lines)


# ---------------------------------------------------------------------------
# JavaScript / TypeScript
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

# ---------------------------------------------------------------------------
# Go
# ---------------------------------------------------------------------------

_GO_FUNC_RE = re.compile(
    r"^\s*func\s+(?:\(\s*\w+\s+\*?\w+\s*\)\s+)?(?P<name>[A-Za-z_]\w*)\s*\(",
)
_GO_TYPE_RE = re.compile(
    r"^\s*type\s+(?P<name>[A-Za-z_]\w*)\s+(?P<kind>struct|interface)\b",
)

# ---------------------------------------------------------------------------
# Rust
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
# C#
# ---------------------------------------------------------------------------

_CS_CLASS_RE = re.compile(
    r"^\s*(?:(?:public|private|protected|internal)\s+)?"
    r"(?:(?:static|abstract|sealed|partial)\s+)*"
    r"(?P<kind>class|struct|interface|enum|record)\s+(?P<name>[A-Za-z_]\w*)",
)
_CS_METHOD_RE = re.compile(
    r"^\s*(?:(?:public|private|protected|internal)\s+)?"
    r"(?:(?:static|virtual|override|abstract|async|new|partial|extern|sealed|unsafe)\s+)*"
    r"(?:[A-Za-z_]\w*(?:\.[A-Za-z_]\w*)*(?:<[^>]*>)?(?:\[\])*\??\s+)"
    r"(?P<name>[A-Za-z_]\w*)\s*[<(]",
)

# ---------------------------------------------------------------------------
# Java
# ---------------------------------------------------------------------------

_JAVA_CLASS_RE = re.compile(
    r"^\s*(?:(?:public|private|protected)\s+)?"
    r"(?:(?:static|final|abstract|sealed)\s+)*"
    r"(?P<kind>class|interface|enum|record)\s+(?P<name>[A-Za-z_]\w*)",
)
_JAVA_METHOD_RE = re.compile(
    r"^\s*(?:(?:public|private|protected)\s+)?"
    r"(?:(?:static|final|abstract|synchronized|native|default)\s+)*"
    r"(?:[A-Za-z_]\w*(?:<[^>]*>)?(?:\[\])*\s+)"
    r"(?P<name>[A-Za-z_]\w*)\s*\(",
)

# ---------------------------------------------------------------------------
# C / C++
# ---------------------------------------------------------------------------

_CPP_CLASS_RE = re.compile(
    r"^\s*(?:template\s*<[^>]*>\s*)?"
    r"(?P<kind>class|struct|namespace)\s+(?P<name>[A-Za-z_]\w*)",
)
_CPP_ENUM_RE = re.compile(
    r"^\s*enum\s+(?:class\s+)?(?P<name>[A-Za-z_]\w*)",
)
_CPP_FUNC_RE = re.compile(
    r"^\s*(?:(?:static|inline|virtual|explicit|constexpr|extern|friend)\s+)*"
    r"(?:[A-Za-z_]\w*(?:::\w+)*(?:\s*<[^>]*>)?\s*[*&]?\s+)"
    r"(?P<name>[A-Za-z_]\w*(?:::[A-Za-z_]\w*)?)\s*\(",
)

# ---------------------------------------------------------------------------
# Kotlin
# ---------------------------------------------------------------------------

_KT_CLASS_RE = re.compile(
    r"^\s*(?:(?:private|public|internal|protected|open|abstract|sealed|data|enum|inner|value)\s+)*"
    r"(?P<kind>class|object|interface)\s+(?P<name>[A-Za-z_]\w*)",
)
_KT_FUN_RE = re.compile(
    r"^\s*(?:(?:private|public|internal|protected|open|override|suspend|inline|tailrec)\s+)*"
    r"fun\s+(?P<name>[A-Za-z_]\w*)\s*[<(]",
)

# ---------------------------------------------------------------------------
# Swift
# ---------------------------------------------------------------------------

_SWIFT_TYPE_RE = re.compile(
    r"^\s*(?:(?:private|public|internal|fileprivate|open|final)\s+)*"
    r"(?P<kind>class|struct|enum|protocol|actor)\s+(?P<name>[A-Za-z_]\w*)",
)
_SWIFT_FUNC_RE = re.compile(
    r"^\s*(?:(?:private|public|internal|fileprivate|open|static|class|override|mutating|final)\s+)*"
    r"func\s+(?P<name>[A-Za-z_]\w*)\s*[<(]",
)

# ---------------------------------------------------------------------------
# PHP
# ---------------------------------------------------------------------------

_PHP_CLASS_RE = re.compile(
    r"^\s*(?:(?:abstract|final)\s+)?(?P<kind>class|interface|trait|enum)\s+(?P<name>[A-Za-z_]\w*)",
)
_PHP_FUNC_RE = re.compile(
    r"^\s*(?:(?:public|private|protected)\s+)?(?:static\s+)?function\s+(?P<name>[A-Za-z_]\w*)\s*\(",
)

# ---------------------------------------------------------------------------
# Dart
# ---------------------------------------------------------------------------

_DART_CLASS_RE = re.compile(
    r"^\s*(?:abstract\s+)?(?P<kind>class|mixin|extension)\s+(?P<name>[A-Za-z_]\w*)",
)
_DART_FUNC_RE = re.compile(
    r"^\s*(?:(?:static|external)\s+)?"
    r"(?:[A-Za-z_]\w*(?:<[^>]*>)?\??\s+)"
    r"(?P<name>[A-Za-z_]\w*)\s*[<(]",
)

# ---------------------------------------------------------------------------
# Ruby (end-delimited, not brace-delimited)
# ---------------------------------------------------------------------------

_RB_CLASS_RE = re.compile(
    r"^(?P<indent>\s*)(?P<kind>class|module)\s+(?P<name>[A-Za-z_]\w*(?:::[A-Za-z_]\w*)*)",
)
_RB_DEF_RE = re.compile(
    r"^(?P<indent>\s*)def\s+(?:self\.)?(?P<name>[A-Za-z_]\w*[?!=]?)\s*[\(;\n]?",
)


def _ruby_block_end(lines: List[str], start_idx: int, indent: int) -> int:
    """Find the closing ``end`` for a Ruby block at the given indent level."""
    for i in range(start_idx + 1, len(lines)):
        stripped = lines[i].strip()
        if not stripped or stripped.startswith("#"):
            continue
        line_indent = len(lines[i]) - len(lines[i].lstrip())
        if line_indent <= indent and (stripped == "end" or stripped.startswith("end ")):
            return i + 1  # 1-based
    return len(lines)


def _extract_ruby(file_path: str, content: str) -> List[CodeUnit]:
    """Extract code units from Ruby source using keyword-based block detection."""
    units: List[CodeUnit] = []
    lines = content.splitlines()

    for idx, line in enumerate(lines):
        lineno = idx + 1
        m = _RB_CLASS_RE.match(line)
        if m:
            indent = len(m.group("indent"))
            end = _ruby_block_end(lines, idx, indent)
            units.append(CodeUnit(file_path, m.group("name"), m.group("kind"), lineno, end))
            continue
        m = _RB_DEF_RE.match(line)
        if m:
            indent = len(m.group("indent"))
            end = _ruby_block_end(lines, idx, indent)
            units.append(CodeUnit(file_path, m.group("name"), "function", lineno, end))

    return units


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

_CS_PATTERNS = [
    (_CS_CLASS_RE, lambda m: (m.group("name"), m.group("kind"))),
    (_CS_METHOD_RE, "function"),
]

_JAVA_PATTERNS = [
    (_JAVA_CLASS_RE, lambda m: (m.group("name"), m.group("kind"))),
    (_JAVA_METHOD_RE, "function"),
]

_CPP_PATTERNS = [
    (_CPP_CLASS_RE, lambda m: (m.group("name"), m.group("kind"))),
    (_CPP_ENUM_RE, "enum"),
    (_CPP_FUNC_RE, "function"),
]

_KT_PATTERNS = [
    (_KT_CLASS_RE, lambda m: (m.group("name"), m.group("kind"))),
    (_KT_FUN_RE, "function"),
]

_SWIFT_PATTERNS = [
    (_SWIFT_TYPE_RE, lambda m: (m.group("name"), m.group("kind"))),
    (_SWIFT_FUNC_RE, "function"),
]

_PHP_PATTERNS = [
    (_PHP_CLASS_RE, lambda m: (m.group("name"), m.group("kind"))),
    (_PHP_FUNC_RE, "function"),
]

_DART_PATTERNS = [
    (_DART_CLASS_RE, lambda m: (m.group("name"), m.group("kind"))),
    (_DART_FUNC_RE, "function"),
]


# ---------------------------------------------------------------------------
# Dispatcher
# ---------------------------------------------------------------------------

_EXTRACTORS = {
    "python": _extract_python_ast,
    "javascript": partial(_extract_brace_lang, patterns=_JS_TS_PATTERNS),
    "typescript": partial(_extract_brace_lang, patterns=_JS_TS_PATTERNS),
    "go": partial(_extract_brace_lang, patterns=_GO_PATTERNS),
    "rust": partial(_extract_brace_lang, patterns=_RS_PATTERNS),
    "csharp": partial(_extract_brace_lang, patterns=_CS_PATTERNS),
    "java": partial(_extract_brace_lang, patterns=_JAVA_PATTERNS),
    "c": partial(_extract_brace_lang, patterns=_CPP_PATTERNS),
    "cpp": partial(_extract_brace_lang, patterns=_CPP_PATTERNS),
    "kotlin": partial(_extract_brace_lang, patterns=_KT_PATTERNS),
    "swift": partial(_extract_brace_lang, patterns=_SWIFT_PATTERNS),
    "php": partial(_extract_brace_lang, patterns=_PHP_PATTERNS),
    "ruby": _extract_ruby,
    "dart": partial(_extract_brace_lang, patterns=_DART_PATTERNS),
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
