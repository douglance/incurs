#!/usr/bin/env bash
# Runs the feature Worker under `wrangler dev` and checks one observable
# result per incurs feature. Exits non-zero on the first failed check.
#
# Requires: worker-build, python3, jq, npm (for wrangler 4.114 or newer).
set -euo pipefail
cd "$(dirname "$0")"

read -r WORKER_PORT API_PORT < <(
  python3 - "${WORKER_PORT:-0}" "${API_PORT:-0}" <<'PY'
import socket
import sys

sockets = []
ports = []
for requested in sys.argv[1:]:
    sock = socket.socket()
    sock.bind(("127.0.0.1", int(requested)))
    sockets.append(sock)
    ports.append(sock.getsockname()[1])
print(*ports)
PY
)
BASE="http://127.0.0.1:${WORKER_PORT}"
LOG="$(mktemp)"

TOKEN=smoke-test-token

python3 -m http.server "$API_PORT" --bind 127.0.0.1 --directory fixtures >/dev/null 2>&1 &
API_PID=$!
WORKER_PID=""
stop_worker() {
  [ -n "$WORKER_PID" ] || return 0
  kill "$WORKER_PID" 2>/dev/null || true
  wait "$WORKER_PID" 2>/dev/null || true
  WORKER_PID=""
  for _ in $(seq 1 30); do
    curl -s -m 1 "$BASE" >/dev/null 2>&1 || return 0
    sleep 1
  done
  echo "FAIL owned Worker port remains open after cleanup"
  exit 1
}
# Starts the Worker; extra arguments go to `wrangler dev`.
start_worker() {
  : >"$LOG"
  python3 owned_process.py "$LOG" -- npm exec --yes --package=wrangler@4 -- wrangler dev \
    --port "$WORKER_PORT" --ip 127.0.0.1 \
    --var "SELF_URL:$BASE" --var "OPENAPI_BASE:http://127.0.0.1:$API_PORT" "$@" &
  WORKER_PID=$!
  for _ in $(seq 1 300); do
    grep -q "Ready on" "$LOG" && return 0
    if ! kill -0 "$WORKER_PID" 2>/dev/null; then cat "$LOG"; exit 1; fi
    sleep 1
  done
  cat "$LOG"
  exit 1
}
cleanup() {
  stop_worker
  kill "$API_PID" 2>/dev/null || true
}
trap cleanup EXIT

failures=0
log_is_clean() {
  if grep -q "panicked\|Uncaught" "$LOG"; then
    echo "FAIL worker log shows a panic or uncaught error"
    grep -m 5 "panicked\|Uncaught" "$LOG"
    failures=$((failures + 1))
  fi
}

echo "== without a token: loopback only"
start_worker
check() {
  local name="$1" body="$2" filter="$3"
  if jq -e "$filter" <<<"$body" >/dev/null 2>&1; then
    echo "ok   $name"
  else
    echo "FAIL $name"
    echo "     $body" | head -c 600
    echo
    failures=$((failures + 1))
  fi
}
argv_json() {
  # jq would read a leading `--flag` as its own option, so encode with python.
  python3 -c 'import json, sys; print(json.dumps({"argv": sys.argv[1:]}))' "$@"
}
run() {
  curl -s -m 30 -X POST "$BASE/run" -H 'content-type: application/json' ${RUN_HEADERS[@]+"${RUN_HEADERS[@]}"} \
    -d "$(argv_json "$@")"
}
RUN_HEADERS=()

check "command runs with Worker vars as env" "$(run greet ada --format json)" \
  '.exitCode == 0 and (.output | fromjson | .message == "hello ada" and .style == "warm")'
check "help" "$(run --help)" '.exitCode == 0 and (.output | contains("worker-demo"))'
check "llms manifest" "$(run --llms)" '.exitCode == 0 and (.output | contains("greet"))'
check "schema" "$(run greet --schema)" '.exitCode == 0 and (.output | contains("name"))'
check "toon output" "$(run greet ada --format toon)" '.exitCode == 0 and (.output | contains("message: hello ada"))'
check "yaml output" "$(run greet ada --format yaml)" '.exitCode == 0 and (.output | contains("message: hello ada"))'
check "markdown output" "$(run greet ada --format md)" '.exitCode == 0 and (.output | contains("hello ada"))'
check "token count" "$(run greet ada --token-count)" '.exitCode == 0 and (.output | test("^[0-9]+\\s*$"))'
check "completions" "$(run completions bash)" '.exitCode == 0 and (.output | contains("worker-demo"))'
check "openapi spec download and operation call" "$(run pets listPets --format json)" \
  '.exitCode == 0 and (.output | contains("rex"))'
check "stdio MCP reports it is unavailable" "$(run --mcp)" \
  '(.exitCode != 0 or .error != null) and ((.output + (.error // "")) | contains("MCP_STDIO_UNAVAILABLE") or contains("not available on wasm32"))'

for install in "skills add" "skills list" "mcp add" "plugin build"; do
  # shellcheck disable=SC2086
  check "$install refuses on Workers" "$(run $install)" \
    '.exitCode != 0 and (.output | contains("LOCAL_INSTALL_UNAVAILABLE"))'
done

check "http command route" "$(curl -s -m 30 -X POST "$BASE/api/greet" -H 'content-type: application/json' -d '{}')" \
  '.ok == true and .data.style == "warm"'
check "http openapi document" "$(curl -s -m 30 "$BASE/api/openapi.json")" '.paths | tostring | contains("greet")'
check "http skills index" "$(curl -s -m 30 "$BASE/api/.well-known/skills/index.json")" '.skills | length > 0'

mcp() {
  curl -s -m 30 -X POST "$BASE/api/mcp" -H 'content-type: application/json' \
    -H 'accept: application/json, text/event-stream' "$@" | sed -n 's/^data: //p' | head -1
}
check "mcp initialize" "$(mcp -d '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"smoke","version":"1"}}}')" \
  '.result.serverInfo.name == "worker-demo"'
check "mcp tool call reaches a command" "$(mcp -H 'mcp-protocol-version: 2025-06-18' -d '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"call_write_tool","arguments":{"name":"greet","arguments":{"name":"ada"}}}}')" \
  '.result.isError == false and (.result.content[0].text | contains("hello ada")) and (.result.content[0].text | contains("warm"))'
check "mcp 2026-07-28 discovery" "$(mcp -H 'mcp-protocol-version: 2026-07-28' -H 'mcp-method: server/discover' -d '{"jsonrpc":"2.0","id":3,"method":"server/discover","params":{"_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientInfo":{"name":"smoke","version":"1"},"io.modelcontextprotocol/clientCapabilities":{}}}}')" \
  '.result.resultType == "complete"'
check "remote MCP tools run as commands" "$(run self greet --name ada --format json)" \
  '.exitCode == 0 and (.output | fromjson | .message == "hello ada" and .style == "warm")'

check "plugin package loads from memory" "$(curl -s -m 30 "$BASE/plugin")" \
  '.name == "worker-demo" and .skills == ["greeting"] and .diagnostics == []'
check "plugin HTTP MCP server connects" "$(curl -s -m 30 "$BASE/plugin")" \
  '.servers | map(select(.name == "self")) | .[0].toolCount > 0'
check "plugin stdio MCP server reports it is unavailable" "$(curl -s -m 30 "$BASE/plugin")" \
  '.servers | map(select(.name == "local")) | .[0].errorCode == "MCP_STDIO_UNAVAILABLE"'

status() {
  curl -s -o /dev/null -w '%{http_code}' -m 30 "$@"
}
check "a non-loopback request is refused without a token" \
  "$(status -X POST "$BASE/api/greet" -H 'host: incurs.example.com' -H 'content-type: application/json' -d '{}')" \
  '. == 401'
log_is_clean
stop_worker

echo "== with a token: bearer required on every route"
start_worker --var "MCP_AUTH_TOKEN:$TOKEN"
AUTH="authorization: Bearer $TOKEN"
for route in /api/greet /api/mcp /run; do
  check "$route refuses a request without the token" \
    "$(status -X POST "$BASE$route" -H 'content-type: application/json' -d '{}')" '. == 401'
  check "$route refuses a wrong token" \
    "$(status -X POST "$BASE$route" -H 'authorization: Bearer wrong' -H 'content-type: application/json' -d '{}')" '. == 401'
done
check "a command route admits the token" \
  "$(curl -s -m 30 -X POST "$BASE/api/greet" -H "$AUTH" -H 'content-type: application/json' -d '{}')" \
  '.ok == true and .data.style == "warm"'
RUN_HEADERS=(-H "$AUTH")
check "remote MCP self-call forwards the token" "$(run self greet --name ada --format json)" \
  '.exitCode == 0 and (.output | fromjson | .message == "hello ada")'
check "plugin HTTP MCP server forwards the token" "$(curl -s -m 30 "$BASE/plugin" -H "$AUTH")" \
  '.servers | map(select(.name == "self")) | .[0].toolCount > 0'
log_is_clean

echo "failures: $failures"
[ "$failures" -eq 0 ]
