"""Agent-facing contract: security posture, response vocabulary, trust hints.

Used by MCP tool descriptions (``schemas.py``) and documented in
``docs/LLM_CONTRACT.md``. Keeps behavior discoverable without extra dependencies.
"""

# Single-line append for high-traffic tool descriptions (MCP /tools list).
HEURISTIC_TRUST_NOTE = (
    "Stdlib-only; no bundled third-party runtime deps. "
    "Parsing is heuristic (regex/ast, subprocess git)—confirm safety-critical paths in CI."
)

# Longer block for docs and optional error hints.
SECURITY_MODEL = (
    "Chisel's default install uses Python's standard library only for core logic "
    "(no PyPI dependencies in the core wheel). That minimizes supply-chain surface "
    "for analysis. Git is invoked via subprocess. Code understanding uses built-in "
    "`ast` for Python and regex-based extractors for other languages—not tree-sitter "
    "or language servers in the default package. Optional user-registered extractors "
    "may add accuracy in the user's environment; see docs/CUSTOM_EXTRACTORS.md."
)

# Diagnostic statuses agents should handle distinctly (never treat as generic empty).
RESPONSE_STATUSES = (
    "no_data",
    "no_changes",
    "no_edges",
    "git_error",
)

# suggest_tests / impact-style items
SUGGEST_SOURCES = (
    "direct",
    "co_change",
    "import_graph",
    "static_require",
    "hybrid",
    "fallback",
    "working_tree",
)

# Recommended read order when present (for agent prompts / docs).
READ_FIRST_KEYS = ("status", "error", "_meta", "message", "hint")
