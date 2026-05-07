# Grok Connectors / BYO MCP Support

This project is ready for Grok Connectors (Bring Your Own MCP) and Grok Build.

The MCP server provides test impact analysis, risk mapping, churn analysis, and code intelligence for LLM agents. It works with any MCP-compatible client, including future Grok Build local agents and remote MCP connections in Grok.

## For Grok Connectors (BYO MCP)

When Grok supports adding custom MCP servers:

1. Start the server locally (example for this project):
   ```bash
   python -m chisel.mcp_server --port 8377
   ```
   Or use stdio mode for local agents if available.

2. For remote access from Grok, expose the HTTP endpoint over HTTPS using a secure tunnel.

3. Add to Grok Connectors / remote MCP config:
   - **server_url**: `https://your-https-tunnel/mcp`
   - **server_label**: `chisel`
   - **server_description**: "Test impact analysis, risk assessment, churn patterns, and code ownership intelligence for safe LLM-driven code changes. Cross-language support."

The server implements MCP tools for impact queries, risk scoring, and ownership mapping. Grok will discover the tools automatically.

## For Grok Build (Local)

Once Grok Build is available:
- Use Chisel alongside Grok Build agents to automatically assess the blast radius of proposed changes, prioritize low-risk paths, and surface high-impact diffs for review (or auto-approve safe ones).
- Prevents risky autonomous changes by providing precise test and ownership data.

No changes to your existing workflows are required. This project was built to be backend-agnostic and work with any LLM/IDE/CLI via MCP.

See README.md for full setup.

For questions or to contribute Grok-specific adapters, open an issue.