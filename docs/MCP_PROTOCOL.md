# MCP Protocol and Client Integration

## Current state
- Local server binary: sam-desktop-mcp.
- Transport: STDIO.
- SDK: official Rust rmcp, pinned in the Cargo manifest.
- Tools currently exposed: list_approved_folders, list_directory, and read_text_file.
- A repeatable subprocess-based STDIO integration test covers initialization, tool discovery, default-deny behavior, approval reload, allowed reads, outside-root denial, revocation, and clean shutdown. Each target AI client still needs an independent compatibility check.

## Repeatable local protocol test

Build the MCP binary and run the integration test from the repository root:

~~~bash
cd app/src-tauri
CARGO_BUILD_JOBS=1 cargo build --no-default-features --features mcp --bin sam-desktop-mcp --locked
cd ../..
python3 scripts/test-mcp-stdio.py
~~~

The test launches a real server subprocess with an isolated temporary `XDG_DATA_HOME` and temporary fixture files. It does not read or modify the user's actual folder approvals. It verifies the JSON-RPC/MCP exchange over stdin/stdout and requires protocol stdout to remain free of diagnostic output. The test needs only Python's standard library and the already-built server binary.

This validates the server protocol and its permission behavior, not a hosted ChatGPT-to-local-STDIO connection. ChatGPT running in the cloud cannot start a process on this computer through STDIO. A future authenticated remote transport would be a separate project phase.

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
Do not add a public HTTP listener for ChatGPT integration. OpenAI Secure MCP Tunnel supports forwarding to this local STDIO command over an outbound-only connection. See [ChatGPT remote integration](CHATGPT_INTEGRATION.md). Keep this disabled until an eligible ChatGPT workspace, Platform tunnel, and runtime key are available. Never expose an unauthenticated local MCP endpoint.
