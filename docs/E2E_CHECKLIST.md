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
| S3b | rs `execute_python` `print('hi'); result = 1` → response `logs` contains `hi`; COMP face LOGS section shows it; `op.Debug.op('debug')` resolves (or shortcut-conflict warn) |
| S3c | rs `execute_python` with `includeLogs: false` → no `logs` field |
| S4 | rs `capture` top on `/project1/e2e_kit/probe` — non-black |
| S5 | rs `inspect` summary on `/project1/e2e_kit` — `children` is an array of `{name, opType}` (not a count); `childCount` present |

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
| 5 | Kill tox / drop IPC → `bridge: disconnected` + `cancelledTasks`; gone from `fleet` after ~15s TTL (or sooner if another bridge handshakes) | ✅ |
| 5b | Kill TD / drop IPC **while idle** (no tool call) → `fleet` shows `disconnected` within ~15s detection, then removed after eviction TTL | |
| 6 | Same pid re-handshake → `resurrected: true`; first failed task keeps stack | ✅ |
| 7 | Successful task clears resurrection stack | ✅ |
| 8 | `execute_python` with `result = 1` returns structured result | ✅ |
| 8b | `execute_python` `print('hi'); result = 1` → `logs` contains `hi`; face LOGS / `./debug` updated; `op.Debug.op('debug')` when shortcut free | |
| 8c | `execute_python` with `includeLogs: false` omits `logs` | |
| 9 | Script failure returns `diagnostics` with `tdmcp.script.execution_failed` | ✅ |
| 9b | Script failure after `print` includes `diagnostics.context.logs` | |
| 10 | `capture` mode `top` on a non-black TOP → ok | ✅ |
| 11 | `capture` mode `preview` on zone COMP with `out1` → non-black | ✅ |
| 12 | Black TOP → `tdmcp.perception.black_frame` | ✅ |

### `mutate_nodes` (P1)

| # | Step | Pass? |
| --- | --- | --- |
| M1 | `fleet` shows the connected pid | ✅ |
| M2 | 1-step `create` of a `noiseTOP` under `/project1` → `ok:true, applied:1, failedAt:null`; echoed path matches | ✅ |
| M3 | `inspect` confirms the created node's `opType` | ✅ |
| M4 | 2-step batch `create` + `set` (`values:{resolutionw:128}`) → both ok | ✅ |
| M5 | `set` with `expressions:{resolutionw:"absTime.seconds*4"}` → re-`inspect` params confirms expression mode | ✅ |
| M6 | `set` with `pulse` on a Pulse par → no error (used `timerCHOP` + `start`) | ✅ |
| M7 | Mid-batch failure — `create` ok, then `set` on a nonexistent param → `failedAt:1`; `tdmcp.par.unknown`; later steps `tdmcp.batch.skipped_dependent`. (Wrong-bag: flag name under `values` keeps `tdmcp.par.unknown` and may nest `tdmcp.par.wrong_collection` — unit-covered; not a live gate.) | ✅ |
| M8 | First-step failure — `create` with bad `opType` → `failedAt:0, applied:0`; `tdmcp.op.unknown_type` | ✅ |
| M9 | `delete` a previously created node → `ok:true`; re-`inspect` confirms gone | ✅ |
| M10 | Structural errors/warnings clean after the whole pass (`inspect` default `errors`+`warnings` / classic `get_td_node_errors`); when a node warns, `node.warnings` is non-empty | ✅ |
| M11 | `capture` top on a created TOP → non-black | ✅ |
| M12 | Create bare `mathCHOP` → immediate `inspect` → `node.errors` non-empty (`Not enough sources`) | ✅ |
| M13 | Batch: create `noiseTOP` + `nullTOP`, `connect` src→dst → `applied:3`; `capture` top on null **non-black** | ✅ |
| M14 | `disconnect` that null’s input `0` → `ok`; re-`capture` → `tdmcp.perception.black_frame` | ✅ |
| M15 | `connect` with `dstInput: 99` → `failedAt` + `tdmcp.wire.bad_index` | ✅ |
| M16 | `connect` missing `src` → `tdmcp.op.not_found`; following step `tdmcp.batch.skipped_dependent` | ✅ |
| M17 | Create `mathCHOP` + `constantCHOP`, `connect`, `inspect` → math `node.errors` empty (pairs with M12) | ✅ |
| M18 | `create` noiseTOP with `flags:{viewer:true,display:true}` → `ok`; `capture` top non-black (no separate `execute_python` for flags) | ✅ |
| M19 | `set` with unrecognized flag name (e.g. `selected`) → `failedAt` + `tdmcp.flag.unknown`; later steps `tdmcp.batch.skipped_dependent`. (Wrong-bag: param name under `flags` keeps `tdmcp.flag.unknown` and may nest `tdmcp.flag.wrong_collection` — unit-covered; not a live gate.) | ✅ |
| M20 | `set` `flags:{allowCooking:false}` on a non-COMP → `tdmcp.mutate.step_failed` (live-only; TD raises; not unit-testable via FakeNode) | ✅ |

**Run record (P0):** 2026-07-29, TouchDesigner 099.2025.33070 (Windows), two
`_agent_tdmcprs_e2e*` sandbox projects, daemon `0.1.0` release build. All 12
rows pass. See "Bugs found and fixed" below — none were pre-existing test
gaps, all were live-only failures (never hit by the mocked/in-memory
integration suite).

**Run record (M12 inspect force-cook):** 2026-07-31, `NewProject.1.toe`
(pid 2192), HTTP `/mcp/tools/call`. Bare `mathCHOP` at
`/project1/agent_zone_test/m12_math` → first `inspect` returned
`Not enough sources specified` (bridge `_force_cook` before read).

**Run record (`mutate_nodes` M1–M11):** 2026-07-31, TouchDesigner via
`_agent_tdmcprs_dev.4.toe` (pid 19168), daemon `0.1.0` release rebuild.
All M1–M11 pass over HTTP JSON `/mcp/tools/call`. Notes:
- M5: `inspect` params report evaluated `resolutionw` (~3554), confirming
  expression mode took (explicit `par.mode` set before `.expr`).
- M6: used `timerCHOP` + `pulse:["start"]` (noiseTOP has no pulse par).
- Same-version `ensure` does **not** re-extract `diagnostics/catalog.yaml`
  when `install.version` already matches — first M8 run fell back to
  `layer:fleet` without mitigation until the stamp was deleted and assets
  re-extracted. Workaround for same-version rebuilds: delete
  `%LOCALAPPDATA%/tdmcp-rs/install.version` before `ensure`.

**Run record (`mutate_nodes` connect/disconnect M13–M17):** 2026-07-31,
`eldenmess.3.toe` (pid 25300), zone `/project1/agent_mutate_probe`, daemon
`0.1.0` release rebuild after adding wire steps. All M13–M17 pass over HTTP
`/mcp/tools/call`. Dump: `tmp/mutate_wire_e2e_20260731_125953.jsonl`.
Connect uses `src.outputConnectors[i].connect(dst.inputConnectors[j])`.

**Run record (`mutate_nodes` flags M18–M20):** 2026-07-31,
`_agent_tdmcprs_dev.4.toe` (pid 15448), daemon `0.1.0` release rebuild after
adding `flags` to create/set. Deleted `%LOCALAPPDATA%/tdmcp-rs/install.version`
before `ensure` so bridge + catalog re-extracted; loaded fresh
`bootstrap.tox` into `/project1/tdmcp_rs`. All M18–M20 pass over HTTP
`/mcp/tools/call`. Notes:
- M18: `create` with `values`+`flags` applied 2 steps; `execute_python`
  readback `viewer=true`/`display=true`; `capture` top `bytes=13569` non-black.
- M19: `selected` → `tdmcp.flag.unknown` with catalog mitigation listing the
  8 allowed flags; following `delete` skipped.
- M20: `allowCooking:false` on noiseTOP → `tdmcp.mutate.step_failed` with TD
  message "This flag can only be disabled for COMPs."

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
  intervening tool call. After loss, fleet eviction TTL is a separate **15s**
  (worst-case idle path ≈ detection + TTL).
