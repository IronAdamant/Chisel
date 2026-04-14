"""Stdio-based MCP server for Chisel using the ``mcp`` Python package.

This module provides an MCP-compliant server that communicates over
stdin/stdout, suitable for integration with Claude Desktop, Cursor, and
other MCP-aware clients.

Entry point::

    chisel-mcp          (installed via pyproject.toml console_scripts)
    python -m chisel.mcp_stdio

Requires the optional ``mcp`` dependency:
    pip install chisel-test-impact[mcp]
"""

import asyncio
import json
import logging
import os
import sys

from chisel.engine import ChiselEngine
from chisel.mcp_server import dispatch_tool
from chisel.schemas import _TOOL_SCHEMAS

logger = logging.getLogger(__name__)

try:
    from mcp.server import Server
    from mcp.server.stdio import stdio_server
    from mcp.types import TextContent, Tool
    _MCP_AVAILABLE = True
except ImportError:
    _MCP_AVAILABLE = False


def _configure_server(engine):
    """Register all Chisel tools on an MCP Server instance.

    Factored out of ``create_server`` so that ``_run_server`` can manage
    the engine lifecycle independently (ensuring it is closed on exit).
    """
    server = Server("chisel")

    @server.list_tools()
    async def list_tools():
        """Return all Chisel tool definitions as MCP Tool objects."""
        return [
            Tool(
                name=name,
                description=schema["description"],
                inputSchema=schema["parameters"],
            )
            for name, schema in _TOOL_SCHEMAS.items()
        ]

    @server.call_tool()
    async def call_tool(name: str, arguments: dict):
        """Dispatch an MCP tool call to the appropriate engine method."""
        try:
            loop = asyncio.get_running_loop()
            result, next_steps = await loop.run_in_executor(
                None, lambda: dispatch_tool(engine, name, arguments),
            )
        except ValueError as exc:
            # Unknown tool or invalid arguments
            logger.warning("Tool error %s: %s", name, exc)
            payload = {"status": "error", "message": str(exc)}
            text = json.dumps(payload, default=str)
            return [TextContent(type="text", text=text)]
        except Exception as exc:
            logger.exception("Error executing tool %s", name)
            payload = {"status": "error", "message": str(exc)}
            text = json.dumps(payload, default=str)
            return [TextContent(type="text", text=text)]

        payload = {"result": result}
        if next_steps:
            payload["next_steps"] = next_steps
        text = json.dumps(payload, indent=2, default=str)
        return [TextContent(type="text", text=text)]

    return server


def create_server(storage_dir=None, project_dir=None):
    """Create and configure an MCP Server with all Chisel tools registered.

    Args:
        storage_dir: Directory for Chisel's persistent storage.
        project_dir: Root of the project to analyze. Defaults to cwd.

    Returns:
        A tuple of (configured ``mcp.server.Server`` instance, ``ChiselEngine``).
    """
    if not _MCP_AVAILABLE:
        raise RuntimeError(
            "The 'mcp' package is not installed. "
            "Install it with: pip install chisel-test-impact[mcp]"
        )

    if project_dir is None:
        project_dir = os.getcwd()

    engine = ChiselEngine(project_dir, storage_dir=storage_dir)
    try:
        server = _configure_server(engine)
    except Exception:
        engine.close()
        raise
    return server, engine


async def _run_server():
    """Start the stdio MCP server and run until the client disconnects."""
    project_dir = os.environ.get("CHISEL_PROJECT_DIR")
    storage_dir = os.environ.get("CHISEL_STORAGE_DIR")

    server, engine = create_server(storage_dir=storage_dir, project_dir=project_dir)
    try:
        async with stdio_server() as (read_stream, write_stream):
            await server.run(
                read_stream,
                write_stream,
                server.create_initialization_options(),
            )
    finally:
        engine.close()


def main():
    """Entry point for ``chisel-mcp`` console script."""
    if not _MCP_AVAILABLE:
        print(
            "Error: The 'mcp' package is not installed.\n"
            "\n"
            "The Chisel stdio MCP server requires the 'mcp' Python package.\n"
            "Install it with:\n"
            "\n"
            "    pip install chisel-test-impact[mcp]\n"
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
