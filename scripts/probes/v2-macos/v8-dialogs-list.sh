#!/usr/bin/env bash
# V8/V9 — dialogs list on a pid (pass TD pid or any process for smoke).
set -euo pipefail
# shellcheck source=common.sh
source "$(dirname "$0")/common.sh"
PID="${1:?usage: v8-dialogs-list.sh <pid>}"
require_daemon
mcp_init
out="$(tool_call dialogs "{\"pid\":$PID,\"action\":\"list\"}")"
echo "$out"
