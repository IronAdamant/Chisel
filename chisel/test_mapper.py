"""Test file discovery, parsing, dependency extraction, and edge building."""

import ast
import os
import re
from pathlib import Path

from chisel.ast_utils import (
    CodeUnit, _SKIP_DIRS, compute_file_hash, detect_language, extract_code_units,
)
from chisel.project import normalize_path

# Framework detection patterns: (regex pattern, framework name)
_FRAMEWORK_PATTERNS = [
    # Python / pytest
    (re.compile(r"^test_.*\.py$"), "pytest"),
    (re.compile(r"^.*_test\.py$"), "pytest"),
    # JavaScript / Jest
    (re.compile(r"^.*\.test\.[jt]sx?$"), "jest"),
    (re.compile(r"^.*\.spec\.[jt]sx?$"), "playwright"),
    # Go
    (re.compile(r"^.*_test\.go$"), "go"),
    # C# (xUnit, NUnit, MSTest)
    (re.compile(r"^.*Tests?\.cs$"), "csharp_test"),
    # Java (JUnit)
    (re.compile(r"^.*Test\.java$"), "junit"),
    (re.compile(r"^Test.*\.java$"), "junit"),
    # Kotlin (JUnit)
    (re.compile(r"^.*Test\.kt$"), "junit"),
    # Swift (XCTest)
    (re.compile(r"^.*Tests?\.swift$"), "xctest"),
    # PHP (PHPUnit)
    (re.compile(r"^.*Test\.php$"), "phpunit"),
    # Ruby (RSpec / Minitest)
    (re.compile(r"^.*_spec\.rb$"), "rspec"),
    (re.compile(r"^test_.*\.rb$"), "minitest"),
    # Dart
    (re.compile(r"^.*_test\.dart$"), "dart_test"),
]


class TestMapper:
    """Discovers test files, parses them, extracts dependencies, builds edges."""

    __test__ = False  # prevent pytest from collecting this class

    def __init__(self, project_dir):
        self.project_dir = str(project_dir)

    # ------------------------------------------------------------------ #
    # Framework detection
    # ------------------------------------------------------------------ #

    @staticmethod
    def detect_framework(file_path):
        """Detect the test framework for a file based on its name and content.

        Returns a framework string or None if the file is not a test file.
        """
        name = os.path.basename(file_path)

        for pattern, framework in _FRAMEWORK_PATTERNS:
            if pattern.match(name):
                # Playwright .spec files override Jest
                if framework == "playwright":
                    return _check_playwright(file_path)
                return framework
        return None

    # ------------------------------------------------------------------ #
    # Test file discovery
    # ------------------------------------------------------------------ #

    def discover_test_files(self):
        """Walk the project tree and return paths to all detected test files."""
        test_files = []
        for root, dirs, files in os.walk(self.project_dir):
            dirs[:] = [d for d in dirs if d not in _SKIP_DIRS]
            for fname in files:
                fpath = os.path.join(root, fname)
                fw = self.detect_framework(fpath)
                if fw is not None:
                    test_files.append(fpath)
                elif fname.endswith(".rs"):
                    if _check_rust_test(fpath):
                        test_files.append(fpath)
                elif fname.endswith((".cpp", ".cc", ".cxx", ".c")):
                    if _check_cpp_test(fpath):
                        test_files.append(fpath)
        return sorted(test_files)

    # ------------------------------------------------------------------ #
    # Test file parsing
    # ------------------------------------------------------------------ #

    def parse_test_file(self, file_path):
        """Parse a test file into a list of TestUnit dicts.

        Each dict: id, file_path, name, framework, line_start, line_end, content_hash
        """
        framework = self.detect_framework(file_path)

        content = _read_file(file_path)
        if content is None:
            return []

        if framework is None and file_path.endswith(".rs"):
            if _check_rust_test_content(content):
                framework = "rust"
        elif framework is None and file_path.endswith((".cpp", ".cc", ".cxx", ".c")):
            if _check_cpp_test_content(content):
                framework = "gtest"
        if framework is None:
            return []

        rel_path = normalize_path(file_path, self.project_dir)
        file_hash = compute_file_hash(file_path)
        units = extract_code_units(file_path, content)

        test_units = []
        for unit in units:
            if _is_test_name(unit.name, framework):
                tid = f"{rel_path}:{unit.name}"
                test_units.append({
                    "id": tid,
                    "file_path": rel_path,
                    "name": unit.name,
                    "framework": framework,
                    "line_start": unit.line_start,
                    "line_end": unit.line_end,
                    "content_hash": file_hash,
                })

        return test_units

    # ------------------------------------------------------------------ #
    # Dependency extraction
    # ------------------------------------------------------------------ #

    @staticmethod
    def extract_test_dependencies(file_path, content):
        """Extract imports and call targets from a test file.

        Returns a list of dependency dicts: {name, dep_type}
        where dep_type is "import" or "call".
        """
        lang = detect_language(file_path)
        extractor = _DEP_EXTRACTORS.get(lang)
        return extractor(content) if extractor else []

    # ------------------------------------------------------------------ #
    # Edge building
    # ------------------------------------------------------------------ #

    def build_test_edges(self, test_units, code_units):
        """Match test dependencies to known code units, producing edges.

        Args:
            test_units: List of TestUnit dicts (from parse_test_file).
            code_units: List of CodeUnit objects or dicts with at least
                        id, file_path, name.

        Returns:
            List of edge dicts: {test_id, code_id, edge_type, weight}
        """
        # Build lookup: name -> list of code unit ids
        name_to_ids = {}
        id_to_file = {}
        for cu in code_units:
            if isinstance(cu, CodeUnit):
                name = cu.name
                cid = f"{cu.file_path}:{cu.name}:{cu.unit_type}"
                cfile = cu.file_path
            else:
                name = cu["name"]
                cid = cu["id"]
                cfile = cu.get("file_path", "")
            name_to_ids.setdefault(name, []).append(cid)
            id_to_file[cid] = cfile

        edges = []
        file_cache = {}
        for tu in test_units:
            file_path = tu["file_path"]
            if file_path not in file_cache:
                try:
                    file_cache[file_path] = Path(
                        os.path.join(self.project_dir, file_path)
                    ).read_text(encoding="utf-8", errors="replace")
                except OSError:
                    file_cache[file_path] = None
            content = file_cache[file_path]
            if content is None:
                continue

            deps = self.extract_test_dependencies(file_path, content)
            seen = set()
            for dep in deps:
                module_path = dep.get("module_path")
                # Try import-path matching for Python deps with module_path
                matched_ids = []
                if module_path:
                    for cid, cfile in id_to_file.items():
                        if _matches_import_path(cfile, module_path):
                            # Only match if the code unit name also matches
                            if cid in name_to_ids.get(dep["name"], []):
                                matched_ids.append(cid)
                # Fall back to name-based matching
                if not matched_ids:
                    matched_ids = name_to_ids.get(dep["name"], [])

                for cid in matched_ids:
                    key = (tu["id"], cid, dep["dep_type"])
                    if key not in seen:
                        seen.add(key)
                        code_file = id_to_file.get(cid, "")
                        proximity = _compute_proximity_weight(
                            file_path, code_file,
                        )
                        edges.append({
                            "test_id": tu["id"],
                            "code_id": cid,
                            "edge_type": dep["dep_type"],
                            "weight": proximity,
                        })
        return edges


# ------------------------------------------------------------------ #
# Internal helpers
# ------------------------------------------------------------------ #

def _is_test_name(name, framework):
    """Check if a code unit name looks like a test."""
    # Strip class prefix for methods like MyClass.test_foo
    base = name.rsplit(".", 1)[-1]
    if framework == "pytest":
        return base.startswith("test_") or base.startswith("Test")
    if framework in ("jest", "playwright"):
        return base in ("describe", "it", "test") or base.startswith("test")
    if framework == "go":
        return base.startswith("Test") or base.startswith("Benchmark")
    if framework == "rust":
        return base.startswith("test_")
    if framework in ("csharp_test", "gtest"):
        return True
    if framework in ("junit", "xctest", "minitest", "phpunit", "dart_test"):
        return base.startswith("test") or base.startswith("Test")
    if framework == "rspec":
        return base in ("describe", "it", "context") or base.startswith("test")
    return False


def _read_file(file_path):
    """Read a file's text content, returning None on failure."""
    try:
        return Path(file_path).read_text(encoding="utf-8", errors="replace")
    except OSError:
        return None


def _check_playwright(file_path):
    """Check if a .spec.ts/.spec.js file actually uses Playwright."""
    content = _read_file(file_path)
    if content is None:
        return "jest"
    if "playwright" in content or "@playwright" in content:
        return "playwright"
    return "jest"


def _check_rust_test_content(content):
    """Check if content contains Rust test markers."""
    return "#[test]" in content or "#[cfg(test)]" in content


def _check_rust_test(file_path):
    """Check if a .rs file contains #[test] or #[cfg(test)]."""
    content = _read_file(file_path)
    return content is not None and _check_rust_test_content(content)


def _check_cpp_test_content(content):
    """Check if content contains C/C++ test framework macros."""
    return ("TEST(" in content or "TEST_F(" in content
            or "TEST_CASE(" in content or "BOOST_AUTO_TEST_CASE(" in content)


def _check_cpp_test(file_path):
    """Check if a C/C++ file contains test framework macros."""
    content = _read_file(file_path)
    return content is not None and _check_cpp_test_content(content)


# ------------------------------------------------------------------ #
# Python dependency extraction
# ------------------------------------------------------------------ #

_PY_KEYWORDS = frozenset({
    "if", "for", "while", "with", "return", "print",
    "class", "def", "import", "from", "raise", "assert",
    "del", "yield", "lambda", "elif", "except", "async",
    "await", "not", "and", "or", "in", "is", "pass",
    "break", "continue", "try", "finally", "global",
    "nonlocal",
})


def _extract_python_deps(content):
    """Extract imports and function calls from Python test content."""
    deps = []
    try:
        tree = ast.parse(content)
    except SyntaxError:
        return _extract_python_deps_regex(content)

    for node in ast.walk(tree):
        if isinstance(node, ast.Import):
            for alias in node.names:
                deps.append({
                    "name": alias.name.split(".")[-1],
                    "dep_type": "import",
                    "module_path": alias.name,
                })
        elif isinstance(node, ast.ImportFrom):
            if node.module:
                for alias in node.names:
                    deps.append({
                        "name": alias.name,
                        "dep_type": "import",
                        "module_path": node.module,
                    })
        elif isinstance(node, ast.Call):
            name = _get_call_name(node)
            if name:
                deps.append({"name": name, "dep_type": "call", "module_path": None})

    return _dedupe_deps(deps)


def _get_call_name(node):
    """Extract the function name from a Call AST node."""
    if isinstance(node.func, ast.Name):
        return node.func.id
    if isinstance(node.func, ast.Attribute):
        return node.func.attr
    return None


def _extract_python_deps_regex(content):
    """Regex fallback for Python dependency extraction."""
    deps = []
    for m in re.finditer(r"^(?:from\s+(\S+)\s+)?import\s+(\w+)", content, re.MULTILINE):
        module = m.group(1)  # from X import Y -> X; import Y -> None
        name = m.group(2)
        deps.append({
            "name": name,
            "dep_type": "import",
            "module_path": module if module else name,
        })
    for m in re.finditer(r"(\w+)\s*\(", content):
        name = m.group(1)
        if name not in _PY_KEYWORDS:
            deps.append({"name": name, "dep_type": "call", "module_path": None})
    return _dedupe_deps(deps)


# ------------------------------------------------------------------ #
# JS/TS dependency extraction
# ------------------------------------------------------------------ #

_JS_KEYWORDS = frozenset({
    "if", "for", "while", "switch", "import", "require",
    "describe", "it", "test", "expect", "beforeEach",
    "afterEach", "beforeAll", "afterAll",
})

_JS_IMPORT_RE = re.compile(
    r"""(?:import\s+(?:\{[^}]*\}|\w+).*?from\s+['"]([^'"]+)['"]|"""
    r"""require\s*\(\s*['"]([^'"]+)['"]\s*\))""",
    re.MULTILINE,
)
_JS_CALL_RE = re.compile(r"(\w+)\s*\(")
_JS_NAMED_IMPORT_RE = re.compile(r"import\s+\{([^}]+)\}")


def _extract_js_deps(content):
    """Extract imports and calls from JS/TS content."""
    deps = []
    for m in _JS_IMPORT_RE.finditer(content):
        mod = m.group(1) or m.group(2)
        name = mod.rsplit("/", 1)[-1].split(".")[0]
        deps.append({"name": name, "dep_type": "import"})

    # Named imports: import { foo, bar } from ...
    for m in _JS_NAMED_IMPORT_RE.finditer(content):
        for name in m.group(1).split(","):
            name = name.strip().split(" as ")[0].strip()
            if name:
                deps.append({"name": name, "dep_type": "import"})

    for m in _JS_CALL_RE.finditer(content):
        name = m.group(1)
        if name not in _JS_KEYWORDS:
            deps.append({"name": name, "dep_type": "call"})
    return _dedupe_deps(deps)


# ------------------------------------------------------------------ #
# Go dependency extraction
# ------------------------------------------------------------------ #

_GO_IMPORT_BLOCK_RE = re.compile(r"import\s*\((.*?)\)", re.DOTALL)
_GO_IMPORT_SINGLE_RE = re.compile(r'^import\s+"([^"]+)"', re.MULTILINE)


_GO_IMPORT_LINE_RE = re.compile(r'"([^"]+)"')


def _extract_go_deps(content):
    """Extract imports from Go content."""
    deps = []
    for m in _GO_IMPORT_BLOCK_RE.finditer(content):
        block = m.group(1)
        for line in block.strip().split("\n"):
            im = _GO_IMPORT_LINE_RE.search(line)
            if im:
                pkg = im.group(1).rsplit("/", 1)[-1]
                deps.append({"name": pkg, "dep_type": "import"})

    for m in _GO_IMPORT_SINGLE_RE.finditer(content):
        pkg = m.group(1).rsplit("/", 1)[-1]
        deps.append({"name": pkg, "dep_type": "import"})

    return _dedupe_deps(deps)


# ------------------------------------------------------------------ #
# Rust dependency extraction
# ------------------------------------------------------------------ #

_RS_USE_RE = re.compile(r"^\s*use\s+([\w:]+)(?:::\{([^}]+)\})?;", re.MULTILINE)


def _extract_rust_deps(content):
    """Extract use statements from Rust content."""
    deps = []
    for m in _RS_USE_RE.finditer(content):
        path = m.group(1)
        names_block = m.group(2)
        if names_block:
            for name in names_block.split(","):
                name = name.strip()
                if name and name != "self":
                    deps.append({"name": name, "dep_type": "import"})
        else:
            name = path.rsplit("::", 1)[-1]
            deps.append({"name": name, "dep_type": "import"})
    return _dedupe_deps(deps)


# ------------------------------------------------------------------ #
# C# dependency extraction
# ------------------------------------------------------------------ #

_CS_USING_RE = re.compile(r"^\s*using\s+(?:static\s+)?([A-Za-z_][\w.]*)\s*;", re.MULTILINE)


def _extract_csharp_deps(content):
    """Extract using statements from C# content."""
    deps = []
    for m in _CS_USING_RE.finditer(content):
        name = m.group(1).rsplit(".", 1)[-1]
        deps.append({"name": name, "dep_type": "import"})
    return _dedupe_deps(deps)


# ------------------------------------------------------------------ #
# Java / Kotlin dependency extraction
# ------------------------------------------------------------------ #

_JAVA_IMPORT_RE = re.compile(r"^\s*import\s+(?:static\s+)?([A-Za-z_][\w.]*)\s*;?", re.MULTILINE)


def _extract_java_deps(content):
    """Extract import statements from Java/Kotlin content."""
    deps = []
    for m in _JAVA_IMPORT_RE.finditer(content):
        name = m.group(1).rsplit(".", 1)[-1]
        if name != "*":
            deps.append({"name": name, "dep_type": "import"})
    return _dedupe_deps(deps)


# ------------------------------------------------------------------ #
# C / C++ dependency extraction
# ------------------------------------------------------------------ #

_CPP_INCLUDE_RE = re.compile(r'^\s*#\s*include\s*[<"]([^>"]+)[>"]', re.MULTILINE)


def _extract_cpp_deps(content):
    """Extract #include directives from C/C++ content."""
    deps = []
    for m in _CPP_INCLUDE_RE.finditer(content):
        header = m.group(1)
        # Extract base name: "mylib/utils.h" -> "utils"
        name = header.rsplit("/", 1)[-1].split(".")[0]
        deps.append({"name": name, "dep_type": "import"})
    return _dedupe_deps(deps)


# ------------------------------------------------------------------ #
# Swift dependency extraction
# ------------------------------------------------------------------ #

_SWIFT_IMPORT_RE = re.compile(r"^\s*import\s+(\w+)", re.MULTILINE)


def _extract_swift_deps(content):
    """Extract import statements from Swift content."""
    deps = []
    for m in _SWIFT_IMPORT_RE.finditer(content):
        deps.append({"name": m.group(1), "dep_type": "import"})
    return _dedupe_deps(deps)


# ------------------------------------------------------------------ #
# PHP dependency extraction
# ------------------------------------------------------------------ #

_PHP_USE_RE = re.compile(r"^\s*use\s+([A-Za-z_][\w\\]*)\s*;", re.MULTILINE)
_PHP_REQUIRE_RE = re.compile(
    r"(?:require|require_once|include|include_once)\s*\(?\s*['\"]([^'\"]+)['\"]\s*\)?",
    re.MULTILINE,
)


def _extract_php_deps(content):
    """Extract use statements and require/include from PHP content."""
    deps = []
    for m in _PHP_USE_RE.finditer(content):
        name = m.group(1).rsplit("\\", 1)[-1]
        deps.append({"name": name, "dep_type": "import"})
    for m in _PHP_REQUIRE_RE.finditer(content):
        path = m.group(1)
        name = path.rsplit("/", 1)[-1].split(".")[0]
        deps.append({"name": name, "dep_type": "import"})
    return _dedupe_deps(deps)


# ------------------------------------------------------------------ #
# Ruby dependency extraction
# ------------------------------------------------------------------ #

_RB_REQUIRE_RE = re.compile(r"^\s*require(?:_relative)?\s+['\"]([^'\"]+)['\"]", re.MULTILINE)


def _extract_ruby_deps(content):
    """Extract require/require_relative from Ruby content."""
    deps = []
    for m in _RB_REQUIRE_RE.finditer(content):
        path = m.group(1)
        name = path.rsplit("/", 1)[-1]
        deps.append({"name": name, "dep_type": "import"})
    return _dedupe_deps(deps)


# ------------------------------------------------------------------ #
# Dart dependency extraction
# ------------------------------------------------------------------ #

_DART_IMPORT_RE = re.compile(r"^\s*import\s+['\"]([^'\"]+)['\"]", re.MULTILINE)


def _extract_dart_deps(content):
    """Extract import statements from Dart content."""
    deps = []
    for m in _DART_IMPORT_RE.finditer(content):
        path = m.group(1)
        # "package:myapp/utils.dart" -> "utils"
        name = path.rsplit("/", 1)[-1].split(".")[0]
        deps.append({"name": name, "dep_type": "import"})
    return _dedupe_deps(deps)


# ------------------------------------------------------------------ #
# Shared utility
# ------------------------------------------------------------------ #

_DEP_EXTRACTORS = {
    "python": _extract_python_deps,
    "javascript": _extract_js_deps,
    "typescript": _extract_js_deps,
    "go": _extract_go_deps,
    "rust": _extract_rust_deps,
    "csharp": _extract_csharp_deps,
    "java": _extract_java_deps,
    "kotlin": _extract_java_deps,
    "c": _extract_cpp_deps,
    "cpp": _extract_cpp_deps,
    "swift": _extract_swift_deps,
    "php": _extract_php_deps,
    "ruby": _extract_ruby_deps,
    "dart": _extract_dart_deps,
}


def _compute_proximity_weight(test_path, code_path):
    """Compute edge weight based on file-path proximity (0.4-1.0).

    Same directory: 1.0, sibling dirs: 0.8, shared ancestor: 0.6, distant: 0.4.
    """
    test_parts = test_path.replace("\\", "/").split("/")[:-1]  # dir parts only
    code_parts = code_path.replace("\\", "/").split("/")[:-1]
    common = 0
    for t, c in zip(test_parts, code_parts):
        if t == c:
            common += 1
        else:
            break
    if not test_parts and not code_parts:
        return 1.0  # both at root
    max_depth = max(len(test_parts), len(code_parts), 1)
    if common == len(test_parts) == len(code_parts):
        return 1.0  # same directory
    ratio = common / max_depth
    if ratio >= 0.5:
        return 0.8
    if common >= 1:
        return 0.6
    return 0.4


def _matches_import_path(code_file_path, module_path):
    """Check if a code file path matches a Python module path.

    'myapp.utils' matches 'myapp/utils.py' or 'src/myapp/utils.py'.
    """
    if not module_path:
        return False
    module_as_path = module_path.replace(".", "/") + ".py"
    normalized = code_file_path.replace("\\", "/")
    return normalized == module_as_path or normalized.endswith("/" + module_as_path)


def _dedupe_deps(deps):
    """Remove duplicate dependencies, keeping first occurrence."""
    seen = set()
    result = []
    for dep in deps:
        key = (dep["name"], dep["dep_type"])
        if key not in seen:
            seen.add(key)
            result.append(dep)
    return result
