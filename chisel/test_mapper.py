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


# Unit types that are inherently test constructs (from Jest/Mocha/Vitest block extraction).
_TEST_UNIT_TYPES = frozenset({"test_suite", "test_case"})


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

        # For annotation-driven frameworks, collect explicitly annotated names
        annotated = _annotated_test_names(content, framework)

        test_units = []
        for unit in units:
            if unit.unit_type in _TEST_UNIT_TYPES:
                keep = True
            elif framework in ("rust", "junit", "xctest", "swift_test", "csharp_test"):
                bare = unit.name.rsplit(".", 1)[-1]
                keep = bare in annotated or _is_test_name(unit.name, framework)
            else:
                keep = _is_test_name(unit.name, framework)
            if keep:
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

        Custom extractors registered via :func:`register_dep_extractor`
        take priority over built-in ones.
        """
        lang = detect_language(file_path)
        extractor = _custom_dep_extractors.get(lang) or _DEP_EXTRACTORS.get(lang)
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

            # Pre-pass: collect the set of code files that this test file's
            # imports resolve to. Restricting tier-4 (name-only) matches to
            # this scope eliminates symbol-name collisions across unrelated
            # files (e.g. a generic `dispatch()` call leaking into edges of
            # every file that happens to define a `dispatch` symbol).
            resolved_import_files = self._resolve_import_targets(
                file_path, deps, id_to_file,
            )

            seen = set()
            for dep in deps:
                module_path = dep.get("module_path")
                matched_ids = []
                if module_path:
                    # 1. Python import-path matching (module.path + name)
                    for cid, cfile in id_to_file.items():
                        if _matches_import_path(cfile, module_path):
                            # Only match if the code unit name also matches
                            if cid in name_to_ids.get(dep["name"], []):
                                matched_ids.append(cid)
                    # 2. JS/TS path-based matching (resolve relative path,
                    #    match ALL code units in the resolved file)
                    if not matched_ids and module_path.startswith("."):
                        resolved = _resolve_js_module_path(
                            file_path, module_path,
                        )
                        if resolved:
                            for cid, cfile in id_to_file.items():
                                if _matches_js_import_path(cfile, resolved):
                                    matched_ids.append(cid)
                    # 3. Go import-path matching (resolve full module path to dir)
                    if not matched_ids and detect_language(file_path) == "go":
                        resolved = _resolve_go_import_path(
                            self.project_dir, module_path,
                        )
                        if resolved:
                            resolved_norm = resolved.replace("\\", "/")
                            for cid, cfile in id_to_file.items():
                                cfile_norm = cfile.replace("\\", "/")
                                # Match any file inside the resolved directory
                                if resolved_norm == ".":
                                    if "/" not in cfile_norm:
                                        matched_ids.append(cid)
                                elif (
                                    cfile_norm == resolved_norm
                                    or cfile_norm.startswith(resolved_norm + "/")
                                ):
                                    matched_ids.append(cid)
                # 4. Fall back to name-based matching
                if not matched_ids:
                    candidate_ids = name_to_ids.get(dep["name"], [])
                    if not module_path and resolved_import_files and candidate_ids:
                        # Symbol-collision guard: a `call`-style dep without a
                        # module_path is just a bare symbol name. Constrain it
                        # to code units in files this test actually imports.
                        scoped = [
                            cid for cid in candidate_ids
                            if id_to_file.get(cid) in resolved_import_files
                        ]
                        # If scoping eliminates every candidate (e.g. the
                        # symbol is defined in an unimported helper file),
                        # drop the edge rather than falling back to a noisy
                        # global match.
                        matched_ids = scoped
                    else:
                        matched_ids = candidate_ids

                for cid in matched_ids:
                    key = (tu["id"], cid, dep["dep_type"])
                    if key not in seen:
                        seen.add(key)
                        code_file = id_to_file.get(cid, "")
                        proximity = _compute_proximity_weight(
                            file_path, code_file,
                        )
                        # Blend dep confidence into weight: confidence^0.5 softens the penalty
                        # e.g., 0.3 confidence → sqrt(0.3) ≈ 0.55 multiplier
                        confidence = dep.get("confidence", 1.0)
                        weight = proximity * (confidence ** 0.5)
                        edges.append({
                            "test_id": tu["id"],
                            "code_id": cid,
                            "edge_type": dep["dep_type"],
                            "weight": weight,
                        })
        return edges

    def _resolve_import_targets(self, test_file_path, deps, id_to_file):
        """Return the set of code-file paths that *deps* with module_path resolve to.

        Used to scope tier-4 (name-only) matches so a bare `dispatch()` call
        in test A cannot accidentally edge to a `dispatch` symbol in
        unrelated file B that test A never imports.
        """
        resolved_files: set[str] = set()
        is_go = detect_language(test_file_path) == "go"
        for dep in deps:
            module_path = dep.get("module_path")
            if not module_path:
                continue
            # Python: module.path resolves to a code file path.
            for cfile in id_to_file.values():
                if _matches_import_path(cfile, module_path):
                    resolved_files.add(cfile)
            # JS/TS relative imports.
            if module_path.startswith("."):
                resolved = _resolve_js_module_path(test_file_path, module_path)
                if resolved:
                    for cfile in id_to_file.values():
                        if _matches_js_import_path(cfile, resolved):
                            resolved_files.add(cfile)
            # Go module imports.
            if is_go:
                resolved = _resolve_go_import_path(self.project_dir, module_path)
                if resolved:
                    resolved_norm = resolved.replace("\\", "/")
                    for cfile in id_to_file.values():
                        cfile_norm = cfile.replace("\\", "/")
                        if resolved_norm == ".":
                            if "/" not in cfile_norm:
                                resolved_files.add(cfile)
                        elif (
                            cfile_norm == resolved_norm
                            or cfile_norm.startswith(resolved_norm + "/")
                        ):
                            resolved_files.add(cfile)
        return resolved_files


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
    if framework == "csharp_test":
        # Fallback heuristic for C#: common xUnit/NUnit/MSTest naming
        return base.startswith("Test") or base.endswith("Test") or base.endswith("Tests")
    if framework == "gtest":
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
    return (
        "#[test]" in content
        or "#[tokio::test]" in content
        or "#[rstest]" in content
        or "#[cfg(test)]" in content
    )


def _check_rust_test(file_path):
    """Check if a .rs file contains #[test] or #[cfg(test)]."""
    content = _read_file(file_path)
    return content is not None and _check_rust_test_content(content)


# Regex to find Rust functions that are preceded by a test attribute
_RUST_TEST_FN_RE = re.compile(
    r"(?:#\[test\]|#\[tokio::test\]|#\[rstest\])\s*\n\s*"
    r"(?:async\s+)?fn\s+([A-Za-z_][A-Za-z0-9_]*)",
    re.MULTILINE,
)

# Regex to find Java/Kotlin methods preceded by @Test
_JAVA_TEST_METHOD_RE = re.compile(
    r"@(?:Test|ParameterizedTest|RepeatedTest)\s*\n\s*"
    r"(?:@[A-Za-z]+\s*\n\s*)*"
    r"(?:public\s+|private\s+|protected\s+)?"
    r"(?:static\s+)?"
    r"(?:<[\w\s,<>?]+>\s+)?"
    r"(?:[\w\[\]<>]+\s+)?"
    r"([A-Za-z_][A-Za-z0-9_]*)\s*\(",
    re.MULTILINE,
)

# Regex to find Swift functions preceded by @Test
_SWIFT_TEST_FN_RE = re.compile(
    r"@Test\s*\n\s*"
    r"(?:async\s+)?"
    r"(?:func\s+)?"
    r"([A-Za-z_][A-Za-z0-9_]*)\s*\(",
    re.MULTILINE,
)

# Regex to find C# methods preceded by [Fact], [Theory], [Test], [TestMethod]
_CS_TEST_METHOD_RE = re.compile(
    r"\[(?:Fact|Theory|Test|TestMethod)\s*(?:\([^)]*\))?\]\s*\n\s*"
    r"(?:public\s+|private\s+|protected\s+|internal\s+)?"
    r"(?:static\s+)?"
    r"(?:async\s+)?"
    r"(?:[\w<>,.?\[\]]+\s+)?"
    r"([A-Za-z_][A-Za-z0-9_]*)\s*\(",
    re.MULTILINE,
)


def _annotated_test_names(content, framework):
    """Return a set of function/method names marked with test annotations."""
    if framework == "rust":
        return set(_RUST_TEST_FN_RE.findall(content))
    if framework in ("junit", "xctest"):
        return set(_JAVA_TEST_METHOD_RE.findall(content))
    if framework == "swift_test":
        return set(_SWIFT_TEST_FN_RE.findall(content))
    if framework == "csharp_test":
        return set(_CS_TEST_METHOD_RE.findall(content))
    return set()


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
    re.MULTILINE | re.DOTALL,
)
_JS_CALL_RE = re.compile(r"(\w+)\s*\(")
_JS_NAMED_IMPORT_RE = re.compile(r"import\s+\{([^}]+)\}")
# Named ESM imports WITH module_path: import { foo, bar } from './path'
_JS_NAMED_IMPORT_WITH_PATH_RE = re.compile(
    r"import\s+\{([^}]+)\}\s+from\s+['\"]([^'\"]+)['\"]",
)

# ESM default import binding: import SearchService from '...'
_JS_ESM_DEFAULT_RE = re.compile(
    r"import\s+(\w+)\s+from\s+['\"]([^'\"]+)['\"]",
)

# CJS default binding: const SearchService = require('...')
_JS_CJS_DEFAULT_RE = re.compile(
    r"(?:const|let|var)\s+(\w+)\s*=\s*require\s*\(\s*['\"]([^'\"]+)['\"]\s*\)",
)

# CJS destructured: const { X, Y } = require('...')
_JS_CJS_DESTRUCTURED_RE = re.compile(
    r"(?:const|let|var)\s+\{([^}]+)\}\s*=\s*require\s*\(\s*['\"]([^'\"]+)['\"]\s*\)",
)

# ESM dynamic import: import('./module') or await import('./module')
_JS_DYNAMIC_IMPORT_RE = re.compile(
    r"(?:await\s+)?import\s*\(\s*['\"]([^'\"]+)['\"]\s*\)",
)

# Dynamic require with variable: require(variableName)
_JS_REQUIRE_VARIABLE_RE = re.compile(
    r"require\s*\(\s*([A-Za-z_$][A-Za-z0-9_$]*)\s*\)",
)

# Dynamic require with template literal: require(`./${module}`)
_JS_REQUIRE_TEMPLATE_RE = re.compile(
    r"require\s*\(\s*`([^`]+)`\s*\)",
)

# Dynamic require with string concatenation: require('./' + name) or require(path + '/foo')
_JS_REQUIRE_CONCAT_RE = re.compile(
    r"require\s*\(\s*['\"]([^'\"]*?)['\"]\s*\+",
)

# Dynamic require with conditional: require(condition ? './prod' : './dev')
_JS_REQUIRE_CONDITIONAL_RE = re.compile(
    r"require\s*\(\s*\S+\s*\?\s*['\"]([^'\"]+)['\"]\s*:\s*['\"]([^'\"]+)['\"]",
)

# Eval/new Function with require: new Function('require', code)(require) or new Function(..., 'require', ...)
_JS_EVAL_WITH_REQUIRE_RE = re.compile(
    r"new\s+Function\s*\(",
)

# Dynamic require with path.join(__dirname, '<dir>', <var>)
# Captures the first literal path token ('plugins') as the directory hint
# so the import graph can resolve it to all files in that subdirectory.
_JS_REQUIRE_PATH_JOIN_RE = re.compile(
    r"require\s*\(\s*path\.join\s*\(\s*(?:__dirname|__filename)\s*,\s*"
    r"['\"]([^'\"]+)['\"]"
)

# Variable assignment tracking for taint analysis:
# const/let/var MODULE = './path' or MODULE = './path'
_JS_VAR_ASSIGN_RE = re.compile(
    r"(?:const|let|var)\s+([A-Za-z_$][A-Za-z0-9_$]*)\s*=\s*['\"]([^'\"]+)['\"]",
)
# Simple assignment: MODULE = './path' (not preceded by another identifier char)
_JS_SIMPLE_ASSIGN_RE = re.compile(
    r"(?<![A-Za-z_$])([A-Za-z_$][A-Za-z0-9_$]*)\s*=\s*['\"]([^'\"]+)['\"]",
)

# JS/TS file extensions for path matching.
_JS_EXTENSIONS = frozenset({".js", ".jsx", ".ts", ".tsx", ".mjs", ".cjs"})

# Confidence levels for require detection (per DynamicRequireChainTracer findings)
_REQUIRE_CONFIDENCE = {
    "static": 1.0,       # require('./literal') - full confidence
    "tainted": 1.0,      # require(var) where var was assigned a path - resolved
    "template": 0.4,     # require(`./${var}`) - template literal
    "variable": 0.3,     # require(variable) - variable reference (unknown var)
    "concat": 0.2,       # require('./' + var) - string concat
    "conditional": 0.3,  # require(cond ? './a' : './b') - ternary
    "eval": 0.0,         # require via eval/new Function - invisible
}


def _extract_js_deps(content):
    """Extract imports and calls from JS/TS content."""
    deps = []

    # Module path imports (file-stem name + module_path for path-based matching)
    for m in _JS_IMPORT_RE.finditer(content):
        mod = m.group(1) or m.group(2)
        name = mod.rsplit("/", 1)[-1].split(".")[0]
        deps.append({"name": name, "dep_type": "import", "module_path": mod})

    # ESM default import binding: import SearchService from '...'
    for m in _JS_ESM_DEFAULT_RE.finditer(content):
        deps.append({
            "name": m.group(1), "dep_type": "import", "module_path": m.group(2),
        })

    # CJS default binding: const SearchService = require('...')
    for m in _JS_CJS_DEFAULT_RE.finditer(content):
        deps.append({
            "name": m.group(1), "dep_type": "import", "module_path": m.group(2),
        })

    # CJS destructured: const { X, Y } = require('...')
    for m in _JS_CJS_DESTRUCTURED_RE.finditer(content):
        mod = m.group(2)
        for name in m.group(1).split(","):
            name = name.strip().split(":")[0].strip()  # handle { X: alias }
            if name:
                deps.append({"name": name, "dep_type": "import", "module_path": mod})

    # Named ESM imports WITH module_path: import { foo, bar } from './path'
    # (process before the fallback named-import block below)
    for m in _JS_NAMED_IMPORT_WITH_PATH_RE.finditer(content):
        mod = m.group(2)
        for name in m.group(1).split(","):
            name = name.strip().split(" as ")[0].strip()
            if name:
                deps.append({"name": name, "dep_type": "import", "module_path": mod})

    # Named ESM imports: import { foo, bar } from ... (fallback, no module_path)
    for m in _JS_NAMED_IMPORT_RE.finditer(content):
        for name in m.group(1).split(","):
            name = name.strip().split(" as ")[0].strip()
            if name:
                deps.append({"name": name, "dep_type": "import"})

    # ESM dynamic imports: import('./module') or await import('./module')
    for m in _JS_DYNAMIC_IMPORT_RE.finditer(content):
        mod = m.group(1)
        deps.append({
            "name": f"import({mod})",
            "dep_type": "dynamic_import",
            "module_path": mod,
        })

    # --- Dynamic require() detection (DynamicRequireChainTracer) ---
    # Build variable taint map first so we can resolve known variable assignments
    taint_map = {}  # var_name -> assigned_path
    for m in _JS_VAR_ASSIGN_RE.finditer(content):
        taint_map[m.group(1)] = m.group(2)
    for m in _JS_SIMPLE_ASSIGN_RE.finditer(content):
        # Only add if not already in map (const/let/var takes precedence)
        if m.group(1) not in taint_map:
            taint_map[m.group(1)] = m.group(2)

    # require(variable) - variable path reference (tainted if variable was assigned a path)
    for m in _JS_REQUIRE_VARIABLE_RE.finditer(content):
        var_name = m.group(1)
        # Only flag if not already matched as static require
        if var_name not in _JS_KEYWORDS:
            if var_name in taint_map:
                # Tainted: we know what path the variable held
                resolved_path = taint_map[var_name]
                deps.append({
                    "name": f"require({var_name})",
                    "dep_type": "tainted_import",
                    "module_path": resolved_path,
                    "require_type": "variable",
                    "confidence": _REQUIRE_CONFIDENCE["static"],  # Resolved → full confidence
                    "taint_source": f"{var_name}={resolved_path}",
                })
            else:
                # Unknown variable - truly dynamic
                deps.append({
                    "name": f"require({var_name})",
                    "dep_type": "dynamic_import",
                    "module_path": var_name,
                    "require_type": "variable",
                    "confidence": _REQUIRE_CONFIDENCE["variable"],
                })

    # require(`./${module}`) - template literal
    for m in _JS_REQUIRE_TEMPLATE_RE.finditer(content):
        template = m.group(1)
        deps.append({
            "name": f"require(`{template}`)",
            "dep_type": "dynamic_import",
            "module_path": template,
            "require_type": "template",
            "confidence": _REQUIRE_CONFIDENCE["template"],
        })

    # require('./' + name) - string concatenation
    for m in _JS_REQUIRE_CONCAT_RE.finditer(content):
        prefix = m.group(1)
        deps.append({
            "name": f"require('{prefix}' + ...)",
            "dep_type": "dynamic_import",
            "module_path": prefix,
            "require_type": "concat",
            "confidence": _REQUIRE_CONFIDENCE["concat"],
        })

    # require(cond ? './prod' : './dev') - conditional requires
    for m in _JS_REQUIRE_CONDITIONAL_RE.finditer(content):
        true_path = m.group(1)
        false_path = m.group(2)
        deps.append({
            "name": f"require(cond ? '{true_path}' : '{false_path}')",
            "dep_type": "dynamic_import",
            "module_path": f"{true_path}|{false_path}",
            "require_type": "conditional",
            "confidence": _REQUIRE_CONFIDENCE["conditional"],
        })

    # new Function(...) with require - eval-based loading (CRITICAL risk)
    for m in _JS_EVAL_WITH_REQUIRE_RE.finditer(content):
        deps.append({
            "name": "new Function(...)",
            "dep_type": "eval_import",
            "require_type": "eval",
            "confidence": _REQUIRE_CONFIDENCE["eval"],
        })

    # require(path.join(__dirname, '<dir>', <var>)) — node-style plugin loaders.
    # Captures the literal directory token so the import-graph builder can
    # resolve it to all files in that subdirectory (soft / dynamic edges).
    for m in _JS_REQUIRE_PATH_JOIN_RE.finditer(content):
        directory = m.group(1)
        deps.append({
            "name": f"require(path.join(__dirname, '{directory}', ...))",
            "dep_type": "dynamic_import",
            "module_path": directory,
            "require_type": "path_join",
            "confidence": _REQUIRE_CONFIDENCE["concat"],
        })

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


def _go_module_name(project_dir: str) -> str | None:
    """Read the module directive from go.mod if present."""
    go_mod = Path(project_dir) / "go.mod"
    if not go_mod.exists():
        return None
    try:
        text = go_mod.read_text(encoding="utf-8", errors="replace")
    except OSError:
        return None
    for line in text.splitlines():
        line = line.strip()
        if line.startswith("module "):
            return line.split(None, 1)[1].split()[0].strip('"')
    return None


_GO_MODULE_CACHE: dict[str, str | None] = {}


def _resolve_go_import_path(project_dir: str, import_path: str) -> str | None:
    """Map a Go import path to a project-relative directory path.

    If the import path belongs to the module declared in go.mod, the
    module prefix is stripped and the remainder is returned as a relative
    directory path. External or stdlib imports return None.
    """
    cache_key = project_dir
    mod = _GO_MODULE_CACHE.get(cache_key)
    if mod is None and cache_key not in _GO_MODULE_CACHE:
        mod = _go_module_name(project_dir)
        _GO_MODULE_CACHE[cache_key] = mod
    if not mod:
        return None
    norm = import_path.rstrip("/")
    if norm.startswith(mod + "/"):
        return norm[len(mod) + 1 :]
    if norm == mod:
        return "."
    return None


def _extract_go_deps(content):
    """Extract imports from Go content."""
    deps = []
    for m in _GO_IMPORT_BLOCK_RE.finditer(content):
        block = m.group(1)
        for line in block.strip().split("\n"):
            im = _GO_IMPORT_LINE_RE.search(line)
            if im:
                full = im.group(1)
                pkg = full.rsplit("/", 1)[-1]
                deps.append({"name": pkg, "dep_type": "import", "module_path": full})

    for m in _GO_IMPORT_SINGLE_RE.finditer(content):
        full = m.group(1)
        pkg = full.rsplit("/", 1)[-1]
        deps.append({"name": pkg, "dep_type": "import", "module_path": full})

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

_custom_dep_extractors: dict[str, object] = {}


def register_dep_extractor(language, extractor):
    """Register a custom dependency extractor for a language.

    Custom extractors override the built-in regex-based ones, allowing
    users to supply precise import/require parsing (e.g. tree-sitter)
    without adding dependencies to Chisel itself.

    Args:
        language: Language string (e.g. "javascript", "go").
        extractor: Callable with signature
                   ``(content: str) -> list[dict]``.

    Raises:
        TypeError: If *extractor* is not callable.
    """
    if not callable(extractor):
        raise TypeError(f"extractor must be callable, got {type(extractor).__name__}")
    _custom_dep_extractors[language] = extractor


def unregister_dep_extractor(language):
    """Remove a custom dependency extractor."""
    del _custom_dep_extractors[language]


def get_registered_dep_extractors():
    """Return a shallow copy of the custom dep extractor registry."""
    return dict(_custom_dep_extractors)


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


def _strip_js_ext(path):
    """Strip a JS/TS extension from *path* if present."""
    for ext in _JS_EXTENSIONS:
        if path.endswith(ext):
            return path[: -len(ext)]
    return path


def _resolve_js_module_path(test_file_path, module_path):
    """Resolve a JS/TS relative import path against the test file's directory.

    Returns the resolved path (without extension) relative to the project
    root, or ``None`` for non-relative imports (npm packages).

    Example::

        _resolve_js_module_path(
            "tests/services/search.test.js",
            "../../src/services/searchService",
        )
        # -> "src/services/searchService"
    """
    if not module_path or not module_path.startswith("."):
        return None  # npm package — not a local file
    test_dir = os.path.dirname(test_file_path.replace("\\", "/"))
    joined = os.path.join(test_dir, module_path)
    resolved = os.path.normpath(joined).replace("\\", "/")
    return _strip_js_ext(resolved)


def _matches_js_import_path(code_file_path, resolved_import):
    """Check if *code_file_path* corresponds to a resolved JS/TS import.

    ``resolved_import`` is an extension-free path from
    :func:`_resolve_js_module_path`.  Matches::

        "src/services/searchService" against
        - "src/services/searchService.js"
        - "src/services/searchService.ts"
        - "src/services/searchService/index.js"
    """
    if not resolved_import:
        return False
    code = _strip_js_ext(code_file_path.replace("\\", "/"))
    return code == resolved_import or code == resolved_import + "/index"


def _dedupe_deps(deps):
    """Remove duplicate dependencies, keeping the richest first occurrence.

    Deduplicates by (name, dep_type) but prefers entries that have a
    ``module_path``. This ensures distinct imports with the same local
    name (e.g. ``from a import foo`` and ``from b import foo``) are
    preserved, while fallback entries without ``module_path`` are dropped
    when a path-aware entry already exists.
    """
    by_key: dict[tuple[str, str], dict] = {}
    for dep in deps:
        key = (dep["name"], dep["dep_type"])
        existing = by_key.get(key)
        if existing is None:
            by_key[key] = dep
        elif not existing.get("module_path") and dep.get("module_path"):
            # Prefer the later entry if it carries path information
            by_key[key] = dep
    return list(by_key.values())
