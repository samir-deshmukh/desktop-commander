# Connect a Local MCP Client

Sam's Desktop Commander now includes a local Model Context Protocol (MCP) server.

## Transport and process model

- Transport: STDIO.
- A compatible AI client starts its own sam-desktop-mcp process.
- No TCP port or remote listener is opened.
- The desktop application does not need to stay open after folder approvals have been saved.
- The MCP server reads the same approved_folders.json file as the desktop app and reloads it before every tool call.

This is a local-client integration. Remote ChatGPT or other hosted clients cannot launch a process on this computer through STDIO; remote connectivity is a separate future phase that will require authenticated Streamable HTTP.

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

## Permission boundaries

- No folders are approved by default.
- Paths outside approved roots are denied.
- Direct symbolic-link paths are rejected; symbolic links found during directory listing are skipped.
- Text reads are limited to 1 MiB.
- Folder approvals are reloaded before every MCP request, so a revocation in the desktop app blocks the next request.
- No MCP tool can write or delete files.
- No arbitrary shell or terminal command is exposed.
- Do not expose this STDIO server through a network wrapper or public tunnel.

## Protocol implementation

The server uses the official Rust MCP SDK crate rmcp, pinned to version 3.6.0, with the server and STDIO transport features. Protocol initialization, tool discovery, and tool calls have been smoke-tested locally. Individual AI-client compatibility still needs to be verified separately.
