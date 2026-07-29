# Live TD E2E checklist (manual)

Run against a real TouchDesigner instance after Gate P0 builds green.
Record date / TD version / pass-fail in a short note when you execute this.

## Prerequisites

1. `cargo build -p tdmcp-daemon --release`
2. Bridge package available (`bridge/` + `manifest.json`)
3. Bootstrap tox dropped into a TD project (or manual IPC client for smoke)
4. Daemon: `tdmcp-daemon start --port 9860`

## Checklist

| # | Step | Pass? |
| --- | --- | --- |
| 1 | `GET http://127.0.0.1:9860/mcp/health` → `{"ok":true}` | |
| 2 | Two TD instances dial IPC and complete handshake | |
| 3 | `fleet` lists both pids with `bridge: connected` | |
| 4 | Enqueue shared task on pid A; exclusive on A fails (`queue_busy`) | |
| 5 | Kill tox / drop IPC → `bridge: disconnected` + `cancelledTasks` | |
| 6 | Same pid re-handshake → `resurrected: true`; first failed task keeps stack | |
| 7 | Successful task clears resurrection stack | |
| 8 | `execute_python` with `result = 1` returns structured result | |
| 9 | Script failure returns `diagnostics` with `tdmcp.script.execution_failed` | |
| 10 | `capture` mode `top` on a non-black TOP → ok | |
| 11 | `capture` mode `preview` on zone COMP with `out1` → non-black | |
| 12 | Black TOP → `tdmcp.perception.black_frame` | |

## Notes

- Lab port conventions from creative-operator still apply for corpus verify;
  this daemon uses **pid**, not sticky ports.
- Do not claim Gate P0 green without rows 1–9 at minimum.
