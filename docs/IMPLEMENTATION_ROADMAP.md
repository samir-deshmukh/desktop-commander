# Implementation Roadmap

Status labels distinguish existing foundation from planned work.

## Phase 0 — Repository and architecture
- [x] Independent local project exists.
- [x] Tauri 2 + React + TypeScript shell exists.
- [x] Local MCP STDIO server foundation and read-only filesystem tools exist.
- [x] Confirm remote GitHub repository linkage and publish repository documentation.
- [ ] Verify build/test commands from a clean checkout; document exact Linux Mint prerequisites and the limitation that a second full Rust build has not been run on the low-memory machine.

## Phase 1 — Read-only reliability
- [x] Review filesystem authorization, canonical containment, direct symlink rejection, bounded text reads, and fail-closed permission reload behavior; continue race-condition review before adding write capabilities.
- [x] Expand tests for outside-root paths, symlinks, file size bounds, invalid UTF-8, and permission revocation.
- [x] Add a repeatable real-subprocess STDIO integration test for initialization, tool discovery, approval/default-deny behavior, allowed reads, outside-root denial, revocation, and shutdown.
- [x] Add bounded recursive filename search and approved-path metadata inspection to MCP; test depth/result bounds and authorization.
- [x] Add bounded multi-file reads and recursive UTF-8 content search; test authorization and output limits.
- [ ] Test MCP protocol with target AI clients individually; hosted ChatGPT cannot launch this local STDIO process, so this remains an external-client compatibility item.
- [x] Document how approval state is persisted and protected, including the same-user limitation.

## Phase 2 — Controlled file changes
- [x] Design separate read and write permissions; read-only remains the default.
- [x] Require explicit confirmation fields for destructive MCP operations.
- [x] Add bounded UTF-8 writes using a temporary sibling file and atomic rename; require overwrite=true for existing files.
- [x] Restrict delete to individual regular files and refuse move destination overwrites.
- [x] Add isolated integration coverage for read-only denial, write approval, create, move, delete, and confirmation checks.
- [ ] Add persistent audit events and a recovery/backup workflow before expanding mutation capabilities.

## Phase 3 — Terminal and processes
- [x] Integrate a PTY backend (`portable-pty`) and terminal renderer (`xterm.js`).
- [x] Implement interactive shell sessions, input-size checks, terminal resize bounds, output events, and explicit close.
- [x] Document that the shell runs as the desktop user and the selected approved directory is not a sandbox.
- [x] Limit concurrent sessions, pace PTY output events, and kill/reap shells on close or app-state drop; reap sessions after shell exit.
- [x] Runtime-test cleanup behavior on app shutdown; a `sleep 300` child was not present in the immediate post-shutdown process check (single-case evidence, not a universal guarantee).
- [ ] Test close behavior, shell exit, resize, output floods, and additional child-process behavior in the running desktop app.
- [x] Add read-only process inspection for processes owned by the current Linux user, with a 500-process cap and no command-line/environment collection.
- [ ] Design carefully scoped process termination with explicit confirmation and ownership checks.
- [ ] Revisit whether any sensitive action approval can be enforced meaningfully; do not rely on command-string blacklists.

## Phase 4 — Linux desktop automation
- [ ] Identify active X11/Wayland session and Cinnamon compatibility.
- [ ] Evaluate accessibility and desktop automation candidates.
- [ ] Run automation in an isolated adapter where practical.
- [ ] Make capture/control permissions explicit and visible.

## Phase 5 — Packaging and remote access
- [ ] Validate packaging, update/signing, and install/uninstall behavior.
- [x] Verify current ChatGPT custom MCP requirements and select OpenAI Secure MCP Tunnel with the existing STDIO server; do not add a public listener.
- [ ] Set up and verify the tunnel only when an eligible ChatGPT workspace, Platform tunnel, and runtime credentials are available.
- [ ] Test resource usage and document limitations before calling the product production-ready.

Each meaningful implementation change should be committed separately and accompanied by tests or an explicit reason tests are not applicable.
