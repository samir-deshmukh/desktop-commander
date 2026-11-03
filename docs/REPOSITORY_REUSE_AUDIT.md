# Open-Source Reuse and Reference Audit

Status: initial shortlist, not a completed license or dependency approval. Verify exact release, license files, transitive licenses, maintenance, and Linux Mint behavior before adding a dependency.

## Candidates

| Project | Potential use | Current decision |
|---|---|---|
| [Official Rust MCP SDK (rmcp)](https://github.com/modelcontextprotocol/rust-sdk) | MCP server/client protocol and STDIO transport | Already used by local MCP server; pinned in Cargo manifest. Keep adapter separate from capability logic. |
| [WezTerm portable-pty](https://github.com/wez/wezterm) | PTY session lifecycle for an interactive terminal | Candidate to evaluate before custom PTY management. |
| [xterm.js](https://github.com/xtermjs/xterm.js) | Terminal rendering in the webview | Candidate; renderer only, not a shell or security boundary. |
| [sysinfo](https://github.com/GuillaumeGomez/sysinfo) | Process and system metrics | Candidate; measure polling cost and required fields. |
| [agent-sh/computer-use-linux](https://github.com/agent-sh/computer-use-linux) | Linux accessibility and desktop interaction | Optional adapter candidate; validate Cinnamon, X11/Wayland, permissions, and maintenance. |
| [Tauri](https://github.com/tauri-apps/tauri) | Native desktop shell and frontend bridge | Current foundation. Use narrow command exposure and explicit capabilities. |
| [MCP filesystem reference server](https://github.com/modelcontextprotocol/servers) | Filesystem tool semantics and tests | Reference implementation; do not assume its behavior is a security boundary or copy code without license review. |
| [Desktop Commander MCP](https://github.com/wonderwhy-er/DesktopCommanderMCP) | Product behavior and session-management reference | Study selectively; do not import the complete Node service into the Rust app. |

## Evaluation criteria
- License compatibility, including transitive dependencies and notices.
- Supported OS/display backend and tested version range.
- Security model and documented limitations.
- Maintenance signals, open issues, release cadence, and API stability.
- Runtime memory/CPU, binary/build impact, and dependency count.
- Whether it can be isolated behind a small adapter with tests.
- Whether it avoids meaningful custom code.

## Rules
1. Reuse proven components when the fit is strong; avoid copying whole products.
2. Do not add a library only because it is popular.
3. Record exact versions and licenses when a dependency is approved.
4. If a candidate fails platform or performance testing, document the reason and compare alternatives.
5. This is a decision log in progress, not an endorsement of every listed project.
