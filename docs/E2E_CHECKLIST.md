# Live TD E2E checklist (manual)

Run against a real TouchDesigner instance after Gate P0 builds green.
Record date / TD version / pass-fail in a short note when you execute this.

For day-to-day interactive work (baseline kit, session tox resume, dual-MCP
smoke), use [`DEV_ENV.md`](DEV_ENV.md) instead of this full gate.

## Dev smoke (shortcut)

Owned host `_agent_tdmcprs_dev` + `fixtures/dev/e2e_kit.tox` + bootstrap drop.
See [`DEV_ENV.md`](DEV_ENV.md) § Dev smoke. Does **not** replace rows 1–12 below.

| # | Check |
| --- | --- |
| S1 | Classic: `get_td_node_errors` on `/project1/e2e_kit` clean |
| S2 | rs `fleet`: bridge `connected`; connected pid has non-empty `title` (`project.name`) and `toePath` when folder+name known |
| S3 | rs `execute_python` → `result = 1` |
| S4 | rs `capture` top on `/project1/e2e_kit/probe` — non-black |
| S5 | rs `inspect` summary on `/project1/e2e_kit` |

## Prerequisites

1. `cargo build -p tdmcp-daemon --release`
2. Bridge package available (`bridge/` + `manifest.json`)
3. Real bootstrap tox dropped into a TD project (`{dataDir}/bootstrap.tox` after `install` / `ensure`; Text-DAT `bridge/bootstrap.py` only as a debug fallback)
4. Daemon: `tdmcp-daemon start --port 9860`

## Checklist

| # | Step | Pass? |
| --- | --- | --- |
| 1 | `GET http://127.0.0.1:9860/mcp/health` → `{"ok":true}` | ✅ |
| 2 | Two TD instances dial IPC and complete handshake | ✅ |
| 3 | `fleet` lists both pids with `bridge: connected`, non-empty `title`, and `toePath` when project folder+name known | ✅ |
| 4 | Enqueue shared task on pid A; exclusive on A fails (`queue_busy`) | ✅ |
| 5 | Kill tox / drop IPC → `bridge: disconnected` + `cancelledTasks` | ✅ |
| 5b | Kill TD / drop IPC **while idle** (no tool call) → `fleet` shows `disconnected` within ~15s | |
| 6 | Same pid re-handshake → `resurrected: true`; first failed task keeps stack | ✅ |
| 7 | Successful task clears resurrection stack | ✅ |
| 8 | `execute_python` with `result = 1` returns structured result | ✅ |
| 9 | Script failure returns `diagnostics` with `tdmcp.script.execution_failed` | ✅ |
| 10 | `capture` mode `top` on a non-black TOP → ok | ✅ |
| 11 | `capture` mode `preview` on zone COMP with `out1` → non-black | ✅ |
| 12 | Black TOP → `tdmcp.perception.black_frame` | ✅ |

**Run record:** 2026-07-29, TouchDesigner 099.2025.33070 (Windows), two
`_agent_tdmcprs_e2e*` sandbox projects, daemon `0.1.0` release build. All 12
rows pass. See "Bugs found and fixed" below — none were pre-existing test
gaps, all were live-only failures (never hit by the mocked/in-memory
integration suite).

## Bugs found and fixed during this run

Live TD is the only environment that exercises the real named-pipe transport,
a real GIL-bearing Python interpreter, and real TOP pixel data — the
in-memory `tdmcp-daemon` integration tests (`rmcp_streamable_http.rs`,
`bridge_session.rs`) cannot catch these by construction. All three are fixed
in `bridge/tdmcp_bridge/__init__.py` and covered by `bridge/tests/test_bridge_queue.py`:

1. **`_NamedPipeStream.read()` buffer offset bug.** `ReadFile`'s buffer
   pointer was `ctypes.byref(self._buf, want)` (offset by the *size*, not a
   real offset use case) while the copy-out read from offset `0` — every
   frame's header decoded as zero bytes. Fixed by passing `self._buf`
   directly. Without this fix every framed read after the first silently
   returned empty/garbage, surfacing as `json.loads` `"Expecting value"`
   errors on the Python side.
2. **`disconnect()` froze TD.** Calling `CloseHandle` on a named pipe handle
   from the main thread while the worker thread had a pending synchronous
   `ReadFile` on that same handle is undefined behavior on Windows — observed
   as a full TD freeze (confirmed via `Get-Process -Id <pid> | Responding`
   returning `False`), because the call originated from a script running on
   TD's main/cook thread. Fixed by cancelling the worker's pending I/O first
   (`CancelSynchronousIo` targeting the worker's OS thread id on Windows;
   `socket.shutdown(SHUT_RDWR)` on POSIX) and joining before `close()`.
3. **`capture`'s black-frame heuristic never actually checked pixels.**
   `len(saveByteArray(".jpg")) < 200` was the entire check — a solid white
   and solid black 256×256 Constant TOP both encode to the identical byte
   count, so black frames were never detected. Fixed by sampling real pixel
   data via `TOP.numpyArray()` and checking mean RGB near zero, with the old
   byte-size check retained only as a last-resort fallback when
   `numpyArray` is unavailable.
4. **`capture` returned only a JPEG byte *count*, not pixels.** Agents got
   `{ bytes, path }` with no MCP image content block. Fixed by base64-encoding
   `saveByteArray(".jpg")` as `jpegBase64` and promoting it to an MCP
   `image/jpeg` content block (stripped from structured content to avoid
   double-payload). Optional `maxSize` (default 256) downscales via a temp
   `resolutionTOP`. Black-frame failures still attach the JPEG so critics
   can see the frame.

## Notes

- Lab port conventions from creative-operator still apply for corpus verify;
  this daemon uses **pid**, not sticky ports.
- Do not claim Gate P0 green without rows 1–9 at minimum.
- Idle liveness: daemon heartbeats with wire `ping` every 5s; either side
  treats the bridge as dead after 15s inbound silence (see CONTRACT
  Disconnect / resurrection). Row **5b** verifies detection without an
  intervening tool call.
