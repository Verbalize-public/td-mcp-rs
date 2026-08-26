#!/usr/bin/env bash
# V1 — td_installs discovers local TouchDesigner .app bundles.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
# shellcheck source=common.sh
source "$(dirname "$0")/common.sh"

require_daemon
mcp_init
out="$(tool_call td_installs)"
echo "$out" | python3 -c 'import json,sys; d=json.load(sys.stdin); assert d.get("structuredContent",d).get("ok") or d.get("result",{}).get("ok"); print("V1 PASS:", len((d.get("structuredContent") or d.get("result") or d).get("installs",[])), "install(s)")'
