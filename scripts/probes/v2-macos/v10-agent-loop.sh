#!/usr/bin/env bash
# V10 — agent loop smoke: fleet → describe_tools (requires live TD optional).
set -euo pipefail
# shellcheck source=common.sh
source "$(dirname "$0")/common.sh"
require_daemon
mcp_init
fleet="$(tool_call fleet)"
tools="$(tool_call describe_tools)"
echo "fleet: $fleet"
echo "tools: $tools"
echo "V10 smoke complete — verify connected pid manually for inspect/mutate/capture"
