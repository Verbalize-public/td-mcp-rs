#!/usr/bin/env bash
# Run all macOS v2 probes that need only a running daemon (V1 + V10).
set -euo pipefail
DIR="$(cd "$(dirname "$0")" && pwd)"
bash "$DIR/v1-td-installs.sh"
bash "$DIR/v10-agent-loop.sh"
echo "Batch probes done. Run v2–v9 with project paths when TouchDesigner is available."
