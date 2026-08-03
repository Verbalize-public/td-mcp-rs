#!/usr/bin/env bash
# Soft-stop then force-kill workspace tdmcp-daemon processes that lock
# target/release or target/dist binaries (start + leftover mcp shims).
#
# Cursor may respawn `tdmcp-daemon mcp` if the MCP server is still connected —
# pause/reload MCP before rebuild when the binary stays locked after this script.
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"

port="${TDMCP_PORT:-9860}"
shutdown_url="http://127.0.0.1:${port}/admin/shutdown"
echo "== soft stop ${shutdown_url} =="
if command -v curl >/dev/null 2>&1; then
  if curl -fsS -X POST --max-time 2 "$shutdown_url" >/dev/null 2>&1; then
    echo "shutdown requested"
  else
    echo "soft stop skipped (daemon not reachable)"
  fi
else
  echo "soft stop skipped (curl not available)"
fi

sleep 0.75

exe_name="tdmcp-daemon"
targets=(
  "$(cd "$root" && pwd)/target/release/${exe_name}"
  "$(cd "$root" && pwd)/target/dist/${exe_name}"
)

echo "== force-kill workspace ${exe_name} =="
killed=0

is_target() {
  local path="$1"
  local t
  for t in "${targets[@]}"; do
    if [[ "$path" == "$t" ]]; then
      return 0
    fi
  done
  return 1
}

# Prefer /proc exe realpath matching (Linux). Fall back to pgrep path heuristics.
if [[ -d /proc ]]; then
  for pid_dir in /proc/[0-9]*; do
    pid="${pid_dir#/proc/}"
    exe_link="${pid_dir}/exe"
    [[ -L "$exe_link" ]] || continue
    path="$(readlink -f "$exe_link" 2>/dev/null || true)"
    [[ -n "$path" ]] || continue
    base="$(basename "$path")"
    [[ "$base" == "$exe_name" ]] || continue
    if is_target "$path"; then
      cmdline="$(tr '\0' ' ' <"${pid_dir}/cmdline" 2>/dev/null || true)"
      echo "kill pid=${pid} path=${path} cmd=${cmdline}"
      kill -TERM "$pid" 2>/dev/null || true
      sleep 0.2
      if kill -0 "$pid" 2>/dev/null; then
        kill -KILL "$pid" 2>/dev/null || true
      fi
      killed=$((killed + 1))
    fi
  done
else
  # macOS / other: match by full path via ps
  while IFS= read -r line; do
    pid="$(awk '{print $1}' <<<"$line")"
    cmd="$(cut -d' ' -f2- <<<"$line")"
    path="$(awk '{print $2}' <<<"$line")"
    [[ -n "$pid" && -n "$path" ]] || continue
    # Resolve if relative
    if [[ "$path" != /* ]]; then
      continue
    fi
    real="$(cd "$(dirname "$path")" 2>/dev/null && pwd)/$(basename "$path")" || real="$path"
    if is_target "$real" || is_target "$path"; then
      echo "kill pid=${pid} path=${path} cmd=${cmd}"
      kill -TERM "$pid" 2>/dev/null || true
      sleep 0.2
      if kill -0 "$pid" 2>/dev/null; then
        kill -KILL "$pid" 2>/dev/null || true
      fi
      killed=$((killed + 1))
    fi
  done < <(ps -axo pid=,command= 2>/dev/null | grep -E "[/]${exe_name}( |$)" || true)
fi

if [[ "$killed" -eq 0 ]]; then
  echo "no matching workspace daemons"
else
  echo "killed ${killed} process(es)"
fi
exit 0
