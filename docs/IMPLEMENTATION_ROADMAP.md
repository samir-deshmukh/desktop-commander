# Implementation Roadmap

Status labels distinguish existing foundation from planned work.

## Phase 0 — Repository and architecture
- [x] Independent local project exists.
- [x] Tauri 2 + React + TypeScript shell exists.
- [x] Local MCP STDIO server foundation and read-only filesystem tools exist.
- [ ] Confirm remote GitHub repository linkage and publish documentation.
- [ ] Verify build/test commands from a clean checkout and document exact Linux Mint prerequisites.

## Phase 1 — Read-only reliability
- [ ] Review every filesystem path check and error path.
- [ ] Expand tests for traversal, symlinks, file size bounds, and permission revocation.
- [ ] Test MCP protocol with target clients individually.
- [ ] Document how approval state is persisted and protected.

## Phase 2 — Controlled file changes
- [ ] Design separate read/write/delete permissions.
- [ ] Add preview and explicit confirmation for destructive changes.
- [ ] Define atomic write, overwrite, backup, and recovery semantics.
- [ ] Add audit events and tests before exposing mutation tools.

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
- [ ] Remote connectivity remains disabled until a separate threat-model and authentication design passes review.
- [ ] Test resource usage and document limitations before calling the product production-ready.

Each meaningful implementation change should be committed separately and accompanied by tests or an explicit reason tests are not applicable.
