# Linux Desktop Automation Evaluation

Status: read-only AT-SPI2 inspection is implemented for MCP; no keyboard, pointer, or screenshot capability has been added.

Evaluation date: 2026-11-04

## Machine observations

Verified on the development machine:

- Linux Mint 22.3, Cinnamon 6.6.4.
- Active session: X11 (`XDG_SESSION_TYPE=x11`, `DISPLAY=:0`); this is not a Wayland session.
- AT-SPI D-Bus service is running. The installed `pyatspi` binding successfully enumerated the desktop and found accessible application entries for Brave Browser and GNOME Terminal.
- The XDG portal exposes `Screenshot`, but does not currently expose `RemoteDesktop` or `ScreenCast`.
- `wmctrl`, `xsel`, and `gnome-screenshot` are installed. `xdotool` and `ydotool` are not installed.
- Native X11 libraries including X11 and Xtst are present.
- The machine has approximately 3.2 GiB RAM. Automation must remain on-demand and avoid continuous screen polling or heavyweight vision pipelines.

A controlled test temporarily enabled toolkit accessibility and started the app through the actual Tauri/Vite development workflow. After allowing WebKitGTK time to populate its tree, AT-SPI exposed 72 nodes, including 24 named nodes and the app navigation buttons, headings, articles, and landmarks. The first probe checked too early; the tree is populated asynchronously.

The test restored both GNOME and Cinnamon `toolkit-accessibility` settings to their original `false` values, and the AT-SPI bus returned to `IsEnabled=false`. The adapter never changes these settings. The user must enable accessibility in the desktop session before requesting inspection.

The live MCP smoke test discovered 31 accessible applications and returned a 34-node snapshot for Sam's Desktop Commander, including its main navigation, headings, and key buttons. The snapshot correctly reported `truncated=true` at the configured depth bound, with no traversal warnings. The desktop settings were restored again after this end-to-end test.

## Candidate evaluation

| Capability | Candidate | Findings | Decision |
|---|---|---|---|
| Semantic UI discovery (read-only) | Rust `atspi-connection` and `atspi-proxies` crates | Pure-Rust AT-SPI2 client using zbus; pinned at 0.14.0. Controlled test confirmed the Tauri/WebKit accessibility tree is readable when toolkit accessibility is enabled. | Implemented for application discovery and bounded tree snapshots. No Action interface calls are made. |
| Keyboard and pointer input on X11 | `enigo` 0.6.1 with default `x11rb` feature | MIT licensed; default Linux backend uses native Rust/X11 rather than requiring the `xdotool` executable. | Preferred candidate for a narrowly scoped X11 input adapter, subject to a controlled proof-of-concept and dependency/build review. |
| Screenshot capture | XDG Screenshot portal | The portal service exposes Screenshot on this machine. The user-confirmation flow and returned image handling have not been tested. | Candidate for explicit, one-shot capture. Do not add continuous capture based on this finding. |
| Wayland capture and remote input | XDG ScreenCast and RemoteDesktop portals | Neither interface is exposed by the currently installed portal backend. Enigo's Wayland/libei paths are documented as experimental. | Not supported on this machine's current desktop stack. Defer Wayland automation until a supported, consent-based backend is available and tested. |

References:

- [AT-SPI Rust crate](https://docs.rs/atspi/latest/atspi/) — protocol client and accessibility interfaces.
- [AT-SPI project and license](https://github.com/odilia-app/atspi) — pure-Rust implementation; MIT or Apache-2.0.
- [Enigo Rust crate](https://docs.rs/enigo/latest/enigo/) — keyboard and pointer input API.
- [Enigo feature and platform notes](https://github.com/enigo-rs/enigo/blob/main/CHANGES.md) — X11 backend and experimental Wayland/libei support.
- [XDG RemoteDesktop portal](https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.RemoteDesktop.html) — consent-based device selection and session lifecycle.
- [XDG ScreenCast portal](https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.ScreenCast.html) — screen/window capture sessions.

## Implemented read-only adapter

- MCP tools: `list_accessible_apps` and `read_accessibility_tree`.
- Both require `confirm=true` on every call.
- Tree output is bounded to 200 nodes, depth 8, and 160 Unicode characters per accessible name.
- Accessible names can contain visible application text; users should only request a snapshot when they intend to share that information with the MCP client.
- The adapter does not enable AT-SPI, click, type, move the pointer, capture screenshots, or read the clipboard.
- If AT-SPI is disabled, the tools return an actionable error and do not change system settings.

## Recommended design

1. Keep automation behind a dedicated Rust adapter and shared policy boundary. MCP and desktop UI must call the same authorization code.
2. The current adapter uses AT-SPI only for read-only discovery. For future control, prefer supported semantic actions over coordinates; use Enigo only when an accessible action is unavailable and explicit input permissions are in place.
3. Do not expose raw desktop input as an unrestricted shell-like tool. Validate action types, text lengths, coordinate bounds, key names, and action counts.
4. Keep automation disabled by default. Require the user to start a visible session, show a persistent active indicator, and provide an immediate Stop control.
5. Show the target application and requested action before execution. Require explicit confirmation for consequential actions; do not infer consent from MCP client identity.
6. Treat screen capture as a separate permission from input control. Start with one-shot capture only after testing the portal's user-confirmation flow. Do not store screenshots or typed text in the audit log.
7. Do not read the clipboard, collect global keystrokes, or run background screen polling by default.
8. Keep the adapter on-demand, with bounded action batches and cancellation. Avoid OCR/continuous computer vision on this low-memory machine until a measured need exists.
9. Test only against a purpose-built dummy window first. Do not run automated clicks or keystrokes against the user's live desktop as part of unattended tests.

## Remaining work

- [x] Re-test the Tauri/WebKit view with AT-SPI temporarily enabled; confirm the accessible tree populates asynchronously and restore the original desktop settings.
- [x] Build the read-only AT-SPI2 MCP adapter for application discovery and bounded accessibility-tree snapshots.
- [x] Pin the AT-SPI client crates at 0.14.0 and review their Rust API and build impact; continue monitoring transitive dependency updates.
- [ ] Prototype X11 input against a disposable test window, with explicit user-started activation and a Stop control.
- [ ] Test one-shot Screenshot portal consent and image handling separately.
- [ ] Decide whether a separate helper process is justified after measuring the adapter's privilege and lifecycle boundaries.
- [ ] Keep Wayland support marked unavailable until the actual Mint portal/compositor stack supports the required interfaces and passes tests.
