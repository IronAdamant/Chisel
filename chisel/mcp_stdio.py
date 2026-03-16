"""Stdio-based MCP server for Chisel using the ``mcp`` Python package.

This module provides an MCP-compliant server that communicates over
stdin/stdout, suitable for integration with Claude Desktop, Cursor, and
other MCP-aware clients.

Entry point::

    chisel-mcp          (installed via pyproject.toml console_scripts)
    python -m chisel.mcp_stdio

Requires the optional ``mcp`` dependency:
    pip install chisel[mcp]
"""

import asyncio
import os
import sys

try:
    from mcp.server import Server
    from mcp.server.stdio import stdio_server
    from mcp.types import TextContent, Tool
    _MCP_AVAILABLE = True
except ImportError:
    _MCP_AVAILABLE = False

from chisel.engine import ChiselEngine
from chisel.mcp_server import _TOOL_SCHEMAS

# ------------------------------------------------------------------ #
# Tool dispatch — maps tool name to (engine_method, [arg_names])
# ------------------------------------------------------------------ #

_TOOL_DISPATCH = {
    "analyze": ("tool_analyze", ["directory", "force"]),
    "impact": ("tool_impact", ["files", "functions"]),
    "suggest_tests": ("tool_suggest_tests", ["file_path", "diff"]),
    "churn": ("tool_churn", ["file_path", "unit_name"]),
    "ownership": ("tool_ownership", ["file_path"]),
    "coupling": ("tool_coupling", ["file_path", "min_count"]),
    "risk_map": ("tool_risk_map", ["directory"]),
    "stale_tests": ("tool_stale_tests", []),
    "history": ("tool_history", ["file_path"]),
    "who_reviews": ("tool_who_reviews", ["file_path"]),
}


def create_server(storage_dir=None, project_dir=None):
    """Create and configure an MCP Server with all Chisel tools registered.

    Args:
        storage_dir: Directory for Chisel's persistent storage.
        project_dir: Root of the project to analyze. Defaults to cwd.

    Returns:
        A configured ``mcp.server.Server`` instance.
    """
    if not _MCP_AVAILABLE:
        raise RuntimeError(
            "The 'mcp' package is not installed. "
            "Install it with: pip install chisel[mcp]"
        )

    if project_dir is None:
        project_dir = os.getcwd()

    engine = ChiselEngine(project_dir, storage_dir=storage_dir)
    server = Server("chisel")

    # -- list_tools ---------------------------------------------------- #

    @server.list_tools()
    async def list_tools():
        """Return all Chisel tool definitions as MCP Tool objects."""
        tools = []
        for name, schema in _TOOL_SCHEMAS.items():
            tools.append(
                Tool(
                    name=name,
                    description=schema["description"],
                    inputSchema=schema["parameters"],
                )
            )
        return tools

    # -- call_tool ----------------------------------------------------- #

    @server.call_tool()
    async def call_tool(name: str, arguments: dict):
        """Dispatch an MCP tool call to the appropriate engine method."""
        if name not in _TOOL_DISPATCH:
            raise ValueError(
                f"Unknown tool: {name!r}. Available: {sorted(_TOOL_DISPATCH)}"
            )

        method_name, allowed_args = _TOOL_DISPATCH[name]
        kwargs = {
            k: v for k, v in arguments.items()
            if k in allowed_args and v is not None
        }

        try:
            method = getattr(engine, method_name)
            # Run the synchronous engine method in a thread to avoid
            # blocking the async event loop.
            loop = asyncio.get_running_loop()
            result = await loop.run_in_executor(None, lambda: method(**kwargs))
        except Exception as exc:
            return [TextContent(type="text", text=f"Error: {exc}")]

        import json
        text = json.dumps(result, indent=2, default=str)
        return [TextContent(type="text", text=text)]

    return server


async def _run_server():
    """Start the stdio MCP server and run until the client disconnects."""
    project_dir = os.environ.get("CHISEL_PROJECT_DIR", os.getcwd())
    storage_dir = os.environ.get("CHISEL_STORAGE_DIR", None)

    server = create_server(storage_dir=storage_dir, project_dir=project_dir)

    async with stdio_server() as (read_stream, write_stream):
        await server.run(
            read_stream,
            write_stream,
            server.create_initialization_options(),
        )


def main():
    """Entry point for ``chisel-mcp`` console script."""
    if not _MCP_AVAILABLE:
        print(
            "Error: The 'mcp' package is not installed.\n"
            "\n"
            "The Chisel stdio MCP server requires the 'mcp' Python package.\n"
            "Install it with:\n"
            "\n"
            "    pip install chisel[mcp]\n"
            "\n"
            "Or install the mcp package directly:\n"
            "\n"
            "    pip install mcp\n",
            file=sys.stderr,
        )
        sys.exit(1)

    asyncio.run(_run_server())


if __name__ == "__main__":
    main()
