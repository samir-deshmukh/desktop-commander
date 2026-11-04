# HTTP MCP endpoint

The desktop MCP server supports Streamable HTTP for clients that connect to an HTTP MCP endpoint.

## Run locally

```bash
./scripts/run-mcp-http.sh
```

The server listens only on `127.0.0.1:18765` and exposes `/mcp`. The launcher creates a random bearer token at `~/.config/sam-desktop/mcp-http-token` with owner-only permissions. Requests without `Authorization: Bearer <token>` are rejected with HTTP 401.

The existing STDIO executable remains available for local MCP clients. This HTTP mode is intended to sit behind a secure, authenticated remote access layer; do not expose port 18765 directly to the internet.

## Current remote-connection limitation

A public ChatGPT connection still requires a stable HTTPS URL and an authentication flow accepted by the ChatGPT connector (OAuth). The local bearer-token endpoint alone is not a complete public deployment. Do not select unauthenticated access for this filesystem/terminal server.
