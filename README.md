# Sam's Desktop Commander

A local-first desktop application for Linux Mint that exposes carefully permissioned computer capabilities to compatible AI clients through the Model Context Protocol (MCP).

## Project status

**Phase:** Local desktop foundation with permissioned filesystem, interactive terminal, process management, and MCP activity history
**Initial platform:** Linux Mint (Ubuntu-based)
**Initial mode:** Local-only
**Remote integration:** Architecture documented for OpenAI Secure MCP Tunnel; not enabled. ChatGPT Free cannot currently add custom MCP servers.

### Implemented so far

- Tauri 2 + React + TypeScript desktop shell.
- Persistent, explicitly approved folder roots.
- Approved-folder browsing, bounded UTF-8 reads, line-range reads (including tail-style offsets), exact-count block editing, paginated recursive filename/content search, file metadata, and multi-file reads.
- Separate per-folder write approval with an in-app text editor, new file/folder creation, append, copy, rename, and confirmed file deletion; the same guarded operations are exposed through MCP.
- Opt-in one-shot MCP command execution with per-call confirmation, a 30-second timeout, bounded output, no shell interpretation, and a default blocklist for high-impact system commands.
- Interactive MCP terminal sessions with explicit start/close, bounded input/output, and a three-session limit.
- Current-user process inspection and confirmed SIGTERM controls, with the application and MCP processes protected.
- Local activity history shared by desktop and MCP actions, without storing file contents or command arguments.
- Interactive local PTY terminal using the desktop user's configured shell; the selected approved folder is only the initial working directory, not a sandbox.
- Unit tests for approval persistence, filesystem authorization, bounded reads, and process metadata filtering.
- Separate `sam-desktop-mcp` STDIO server built with the official Rust `rmcp` SDK.
- MCP requests reload the shared permission file, so folder revocation applies to the next request.
- MCP server binary is built separately from the Tauri desktop app to keep the desktop build lighter.

### Still outstanding

- MCP client compatibility testing across individual AI applications; the automated local STDIO integration test passes, but each client still needs its own configuration test.
- Terminal lifecycle tests in the running desktop app (shell exit, resize, output floods, and additional child-process cases).
- Process-exit race and stale-PID checks.
- Desktop automation and authenticated ChatGPT connectivity. ChatGPT Free currently cannot add custom MCP servers.

## Product goals

- Provide a native-feeling desktop interface.
- Offer filesystem, process, and terminal tools through a local MCP server.
- Require explicit permission boundaries and approval for sensitive actions.
- Support compatible AI clients through standard MCP transports where their capabilities allow.
- Remain independent of Desktop Commander's hosted service and usage points.

## Non-goals for the first milestone

- Publicly exposed remote server or relay.
- Unattended privileged commands.
- Desktop input automation until Linux display-session support and capture/control permissions are designed and tested.
- Claiming compatibility with every AI client without testing it.

## Proposed stack

- Desktop shell: Tauri 2
- UI: React + TypeScript
- Native application core: Rust
- Local persistence: SQLite, introduced when settings and audit history require it
- AI tool protocol: MCP, using a maintained official SDK where suitable

Tauri, Rust, React, and TypeScript are in the current implementation. Their dependency versions are recorded in the Cargo and npm lockfiles. Linux Mint 22.x build prerequisites and the tested development toolchain are documented in [Development setup](docs/DEVELOPMENT.md). SQLite remains a future option; it is not currently used.

## Documentation

- [Development setup and verification](docs/DEVELOPMENT.md)
- [Security architecture](docs/SECURITY_ARCHITECTURE.md)
- [Implementation roadmap](docs/IMPLEMENTATION_ROADMAP.md)
- [Local MCP client setup and protocol test](docs/local-mcp-clients.md)
- [MCP protocol details and repeatable integration test](docs/MCP_PROTOCOL.md)
- [ChatGPT remote integration requirements](docs/CHATGPT_INTEGRATION.md)
- [Linux desktop automation evaluation](docs/DESKTOP_AUTOMATION_EVALUATION.md)

## Development approach

Small, reviewable changes; a separate commit for each meaningful change; tests for security-sensitive behavior; and documentation that explains the code without overwhelming it.

## Roadmap

1. Architecture and environment verification
2. Tauri application shell and dashboard
3. Local MCP STDIO server and protocol smoke test — complete
4. Permissioned filesystem tools and desktop file manager — complete
5. Opt-in MCP command execution and interactive terminal sessions — implemented; lifecycle testing remains
6. Current-user process management and activity history — implemented
7. Desktop automation, after evaluating Linux display-session constraints — evaluation documented; implementation remains outstanding
8. Client compatibility tests
9. Linux packaging and installation guide
10. Optional authenticated remote connectivity

## Security baseline

The application must start with no approved folders and no remote listener. Paths must be canonicalized and checked against approved roots. Sensitive operations must be explicit, logged, and deny-by-default. Never treat MCP client identity alone as authorization.