#!/usr/bin/env bash
# V6/V7 — spawn_td + kill_td lifecycle (requires .toe path).
set -euo pipefail
# shellcheck source=common.sh
source "$(dirname "$0")/common.sh"
TOE="${1:?usage: v6-spawn-kill.sh <path/to/project.toe>}"
EXE="${TDMCP_TD_EXE:-}"
require_daemon
mcp_init
args="{\"projectPath\":\"$TOE\",\"waitTimeoutMs\":120000"
if [[ -n "$EXE" ]]; then args+=",\"exePath\":\"$EXE\""; fi
args+="}"
spawn="$(tool_call spawn_td "$args")"
echo "spawn: $spawn"
pid="$(echo "$spawn" | python3 -c 'import json,sys; d=json.load(sys.stdin); c=d.get("structuredContent") or d.get("result") or d; print(c.get("pid",""))')"
[[ -n "$pid" ]] || { echo "no pid from spawn" >&2; exit 1; }
kill="$(tool_call kill_td "{\"pid\":$pid,\"mode\":\"graceful\",\"graceMs\":8000}")"
echo "kill: $kill"
