"""Verify request schema rejection through native CLI and progressive MCP over loopback HTTP."""
import copy
import json
from pathlib import Path
import subprocess
import tempfile
import threading
from http.server import BaseHTTPRequestHandler, HTTPServer

from probe_cli import McpClient


VALID = {
    "query": {"limit": 3},
    "media_type": "application/json",
    "body": {"profile": {"age": 21, "state": "active", "name": "Alice", "roles": ["admin"],
                          "id": 9007199254740993, "mode": 5, "child": {"age": 22, "state": "active"}}},
}
OTHER = {"media_type": "application/vnd.other+json", "body": {"other": True}}


def invalid_cases():
    cases = []
    for keys, value in [
        (("body", "profile", "age"), 17),
        (("body", "profile", "age"), 21.5),
        (("body", "profile", "state"), "deleted"),
        (("body", "profile"), {}),
        (("body", "profile"), "wrong"),
        (("body", "profile"), None),
        (("body", "profile", "name"), "a"),
        (("body", "profile", "roles"), []),
        (("body", "profile", "roles"), ["admin", "admin"]),
        (("body", "profile", "roles"), ["unknown"]),
        (("body", "profile", "id"), 9007199254740992),
        (("body", "profile", "mode"), 10),
        (("body", "profile", "child", "age"), 17),
        (("query", "limit"), 0),
        (("query", "limit"), "3"),
        (("media_type",), "application/vnd.other+json"),
        (("body", "extra"), True),
        (("body", "profile", "extra"), True),
    ]:
        arguments = copy.deepcopy(VALID)
        parent = arguments
        for key in keys[:-1]:
            parent = parent[key]
        parent[keys[-1]] = value
        cases.append(arguments)
    return cases


def main():
    root = Path(__file__).resolve().parents[2]
    binary = root / "extensions/forge/target/debug/examples/openapi"
    observations = []

    class Handler(BaseHTTPRequestHandler):
        def log_message(self, *args):
            pass

        def do_POST(self):
            body = self.rfile.read(int(self.headers.get("Content-Length", 0)))
            observations.append((self.path, self.headers.get("Content-Type"), json.loads(body)))
            self.send_response_only(200)
            self.send_header("Content-Length", "2")
            self.send_header("Connection", "close")
            self.end_headers()
            self.wfile.write(b"{}")

    server = HTTPServer(("127.0.0.1", 0), Handler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    client = None
    try:
        with tempfile.TemporaryDirectory(prefix="forge-request-validation-") as temporary:
            document = json.loads((root / "extensions/forge/tests/fixtures/request_validation_openapi.json").read_text())
            uri = f"http://127.0.0.1:{server.server_port}"
            document["servers"] = [{"url": uri}]
            fixture = Path(temporary) / "openapi.json"
            fixture.write_text(json.dumps(document))
            invalid = invalid_cases()
            for arguments, allowed in [(value, False) for value in invalid] + [(VALID, True), (OTHER, True)]:
                flags = ["--body-json", json.dumps(arguments["body"]), "--media-type", arguments["media_type"]]
                if "query" in arguments:
                    flags.extend(["--query", json.dumps(arguments["query"])])
                count = len(observations)
                result = subprocess.run([str(binary), str(fixture), uri + "/openapi.json",
                                         "op_submit", *flags, "--format", "json"],
                                        text=True, capture_output=True, timeout=20)
                output = json.loads(result.stdout)
                if allowed:
                    assert result.returncode == 0 and output["status"] == 200, (output, result.stderr)
                    assert len(observations) == count + 1
                else:
                    assert result.returncode != 0, output
                    error = output.get("error", output)
                    assert error["code"] == "FORGE_VALIDATION", output
                    assert "validation" in error["message"], output
                    assert len(observations) == count, observations
            client = McpClient([str(binary), str(fixture), uri + "/openapi.json", "--mcp"])
            client.send({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
                "protocolVersion": "2025-06-18", "capabilities": {},
                "clientInfo": {"name": "request-validation-proof", "version": "1"}}})
            assert client.reply(1)["result"]["protocolVersion"] == "2025-06-18"
            client.send({"jsonrpc": "2.0", "method": "notifications/initialized"})
            client.send({"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {
                "name": "get_tool_details", "arguments": {"name": "op_submit"}}})
            schema = client.reply(2)["result"]["structuredContent"]["inputSchema"]
            assert schema["$defs"]["Profile"]["properties"]["age"]["minimum"] == 18, schema
            assert schema["$defs"]["Profile"]["properties"]["id"]["minimum"] == 9007199254740993, schema
            for identifier, (arguments, allowed) in enumerate(
                [(value, False) for value in invalid] + [(VALID, True), (OTHER, True)], 10
            ):
                count = len(observations)
                client.send({"jsonrpc": "2.0", "id": identifier, "method": "tools/call", "params": {
                    "name": "call_write_tool", "arguments": {"name": "op_submit", "arguments": arguments}}})
                result = client.reply(identifier)["result"]
                if allowed:
                    assert result.get("isError") is not True, result
                    assert result["structuredContent"]["status"] == 200, result
                    assert len(observations) == count + 1
                else:
                    assert result["isError"] is True, result
                    assert result["structuredContent"]["code"] == "FORGE_VALIDATION", result
                    assert len(observations) == count, observations
            expected = [
                ("/submit?limit=3", "application/json", VALID["body"]),
                ("/submit", "application/vnd.other+json", {"other": True}),
            ] * 2
            assert observations == expected, observations
            print(json.dumps({"passed": True, "cli_schema_rejections": len(invalid),
                              "mcp_schema_rejections": len(invalid), "observed_http_requests": len(observations),
                              "scope": "Real CLI and progressive MCP; invalid schema values cause zero outbound HTTP"}, indent=2))
    finally:
        try:
            if client is not None:
                client.close()
        finally:
            server.shutdown()
            server.server_close()
            thread.join(timeout=5)


if __name__ == "__main__":
    main()
