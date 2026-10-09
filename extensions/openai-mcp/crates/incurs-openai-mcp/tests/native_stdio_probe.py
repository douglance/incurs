#!/usr/bin/env python3
"""Exercise the OpenAI MCP facade through a real native stdio MCP process."""

from __future__ import annotations

import json
import os
import queue
import subprocess
import sys
import threading
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[5]
MANIFEST = Path(
    os.environ.get("OPENAI_SDK_MANIFEST", ROOT / "extensions" / "openai-mcp" / "Cargo.toml")
)
EXAMPLE = os.environ.get("OPENAI_SDK_EXAMPLE", "openai_mcp_stdio")
LEGACY_VERSION = "2025-11-25"
RESOURCE_URI = "ui://openai/native-settings.html"
OPENAI_ELICITATION = "openai/elicitation/create"


def main() -> int:
    cargo = os.environ.get("CARGO", "cargo")
    proc = subprocess.Popen(
        [
            cargo,
            "run",
            "--quiet",
            "--locked",
            "--manifest-path",
            str(MANIFEST),
            "--example",
            EXAMPLE,
            "--",
            "--mcp",
        ],
        cwd=str(ROOT),
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        bufsize=1,
    )
    assert proc.stdin is not None
    assert proc.stdout is not None
    assert proc.stderr is not None

    stdout: queue.Queue[object] = queue.Queue()
    stderr_lines: list[str] = []
    threading.Thread(target=_read_stdout, args=(proc.stdout, stdout), daemon=True).start()
    threading.Thread(target=_read_stderr, args=(proc.stderr, stderr_lines), daemon=True).start()

    try:
        initialize = request(
            proc,
            stdout,
            1,
            "initialize",
            {
                "protocolVersion": LEGACY_VERSION,
                "capabilities": {
                    "extensions": {"openai/elicitation": {"form": {}}},
                    "experimental": {"openai/elicitation": {"form": {}}},
                },
                "clientInfo": {"name": "native-stdio-probe", "version": "1.0.0"},
            },
        )
        assert initialize["result"]["protocolVersion"] == LEGACY_VERSION, initialize
        assert "tools" in initialize["result"]["capabilities"], initialize
        assert "resources" in initialize["result"]["capabilities"], initialize
        assert_settings_caps(initialize["result"]["capabilities"], initialize)

        notify(proc, "notifications/initialized", {})

        discovered = request(
            proc,
            stdout,
            7,
            "server/discover",
            {
                "_meta": {
                    "io.modelcontextprotocol/protocolVersion": LEGACY_VERSION,
                    "io.modelcontextprotocol/clientCapabilities": {
                        "extensions": {"openai/elicitation": {"form": {}}}
                    },
                    "io.modelcontextprotocol/clientInfo": {
                        "name": "native-stdio-probe",
                        "version": "1.0.0",
                    },
                }
            },
        )
        assert discovered["result"]["resultType"] == "complete", discovered
        assert_settings_caps(discovered["result"]["capabilities"], discovered)

        listed = request(proc, stdout, 2, "tools/list", {})
        tools = {tool["name"]: tool for tool in listed["result"]["tools"]}
        assert "settings.read" in tools, listed
        assert "settings.update" in tools, listed
        assert "search_mentions" in tools, listed
        assert "openai.elicit_units" in tools, listed
        assert tools["settings.read"]["annotations"]["readOnlyHint"] is True, listed
        assert tools["search_mentions"]["_meta"]["openai/extensions"] == {
            "mentions/search": {}
        }, listed

        read = request(
            proc,
            stdout,
            3,
            "tools/call",
            {"name": "settings.read", "arguments": {}},
        )
        assert read["result"]["content"] == [], read
        assert read["result"]["structuredContent"]["values"] == {"units": "mm"}, read

        updated = request(proc, stdout, 8, "tools/call",
            {"name": "settings.update", "arguments": {"set": {"units": "in"}}})
        assert updated["result"]["content"] == [], updated
        assert updated["result"]["structuredContent"]["values"] == {"units": "in"}, updated
        persisted = request(proc, stdout, 9, "tools/call",
            {"name": "settings.read", "arguments": {}})
        assert persisted["result"]["structuredContent"]["values"] == {"units": "in"}, persisted

        invalid = request(proc, stdout, 10, "tools/call",
            {"name": "settings.update", "arguments": {"set": {"units": "cm"}}})
        assert invalid["result"]["isError"] is True, invalid
        unchanged = request(proc, stdout, 11, "tools/call",
            {"name": "settings.read", "arguments": {}})
        assert unchanged["result"]["structuredContent"]["values"] == {"units": "in"}, unchanged

        mentions = request(
            proc,
            stdout,
            4,
            "tools/call",
            {"name": "search_mentions", "arguments": {"query": "native"}},
        )
        assert mentions["result"]["content"] == [], mentions
        assert mentions["result"]["structuredContent"]["items"][0]["title"] == "native-part", mentions

        elicited = request_with_elicitation(proc, stdout, 5)
        assert elicited["result"]["content"] == [], elicited
        assert elicited["result"]["structuredContent"]["answer"]["content"] == {"units": "in"}, elicited

        resources = request(proc, stdout, 6, "resources/list", {})
        assert resources["result"]["resources"][0]["uri"] == RESOURCE_URI, resources
        resource = request(proc, stdout, 7, "resources/read", {"uri": RESOURCE_URI})
        content = resource["result"]["contents"][0]
        assert content["uri"] == RESOURCE_URI, resource
        assert "data-openai-native-settings" in content["text"], resource
    finally:
        try:
            proc.stdin.close()
        except BrokenPipeError:
            pass
        try:
            proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            proc.kill()
            proc.wait(timeout=5)

    if proc.returncode not in (0, None):
        raise AssertionError(
            f"stdio example exited {proc.returncode}; stderr:\n" + "".join(stderr_lines[-80:])
        )
    return 0


def _read_stdout(stream, out: queue.Queue[object]) -> None:
    for line in stream:
        line = line.strip()
        if not line:
            continue
        try:
            out.put(json.loads(line))
        except json.JSONDecodeError as exc:
            out.put(exc)


def _read_stderr(stream, lines: list[str]) -> None:
    for line in stream:
        lines.append(line)


def send(proc: subprocess.Popen[str], message: dict) -> None:
    assert proc.stdin is not None
    proc.stdin.write(json.dumps(message, separators=(",", ":")) + "\n")
    proc.stdin.flush()


def notify(proc: subprocess.Popen[str], method: str, params: dict) -> None:
    send(proc, {"jsonrpc": "2.0", "method": method, "params": params})


def request(
    proc: subprocess.Popen[str],
    stdout: queue.Queue[object],
    request_id: int,
    method: str,
    params: dict,
) -> dict:
    send(
        proc,
        {"jsonrpc": "2.0", "id": request_id, "method": method, "params": params},
    )
    deadline = time.monotonic() + 30
    while time.monotonic() < deadline:
        if proc.poll() is not None:
            raise AssertionError(f"process exited before response {request_id}")
        try:
            message = stdout.get(timeout=0.2)
        except queue.Empty:
            continue
        if isinstance(message, BaseException):
            raise AssertionError(f"invalid JSON from stdio: {message}") from message
        if message.get("id") == request_id:
            if "error" in message:
                raise AssertionError(message)
            return message


    raise AssertionError(f"timed out waiting for response {request_id}")


def request_with_elicitation(proc: subprocess.Popen[str], stdout: queue.Queue[object], request_id: int) -> dict:
    send(
        proc,
        {
            "jsonrpc": "2.0",
            "id": request_id,
            "method": "tools/call",
            "params": {
                "name": "openai.elicit_units",
                "arguments": {},
                "_meta": {
                    "io.modelcontextprotocol/protocolVersion": LEGACY_VERSION,
                    "io.modelcontextprotocol/clientCapabilities": {
                        "extensions": {"openai/elicitation": {"form": {}}}
                    },
                },
            },
        },
    )
    saw_elicitation = False
    deadline = time.monotonic() + 30
    while time.monotonic() < deadline:
        if proc.poll() is not None:
            raise AssertionError(f"process exited before elicitation response {request_id}")
        try:
            message = stdout.get(timeout=0.2)
        except queue.Empty:
            continue
        if isinstance(message, BaseException):
            raise AssertionError(f"invalid JSON from stdio: {message}") from message
        if message.get("method") == OPENAI_ELICITATION:
            saw_elicitation = True
            params = message["params"]
            assert params["mode"] == "form", message
            assert params["requestedSchema"]["required"] == ["units"], message
            send(
                proc,
                {
                    "jsonrpc": "2.0",
                    "id": message["id"],
                    "result": {"action": "accept", "content": {"units": "in"}},
                },
            )
            continue
        if message.get("id") == request_id:
            if "error" in message:
                raise AssertionError(message)
            assert saw_elicitation, message
            return message
    raise AssertionError(f"timed out waiting for elicitation response {request_id}")


def assert_settings_caps(capabilities, response):
    for namespace in ("extensions", "experimental"):
        assert capabilities[namespace]["openai/settings"] == {
            "readTool": "settings.read",
            "updateTool": "settings.update",
        }, response


if __name__ == "__main__":
    sys.exit(main())
