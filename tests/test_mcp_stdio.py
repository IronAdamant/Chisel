"""Tests for chisel.mcp_stdio — stdio-based MCP server.

Tests verify import-time behaviour, the _MCP_AVAILABLE flag, and
that create_server / main behave correctly when the optional ``mcp``
package is or is not installed.
"""

import builtins as _builtins
import importlib
import sys
from unittest import mock

import pytest


# ------------------------------------------------------------------ #
# Tests: Shared dispatch/schema imports (no mocking needed)
# ------------------------------------------------------------------ #

class TestSharedImports:
    """Verify that mcp_stdio re-uses dispatch tables from mcp_server."""

    def test_dispatch_tool_is_same_object_as_mcp_server(self):
        """dispatch_tool should be imported from mcp_server, not duplicated."""
        from chisel.mcp_stdio import dispatch_tool as stdio_fn
        from chisel.mcp_server import dispatch_tool as server_fn
        assert stdio_fn is server_fn

    def test_tool_schemas_is_same_object_as_mcp_server(self):
        """_TOOL_SCHEMAS should be the same object in both modules (both import from schemas.py)."""
        from chisel.mcp_stdio import _TOOL_SCHEMAS as stdio_schemas
        from chisel.mcp_server import _TOOL_SCHEMAS as server_schemas
        assert stdio_schemas is server_schemas


# ------------------------------------------------------------------ #
# Tests: _MCP_AVAILABLE flag
# ------------------------------------------------------------------ #

class TestMCPAvailableFlag:
    def test_mcp_available_is_bool(self):
        """_MCP_AVAILABLE should be a boolean."""
        from chisel.mcp_stdio import _MCP_AVAILABLE
        assert isinstance(_MCP_AVAILABLE, bool)

    def test_mcp_available_false_when_mcp_missing(self):
        """When the 'mcp' package is absent, _MCP_AVAILABLE should be False."""
        # Temporarily remove 'mcp' and related entries from sys.modules
        # and make the import fail, then reload chisel.mcp_stdio.
        saved_modules = {}
        keys_to_remove = [k for k in sys.modules if k == "mcp" or k.startswith("mcp.")]
        for key in keys_to_remove:
            saved_modules[key] = sys.modules.pop(key)

        # Also remove the cached chisel.mcp_stdio so reload picks up the change
        saved_stdio = sys.modules.pop("chisel.mcp_stdio", None)

        fake_import = _make_import_blocker("mcp")
        try:
            with mock.patch("builtins.__import__", side_effect=fake_import):
                mod = importlib.import_module("chisel.mcp_stdio")
                assert mod._MCP_AVAILABLE is False
        finally:
            # Restore original modules
            sys.modules.update(saved_modules)
            if saved_stdio is not None:
                sys.modules["chisel.mcp_stdio"] = saved_stdio

    def test_mcp_available_true_when_mcp_present(self):
        """When the 'mcp' package is importable, _MCP_AVAILABLE should be True.

        Since we cannot guarantee that the real ``mcp`` package is
        installed in the test environment, we inject a fake one.
        """
        saved_modules = {}
        keys_to_remove = [k for k in sys.modules if k == "mcp" or k.startswith("mcp.")]
        for key in keys_to_remove:
            saved_modules[key] = sys.modules.pop(key)

        saved_stdio = sys.modules.pop("chisel.mcp_stdio", None)

        # Create fake mcp modules that satisfy the imports in mcp_stdio
        fake_mcp = _build_fake_mcp_modules()
        sys.modules.update(fake_mcp)

        try:
            mod = importlib.import_module("chisel.mcp_stdio")
            assert mod._MCP_AVAILABLE is True
        finally:
            # Clean up fake modules
            for key in fake_mcp:
                sys.modules.pop(key, None)
            sys.modules.update(saved_modules)
            if saved_stdio is not None:
                sys.modules["chisel.mcp_stdio"] = saved_stdio


# ------------------------------------------------------------------ #
# Tests: create_server when mcp is NOT available
# ------------------------------------------------------------------ #

class TestCreateServerUnavailable:
    def test_create_server_raises_when_mcp_not_available(self):
        """create_server should raise RuntimeError if _MCP_AVAILABLE is False."""
        from chisel import mcp_stdio

        original = mcp_stdio._MCP_AVAILABLE
        try:
            mcp_stdio._MCP_AVAILABLE = False
            with pytest.raises(RuntimeError, match="mcp.*not installed"):
                mcp_stdio.create_server()
        finally:
            mcp_stdio._MCP_AVAILABLE = original


# ------------------------------------------------------------------ #
# Tests: main() when mcp is NOT available
# ------------------------------------------------------------------ #

class TestMainUnavailable:
    def test_main_prints_error_and_exits_when_mcp_not_available(self, capsys):
        """main() should print an error to stderr and call sys.exit(1)."""
        from chisel import mcp_stdio

        original = mcp_stdio._MCP_AVAILABLE
        try:
            mcp_stdio._MCP_AVAILABLE = False
            with pytest.raises(SystemExit) as exc_info:
                mcp_stdio.main()
            assert exc_info.value.code == 1
            captured = capsys.readouterr()
            assert "mcp" in captured.err.lower()
            assert "not installed" in captured.err.lower()
        finally:
            mcp_stdio._MCP_AVAILABLE = original


# ------------------------------------------------------------------ #
# Tests: _configure_server handlers (list_tools, call_tool)
# ------------------------------------------------------------------ #

mcp = pytest.importorskip("mcp", reason="requires optional 'mcp' package")


class TestConfigureServerHandlers:
    """Test the async handlers registered by _configure_server."""

    def test_list_tools_returns_tool_objects(self, tmp_path):
        """list_tools handler should return Tool objects for all schemas."""
        from mcp.types import ListToolsRequest
        from chisel.engine import ChiselEngine
        from chisel.mcp_stdio import _configure_server
        from chisel.mcp_server import _TOOL_SCHEMAS

        engine = ChiselEngine(str(tmp_path), storage_dir=str(tmp_path / "db"))
        try:
            server = _configure_server(engine)
            handler = server.request_handlers[ListToolsRequest]
            req = ListToolsRequest()

            async def run():
                return await handler(req)

            import asyncio
            result = asyncio.run(run())
            tools = result.root.tools
            assert len(tools) == len(_TOOL_SCHEMAS)
            names = {t.name for t in tools}
            assert "analyze" in names
            assert "impact" in names
        finally:
            engine.close()

    def test_call_tool_dispatches_and_returns_text(self, tmp_path):
        """call_tool handler should dispatch to engine and return TextContent."""
        import asyncio
        import json
        from mcp.types import CallToolRequest, CallToolRequestParams
        from chisel.engine import ChiselEngine
        from chisel.mcp_stdio import _configure_server

        engine = ChiselEngine(str(tmp_path), storage_dir=str(tmp_path / "db"))
        try:
            server = _configure_server(engine)
            handler = server.request_handlers[CallToolRequest]
            req = CallToolRequest(
                params=CallToolRequestParams(name="stale_tests", arguments={}),
            )

            async def run():
                return await handler(req)

            result = asyncio.run(run())
            content = result.root.content
            assert len(content) == 1
            assert content[0].type == "text"
            parsed = json.loads(content[0].text)
            # Empty DB returns a no-data warning dict instead of []
            assert isinstance(parsed, (list, dict))
        finally:
            engine.close()

    def test_call_tool_returns_error_on_unknown_tool(self, tmp_path):
        """call_tool handler should return error text for unknown tools."""
        import asyncio
        from mcp.types import CallToolRequest, CallToolRequestParams
        from chisel.engine import ChiselEngine
        from chisel.mcp_stdio import _configure_server

        engine = ChiselEngine(str(tmp_path), storage_dir=str(tmp_path / "db"))
        try:
            server = _configure_server(engine)
            handler = server.request_handlers[CallToolRequest]
            req = CallToolRequest(
                params=CallToolRequestParams(name="no_such_tool", arguments={}),
            )

            async def run():
                return await handler(req)

            result = asyncio.run(run())
            assert "Error:" in result.root.content[0].text
        finally:
            engine.close()


# ------------------------------------------------------------------ #
# Tests: _run_server lifecycle and main()
# ------------------------------------------------------------------ #

class TestRunServer:
    def test_run_server_closes_engine_on_exit(self, tmp_path, monkeypatch):
        """_run_server should close the engine even when server exits normally."""
        import asyncio
        from chisel import mcp_stdio

        monkeypatch.setenv("CHISEL_PROJECT_DIR", str(tmp_path))
        monkeypatch.setenv("CHISEL_STORAGE_DIR", str(tmp_path / "db"))

        closed = []
        original_close = mcp_stdio.ChiselEngine.close

        def tracked_close(self_eng):
            closed.append(True)
            original_close(self_eng)

        # Mock the server returned by _configure_server
        fake_server = mock.MagicMock()
        fake_server.run = mock.AsyncMock()
        fake_server.create_initialization_options.return_value = {}

        fake_ctx = mock.MagicMock()
        fake_ctx.__aenter__ = mock.AsyncMock(
            return_value=(mock.AsyncMock(), mock.AsyncMock())
        )
        fake_ctx.__aexit__ = mock.AsyncMock(return_value=False)

        with mock.patch.object(mcp_stdio.ChiselEngine, "close", tracked_close), \
             mock.patch.object(mcp_stdio, "_configure_server", return_value=fake_server), \
             mock.patch.object(mcp_stdio, "stdio_server", return_value=fake_ctx, create=True):
            asyncio.run(mcp_stdio._run_server())

        assert len(closed) == 1

    def test_main_calls_asyncio_run_when_available(self):
        """main() should call asyncio.run(_run_server) when mcp is available."""
        from chisel import mcp_stdio

        original_avail = mcp_stdio._MCP_AVAILABLE
        try:
            mcp_stdio._MCP_AVAILABLE = True
            with mock.patch("asyncio.run") as mock_run:
                mcp_stdio.main()
                mock_run.assert_called_once()
        finally:
            mcp_stdio._MCP_AVAILABLE = original_avail


# ------------------------------------------------------------------ #
# Tests: create_server when mcp IS available (mocked)
# ------------------------------------------------------------------ #

class TestCreateServerAvailable:
    def test_create_server_returns_server_with_engine(self, tmp_path):
        """When mcp is available, create_server should create an engine
        and return a configured Server object.
        """
        from chisel import mcp_stdio

        fake_server_instance = mock.MagicMock()
        fake_server_cls = mock.MagicMock(return_value=fake_server_instance)
        fake_server_instance.list_tools.return_value = lambda fn: fn
        fake_server_instance.call_tool.return_value = lambda fn: fn

        mock_engine_cls = mock.MagicMock()

        original_avail = mcp_stdio._MCP_AVAILABLE
        try:
            mcp_stdio._MCP_AVAILABLE = True
            # Patch both Server and ChiselEngine on the module object.
            # Server may not exist (conditional import), so use create=True.
            # ChiselEngine is a module-level name from "from chisel.engine import".
            with mock.patch.object(mcp_stdio, "Server", fake_server_cls, create=True), \
                 mock.patch.object(mcp_stdio, "ChiselEngine", mock_engine_cls):
                server, engine = mcp_stdio.create_server(
                    storage_dir=str(tmp_path / "storage"),
                    project_dir=str(tmp_path),
                )
        finally:
            mcp_stdio._MCP_AVAILABLE = original_avail

        # Engine should have been created with the supplied project_dir
        mock_engine_cls.assert_called_once_with(
            str(tmp_path), storage_dir=str(tmp_path / "storage")
        )
        # Server("chisel") should have been called
        fake_server_cls.assert_called_once_with("chisel")
        # The return value is the mocked server instance and engine
        assert server is fake_server_instance
        assert engine is mock_engine_cls.return_value

    def test_create_server_defaults_project_dir_to_cwd(self, tmp_path):
        """When project_dir is None, create_server should default to os.getcwd()."""
        from chisel import mcp_stdio

        fake_server_instance = mock.MagicMock()
        fake_server_cls = mock.MagicMock(return_value=fake_server_instance)
        fake_server_instance.list_tools.return_value = lambda fn: fn
        fake_server_instance.call_tool.return_value = lambda fn: fn

        mock_engine_cls = mock.MagicMock()

        original_avail = mcp_stdio._MCP_AVAILABLE
        try:
            mcp_stdio._MCP_AVAILABLE = True
            with mock.patch.object(mcp_stdio, "Server", fake_server_cls, create=True), \
                 mock.patch.object(mcp_stdio, "ChiselEngine", mock_engine_cls), \
                 mock.patch("os.getcwd", return_value="/fake/cwd"):
                mcp_stdio.create_server(storage_dir=None, project_dir=None)
        finally:
            mcp_stdio._MCP_AVAILABLE = original_avail

        mock_engine_cls.assert_called_once_with("/fake/cwd", storage_dir=None)


# ------------------------------------------------------------------ #
# Helpers
# ------------------------------------------------------------------ #

_real_import = _builtins.__import__


def _make_import_blocker(blocked_name):
    """Return an import function that raises ImportError for *blocked_name*."""
    def fake_import(name, *args, **kwargs):
        if name == blocked_name or name.startswith(blocked_name + "."):
            raise ImportError(f"No module named {name!r} (blocked by test)")
        return _real_import(name, *args, **kwargs)
    return fake_import


def _build_fake_mcp_modules():
    """Build fake sys.modules entries for mcp.server, mcp.server.stdio, mcp.types."""
    import types

    fake_mcp = types.ModuleType("mcp")
    fake_server_mod = types.ModuleType("mcp.server")
    fake_stdio_mod = types.ModuleType("mcp.server.stdio")
    fake_types_mod = types.ModuleType("mcp.types")

    # Provide the names that mcp_stdio.py imports
    fake_server_mod.Server = type("Server", (), {})
    fake_stdio_mod.stdio_server = mock.MagicMock()
    fake_types_mod.TextContent = type("TextContent", (), {})
    fake_types_mod.Tool = type("Tool", (), {})

    return {
        "mcp": fake_mcp,
        "mcp.server": fake_server_mod,
        "mcp.server.stdio": fake_stdio_mod,
        "mcp.types": fake_types_mod,
    }
