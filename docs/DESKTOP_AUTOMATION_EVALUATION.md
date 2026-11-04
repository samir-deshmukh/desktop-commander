# Linux Desktop Automation Evaluation

Status: platform reconnaissance complete; no automation dependency or input-control capability has been added.

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

The AT-SPI check proves that the accessibility bus and some application trees are reachable. It does not yet prove that the Tauri/WebKit application exposes a useful tree or that accessible actions work reliably.

## Candidate evaluation

| Capability | Candidate | Findings | Decision |
|---|---|---|---|
| Semantic UI discovery and accessible actions | Rust `atspi` crate | Pure-Rust AT-SPI2 client using zbus; dual MIT/Apache-2.0. The system AT-SPI bus is active and at least two desktop applications are visible. | Preferred candidate for inspecting controls and invoking supported accessible actions. Test against this app's WebKit view before adoption. |
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

## Recommended design

1. Keep automation behind a dedicated Rust adapter and shared policy boundary. MCP and desktop UI must call the same authorization code.
2. Prefer AT-SPI element discovery and supported semantic actions over coordinates. Use Enigo only when an accessible action is unavailable.
3. Do not expose raw desktop input as an unrestricted shell-like tool. Validate action types, text lengths, coordinate bounds, key names, and action counts.
4. Keep automation disabled by default. Require the user to start a visible session, show a persistent active indicator, and provide an immediate Stop control.
5. Show the target application and requested action before execution. Require explicit confirmation for consequential actions; do not infer consent from MCP client identity.
6. Treat screen capture as a separate permission from input control. Start with one-shot capture only after testing the portal's user-confirmation flow. Do not store screenshots or typed text in the audit log.
7. Do not read the clipboard, collect global keystrokes, or run background screen polling by default.
8. Keep the adapter on-demand, with bounded action batches and cancellation. Avoid OCR/continuous computer vision on this low-memory machine until a measured need exists.
9. Test only against a purpose-built dummy window first. Do not run automated clicks or keystrokes against the user's live desktop as part of unattended tests.

## Remaining work

- [ ] Confirm whether the Tauri/WebKit view exposes useful AT-SPI nodes and actions.
- [ ] Build a non-invasive proof-of-concept that lists accessible application/window metadata without sending input.
- [ ] Review exact crate versions, transitive dependencies, build impact, and licenses before adding dependencies.
- [ ] Prototype X11 input against a disposable test window, with explicit user-started activation and a Stop control.
- [ ] Test one-shot Screenshot portal consent and image handling separately.
- [ ] Decide whether a separate helper process is justified after measuring the adapter's privilege and lifecycle boundaries.
- [ ] Keep Wayland support marked unavailable until the actual Mint portal/compositor stack supports the required interfaces and passes tests.
