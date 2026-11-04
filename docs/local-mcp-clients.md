# Connect a Local MCP Client

Sam's Desktop Commander now includes a local Model Context Protocol (MCP) server.

## Transport and process model

- Transport: STDIO.
- A compatible AI client starts its own sam-desktop-mcp process.
- No TCP port or remote listener is opened.
- The desktop application does not need to stay open after folder approvals have been saved.
- The MCP server reads the same approved_folders.json file as the desktop app and reloads it before every tool call.

This is a local-client integration. Hosted ChatGPT cannot launch a process on this computer directly through STDIO. For eligible ChatGPT workspaces, OpenAI Secure MCP Tunnel can bridge a local STDIO command without adding a public HTTP listener. See [ChatGPT remote integration](CHATGPT_INTEGRATION.md) for account requirements and the setup boundary.

## Build the server

From the repository:

~~~bash
cd app/src-tauri
CARGO_BUILD_JOBS=1 cargo build --no-default-features --features mcp --bin sam-desktop-mcp --locked
~~~

The development executable is:

~~~text
app/src-tauri/target/debug/sam-desktop-mcp
~~~

The MCP server is an optional Cargo feature so the Tauri desktop application does not need to compile or link the MCP runtime.

## Example client configuration

Some local MCP clients accept a JSON configuration shaped like this. Use the absolute executable path shown in the application's MCP server page:

~~~json
{
  "mcpServers": {
    "sams-desktop-commander": {
      "command": "/absolute/path/to/sam-desktop-mcp",
      "args": []
    }
  }
}
~~~

Each AI application has its own configuration file and may use a different configuration format. Follow that client's documentation; do not assume one config location works for every client.

## Tools exposed in this first version

| Tool | Purpose |
|---|---|
| list_approved_folders | Return the saved approved folder roots |
| list_directory | List one directory inside an approved root |
| read_text_file | Read a UTF-8 text file up to 1 MiB inside an approved root |
| search_files | Search file and directory names recursively with depth and result caps |
| get_file_info | Read basic metadata for an approved file or directory |
| read_multiple_files | Read up to 10 approved UTF-8 files with a combined 4 MiB cap |
| search_content | Search bounded UTF-8 file contents recursively inside an approved folder |

## Permission boundaries

- No folders are approved by default.
- Paths outside approved roots are denied.
- Direct symbolic-link paths are rejected; symbolic links found during directory listing are skipped.
- Text reads are limited to 1 MiB.
- Folder approvals are reloaded before every MCP request, so a revocation in the desktop app blocks the next request.
- No MCP tool can write or delete files.
- No arbitrary shell or terminal command is exposed.
- Do not expose this STDIO server through a network wrapper or public tunnel.

## Protocol implementation and local test

The server uses the official Rust MCP SDK crate rmcp, pinned to version 3.6.0, with the server and STDIO transport features. A repeatable integration test launches the real server binary and checks initialization, tool discovery, default-deny behavior, approval reload, allowed directory listing and text reads, outside-root denial, revocation, and clean shutdown.

Run it from the repository root after building the binary:

~~~bash
cd app/src-tauri
CARGO_BUILD_JOBS=1 cargo build --no-default-features --features mcp --bin sam-desktop-mcp --locked
cd ../..
python3 scripts/test-mcp-stdio.py
~~~

The test uses temporary files and an isolated temporary `XDG_DATA_HOME`; it does not touch the user's real approvals. It requires only Python's standard library. Passing this test verifies the server's local protocol behavior, not compatibility with a specific AI application's configuration.

Hosted ChatGPT cannot launch a local STDIO process on this computer. Testing the server from this ChatGPT conversation therefore means running this local protocol harness on the machine; it is not a live hosted ChatGPT-to-local-MCP connection. For a future hosted connection, use OpenAI Secure MCP Tunnel with this STDIO binary; do not add a public listener. The tunnel remains disabled until the account, workspace, and Platform requirements in [ChatGPT remote integration](CHATGPT_INTEGRATION.md) are met.
