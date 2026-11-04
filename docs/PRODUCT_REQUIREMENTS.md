# Product Requirements

Status: working requirements for the local-first milestone. Requirements do not imply every capability exists today.

## Purpose
Provide a user-controlled Linux desktop application that exposes selected computer capabilities to compatible AI clients through MCP. Users must understand and revoke what an AI client can access.

## Initial user and platform
- Primary user: one desktop owner operating their own machine.
- First platform: Linux Mint; support details depend on the active Cinnamon/X11/Wayland session.
- Initial connection: local MCP STDIO process launched by an MCP-capable client.
- Network access and multi-user hosting are excluded from the first milestone.

## Functional requirements
1. Show app status, approved locations, and the available local MCP server.
2. Let the user explicitly approve and revoke filesystem roots.
3. Provide directory listing and bounded text reading within approved roots.
4. Add file mutation only after write/delete permissions, confirmation, and audit behavior are implemented.
5. Provide a real local PTY terminal. Clearly disclose that it runs with desktop-user permissions and the approved starting folder is not a sandbox; harden session lifecycle and output handling before claiming production readiness.
6. Add process inspection and termination with explicit action policy.
7. Add desktop input automation only after validating the active display backend and consent model.
8. Expose capabilities through one shared Rust core, used by both UI and MCP adapters.
9. Record security-relevant decisions without retaining sensitive file contents by default.
10. Document and test each supported MCP client independently.

## Non-functional requirements
- Local-first: no required cloud account, hosted relay, or remote service.
- No network listener by default.
- Low idle resource use on modest hardware; start expensive adapters only when needed.
- Explicit errors, bounded output, and cancellation for long-running tasks.
- Deterministic permission checks and tests for traversal, symlinks, revoked roots, and size limits.
- Small reviewable changes; each meaningful change has a dedicated commit.

## First milestone acceptance
- Desktop app builds on the documented Linux Mint setup.
- MCP server initializes over STDIO and lists/calls its tools.
- Unapproved paths are denied; approved read operations work; revoked paths are denied on the next request.
- File-write and delete capabilities are not exposed. The desktop UI intentionally exposes a local interactive shell; it is not sandboxed. No remote-network capability is exposed.
- Build/test commands and known limitations are documented.
