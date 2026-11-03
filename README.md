# Sam's Desktop Commander

A local-first desktop application for Linux Mint that exposes carefully permissioned computer capabilities to compatible AI clients through the Model Context Protocol (MCP).

## Project status

**Phase:** Local desktop foundation, restricted command runner, and read-only MCP server
**Initial platform:** Linux Mint (Ubuntu-based)
**Initial mode:** Local-only
**Remote access:** Out of scope until local security and core functionality are verified

### Implemented so far

- Tauri 2 + React + TypeScript desktop shell.
- Persistent, explicitly approved folder roots.
- Read-only directory browsing and bounded UTF-8 text previews.
- Restricted command runner for a fixed set of read-only diagnostics and Git inspection commands. It does not invoke a shell or accept arbitrary commands.
- Unit tests for approval persistence and command allowlisting.
- Separate `sam-desktop-mcp` STDIO server built with the official Rust `rmcp` SDK.
- Three read-only MCP tools: `list_approved_folders`, `list_directory`, and `read_text_file`.
- MCP requests reload the shared permission file, so folder revocation applies to the next request.
- MCP server binary is built separately from the Tauri desktop app to keep the desktop build lighter.

### Still outstanding

- MCP client compatibility testing across individual AI applications; the protocol smoke test passes, but each client still needs its own configuration test.
- File writes, file creation, and deletion through MCP.
- A full terminal with interactive sessions; arbitrary command execution remains disabled.
- Process management, activity audit history, desktop automation, and authenticated remote connectivity.

## Product goals

- Provide a native-feeling desktop interface.
- Offer filesystem, process, and terminal tools through a local MCP server.
- Require explicit permission boundaries and approval for sensitive actions.
- Support compatible AI clients through standard MCP transports where their capabilities allow.
- Remain independent of Desktop Commander's hosted service and usage points.

## Non-goals for the first milestone

- Publicly exposed remote server or relay.
- Unattended privileged commands.
- Broad desktop input automation before the permission model is implemented.
- Claiming compatibility with every AI client without testing it.

## Proposed stack

- Desktop shell: Tauri 2
- UI: React + TypeScript
- Native application core: Rust
- Local persistence: SQLite, introduced when settings and audit history require it
- AI tool protocol: MCP, using a maintained official SDK where suitable

These are initial architecture choices, not yet validated dependency versions. Dependency selection and Linux Mint build prerequisites will be checked before implementation.

## Development approach

Small, reviewable changes; a separate commit for each meaningful change; tests for security-sensitive behavior; and documentation that explains the code without overwhelming it.

## Roadmap

1. Architecture and environment verification
2. Tauri application shell and dashboard
3. Local MCP STDIO server and protocol smoke test — complete
4. Approved-folder read-only filesystem tools — complete
5. File writes and command execution with policy checks and approval
6. Process management and audit history
7. Desktop automation, only after evaluating Linux display-session constraints
8. Client compatibility tests
9. Linux packaging and installation guide
10. Optional authenticated remote connectivity

## Security baseline

The application must start with no approved folders and no remote listener. Paths must be canonicalized and checked against approved roots. Sensitive operations must be explicit, logged, and deny-by-default. Never treat MCP client identity alone as authorization.