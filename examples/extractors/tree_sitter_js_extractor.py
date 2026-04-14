"""Tree-sitter extractor for JavaScript / TypeScript.

Install dependencies in your environment (not in Chisel's core):
    pip install tree-sitter tree-sitter-javascript tree-sitter-typescript

Usage:
    export CHISEL_BOOTSTRAP=examples.extractors.tree_sitter_js_extractor
    chisel analyze .
"""

from chisel.ast_utils import CodeUnit, register_extractor

try:
    import tree_sitter_javascript as tsjs
    import tree_sitter_typescript as tsts
    from tree_sitter import Language, Parser
except ImportError as exc:
    raise ImportError(
        "tree-sitter JS/TS extractor requires: "
        "pip install tree-sitter tree-sitter-javascript tree-sitter-typescript"
    ) from exc


JS_LANG = Language(tsjs.language())
TS_LANG = Language(tsts.language())

_js_parser = Parser(JS_LANG)
_ts_parser = Parser(TS_LANG)


def _walk_js_ts_units(node, file_path, units):
    """Recursively collect function/class declarations."""
    if node.type in (
        "function_declaration",
        "function",
        "arrow_function",
        "class_declaration",
        "class",
        "method_definition",
    ):
        name = "anonymous"
        for child in node.children:
            if child.type == "identifier":
                name = child.text.decode("utf-8") if hasattr(child.text, "decode") else child.text
                break
            if child.type == "property_identifier" and node.type == "method_definition":
                name = child.text.decode("utf-8") if hasattr(child.text, "decode") else child.text
                break
        unit_type = "class" if "class" in node.type else "function"
        units.append(CodeUnit(
            file_path=file_path,
            name=name,
            unit_type=unit_type,
            line_start=node.start_point[0] + 1,
            line_end=node.end_point[0] + 1,
        ))
    for child in node.children:
        _walk_js_ts_units(child, file_path, units)


def _extract_js_ts_deps(node, scope, deps):
    """Recursively extract require/import dependencies with variable taint."""
    if node.type == "call_expression":
        func = node.child_by_field_name("function")
        args = node.child_by_field_name("arguments")
        if func and func.type == "identifier" and func.text == b"require" and args:
            first_arg = args.child(1)  # skip (
            if first_arg:
                arg_text = (
                    first_arg.text.decode("utf-8")
                    if hasattr(first_arg.text, "decode")
                    else str(first_arg.text)
                )
                if arg_text.startswith(("'", '"')):
                    deps.append({"name": arg_text.strip('"\''), "dep_type": "import"})
                elif arg_text in scope:
                    deps.append({"name": scope[arg_text], "dep_type": "import"})
                else:
                    deps.append({"name": arg_text, "dep_type": "dynamic_import"})

    if node.type in ("variable_declarator", "assignment_expression"):
        lhs = node.child_by_field_name("left") or node.child(0)
        rhs = node.child_by_field_name("right") or node.child(1)
        if lhs and lhs.type == "identifier" and rhs:
            rhs_text = (
                rhs.text.decode("utf-8")
                if hasattr(rhs.text, "decode")
                else str(rhs.text)
            )
            if rhs_text.startswith(("'", '"')):
                scope[lhs.text.decode("utf-8") if hasattr(lhs.text, "decode") else lhs.text] = rhs_text.strip('"\'')

    for child in node.children:
        _extract_js_ts_deps(child, scope, deps)


def js_ts_tree_sitter_extractor(file_path: str, content: str) -> list[CodeUnit]:
    """Extract code units and dependencies via tree-sitter for JS/TS."""
    parser = _ts_parser if file_path.endswith((".ts", ".tsx")) else _js_parser
    tree = parser.parse(bytes(content, "utf-8"))
    units = []
    _walk_js_ts_units(tree.root_node, file_path, units)

    # Dependency extraction (stored on units for edge building)
    deps = []
    _extract_js_ts_deps(tree.root_node, {}, deps)
    # Attach deps to the first unit so Chisel's test_mapper can read them
    if units and deps:
        units[0].__dict__.setdefault("_extractor_deps", deps)

    return units


register_extractor("javascript", js_ts_tree_sitter_extractor)
register_extractor("typescript", js_ts_tree_sitter_extractor)
