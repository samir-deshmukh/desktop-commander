#!/usr/bin/env python3
"""End-to-end STDIO smoke test for Sam's Desktop Commander MCP server.

Build from app/src-tauri:
  CARGO_BUILD_JOBS=1 cargo build --no-default-features --features mcp --bin sam-desktop-mcp --locked

Run from the repository root:
  python3 scripts/test-mcp-stdio.py
"""
from __future__ import annotations

import json
import os
import select
import subprocess
import sys
import tempfile
from pathlib import Path
from typing import Any

APP_IDENTIFIER = "com.samdeshmukh.desktopcommander"
PROTOCOL_VERSION = "2025-03-26"


class McpProcess:
    def __init__(self, executable: Path, data_home: Path) -> None:
        env = os.environ.copy()
        env["XDG_DATA_HOME"] = str(data_home)
        self.process = subprocess.Popen(
            [str(executable)], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
            stderr=subprocess.PIPE, text=True, encoding="utf-8", bufsize=1, env=env,
        )
        self.next_id = 1

    def send(self, message: dict[str, Any]) -> None:
        assert self.process.stdin is not None
        self.process.stdin.write(json.dumps(message, separators=(",", ":")) + "\n")
        self.process.stdin.flush()

    def receive(self, expected_id: int) -> dict[str, Any]:
        assert self.process.stdout is not None
        ready, _, _ = select.select([self.process.stdout], [], [], 3)
        if not ready:
            raise TimeoutError(f"Timed out waiting for MCP response id={expected_id}")
        line = self.process.stdout.readline()
        if not line:
            stderr = self.process.stderr.read() if self.process.stderr else ""
            raise RuntimeError(f"MCP server closed stdout unexpectedly; stderr={stderr!r}")
        response = json.loads(line)
        if response.get("id") != expected_id or response.get("jsonrpc") != "2.0":
            raise AssertionError(f"Unexpected JSON-RPC response: {response!r}")
        if "error" in response:
            raise AssertionError(f"MCP request failed: {response['error']!r}")
        return response.get("result", {})

    def request(self, method: str, params: dict[str, Any] | None = None) -> dict[str, Any]:
        request_id = self.next_id
        self.next_id += 1
        message: dict[str, Any] = {"jsonrpc": "2.0", "id": request_id, "method": method}
        if params is not None:
            message["params"] = params
        self.send(message)
        return self.receive(request_id)

    def notify(self, method: str) -> None:
        self.send({"jsonrpc": "2.0", "method": method})

    def close(self) -> tuple[int, str, str]:
        if self.process.stdin and not self.process.stdin.closed:
            self.process.stdin.close()
        try:
            code = self.process.wait(timeout=3)
        except subprocess.TimeoutExpired:
            self.process.terminate()
            try:
                code = self.process.wait(timeout=2)
            except subprocess.TimeoutExpired:
                self.process.kill()
                code = self.process.wait(timeout=2)
        stdout = self.process.stdout.read() if self.process.stdout else ""
        stderr = self.process.stderr.read() if self.process.stderr else ""
        return code, stdout, stderr


def response_text(result: dict[str, Any]) -> str:
    return "\n".join(
        item.get("text", "") for item in result.get("content", [])
        if item.get("type") == "text"
    )


def main() -> int:
    root = Path(__file__).resolve().parents[1]
    binary = Path(sys.argv[1]).expanduser().resolve() if len(sys.argv) > 1 else (
        root / "app/src-tauri/target/debug/sam-desktop-mcp"
    )
    if not binary.is_file():
        print(f"ERROR: MCP binary not found: {binary}", file=sys.stderr)
        print("Build it using the command in this script's header.", file=sys.stderr)
        return 2

    passed: list[str] = []
    with tempfile.TemporaryDirectory(prefix="sam-mcp-stdio-test-") as temporary:
        temp = Path(temporary)
        data_home = temp / "xdg-data"
        policy_dir = data_home / APP_IDENTIFIER
        policy_dir.mkdir(parents=True)
        policy_file = policy_dir / "approved_folders.json"

        approved = temp / "approved"
        approved.mkdir()
        sample = approved / "hello.txt"
        sample.write_text("MCP integration test file\n", encoding="utf-8")
        outside = temp / "outside.txt"
        outside.write_text("must remain inaccessible\n", encoding="utf-8")

        client = McpProcess(binary, data_home)
        try:
            initialized = client.request("initialize", {
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": {"name": "sam-mcp-stdio-smoke-test", "version": "1.0"},
            })
            assert initialized.get("protocolVersion") == PROTOCOL_VERSION
            assert "tools" in initialized.get("capabilities", {})
            passed.append("initialize handshake")

            client.notify("notifications/initialized")
            discovered = client.request("tools/list")
            names = {tool["name"] for tool in discovered.get("tools", [])}
            expected = {"list_approved_folders", "list_directory", "read_text_file", "search_files", "get_file_info", "read_multiple_files", "search_content", "write_text_file", "create_directory", "delete_file", "move_file", "run_command"}
            assert names == expected, f"Unexpected tool set: {names!r}"
            passed.append("tool discovery")

            roots = json.loads(response_text(client.request(
                "tools/call", {"name": "list_approved_folders", "arguments": {}}
            )))
            assert roots == []
            passed.append("no folders approved by default")

            disabled_command = response_text(client.request(
                "tools/call", {"name": "run_command", "arguments": {
                    "program": "pwd", "args": [], "cwd": str(approved), "confirm": True
                }}
            ))
            assert "disabled in the desktop app" in disabled_command
            passed.append("command execution disabled by default")

            denied = response_text(client.request(
                "tools/call", {"name": "list_directory", "arguments": {"path": str(approved)}}
            ))
            assert "ERROR:" in denied and "access denied" in denied.casefold(), (
                f"Unexpected denial response: {denied!r}"
            )
            passed.append("unapproved folder denied")

            canonical = str(approved.resolve())
            policy_file.write_text(
                json.dumps({"version": 1, "folders": [canonical], "writable_folders": [canonical], "command_execution_enabled": True}), encoding="utf-8"
            )
            roots = json.loads(response_text(client.request(
                "tools/call", {"name": "list_approved_folders", "arguments": {}}
            )))
            assert roots == [canonical]
            passed.append("approval reload")

            listing = json.loads(response_text(client.request(
                "tools/call", {"name": "list_directory", "arguments": {"path": canonical}}
            )))
            assert any(e["name"] == sample.name and e["kind"] == "file" for e in listing)
            passed.append("approved directory listing")

            contents = response_text(client.request(
                "tools/call", {"name": "read_text_file", "arguments": {"path": str(sample)}}
            ))
            assert contents == "MCP integration test file\n"
            passed.append("approved text-file read")

            info = json.loads(response_text(client.request(
                "tools/call", {"name": "get_file_info", "arguments": {"path": str(sample)}}
            )))
            assert info["kind"] == "file" and info["size_bytes"] == sample.stat().st_size
            passed.append("approved file metadata")

            search = json.loads(response_text(client.request(
                "tools/call", {"name": "search_files", "arguments": {
                    "path": canonical, "query": "hello", "max_results": 10
                }}
            )))
            assert any(path.endswith("hello.txt") for path in search)
            passed.append("bounded recursive filename search")

            multiple = json.loads(response_text(client.request(
                "tools/call", {"name": "read_multiple_files", "arguments": {"paths": [str(sample)]}}
            )))
            assert len(multiple) == 1 and "MCP integration test file" in multiple[0][1]
            passed.append("multi-file read")

            content_search = json.loads(response_text(client.request(
                "tools/call", {"name": "search_content", "arguments": {
                    "path": canonical, "query": "integration test", "max_results": 10
                }}
            )))
            assert any(path.endswith("hello.txt") for path in content_search)
            passed.append("bounded file-content search")

            created = approved / "created-by-mcp.txt"
            policy_file.write_text(
                json.dumps({"version": 1, "folders": [canonical]}), encoding="utf-8"
            )
            denied_write = response_text(client.request(
                "tools/call", {"name": "write_text_file", "arguments": {
                    "path": str(created), "content": "must be denied", "overwrite": False
                }}
            ))
            assert "not explicitly approved" in denied_write
            passed.append("read approval does not grant write access")

            policy_file.write_text(
                json.dumps({"version": 1, "folders": [canonical], "writable_folders": [canonical], "command_execution_enabled": True}),
                encoding="utf-8",
            )
            written = response_text(client.request(
                "tools/call", {"name": "write_text_file", "arguments": {
                    "path": str(created), "content": "created by MCP", "overwrite": False
                }}
            ))
            assert "successfully" in written.lower() and created.read_text() == "created by MCP"
            passed.append("write with separate writable-root approval")

            command_output = json.loads(response_text(client.request(
                "tools/call", {"name": "run_command", "arguments": {
                    "program": "pwd", "args": [], "cwd": canonical,
                    "timeout_ms": 2_000, "confirm": True
                }}
            )))
            assert command_output["exit_code"] == 0 and canonical in command_output["stdout"]
            passed.append("confirmed command execution in approved working folder")

            denied_delete = response_text(client.request(
                "tools/call", {"name": "delete_file", "arguments": {
                    "path": str(created), "confirm": False
                }}
            ))
            assert "confirm=true" in denied_delete
            passed.append("destructive operation confirmation")

            new_dir = approved / "new-folder"
            made_dir = response_text(client.request(
                "tools/call", {"name": "create_directory", "arguments": {
                    "path": str(new_dir), "confirm": True
                }}
            ))
            assert "successfully" in made_dir.lower() and new_dir.is_dir()
            passed.append("approved directory creation")

            moved = approved / "renamed-by-mcp.txt"
            moved_result = response_text(client.request(
                "tools/call", {"name": "move_file", "arguments": {
                    "source": str(created), "destination": str(moved), "confirm": True
                }}
            ))
            assert "successfully" in moved_result.lower() and moved.exists() and not created.exists()
            passed.append("confirmed file move")

            deleted = response_text(client.request(
                "tools/call", {"name": "delete_file", "arguments": {
                    "path": str(moved), "confirm": True
                }}
            ))
            assert "successfully" in deleted.lower() and not moved.exists()
            passed.append("confirmed file deletion")

            denied = response_text(client.request(
                "tools/call", {"name": "read_text_file", "arguments": {"path": str(outside)}}
            ))
            assert "ERROR:" in denied and "access denied" in denied.casefold()
            passed.append("outside-root file denied")

            policy_file.write_text(
                json.dumps({"version": 1, "folders": []}), encoding="utf-8"
            )
            denied = response_text(client.request(
                "tools/call", {"name": "list_directory", "arguments": {"path": canonical}}
            ))
            assert "ERROR:" in denied and "access denied" in denied.casefold()
            passed.append("revocation enforced on next request")
        finally:
            code, trailing_stdout, stderr = client.close()

        if code != 0:
            raise AssertionError(f"MCP server exited with status {code}; stderr={stderr!r}")
        if trailing_stdout.strip():
            raise AssertionError(f"Unexpected non-protocol stdout: {trailing_stdout!r}")
        if stderr.strip():
            raise AssertionError(f"Unexpected stderr output: {stderr!r}")
        passed.append("clean STDIO shutdown")

    for check in passed:
        print(f"PASS  {check}")
    print(f"\n{len(passed)} MCP STDIO integration checks passed.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
