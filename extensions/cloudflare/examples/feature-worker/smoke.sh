#!/usr/bin/env bash
# Runs the feature Worker under `wrangler dev` and checks one observable
# result per incurs feature. Exits non-zero on the first failed check.
#
# Requires: worker-build, python3, jq, npm (for wrangler 4.114 or newer).
set -euo pipefail
cd "$(dirname "$0")"

WORKER_PORT="${WORKER_PORT:-8799}"
API_PORT=8801
BASE="http://127.0.0.1:${WORKER_PORT}"
LOG="$(mktemp)"

python3 -m http.server "$API_PORT" --bind 127.0.0.1 --directory fixtures >/dev/null 2>&1 &
API_PID=$!
npm exec --yes --package=wrangler@4 -- wrangler dev --port "$WORKER_PORT" --ip 127.0.0.1 >"$LOG" 2>&1 &
WORKER_PID=$!
cleanup() {
  kill "$API_PID" "$WORKER_PID" 2>/dev/null || true
  pkill -P "$WORKER_PID" 2>/dev/null || true
}
trap cleanup EXIT

for _ in $(seq 1 300); do
  grep -q "Ready on" "$LOG" && break
  if ! kill -0 "$WORKER_PID" 2>/dev/null; then cat "$LOG"; exit 1; fi
  sleep 1
done
grep -q "Ready on" "$LOG" || { cat "$LOG"; exit 1; }

failures=0
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
run() {
  curl -s -m 30 -X POST "$BASE/run" -H 'content-type: application/json' \
    -d "$(jq -cn --args '{argv: $ARGS.positional}' "$@")"
}

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
check "remote MCP tools run as commands" "$(run self search_tools --format json)" \
  '.exitCode == 0 and (.output | contains("greet"))'

check "plugin package loads from memory" "$(curl -s -m 30 "$BASE/plugin")" \
  '.name == "worker-demo" and .skills == ["greeting"] and .diagnostics == []'
check "plugin HTTP MCP server connects" "$(curl -s -m 30 "$BASE/plugin")" \
  '.servers | map(select(.name == "self")) | .[0].toolCount > 0'
check "plugin stdio MCP server reports it is unavailable" "$(curl -s -m 30 "$BASE/plugin")" \
  '.servers | map(select(.name == "local")) | .[0].errorCode == "MCP_STDIO_UNAVAILABLE"'

if grep -q "panicked\|Uncaught" "$LOG"; then
  echo "FAIL worker log shows a panic or uncaught error"
  grep -m 5 "panicked\|Uncaught" "$LOG"
  failures=$((failures + 1))
fi

echo "failures: $failures"
[ "$failures" -eq 0 ]
