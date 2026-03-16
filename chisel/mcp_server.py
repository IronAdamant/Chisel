"""HTTP-based MCP server for Chisel — zero external dependencies.

Exposes all ChiselEngine tool methods over HTTP with JSON request/response.
Endpoints:
    GET  /tools   — list available tool schemas
    GET  /health  — health check
    POST /call    — invoke a tool by name with arguments
"""

import json
import logging
import threading
from http.server import BaseHTTPRequestHandler, HTTPServer
from socketserver import ThreadingMixIn

from chisel.engine import ChiselEngine

logger = logging.getLogger(__name__)

# ------------------------------------------------------------------ #
# Tool schemas — JSON Schema definitions for all 10 engine tools
# ------------------------------------------------------------------ #

_TOOL_SCHEMAS = {
    "analyze": {
        "name": "analyze",
        "description": (
            "Run full code analysis on the project. Scans files, parses code "
            "units, discovers tests, parses git history, and builds test edges."
        ),
        "parameters": {
            "type": "object",
            "properties": {
                "directory": {
                    "type": "string",
                    "description": "Subdirectory to analyze (default: entire project).",
                },
                "force": {
                    "type": "boolean",
                    "description": "Force re-analysis of all files even if unchanged.",
                },
            },
            "required": [],
        },
    },
    "impact": {
        "name": "impact",
        "description": (
            "Get impacted tests for the given changed files and optional functions."
        ),
        "parameters": {
            "type": "object",
            "properties": {
                "files": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "List of changed file paths.",
                },
                "functions": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "Optional list of changed function names.",
                },
            },
            "required": ["files"],
        },
    },
    "suggest_tests": {
        "name": "suggest_tests",
        "description": "Suggest tests to run for a given file, optionally with a diff.",
        "parameters": {
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "Path to the file to suggest tests for.",
                },
                "diff": {
                    "type": "string",
                    "description": "Optional unified diff content.",
                },
            },
            "required": ["file_path"],
        },
    },
    "churn": {
        "name": "churn",
        "description": "Get churn statistics for a file or a specific unit within it.",
        "parameters": {
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "Path to the file.",
                },
                "unit_name": {
                    "type": "string",
                    "description": "Optional name of a specific code unit.",
                },
            },
            "required": ["file_path"],
        },
    },
    "ownership": {
        "name": "ownership",
        "description": "Get code ownership breakdown showing original authors (blame-based). Each entry has role='original_author'.",
        "parameters": {
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "Path to the file.",
                },
            },
            "required": ["file_path"],
        },
    },
    "coupling": {
        "name": "coupling",
        "description": "Get co-change coupling partners for a file.",
        "parameters": {
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "Path to the file.",
                },
                "min_count": {
                    "type": "integer",
                    "description": "Minimum co-commit count threshold (default: 3).",
                },
            },
            "required": ["file_path"],
        },
    },
    "risk_map": {
        "name": "risk_map",
        "description": "Compute risk scores for all files in the project.",
        "parameters": {
            "type": "object",
            "properties": {
                "directory": {
                    "type": "string",
                    "description": "Optional subdirectory to scope the risk map.",
                },
            },
            "required": [],
        },
    },
    "stale_tests": {
        "name": "stale_tests",
        "description": "Detect stale tests whose source code has changed since last run.",
        "parameters": {
            "type": "object",
            "properties": {},
            "required": [],
        },
    },
    "history": {
        "name": "history",
        "description": "Get commit history for a specific file.",
        "parameters": {
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "Path to the file.",
                },
            },
            "required": ["file_path"],
        },
    },
    "who_reviews": {
        "name": "who_reviews",
        "description": "Suggest reviewers for a file based on recent commit activity. Each entry has role='suggested_reviewer' — these are not original authors but active maintainers best suited to review changes.",
        "parameters": {
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "Path to the file.",
                },
            },
            "required": ["file_path"],
        },
    },
}

# ------------------------------------------------------------------ #
# Tool dispatch — map tool name to engine method + argument names
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
        content_length = int(self.headers.get("Content-Length", 0))
        if content_length == 0:
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

        if tool_name not in _TOOL_DISPATCH:
            self._send_error_json(
                404, f"Unknown tool: {tool_name!r}. Available: {sorted(_TOOL_DISPATCH)}"
            )
            return

        arguments = body.get("arguments", {})
        if not isinstance(arguments, dict):
            self._send_error_json(400, "'arguments' must be a JSON object")
            return

        method_name, allowed_args = _TOOL_DISPATCH[tool_name]
        engine = self.server.engine  # type: ChiselEngine

        # Filter arguments to only those the method accepts
        kwargs = {k: v for k, v in arguments.items() if k in allowed_args and v is not None}

        try:
            method = getattr(engine, method_name)
            result = method(**kwargs)
            self._send_json({"result": result})
        except TypeError as exc:
            self._send_error_json(400, f"Invalid arguments for tool {tool_name!r}: {exc}")
        except Exception as exc:
            logger.exception("Error executing tool %s", tool_name)
            self._send_error_json(500, f"Tool execution error: {exc}")


# ------------------------------------------------------------------ #
# Threaded HTTP server
# ------------------------------------------------------------------ #

class ThreadedHTTPServer(ThreadingMixIn, HTTPServer):
    """HTTPServer that handles each request in a new thread."""

    daemon_threads = True
    allow_reuse_address = True

    def __init__(self, server_address, handler_class, engine):
        self.engine = engine
        super().__init__(server_address, handler_class)


# ------------------------------------------------------------------ #
# High-level wrapper
# ------------------------------------------------------------------ #

class ChiselMCPServer:
    """Convenience wrapper around ThreadedHTTPServer + ChiselEngine.

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

    def start(self, blocking=True):
        """Start the HTTP server.

        Args:
            blocking: If True (default), serve forever on the calling thread.
                      If False, spawn a daemon thread and return immediately.
        """
        self._httpd = ThreadedHTTPServer(
            (self._host, self._port), MCPRequestHandler, self._engine,
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

    def get_url(self):
        """Return the base URL the server is listening on."""
        return f"http://{self._host}:{self._port}"

    @property
    def engine(self):
        """Expose the underlying ChiselEngine instance."""
        return self._engine
