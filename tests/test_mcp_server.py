"""Tests for chisel.mcp_server — HTTP-based MCP server.

Tests use a real server started on an OS-assigned port (port=0) against
a temporary git repository.
"""

import json
import urllib.request
import urllib.error

import pytest

from chisel.mcp_server import ChiselMCPServer, _TOOL_SCHEMAS


def _request(base_url, method, path, body=None):
    """Send an HTTP request and return (status_code, parsed_json)."""
    url = base_url + path
    data = None
    if body is not None:
        data = json.dumps(body).encode("utf-8")
    req = urllib.request.Request(url, data=data, method=method)
    req.add_header("Content-Type", "application/json")
    try:
        with urllib.request.urlopen(req) as resp:
            raw = resp.read()
            return resp.status, json.loads(raw)
    except urllib.error.HTTPError as exc:
        raw = exc.read()
        return exc.code, json.loads(raw)


# ------------------------------------------------------------------ #
# Fixtures
# ------------------------------------------------------------------ #

@pytest.fixture
def mcp_server(git_project, tmp_path):
    """Start a ChiselMCPServer on an OS-assigned port and yield it."""
    storage_dir = tmp_path / "chisel_storage"
    server = ChiselMCPServer(
        project_dir=str(git_project),
        storage_dir=storage_dir,
        host="127.0.0.1",
        port=0,
    )
    server.start(blocking=False)
    yield server
    server.stop()


@pytest.fixture
def base_url(mcp_server):
    """Return the base URL of the running server."""
    return mcp_server.get_url()


# ------------------------------------------------------------------ #
# Tests: Health endpoint
# ------------------------------------------------------------------ #

class TestHealth:
    def test_health_returns_ok(self, base_url):
        status, body = _request(base_url, "GET", "/health")
        assert status == 200
        assert body == {"status": "ok"}


# ------------------------------------------------------------------ #
# Tests: Tool discovery
# ------------------------------------------------------------------ #

class TestToolDiscovery:
    def test_list_tools_returns_all(self, base_url):
        status, body = _request(base_url, "GET", "/tools")
        assert status == 200
        tools = body["tools"]
        tool_names = {t["name"] for t in tools}
        assert tool_names == set(_TOOL_SCHEMAS.keys())

    def test_list_tools_has_descriptions(self, base_url):
        status, body = _request(base_url, "GET", "/tools")
        for tool in body["tools"]:
            assert "description" in tool
            assert len(tool["description"]) > 0

    def test_list_tools_has_parameters(self, base_url):
        status, body = _request(base_url, "GET", "/tools")
        for tool in body["tools"]:
            assert "parameters" in tool
            assert tool["parameters"]["type"] == "object"

    def test_tool_count(self, base_url):
        status, body = _request(base_url, "GET", "/tools")
        assert len(body["tools"]) == 25


# ------------------------------------------------------------------ #
# Tests: Tool execution
# ------------------------------------------------------------------ #

class TestToolExecution:
    def test_analyze(self, base_url):
        """Invoke the analyze tool and verify it returns stats."""
        status, body = _request(base_url, "POST", "/call", {
            "tool": "analyze",
            "arguments": {},
        })
        assert status == 200
        result = body["result"]
        assert "code_files_scanned" in result
        assert result["code_files_scanned"] > 0
        assert "code_units_found" in result
        assert "test_files_found" in result

    def test_churn_after_analyze(self, base_url):
        """Run analyze first, then request churn data."""
        _request(base_url, "POST", "/call", {
            "tool": "analyze",
            "arguments": {},
        })
        status, body = _request(base_url, "POST", "/call", {
            "tool": "churn",
            "arguments": {"file_path": "app.py"},
        })
        assert status == 200
        result = body["result"]
        assert result is not None

    def test_impact_after_analyze(self, base_url):
        """Run analyze first, then request impact for a file."""
        _request(base_url, "POST", "/call", {
            "tool": "analyze",
            "arguments": {},
        })
        status, body = _request(base_url, "POST", "/call", {
            "tool": "impact",
            "arguments": {"files": ["app.py"]},
        })
        assert status == 200
        assert isinstance(body["result"], list)

    def test_history_after_analyze(self, base_url):
        """Run analyze first, then request history."""
        _request(base_url, "POST", "/call", {
            "tool": "analyze",
            "arguments": {},
        })
        status, body = _request(base_url, "POST", "/call", {
            "tool": "history",
            "arguments": {"file_path": "app.py"},
        })
        assert status == 200
        assert isinstance(body["result"], list)
        assert len(body["result"]) >= 1

    def test_stale_tests_after_analyze(self, base_url):
        """Run analyze first, then request stale tests."""
        _request(base_url, "POST", "/call", {
            "tool": "analyze",
            "arguments": {},
        })
        status, body = _request(base_url, "POST", "/call", {
            "tool": "stale_tests",
            "arguments": {},
        })
        assert status == 200
        assert isinstance(body["result"], list)

    def test_risk_map_after_analyze(self, base_url):
        """Run analyze first, then request the risk map."""
        _request(base_url, "POST", "/call", {
            "tool": "analyze",
            "arguments": {},
        })
        status, body = _request(base_url, "POST", "/call", {
            "tool": "risk_map",
            "arguments": {},
        })
        assert status == 200
        result = body["result"]
        assert isinstance(result, dict)
        assert "files" in result
        assert "_meta" in result
        assert isinstance(result["files"], list)

    def test_triage_after_analyze(self, base_url):
        """Run analyze first, then request triage."""
        _request(base_url, "POST", "/call", {
            "tool": "analyze",
            "arguments": {},
        })
        status, body = _request(base_url, "POST", "/call", {
            "tool": "triage",
            "arguments": {},
        })
        assert status == 200
        result = body["result"]
        assert "top_risk_files" in result
        assert "test_gaps" in result
        assert "summary" in result

    def test_suggest_tests_after_analyze(self, base_url):
        """Run analyze first, then request test suggestions."""
        _request(base_url, "POST", "/call", {
            "tool": "analyze",
            "arguments": {},
        })
        status, body = _request(base_url, "POST", "/call", {
            "tool": "suggest_tests",
            "arguments": {"file_path": "app.py"},
        })
        assert status == 200
        assert isinstance(body["result"], list)

    def test_analyze_with_force(self, base_url):
        """Invoke analyze with force=True."""
        status, body = _request(base_url, "POST", "/call", {
            "tool": "analyze",
            "arguments": {"force": True},
        })
        assert status == 200
        result = body["result"]
        assert result["code_units_found"] > 0


# ------------------------------------------------------------------ #
# Tests: Error handling
# ------------------------------------------------------------------ #

class TestErrorHandling:
    def test_unknown_tool(self, base_url):
        status, body = _request(base_url, "POST", "/call", {
            "tool": "nonexistent_tool",
            "arguments": {},
        })
        assert status == 404
        assert "error" in body
        assert "nonexistent_tool" in body["error"]

    def test_missing_tool_field(self, base_url):
        status, body = _request(base_url, "POST", "/call", {
            "arguments": {},
        })
        assert status == 400
        assert "error" in body

    def test_bad_json_body(self, base_url):
        """Send malformed JSON to POST /call."""
        url = base_url + "/call"
        req = urllib.request.Request(
            url,
            data=b"this is not json",
            method="POST",
        )
        req.add_header("Content-Type", "application/json")
        try:
            with urllib.request.urlopen(req) as resp:
                status = resp.status
                body = json.loads(resp.read())
        except urllib.error.HTTPError as exc:
            status = exc.code
            body = json.loads(exc.read())
        assert status == 400
        assert "error" in body

    def test_unknown_get_endpoint(self, base_url):
        status, body = _request(base_url, "GET", "/nonexistent")
        assert status == 404
        assert "error" in body

    def test_unknown_post_endpoint(self, base_url):
        status, body = _request(base_url, "POST", "/nonexistent", {})
        assert status == 404
        assert "error" in body

    def test_empty_body(self, base_url):
        """Send a POST /call with an empty body."""
        url = base_url + "/call"
        req = urllib.request.Request(url, data=None, method="POST")
        req.add_header("Content-Type", "application/json")
        req.add_header("Content-Length", "0")
        try:
            with urllib.request.urlopen(req) as resp:
                status = resp.status
                body = json.loads(resp.read())
        except urllib.error.HTTPError as exc:
            status = exc.code
            body = json.loads(exc.read())
        assert status == 400
        assert "error" in body

    def test_arguments_not_dict(self, base_url):
        """Send arguments as a list instead of a dict."""
        status, body = _request(base_url, "POST", "/call", {
            "tool": "analyze",
            "arguments": [1, 2, 3],
        })
        assert status == 400
        assert "error" in body


# ------------------------------------------------------------------ #
# Tests: Limit parameter pass-through
# ------------------------------------------------------------------ #

class TestNextSteps:
    def test_analyze_returns_next_steps(self, base_url):
        """Analyze should include next_steps in the response."""
        status, body = _request(base_url, "POST", "/call", {
            "tool": "analyze",
            "arguments": {},
        })
        assert status == 200
        assert "next_steps" in body
        assert isinstance(body["next_steps"], list)
        assert any(s.get("tool") == "risk_map" for s in body["next_steps"])

    def test_risk_map_returns_next_steps(self, base_url):
        """Risk map should include next_steps when results exist."""
        _request(base_url, "POST", "/call", {
            "tool": "analyze",
            "arguments": {},
        })
        status, body = _request(base_url, "POST", "/call", {
            "tool": "risk_map",
            "arguments": {},
        })
        assert status == 200
        assert "next_steps" in body
        assert len(body["next_steps"]) >= 1

    def test_churn_returns_next_steps(self, base_url):
        """Churn should include next_steps when results exist."""
        _request(base_url, "POST", "/call", {
            "tool": "analyze",
            "arguments": {},
        })
        status, body = _request(base_url, "POST", "/call", {
            "tool": "churn",
            "arguments": {"file_path": "app.py"},
        })
        assert status == 200
        assert "next_steps" in body
        assert any(s.get("tool") == "risk_map" for s in body["next_steps"])


class TestToolCall:
    def test_call_with_limit(self, base_url):
        """Verify that the limit parameter caps the number of results."""
        # Seed the database so history has data to return
        _request(base_url, "POST", "/call", {
            "tool": "analyze",
            "arguments": {},
        })
        status, body = _request(base_url, "POST", "/call", {
            "tool": "history",
            "arguments": {"file_path": "app.py", "limit": 1},
        })
        assert status == 200
        assert isinstance(body["result"], list)
        assert len(body["result"]) <= 1
