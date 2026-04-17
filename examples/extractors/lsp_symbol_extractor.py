"""Generic LSP-based symbol extractor.

This extractor spawns a language server and queries document symbols,
then maps them to Chisel CodeUnits. It works with any LSP server that
supports textDocument/documentSymbol (e.g. pylsp, typescript-language-server,
rust-analyzer, clangd).

Install an LSP server in your environment (not in Chisel's core):
    pip install python-lsp-server
    # or
    npm install -g typescript-language-server

Usage:
    export CHISEL_BOOTSTRAP=examples.extractors.lsp_symbol_extractor
    chisel analyze .
"""

import json
import subprocess
import threading

from chisel.ast_utils import CodeUnit, register_extractor


class LSPClient:
    """Minimal JSON-RPC LSP client over stdio."""

    def __init__(self, cmd):
        self.proc = subprocess.Popen(
            cmd,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
        self._lock = threading.Lock()
        self._id = 0

    def _send(self, method, params):
        with self._lock:
            self._id += 1
            msg = {
                "jsonrpc": "2.0",
                "id": self._id,
                "method": method,
                "params": params,
            }
            payload = json.dumps(msg)
            data = f"Content-Length: {len(payload)}\r\n\r\n{payload}"
            self.proc.stdin.write(data.encode("utf-8"))
            self.proc.stdin.flush()
            return self._id

    def _read_response(self, expected_id):
        while True:
            headers = {}
            while True:
                line = self.proc.stdout.readline().decode("utf-8").strip()
                if line == "":
                    break
                key, value = line.split(":", 1)
                headers[key.strip().lower()] = value.strip()
            length = int(headers.get("content-length", 0))
            body = self.proc.stdout.read(length).decode("utf-8")
            resp = json.loads(body)
            if resp.get("id") == expected_id:
                return resp.get("result")

    def initialize(self, root_uri):
        req_id = self._send("initialize", {
            "processId": None,
            "rootUri": root_uri,
            "capabilities": {"textDocument": {"documentSymbol": {"hierarchicalDocumentSymbolSupport": True}}},
        })
        return self._read_response(req_id)

    def document_symbol(self, uri):
        req_id = self._send("textDocument/documentSymbol", {
            "textDocument": {"uri": uri},
        })
        return self._read_response(req_id)

    def shutdown(self):
        req_id = self._send("shutdown", {})
        self._read_response(req_id)
        self.proc.stdin.close()
        self.proc.wait()


def _symbol_to_units(symbol, file_path, units):
    kind = symbol.get("kind", 0)
    name = symbol.get("name", "anonymous")
    # LSP SymbolKind: Function=12, Method=6, Class=5, Interface=11, Struct=23, Enum=10
    unit_type = None
    if kind in (12, 6):
        unit_type = "function"
    elif kind == 5:
        unit_type = "class"
    elif kind == 11:
        unit_type = "interface"
    elif kind in (23, 10, 22):
        unit_type = "struct"

    if unit_type and "range" in symbol:
        rng = symbol["range"]
        units.append(CodeUnit(
            file_path=file_path,
            name=name,
            unit_type=unit_type,
            line_start=rng["start"]["line"] + 1,
            line_end=rng["end"]["line"] + 1,
        ))

    for child in symbol.get("children", []):
        _symbol_to_units(child, file_path, units)


# Map file extensions to LSP server commands
_LSP_COMMANDS = {
    ".py": ["pylsp"],
    ".js": ["typescript-language-server", "--stdio"],
    ".ts": ["typescript-language-server", "--stdio"],
    ".rs": ["rust-analyzer"],
    ".cpp": ["clangd"],
    ".c": ["clangd"],
}


def lsp_symbol_extractor(file_path: str, content: str) -> list[CodeUnit]:
    """Extract code units via LSP documentSymbol."""
    ext = "." + file_path.rsplit(".", 1)[-1].lower()
    cmd = _LSP_COMMANDS.get(ext)
    if not cmd:
        return []

    client = LSPClient(cmd)
    try:
        import urllib.parse
        uri = urllib.parse.urljoin("file:", urllib.parse.quote(file_path))
        root = uri.rsplit("/", 1)[0] + "/"
        client.initialize(root)
        symbols = client.document_symbol(uri) or []
        units = []
        for sym in symbols:
            _symbol_to_units(sym, file_path, units)
        return units
    finally:
        client.shutdown()


# Register for multiple languages so Chisel can use it as a fallback
for _ext in _LSP_COMMANDS:
    _lang = {".py": "python", ".js": "javascript", ".ts": "typescript",
             ".rs": "rust", ".cpp": "cpp", ".c": "c"}.get(_ext)
    if _lang:
        register_extractor(_lang, lsp_symbol_extractor)
