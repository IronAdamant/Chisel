"""Swift-syntax extractor for Swift source files.

Install dependencies in your environment (not in Chisel's core):
    pip install swift-syntax

Usage:
    export CHISEL_BOOTSTRAP=examples.extractors.swift_syntax_extractor
    chisel analyze .
"""

from chisel.ast_utils import CodeUnit, register_extractor

try:
    from swift_syntax import *  # noqa: F403
    from swift_syntax.parser import Parser
except ImportError as exc:
    raise ImportError(
        "Swift extractor requires: pip install swift-syntax"
    ) from exc


def swift_syntax_extractor(file_path: str, content: str) -> list[CodeUnit]:
    """Extract code units from Swift source using swift-syntax.

    Collects functions, classes, structs, enums, actors, and protocols.
    """
    tree = Parser.parse(source=content)
    units = []

    def _walk(node):
        if isinstance(node, FunctionDeclSyntax):  # noqa: F405
            name = node.name.text if node.name else "anonymous"
            units.append(CodeUnit(
                file_path=file_path,
                name=name,
                unit_type="function",
                line_start=node.position.line,
                line_end=node.end_position.line,
            ))
        elif isinstance(node, (ClassDeclSyntax, StructDeclSyntax,  # noqa: F405
                               EnumDeclSyntax, ActorDeclSyntax,  # noqa: F405
                               ProtocolDeclSyntax)):  # noqa: F405
            name = node.name.text if node.name else "anonymous"
            kind_map = {
                ClassDeclSyntax: "class",  # noqa: F405
                StructDeclSyntax: "struct",  # noqa: F405
                EnumDeclSyntax: "enum",  # noqa: F405
                ActorDeclSyntax: "actor",  # noqa: F405
                ProtocolDeclSyntax: "interface",  # noqa: F405
            }
            units.append(CodeUnit(
                file_path=file_path,
                name=name,
                unit_type=kind_map.get(type(node), "class"),
                line_start=node.position.line,
                line_end=node.end_position.line,
            ))
        for child in node.children:
            _walk(child)

    _walk(tree)
    return units


register_extractor("swift", swift_syntax_extractor)
