"""Test file discovery, parsing, dependency extraction, and edge building."""

import ast
import os
import re
from pathlib import Path

from chisel.ast_utils import (
    CodeUnit, _SKIP_DIRS, compute_file_hash, detect_language, extract_code_units,
)

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
        return sorted(test_files)

    # ------------------------------------------------------------------ #
    # Test file parsing
    # ------------------------------------------------------------------ #

    def parse_test_file(self, file_path):
        """Parse a test file into a list of TestUnit dicts.

        Each dict: id, file_path, name, framework, line_start, line_end, content_hash
        """
        framework = self.detect_framework(file_path)

        try:
            content = Path(file_path).read_text(encoding="utf-8", errors="replace")
        except OSError:
            return []

        if framework is None and file_path.endswith(".rs"):
            if "#[test]" in content or "#[cfg(test)]" in content:
                framework = "rust"
        if framework is None:
            return []

        rel_path = os.path.relpath(file_path, self.project_dir)
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

    def extract_test_dependencies(self, file_path, content):
        """Extract imports and call targets from a test file.

        Returns a list of dependency dicts: {name, dep_type}
        where dep_type is "import" or "call".
        """
        lang = detect_language(file_path)
        if lang == "python":
            return _extract_python_deps(content)
        if lang in ("javascript", "typescript"):
            return _extract_js_deps(content)
        if lang == "go":
            return _extract_go_deps(content)
        if lang == "rust":
            return _extract_rust_deps(content)
        return []

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
        for cu in code_units:
            if isinstance(cu, CodeUnit):
                name = cu.name
                cid = f"{cu.file_path}:{cu.name}:{cu.unit_type}"
            else:
                name = cu["name"]
                cid = cu["id"]
            name_to_ids.setdefault(name, []).append(cid)

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
                for cid in name_to_ids.get(dep["name"], []):
                    key = (tu["id"], cid, dep["dep_type"])
                    if key not in seen:
                        seen.add(key)
                        edges.append({
                            "test_id": tu["id"],
                            "code_id": cid,
                            "edge_type": dep["dep_type"],
                            "weight": 1.0,
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
    return False


def _check_playwright(file_path):
    """Check if a .spec.ts/.spec.js file actually uses Playwright."""
    try:
        content = Path(file_path).read_text(encoding="utf-8", errors="replace")
    except OSError:
        return "jest"
    if "playwright" in content or "@playwright" in content:
        return "playwright"
    return "jest"


def _check_rust_test(file_path):
    """Check if a .rs file contains #[test] or #[cfg(test)]."""
    try:
        content = Path(file_path).read_text(encoding="utf-8", errors="replace")
    except OSError:
        return False
    return "#[test]" in content or "#[cfg(test)]" in content


# ------------------------------------------------------------------ #
# Python dependency extraction
# ------------------------------------------------------------------ #

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
                deps.append({"name": alias.name.split(".")[-1], "dep_type": "import"})
        elif isinstance(node, ast.ImportFrom):
            if node.module:
                for alias in node.names:
                    deps.append({"name": alias.name, "dep_type": "import"})
        elif isinstance(node, ast.Call):
            name = _get_call_name(node)
            if name:
                deps.append({"name": name, "dep_type": "call"})

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
    for m in re.finditer(r"^(?:from\s+\S+\s+)?import\s+(\w+)", content, re.MULTILINE):
        deps.append({"name": m.group(1), "dep_type": "import"})
    for m in re.finditer(r"(\w+)\s*\(", content):
        name = m.group(1)
        if name not in ("if", "for", "while", "with", "return", "print",
                         "class", "def", "import", "from", "raise", "assert",
                         "del", "yield", "lambda", "elif", "except", "async",
                         "await", "not", "and", "or", "in", "is", "pass",
                         "break", "continue", "try", "finally", "global",
                         "nonlocal"):
            deps.append({"name": name, "dep_type": "call"})
    return _dedupe_deps(deps)


# ------------------------------------------------------------------ #
# JS/TS dependency extraction
# ------------------------------------------------------------------ #

_JS_IMPORT_RE = re.compile(
    r"""(?:import\s+(?:\{[^}]*\}|\w+).*?from\s+['"]([^'"]+)['"]|"""
    r"""require\s*\(\s*['"]([^'"]+)['"]\s*\))""",
    re.MULTILINE,
)
_JS_CALL_RE = re.compile(r"(\w+)\s*\(")


def _extract_js_deps(content):
    """Extract imports and calls from JS/TS content."""
    deps = []
    for m in _JS_IMPORT_RE.finditer(content):
        mod = m.group(1) or m.group(2)
        name = mod.rsplit("/", 1)[-1].split(".")[0]
        deps.append({"name": name, "dep_type": "import"})

    # Named imports: import { foo, bar } from ...
    for m in re.finditer(r"import\s+\{([^}]+)\}", content):
        for name in m.group(1).split(","):
            name = name.strip().split(" as ")[0].strip()
            if name:
                deps.append({"name": name, "dep_type": "import"})

    for m in _JS_CALL_RE.finditer(content):
        name = m.group(1)
        if name not in ("if", "for", "while", "switch", "import", "require",
                         "describe", "it", "test", "expect", "beforeEach",
                         "afterEach", "beforeAll", "afterAll"):
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
