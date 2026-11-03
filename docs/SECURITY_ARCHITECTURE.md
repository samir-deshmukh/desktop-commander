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

The initial implementation maps a small reviewed set of diagnostic commands to executable-and-argument arrays and does not invoke a shell. A future full terminal must be a real PTY with user-account permissions, explicit capability policy, approvals, process limits, output limits, cancellation, and audit events. Sensitive actions may require user approval. Do not silently run as root.

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
- Terminal arguments containing shell metacharacters (they remain data when no shell is invoked).
- Confirm no remote listener starts in default mode.
