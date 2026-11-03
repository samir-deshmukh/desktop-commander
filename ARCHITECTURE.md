# Architecture Decisions

## 1. Product boundary

Sam's Desktop Commander is an independent desktop app. It may learn from open-source projects, but reuse must be verified against each repository's license and notices. It does not depend on Desktop Commander's hosted relay or points system.

## 2. Initial topology

```text
Compatible MCP client
        |
        | local MCP transport (initially STDIO)
        v
Sam's Desktop Commander MCP host
        |
        +-- Policy and approval layer
        |       +-- approved filesystem roots
        |       +-- command rules
        |       +-- action audit records
        |
        +-- Capability modules
        |       +-- filesystem (first)
        |       +-- terminal (restricted read-only command runner initially)
        |       +-- process control (later)
        |       +-- desktop automation (later)
        |
        +-- OS integration (Linux)

Tauri desktop UI <----> Rust application core
```

The UI and MCP server should call the same Rust capability and policy layer. Avoid implementing separate, weaker authorization logic for MCP calls.

## 3. Process model

- Tauri provides the desktop window and frontend bridge.
- Rust owns filesystem operations, process creation, policy checks, and durable settings.
- The MCP server is a separate `sam-desktop-mcp` binary, compiled with the `mcp` Cargo feature and the official Rust `rmcp` SDK. The initial client transport is STDIO: each compatible local AI client launches its own server process, so no network port is opened. The Tauri app and MCP process share the same `approved_folders.json` file. The MCP process reloads that file before each tool call so UI revocations apply to subsequent requests.
- Remote Streamable HTTP is postponed. When considered, it requires authentication, transport protection, explicit user activation, and a threat-model review.

## 4. Permission model

- Default: no approved user folders.
- Filesystem access: canonicalize the requested path and ensure the final target remains within an approved root. MCP directory listing skips symbolic links; direct symbolic-link paths are rejected. MCP text reads are UTF-8 only and capped at 1 MiB. Permissions are reloaded before each MCP tool call. Re-check before future writes and deletes to reduce time-of-check/time-of-use risk.
- Read, create/update, and delete permissions should be separable.
- Terminal: the initial UI exposes only a fixed allowlist of read-only diagnostic and Git inspection commands. Commands are mapped to executable + argument arrays; no shell is invoked and arbitrary user command text is rejected. Full interactive execution remains disabled until command-level permissions, approvals, process limits, and audit history are designed. Do not rely only on a blacklist; shell syntax, wrappers, and child processes make blacklist-only controls insufficient.
- Destructive and privileged actions require explicit approval. The application must not silently elevate privileges.
- Audit log records tool, time, decision, and bounded result metadata; avoid storing secrets or full sensitive file contents.

## 5. Implementation sequence

1. Verify Linux Mint prerequisites and selected dependency versions.
2. Create a minimal Tauri + React + TypeScript shell and build it locally.
3. Establish Rust module boundaries and unit-testable policy interfaces.
4. Add MCP initialization and STDIO transport using the official `rmcp` SDK — complete.
5. Add approved-root filesystem listing/reading and tests for path traversal, symlink rejection, size limits, and live revocation — complete.
6. Add writes only after the read-only policy is verified.
7. Add command execution and process management with approvals.
8. Evaluate desktop automation against the active X11/Wayland session; document capability limits.
9. Add client-specific connection guides and test each client instead of assuming universal support.

## 6. Initial module layout (target)

```text
sams-desktop/
  src/                    # React UI
  src-tauri/
    src/
      main.rs             # Tauri entry point
      app/                 # app state and command handlers
      policy/              # authorization and approvals
      capabilities/         # filesystem, process, desktop modules
      mcp/                 # MCP server and tool registration
      audit/               # event recording
  docs/
  tests/
```

This is a target structure, not a claim that these source files have already been generated.

## 7. Decisions to validate

- Current stable Tauri and MCP Rust SDK versions at implementation time.
- Whether the chosen MCP SDK supports the required local STDIO lifecycle cleanly.
- Linux Mint's active display server and available desktop automation APIs.
- Packaging format and dependency requirements for the target Mint release.