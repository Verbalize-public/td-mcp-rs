#!/usr/bin/env bash
# V4 — project_install_bridge on a copy (requires packed project path).
set -euo pipefail
# shellcheck source=common.sh
source "$(dirname "$0")/common.sh"
TOE="${1:?usage: v4-install-bridge.sh <path/to/project.toe>}"
require_daemon
mcp_init
out="$(tool_call project_install_bridge "{\"targetPath\":\"$TOE\",\"force\":true}")"
echo "$out"
