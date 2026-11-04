# ChatGPT Remote Integration

## Goal

Connect ChatGPT to the local Sam's Desktop Commander MCP server while keeping the MCP process and filesystem on this Linux computer.

## Supported architecture

Use OpenAI Secure MCP Tunnel with the existing STDIO server. Do not add a public HTTP listener or expose a local port.

The data path is:

1. ChatGPT sends an MCP request to an OpenAI-hosted tunnel endpoint.
2. The local `tunnel-client` process polls OpenAI over outbound HTTPS.
3. `tunnel-client` forwards the request to the local `sam-desktop-mcp` executable over STDIO.
4. The local MCP server applies the existing folder approval policy and returns the result through the tunnel.

The desktop app does not need a new HTTP transport for this connection. OpenAI's tunnel client supports a local MCP command over STDIO.

## Account and access requirements

ChatGPT custom MCP connections are currently available on ChatGPT Business and Enterprise/Edu. Pro can connect custom MCP servers with read/fetch permissions only. ChatGPT Free does not currently have custom MCP server connection access.

Secure MCP Tunnel also requires:
- A tunnel created in an OpenAI Platform organization.
- A runtime API key for `tunnel-client`.
- Platform permissions: Tunnels Read + Manage to create/edit a tunnel, and Tunnels Read + Use to run it.
- Permission in the target ChatGPT workspace to add and use a custom MCP server.

These are separate requirements. Having a Platform tunnel does not grant ChatGPT workspace access.

## Current project status

The local MCP server now exposes filesystem, terminal, process, audit-history, and usage-statistics tools. Its current tool catalog is documented in the local MCP client setup guide.

Permission gates remain local:
- Folder reads require an approved folder.
- File mutations require a separate writable-folder approval.
- Command execution is disabled by default and must be enabled in the desktop app.
- Each command and terminal start requires an explicit confirmation field.
- Process termination requires explicit confirmation and is limited to eligible current-user processes.

The local terminal and MCP command runner execute with the desktop user's normal OS permissions. An approved working directory is not a sandbox. Desktop automation is not implemented yet.

## Setup outline (only after account access is available)

1. Build the MCP binary:

   ```bash
   cd ~/Projects/sams-desktop/app/src-tauri
   CARGO_BUILD_JOBS=1 cargo build --no-default-features --features mcp --bin sam-desktop-mcp --locked
   ```

2. Install the current official `tunnel-client` release from OpenAI's Platform tunnel settings or official release page.
3. Enter the runtime API key locally in the shell environment. Never paste it into ChatGPT, source files, command history, screenshots, or project documentation.
4. Create a named STDIO profile using the tunnel ID and the absolute path to `sam-desktop-mcp`. Follow the current `tunnel-client help quickstart`; its CLI flags may change.
5. Run `tunnel-client doctor --profile <profile> --explain`, then start the profile.
6. In ChatGPT web, add a custom MCP server and choose the Tunnel connection. This step requires an eligible ChatGPT workspace and the relevant workspace permissions.
7. Keep writable-folder approvals and MCP command execution disabled initially. Verify read access and denial outside approved roots first; enable write or command access only from the local desktop app when needed.

## Security rules

- Never expose an unauthenticated HTTP listener or use a generic public tunnel.
- Keep the tunnel runtime key outside the repository and outside chat messages.
- Keep folder approvals empty by default; the desktop owner must approve roots locally.
- Treat all MCP inputs and file contents as untrusted.
- Keep write, delete, command, terminal, and process-termination operations behind their existing local permission and confirmation gates; extend tests before broadening them.
- Keep the tunnel client stopped unless remote access is intentionally being used.
- Revoke the tunnel and runtime key if access is no longer needed.

## Official references

- [OpenAI Secure MCP Tunnel](https://developers.openai.com/api/docs/guides/secure-mcp-tunnels)
- [Connecting custom MCP servers in ChatGPT](https://help.openai.com/en/articles/12584461-connecting-custom-mcp-servers-in-chatgpt)

