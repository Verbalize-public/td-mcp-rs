#!/bin/bash
# capture_previews.sh — render preview-harness scenes and capture each window
# to docs/screens/<scene>.png (macOS: swift CGWindowList finder + screencapture;
# window-matching technique from the previous .ua/gui-shot.ps1 flow).
#
# Usage: scripts/capture_previews.sh [scene ...]
#        (default: the 10 scenes referenced by README/docs)
set -u
cd "$(dirname "$0")/.."

BIN="target/debug/examples/dashboard_preview"
OUT="docs/screens"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"; [ -n "${PID:-}" ] && kill "$PID" 2>/dev/null' EXIT

SCENES="${*:-overview-empty overview-populated overview-offline modal-add-slave stop-confirm logs-filtered settings-dirty palette-tree palette-empty palette-analyse}"

cat > "$TMP/find_window.swift" <<'EOF'
import CoreGraphics
import Foundation
let opts = CGWindowListOption([.optionOnScreenOnly, .excludeDesktopElements])
if let list = CGWindowListCopyWindowInfo(opts, kCGNullWindowID) as? [[String: Any]] {
  for w in list {
    let owner = w["kCGWindowOwnerName"] as? String ?? ""
    let layer = w["kCGWindowLayer"] as? Int ?? -1
    let num = w["kCGWindowNumber"] as? Int ?? 0
    let b = w["kCGWindowBounds"] as? [String: Any] ?? [:]
    let width = b["Width"] as? Double ?? 0
    if owner.contains("dashboard_preview"), layer == 0, width >= 300 {
      print(num)
      break
    }
  }
}
EOF
swiftc "$TMP/find_window.swift" -o "$TMP/find_window" 2>/dev/null

for scene in $SCENES; do
  TDMCP_PREVIEW_SCENE="$scene" "$BIN" >/dev/null 2>&1 &
  PID=$!
  WID=""
  for _ in $(seq 1 50); do
    WID="$("$TMP/find_window" 2>/dev/null | head -1)"
    [ -n "$WID" ] && break
    sleep 0.2
  done
  if [ -z "$WID" ]; then
    echo "$scene: window not found" >&2
    kill "$PID" 2>/dev/null; wait "$PID" 2>/dev/null; PID=""
    continue
  fi
  sleep 0.8  # let a couple of frames paint
  screencapture -o -x -l"$WID" "$TMP/$scene.png"
  kill "$PID" 2>/dev/null; wait "$PID" 2>/dev/null; PID=""
  cp "$TMP/$scene.png" "$OUT/$scene.png"
  echo "$scene: captured"
done
