# Example: copy into your repo and point CHISEL_BOOTSTRAP at it (stdlib-only template).
#
#   export PYTHONPATH=/path/to/parent/of/this/file
#   export CHISEL_BOOTSTRAP=chisel_bootstrap_example
#   chisel analyze .
#
# To use tree-sitter or other libraries, install them in YOUR environment and
# implement my_extractor below — Chisel does not bundle those dependencies.

from __future__ import annotations

# from chisel.ast_utils import CodeUnit, register_extractor
#
#
# def my_extractor(file_path: str, content: str) -> list[CodeUnit]:
#     """Return code units for *file_path* using your parser of choice."""
#     return []  # replace with tree-sitter / LSP / etc.
#
#
# def _register():
#     register_extractor("python", my_extractor)  # or rust, javascript, ...
#
#
# _register()

# Marker so a quick import test can verify the module loaded:
CHISEL_BOOTSTRAP_EXAMPLE = True
