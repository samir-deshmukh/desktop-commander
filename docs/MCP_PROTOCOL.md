# MCP Protocol and Client Integration

## Current state
- Local server binary: sam-desktop-mcp.
- Transport: STDIO.
- SDK: official Rust rmcp, pinned in the Cargo manifest.
- Tools currently exposed: list_approved_folders, list_directory, and read_text_file.
- Protocol initialization, tool discovery, and tool calls have a local smoke test; each target AI client still needs an independent compatibility check.

## Adapter rule
MCP handlers validate protocol inputs, then call shared core services. They must not implement their own filesystem authorization. UI and MCP requests use the same policy layer.

## STDIO lifecycle
The MCP client launches the executable and communicates over stdin/stdout. Protocol messages go to stdout; diagnostic logs must go to stderr. Do not assume the desktop window remains open. Permission changes must be read from authoritative persisted state on each request or delivered through a reliable shared state mechanism.

## Client test matrix
For every supported client, record:
- client name/version and OS;
- exact configuration and executable path;
- successful initialization and tool discovery;
- successful allowed read;
- denial for unapproved path;
- behavior after folder revocation;
- stderr/log behavior and process shutdown.

## Remote transport gate
Streamable HTTP is not part of the initial local milestone. Before enabling it, verify the current MCP specification and SDK behavior, then design authentication, TLS, origin/host validation, explicit listener binding, rate limiting, access revocation, and abuse monitoring. Never expose an unauthenticated local MCP endpoint.
