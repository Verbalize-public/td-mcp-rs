#!/usr/bin/env bash
# V3 — project_pack roundtrip (requires expand dir arg).
set -euo pipefail
# shellcheck source=common.sh
source "$(dirname "$0")/common.sh"
DIR="${1:?usage: v3-pack.sh <expand-dir>}"
OUT="${2:-${DIR%.dir}.packed.toe}"
require_daemon
mcp_init
out="$(tool_call project_pack "{\"sourceDir\":\"$DIR\",\"outputPath\":\"$OUT\"}")"
echo "$out"
