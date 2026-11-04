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

## Terminal policy
A command-string blacklist is not a reliable security boundary: shells, interpreters, aliases, scripts, and child processes can change command meaning. Do not claim otherwise.

The desktop UI currently exposes an interactive local PTY using the user's configured shell (falling back to `/bin/bash`). It runs as the desktop account and starts in an approved directory, but that directory is only the initial working directory—not a sandbox or restriction on shell access. Shell commands can access anything permitted to the desktop account. There is no command allowlist and no silent root elevation.

Current terminal controls include bounded input chunks, clamped terminal dimensions, a three-session limit, paced PTY output events, session IDs, explicit close, and shell cleanup when session state is dropped. These controls do not sandbox commands or guarantee that child processes spawned by a shell are contained. Process inspection/termination and full runtime lifecycle testing remain follow-up work. Do not describe the terminal as isolated or production-hardened. Users must not run untrusted commands.

Optional isolation (for example, a dedicated sandbox profile) is defense in depth, not a substitute for user consent or OS permissions. Do not promise containment until the selected sandbox is tested on supported Mint configurations.

## MCP and networking
- STDIO is the default local transport; it does not require a listening TCP port.
- Do not wrap the local server in a public tunnel.
- Remote transport is a separate future phase requiring authentication, encrypted transport, origin/host validation where applicable, explicit activation, rate limits, revocation, and threat-model review.
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
