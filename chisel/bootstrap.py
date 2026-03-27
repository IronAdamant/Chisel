"""Optional user bootstrap — import a module before analysis (stdlib only).

Set environment variable ``CHISEL_BOOTSTRAP`` to a dotted import path
(e.g. ``my_project.chisel_plugins``). That module should call
``register_extractor`` and any other one-time setup. Chisel does not ship
tree-sitter or other third-party parsers; this hook lets *your* environment
provide them without forking Chisel.
"""

from __future__ import annotations

import importlib
import os


def load_user_bootstrap():
    """Import ``CHISEL_BOOTSTRAP`` if set; no-op otherwise."""
    name = os.environ.get("CHISEL_BOOTSTRAP", "").strip()
    if not name:
        return
    importlib.import_module(name)
