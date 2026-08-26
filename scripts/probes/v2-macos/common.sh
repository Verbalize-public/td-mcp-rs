#!/usr/bin/env bash
# Shared helpers for macOS v2 E2E probes (mirror docs/E2E_CHECKLIST.md V1–V10).
set -euo pipefail

DAEMON_URL="${TDMCP_DAEMON_URL:-http://127.0.0.1:9860/mcp/rpc}"
SID_FILE="${TMPDIR:-/tmp}/tdmcp-macos-probe.sid"

mcp_init() {
  local resp sid
  resp="$(curl -sS -i -X POST "$DAEMON_URL" \
    -H 'Content-Type: application/json' \
    -H 'Accept: application/json, text/event-stream' \
    -d '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"v2-macos-probe","version":"0"}}}')"
  sid="$(printf '%s' "$resp" | awk -F': ' '/^[Mm]cp-[Ss]ession-[Ii]d:/ {print $2; exit}' | tr -d '\r')"
  if [[ -z "$sid" ]]; then
    echo "initialize failed — no Mcp-Session-Id" >&2
    exit 1
  fi
  printf '%s' "$sid" >"$SID_FILE"
}

mcp_call() {
  local body="$1"
  local sid
  sid="$(cat "$SID_FILE")"
  curl -sS -X POST "$DAEMON_URL" \
    -H 'Content-Type: application/json' \
    -H 'Accept: application/json, text/event-stream' \
    -H "Mcp-Session-Id: $sid" \
    -d "$body" | sed -n 's/^data: //p' | head -1
}

tool_call() {
  local name="$1"
  local args="${2:-{}}"
  mcp_call "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":\"$name\",\"arguments\":$args}}"
}

require_daemon() {
  if ! curl -sf "http://127.0.0.1:9860/admin/status" >/dev/null 2>&1; then
    echo "daemon not reachable at 127.0.0.1:9860 — start tdmcp-daemon first" >&2
    exit 1
  fi
}
