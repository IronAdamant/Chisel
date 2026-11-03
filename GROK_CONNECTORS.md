# Grok Connectors / BYO MCP Support

Chisel's MCP server is the Rust binary over stdio. It is not an HTTP URL and it is not the Python package.

## Local MCP

```bash
cargo build --release
```

Point the client at the binary:

- **command**: `chisel-mcp` (or `chisel serve-mcp`)
- **env**: `CHISEL_PROJECT_DIR` set to the project root, and optionally `CHISEL_STORAGE_DIR` set to `<project>/.chisel`

`chisel-mcp` speaks newline-delimited JSON-RPC on stdin and stdout, and also accepts Content-Length framing. The first tool call on an empty snapshot returns `status: "no_data"`. Call `analyze`, then `diff_impact` after edits.

## HTTP

`chisel serve --port 8377` is a one-shot JSON API:

- `GET /health`
- `GET /tools`
- `POST /call` with `{"tool","arguments"}`

That port is not MCP. Do not configure a connector `server_url` of `https://…/mcp` against it.

## What the agent should branch on

`no_data`, `no_changes`, `no_edges`, `git_error`, `stale_db`, `error`, `unsupported`, `auto_queued`. Pass `--json` on the CLI. Exit code alone is not enough: `no_data` and `stale_db` exit 0.

Storage is the snapshot `.chisel/store`. Advisory file locks are records in that snapshot. They do not lock source files on disk.
