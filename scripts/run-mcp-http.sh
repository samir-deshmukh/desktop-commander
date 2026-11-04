#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CONFIG_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/sam-desktop"
TOKEN_FILE="$CONFIG_DIR/mcp-http-token"
mkdir -p "$CONFIG_DIR"
chmod 700 "$CONFIG_DIR"
if [[ ! -s "$TOKEN_FILE" ]]; then
  umask 077
  python3 -c 'import secrets; print(secrets.token_urlsafe(48))' > "$TOKEN_FILE"
fi
chmod 600 "$TOKEN_FILE"
export SAM_DESKTOP_MCP_TOKEN="$(cat "$TOKEN_FILE")"
export SAM_DESKTOP_MCP_BIND="127.0.0.1:18765"
cd "$ROOT/app/src-tauri"
exec cargo run --no-default-features --features mcp --bin sam-desktop-mcp-http --locked
