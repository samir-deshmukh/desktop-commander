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
| read_file | Read a UTF-8 file by zero-based or negative line offset (tail-style) |
| edit_block | Replace exact text with an expected match count and atomic write |
| search_files | Search file and directory names recursively with depth and result caps |
| start_search | Start a bounded paginated filename or content search |
| get_more_search_results | Read the next page of search matches |
| list_searches | List active paginated search sessions |
| stop_search | Stop a search and release retained results |
| get_file_info | Read basic metadata for an approved file or directory |
| read_multiple_files | Read up to 10 approved UTF-8 files with a combined 4 MiB cap |
| search_content | Search bounded UTF-8 file contents recursively inside an approved folder |
| write_text_file | Create or replace a UTF-8 text file inside a separately writable folder |
| create_directory | Create one directory inside a separately writable folder |
| move_file | Move or rename one regular file inside writable folders |
| append_file | Append UTF-8 text while keeping the combined file within 1 MiB |
| copy_file | Copy a regular file up to 50 MiB into a writable folder after confirmation |
| delete_file | Delete one regular file after explicit confirmation |
| run_command | Run one executable directly in an approved working directory, with bounded output and timeout |
| start_terminal | Start an interactive local shell session (maximum 3) |
| write_terminal | Send bounded input to an active terminal session |
| read_terminal | Read bounded output from an active terminal session |
| close_terminal | Close a terminal session and terminate its shell |
| list_terminal_sessions | List active MCP terminal session IDs |
| list_processes | Inspect metadata for current-user Linux processes |
| terminate_process | Send SIGTERM to a current-user process after explicit confirmation |
| get_recent_tool_calls | Read recent MCP activity metadata |
| get_usage_stats | Aggregate recent MCP tool usage counts |

## Permission boundaries

- No folders are approved by default.
- Paths outside approved roots are denied.
- Direct symbolic-link paths are rejected; symbolic links found during directory listing are skipped.
- Text reads are limited to 1 MiB. Search sessions retain at most 1,000 matches, return 50 per page, and allow at most five active sessions.
- Folder approvals are reloaded before every MCP request, so a revocation in the desktop app blocks the next request.
- Read approval and write approval are separate. A folder is read-only unless the owner explicitly enables changes in the desktop app.
- Text writes are limited to 1 MiB and use a temporary sibling file followed by atomic rename. The desktop file manager uses the same write policy and limit.
- Existing files require overwrite=true; delete, move, and directory creation require confirm=true.
- Delete only handles regular files; directory deletion and recursive deletion are not exposed.
- Move and copy refuse to overwrite an existing destination. Copy is capped at 50 MiB; append keeps the combined UTF-8 file within 1 MiB.
- MCP command execution is disabled by default and must be enabled in the desktop app. Each call also requires confirm=true.
- Commands are launched directly without shell interpretation, with a 30-second timeout and 256 KiB caps for stdout and stderr.
- Commands run with the desktop user's normal OS permissions; an approved working directory is not a sandbox.
- Interactive MCP terminal sessions use a local PTY, require command access to be enabled and confirm=true when starting, and are limited to three concurrent sessions. Disabling command access closes an active session on its next read or input request.
- Terminal input is capped at 16 KiB per call; output is delivered in chunks through a bounded channel. Closing the session kills and reaps its shell.
- Process inspection is limited to the current Linux user, capped at 500 entries, and omits command-line arguments and environment variables.
- Process termination requires confirm=true and sends SIGTERM only. PID 1, the current MCP process, and Sam's Desktop Commander processes are protected.
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

## Activity history

Desktop and MCP actions append local audit events to activity.jsonl. Events include timestamp, source, action, target metadata, and outcome. File contents and command arguments are excluded. The log rotates after approximately 1 MiB. The desktop Activity page displays the latest 100 events.
