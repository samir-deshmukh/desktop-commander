# Security Architecture

Status: baseline design; review whenever a new capability or transport is introduced.

## Security goals
- The desktop owner controls the scope of access.
- MCP tool requests are untrusted, even when launched by a familiar AI client.
- Authorization is enforced in the shared Rust capability layer, not only in the UI.
- Default configuration grants no approved folders and opens no network listener.
- Security decisions are auditable without logging secrets or full file contents.

## Trust boundaries
1. AI client/model: untrusted input; can request actions and provide malicious paths or arguments.
2. MCP adapter: protocol boundary; validates request shape and delegates to core.
3. Policy/approval layer: authoritative decision point.
4. Capability implementation: performs bounded OS operations only after authorization.
5. Operating system/user account: actual privilege boundary. SAM must not silently elevate.

## Filesystem policy
- Canonicalize paths and verify containment under a currently approved root.
- Reject direct symbolic-link targets and do not follow symlinks during traversal unless a future reviewed policy explicitly permits it.
- Re-check authorization immediately before each operation; mitigate time-of-check/time-of-use races where platform semantics permit.
- Keep separate capabilities for list/read/create/update/delete.
- Bound file size, output size, traversal depth, and execution time.
- Treat file names and contents as untrusted data, never as policy instructions.

## Approval storage

The desktop application stores versioned folder approvals in Tauri's application data directory as `approved_folders.json`. On Linux, Tauri resolves this under the user's application data location (normally `$XDG_DATA_HOME/com.samdeshmukh.desktopcommander`, or `~/.local/share/com.samdeshmukh.desktopcommander` when `XDG_DATA_HOME` is unset). The standalone MCP process uses the same identifier and path convention. The file contains canonical folder paths, not file contents or credentials. Each MCP request reloads the file; malformed or unsupported data fails closed and clears the in-memory approval set. Updates are written to a sibling temporary file and renamed into place. The file is protected by the operating system's user-home permissions; this is not protection against another process running as the same user. Do not manually edit it while either process is active.

## Terminal policy
A command-string blacklist is not a reliable security boundary: shells, interpreters, aliases, scripts, and child processes can change command meaning. Do not claim otherwise.

The desktop UI currently exposes an interactive local PTY using the user's configured shell (falling back to `/bin/bash`). It runs as the desktop account and starts in an approved directory, but that directory is only the initial working directory—not a sandbox or restriction on shell access. Shell commands can access anything permitted to the desktop account. There is no command allowlist and no silent root elevation.

The separate one-shot MCP `run_command` capability launches an executable directly with argv (no shell parsing), requires app-level opt-in and per-call confirmation, and applies a default blocklist to high-impact system administration, disk/partition, shutdown, firewall, and process-termination commands. It checks executable basenames after resolving paths and recognizes selected `env`/`busybox` wrappers plus dangerous `systemctl`/`loginctl` actions. This reduces accidental or straightforward dangerous invocations; it is not a security boundary. An allowed executable can run arbitrary code, invoke other programs, or modify user data. The interactive PTY does not use this blocklist.

Current terminal controls include bounded input chunks, clamped terminal dimensions, a three-session limit, paced PTY output events, session IDs, explicit close, and shell cleanup when session state is dropped. A runtime check with `sleep 300` did not find that child process after app shutdown; this is evidence for that test case, not a guarantee that every descendant process is contained. Read-only process inspection is available for processes owned by the current Linux user; it omits command-line arguments and environment variables and is capped at 500 entries. Process termination sends SIGTERM only after confirmation and rejects PID 1, the desktop application's process ancestry, known desktop/login/session infrastructure, and Sam's Desktop Commander processes. Additional process-exit race and stale-PID tests remain outstanding. Do not describe the terminal as isolated or production-hardened. Users must not run untrusted commands.

Optional isolation (for example, a dedicated sandbox profile) is defense in depth, not a substitute for user consent or OS permissions. Do not promise containment until the selected sandbox is tested on supported Mint configurations.

## Desktop accessibility inspection
The MCP server has two read-only Linux AT-SPI2 tools: application discovery and a bounded accessibility-tree snapshot for one exact application name. Each call requires `confirm=true`; tree output is limited to 200 nodes, depth 8, and 160 characters per accessible name. Names may contain visible application text, so a snapshot is treated as user-authorized disclosure to the connected MCP client. The adapter only reads Accessible metadata and child references. It does not invoke Action or Component input methods, synthesize keyboard/pointer events, capture screenshots, inspect clipboard contents, or alter desktop settings. It fails closed when the session accessibility status is disabled and never enables it automatically. This is read-only inspection, not desktop control.

## MCP and networking
- STDIO is the default local transport; it does not require a listening TCP port.
- Do not expose a public HTTP listener or use a generic public tunnel.
- For eligible OpenAI products, the documented remote path is OpenAI Secure MCP Tunnel forwarding to the local STDIO command over outbound HTTPS. It keeps the MCP server private; it does not replace filesystem authorization.
- Keep tunnel operation opt-in. Store runtime credentials outside the repository and never log them.
- MCP client identity alone is not authorization.

## Audit and privacy
Record action type, timestamp, policy decision, and bounded metadata. Avoid storing secrets, tokens, or full user-file contents. Make retention and clearing behavior visible.

## Security test cases
- Parent traversal and encoded path variants.
- Symlink inside an approved root pointing outside it.
- Root revocation while MCP server remains running.
- Oversized files and output floods.
- Concurrent rename/replacement between validation and operation.
- Malformed MCP requests and repeated requests.
- Terminal input is intentionally interpreted by the user's shell; verify and document that the PTY is powerful local shell access, not an argument-safe command API.
- Confirm no remote listener starts in default mode.
- Accessibility inspection requires per-call confirmation, respects disabled AT-SPI status, and never changes the user's desktop accessibility setting.
