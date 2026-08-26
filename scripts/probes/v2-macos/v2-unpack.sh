#!/usr/bin/env bash
# V2 — project_unpack on a sample .toe (requires TOE path arg).
set -euo pipefail
# shellcheck source=common.sh
source "$(dirname "$0")/common.sh"
TOE="${1:?usage: v2-unpack.sh <path/to/project.toe>}"
require_daemon
mcp_init
out="$(tool_call project_unpack "{\"sourcePath\":\"$TOE\"}")"
echo "$out"
