#!/usr/bin/env bash
# V5 — project_lint on packed or expand dir.
set -euo pipefail
# shellcheck source=common.sh
source "$(dirname "$0")/common.sh"
TARGET="${1:?usage: v5-lint.sh <path>}"
require_daemon
mcp_init
out="$(tool_call project_lint "{\"targetPath\":\"$TARGET\"}")"
echo "$out"
