"""HTTP-based MCP server for Chisel — zero external dependencies.

Exposes all ChiselEngine tool methods over HTTP with JSON request/response.
Endpoints:
    GET  /tools   — list available tool schemas
    GET  /health  — health check
    POST /call    — invoke a tool by name with arguments
"""

import json
import logging
import os
import re
import threading
from http.server import BaseHTTPRequestHandler, HTTPServer

from chisel.engine import ChiselEngine
from chisel.next_steps import compute_next_steps
from chisel.schemas import _TOOL_DISPATCH, _TOOL_SCHEMAS

logger = logging.getLogger(__name__)


def dispatch_tool(engine, tool_name, arguments):
    """Dispatch a tool call to the appropriate engine method.

    Shared by the HTTP and stdio MCP servers so dispatch logic is not
    duplicated.  Raises ``ValueError`` for unknown tools, ``TypeError``
    for invalid arguments.
    """
    if tool_name not in _TOOL_DISPATCH:
        raise ValueError(
            f"Unknown tool: {tool_name!r}. Available: {sorted(_TOOL_DISPATCH)}"
        )
    method_name, allowed_args = _TOOL_DISPATCH[tool_name]
    limit = arguments.get("limit")
    kwargs = {k: v for k, v in arguments.items() if k in allowed_args and v is not None}
    result = getattr(engine, method_name)(**kwargs)
    if limit is not None:
        limit = min(int(limit), 1000)
        if isinstance(result, list):
            result = result[:limit]
        elif isinstance(result, dict) and isinstance(result.get("files"), list):
            result = {**result, "files": result["files"][:limit]}
    next_steps = compute_next_steps(tool_name, result)

    # When suggest_tests is called on a JS/TS file that uses eval or
    # new Function(), test edges through the runtime-generated code are
    # invisible to static analysis. Surface that as a warning so agents
    # know the suggestion list may be incomplete.
    if tool_name == "suggest_tests":
        warning = _eval_warning(engine, arguments.get("file_path"))
        if warning:
            next_steps.append(warning)

    return result, next_steps


# Regex matches eval(...) and new Function(...) — both can dynamically
# load code at runtime in ways the static parser can never see.
_EVAL_PATTERN_RE = re.compile(r"\beval\s*\(|\bnew\s+Function\s*\(")
_JS_LIKE_EXTS = (".js", ".jsx", ".ts", ".tsx", ".mjs", ".cjs")


def _eval_warning(engine, file_path):
    """Return a next-step warning if *file_path* uses eval/new Function.

    Targeted at JS/TS files since the patterns are JS-specific. Returns
    ``None`` when not applicable so callers can no-op cheaply.
    """
    if not file_path or not file_path.endswith(_JS_LIKE_EXTS):
        return None
    abs_path = os.path.join(engine.project_dir, file_path)
    try:
        with open(abs_path, encoding="utf-8", errors="replace") as fh:
            content = fh.read()
    except OSError:
        return None
    if not _EVAL_PATTERN_RE.search(content):
        return None
    return {
        "action": "warn_eval_used",
        "reason": (
            f"{file_path} uses eval(...) or new Function(...). Modules "
            f"loaded that way are invisible to static analysis, so tests "
            f"of runtime-generated targets are not in this list. Cross-"
            f"check risk_map → unknown_require_count for affected files."
        ),
    }


# ------------------------------------------------------------------ #
# HTTP request handler
# ------------------------------------------------------------------ #

class MCPRequestHandler(BaseHTTPRequestHandler):
    """Handles MCP HTTP requests: tool listing, health check, tool calls."""

    # Suppress default stderr logging per request
    def log_message(self, format, *args):  # noqa: A002
        logger.debug(format, *args)

    def _send_json(self, data, status=200):
        """Serialize *data* as JSON and send it with the given status code."""
        body = json.dumps(data, default=str).encode("utf-8")
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def _send_error_json(self, status, message):
        """Send a JSON error response."""
        self._send_json({"error": message}, status=status)

    # -- GET endpoints ------------------------------------------------- #

    def do_GET(self):  # noqa: N802
        if self.path == "/tools":
            self._handle_list_tools()
        elif self.path == "/health":
            self._handle_health()
        else:
            self._send_error_json(404, f"Unknown endpoint: {self.path}")

    def _handle_list_tools(self):
        """GET /tools — return all tool schemas."""
        tools = list(_TOOL_SCHEMAS.values())
        self._send_json({"tools": tools})

    def _handle_health(self):
        """GET /health — simple health check."""
        self._send_json({"status": "ok"})

    # -- POST endpoints ------------------------------------------------ #

    def do_POST(self):  # noqa: N802
        if self.path == "/call":
            self._handle_call()
        else:
            self._send_error_json(404, f"Unknown endpoint: {self.path}")

    def _read_json_body(self):
        """Read and parse the JSON request body. Returns None on failure."""
        try:
            content_length = int(self.headers.get("Content-Length", 0))
        except ValueError:
            self._send_error_json(400, "Invalid Content-Length header")
            return None
        if content_length <= 0:
            self._send_error_json(400, "Empty request body")
            return None
        raw = self.rfile.read(content_length)
        try:
            return json.loads(raw)
        except (json.JSONDecodeError, UnicodeDecodeError) as exc:
            self._send_error_json(400, f"Invalid JSON: {exc}")
            return None

    def _handle_call(self):
        """POST /call — invoke a tool.

        Expected body: {"tool": "<name>", "arguments": {<kwargs>}}
        """
        body = self._read_json_body()
        if body is None:
            return

        tool_name = body.get("tool")
        if not tool_name:
            self._send_error_json(400, "Missing 'tool' field in request body")
            return

        arguments = body.get("arguments", {})
        if not isinstance(arguments, dict):
            self._send_error_json(400, "'arguments' must be a JSON object")
            return

        try:
            result, next_steps = dispatch_tool(self.server.engine, tool_name, arguments)
            response = {"result": result}
            if next_steps:
                response["next_steps"] = next_steps
            self._send_json(response)
        except ValueError as exc:
            self._send_error_json(404, str(exc))
        except TypeError as exc:
            self._send_error_json(400, f"Invalid arguments for tool {tool_name!r}: {exc}")
        except Exception as exc:
            logger.exception("Error executing tool %s", tool_name)
            self._send_error_json(500, f"Tool execution error: {exc}")


# ------------------------------------------------------------------ #
# Threaded HTTP server (bounded thread pool)
# ------------------------------------------------------------------ #

class ThreadPoolHTTPServer(HTTPServer):
    """HTTPServer that handles each request in a bounded thread pool."""

    allow_reuse_address = True
    request_queue_size = 128

    def __init__(self, server_address, handler_class, engine, max_workers=32):
        self.engine = engine
        self._max_workers = max_workers
        self._executor = None
        super().__init__(server_address, handler_class)

    def serve_forever(self, poll_interval=0.5):
        from concurrent.futures import ThreadPoolExecutor
        self._executor = ThreadPoolExecutor(
            max_workers=self._max_workers, thread_name_prefix="chisel-mcp-"
        )
        try:
            super().serve_forever(poll_interval)
        finally:
            self._executor.shutdown(wait=False)
            self._executor = None

    def process_request(self, request, client_address):
        self._executor.submit(self.process_request_thread, request, client_address)

    def process_request_thread(self, request, client_address):
        try:
            self.finish_request(request, client_address)
        except Exception:
            self.handle_error(request, client_address)
        finally:
            self.close_request(request)


# ------------------------------------------------------------------ #
# High-level wrapper
# ------------------------------------------------------------------ #

class ChiselMCPServer:
    """Convenience wrapper around ThreadPoolHTTPServer + ChiselEngine.

    Usage::

        server = ChiselMCPServer("/path/to/project")
        server.start()          # blocks until Ctrl-C
        # or:
        server.start(blocking=False)  # runs in background thread
        print(server.get_url())
        server.stop()
    """

    def __init__(self, project_dir, storage_dir=None, host="127.0.0.1", port=8377):
        self._host = host
        self._port = port
        self._engine = ChiselEngine(project_dir, storage_dir=storage_dir)
        self._httpd = None
        self._thread = None

    # -- Public API ---------------------------------------------------- #

    def start(self, blocking=True, max_workers=32):
        """Start the HTTP server.

        Args:
            blocking: If True (default), serve forever on the calling thread.
                      If False, spawn a daemon thread and return immediately.
            max_workers: Maximum size of the request thread pool (default 32).
        """
        self._httpd = ThreadPoolHTTPServer(
            (self._host, self._port), MCPRequestHandler, self._engine,
            max_workers=max_workers,
        )
        # Update port in case 0 was passed (OS-assigned)
        self._port = self._httpd.server_address[1]

        logger.info("Chisel MCP server listening on %s", self.get_url())

        if blocking:
            try:
                self._httpd.serve_forever()
            except KeyboardInterrupt:
                self.stop()
        else:
            self._thread = threading.Thread(target=self._httpd.serve_forever, daemon=True)
            self._thread.start()

    def stop(self):
        """Shut down the server gracefully and close the engine."""
        if self._httpd is not None:
            self._httpd.shutdown()
            self._httpd.server_close()
            self._httpd = None
        if self._thread is not None:
            self._thread.join(timeout=5)
            self._thread = None
        if self._engine is not None:
            self._engine.close()
            self._engine = None

    def get_url(self):
        """Return the base URL the server is listening on."""
        return f"http://{self._host}:{self._port}"

    @property
    def engine(self):
        """Expose the underlying ChiselEngine instance."""
        return self._engine
