# Live TD E2E checklist (manual)

Run against a real TouchDesigner instance after builds are green. Record
executions in the Verification log below (date / build / scope / result).

For day-to-day interactive work (baseline kit, session tox resume, dual-MCP
smoke), use [`DEV_ENV.md`](DEV_ENV.md) instead of this full gate.

## Verification log

| Date | Build | Scope | Result |
| --- | --- | --- | --- |
| 2026-07-29 | daemon 0.1.0, TD 099.2025.33070 (Windows) | core rows 1–12 | all pass |
| 2026-07-31 | daemon 0.1.0 (Windows) | M1–M12, M13–M20, M23 (M5 re-run with the `mode`/`expr` criterion) | all pass |
| 2026-08-01 | daemon 0.1.0 (Windows) | rows 11, 19 (shared OP Viewer preview; inspect batch) | pass |
| 2026-08-26 | TD 2025.32460 (Windows) | v2 rows V1–V10 | all pass |
| 2026-08-26 | macOS port | macOS rows V1–V10 | automated rows pass; live rows open |
| 2026-08-31 | TD 2025.x (macOS) | palette rows P1–P10 | all pass |

## Dev smoke (shortcut)

Owned host `_agent_tdmcprs_dev` + `fixtures/dev/e2e_kit.tox` + bootstrap drop.
See [`DEV_ENV.md`](DEV_ENV.md) § Dev smoke. Does **not** replace rows 1–12
below.

| # | Check |
| --- | --- |
| S1 | Classic: `get_td_node_errors` on `/project1/e2e_kit` clean |
| S2 | rs `fleet`: bridge `connected`; connected pid has non-empty `title` (`project.name`) and `toePath` when folder+name known |
| S3 | rs `execute_python` → `result = 1` |
| S3b | rs `execute_python` `print('hi'); result = 1` → response `logs` contains `hi`; COMP face LOGS section shows it; `op.Debug.op('debug')` resolves (or shortcut-conflict warn) |
| S3c | rs `execute_python` with `includeLogs: false` → no `logs` field |
| S3d | rs `execute_python` raise → `items[0].exception.type` set; default `rawTraceback` present; `AttributeError` on None → nested `tdmcp.script.none_op` |
| S4 | rs `capture` top on `/project1/e2e_kit/probe` — non-black; structured success has top-level `path`/`bytes` (not nested under `capture`) |
| S5 | rs `inspect` `paths:["/project1/e2e_kit"]` summary — structured success has top-level `nodes` array; entry `ok:true`; `children` is an array of `{name, opType}` (not a count); `childCount` present |
| S6 | rs `editor_context` — `ok:true` with `panes[]`; focused network editor has `focused:true` + `ownerPath`; with 2+ selected nodes, `selection` has paths and exactly one `current:true` |

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
| 11 | `capture` mode `preview` on any non-TOP (zone COMP / CHOP / SOP) → PNG via shared `./capture_viewer` (may soft-fail `uniform_frame` for empty viewers); bridge host has `capture_viewer` child | ✅ |
| 12 | Black TOP → `tdmcp.perception.black_frame` | ✅ |
| 12b | Constant TOP non-black solid (e.g. white) → `tdmcp.perception.uniform_frame` | |
| 13 | Create ephemeral non-empty `constantCHOP` under `/project1/e2e_kit/zone` → `capture` mode `chop_data` → `ok`; top-level `channels` / `numChans` / `numSamples` (not nested); no `imageBase64` | ✅ |
| 14 | `capture` mode `chop_data` on a TOP (e.g. `/project1/e2e_kit/probe`) → `tdmcp.perception.wrong_family` | ✅ |
| 15 | `capture` mode `auto` on that CHOP → same chop_data success shape (`mode: chop_data`) | ✅ |
| 16 | Empty CHOP (`numChans` or `numSamples` 0) → `tdmcp.perception.empty_chop` | ✅ |
| 17 | `capture` mode `chop_image` on non-empty CHOP → PNG via shared `capture_viewer` (alias of preview); no leftover `__tdmcp_tmp_chopimg__*` under parent | ✅ |
| 18 | `capture` mode `pop` / `auto` on a POP or SOP → PNG via shared `capture_viewer` (may soft-fail `black_frame` / `uniform_frame`); no leftover `__tdmcp_tmp_pop__*` | ✅ |
| 19 | `inspect` `paths:[a, b, missing]` → top-level `ok:true`; two ok entries + one `tdmcp.op.not_found` inline; no auto-recursion beyond direct-child roster | ✅ |
| 20 | `editor_context` with 2 network panes open → `panes` length ≥ 2; exactly one `focused:true`; focused entry has `ownerPath` | |
| 21 | Select 2+ ops in the focused network → `editor_context` returns `selection` with those paths and exactly one `current:true` | |

### `mutate_nodes`

| # | Step | Pass? |
| --- | --- | --- |
| M1 | `fleet` shows the connected pid | ✅ |
| M2 | 1-step `create` of a `noiseTOP` under `/project1` → `ok:true, applied:1, failedAt:null`; echoed path matches | ✅ |
| M3 | `inspect` `paths:[created]` confirms the created node's `opType` | ✅ |
| M4 | 2-step batch `create` + `set` (`values:{resolutionw:128}`) → both ok | ✅ |
| M5 | `set` with `expressions:{resolutionw:"absTime.seconds*4"}` → re-`inspect` `include:["params"]` shows `mode=="EXPRESSION"` and `expr` matching the string set | ✅ |
| M6 | `set` with `pulse` on a Pulse par → no error (used `timerCHOP` + `start`) | ✅ |
| M7 | Mid-batch failure — `create` ok, then `set` on a nonexistent param → `failedAt:1`; `tdmcp.par.unknown`; later steps `tdmcp.batch.skipped_dependent`. (Wrong-bag: flag name under `values` keeps `tdmcp.par.unknown` and may nest `tdmcp.par.wrong_collection` — unit-covered; not a live gate.) | ✅ |
| M8 | First-step failure — `create` with bad `opType` → `failedAt:0, applied:0`; `tdmcp.op.unknown_type` | ✅ |
| M9 | `delete` a previously created node → `ok:true`; re-`inspect` confirms gone | ✅ |
| M10 | Structural errors/warnings clean after the whole pass (`inspect` default `errors`+`warnings` / classic `get_td_node_errors`); when a node warns, `node.warnings` is non-empty | ✅ |
| M10b | Broken custom `enableExpr` (e.g. `app(1`) on a COMP → default `inspect` keeps coarse enable-parm `warnings[]`, attaches `parmExprIssues` (`kind: enableExpr`, `errorType`, `message`) + soft `diagnostics` (`tdmcp.par.enable_expr_failed`); top-level and node `ok: true` | |
| M11 | `capture` top on a created TOP → non-black | ✅ |
| M12 | Create bare `mathCHOP` → immediate `inspect` → `node.errors` non-empty (`Not enough sources`). Contract: inspect does not force-cook; cook is caller/downstream | ✅ |
| M13 | Batch: create `noiseTOP` + `nullTOP`, `connect` src→dst → `applied:3`; `capture` top on null **non-black** | ✅ |
| M14 | `disconnect` that null’s input `0` → `ok`; re-`capture` → `tdmcp.perception.black_frame` | ✅ |
| M15 | `connect` with `dstInput: 99` → `failedAt` + `tdmcp.wire.bad_index` | ✅ |
| M16 | `connect` missing `src` → `tdmcp.op.not_found`; following step `tdmcp.batch.skipped_dependent` | ✅ |
| M17 | Create `mathCHOP` + `constantCHOP`, `connect`, `inspect` → math `node.errors` empty (pairs with M12) | ✅ |
| M17b | After `connect` src→dst, `inspect` on dst shows non-null `inputs[dstInput]` peer with `path`/`name` matching src (wires ride with `nodes`) | |
| M18 | `create` noiseTOP with `flags:{viewer:true,display:true}` → `ok`; `capture` top non-black (no separate `execute_python` for flags) | ✅ |
| M19 | `set` with unrecognized flag name (e.g. `selected`) → `failedAt` + `tdmcp.flag.unknown`; later steps `tdmcp.batch.skipped_dependent`. (Wrong-bag: param name under `flags` keeps `tdmcp.flag.unknown` and may nest `tdmcp.flag.wrong_collection` — unit-covered; not a live gate.) | ✅ |
| M20 | `set` `flags:{allowCooking:false}` on a non-COMP → `tdmcp.mutate.step_failed` (live-only; TD raises; not unit-testable via FakeNode) | ✅ |
| M21 | Failing `mutate_nodes` MCP response is flat `{ok:false, summary, items, applied, failedAt, steps}` (no nested `data`) over axum + rmcp | |
| M22 | Transport mid-frame timeout disconnects cleanly (no silent desync / dead read thread); idle timeout still polls until idle-dead | |
| M23 | Pre-create occupant `nullTOP` at a path (e.g. `/project1/…/null1`). Batch: `create` same path + peer TOP + `connect` using the **requested** path → `ok`; create step has nested `tdmcp.op.renamed` and echoed path ≠ requested; connect wires the **renamed** node (inspect connections and/or `capture` on dst non-black), not the occupant | ✅ |

### Observability

All observability milestones are implemented (central JSONL sink, bridge
uplink, TD-side textport mirror + face LOGS upgrade, admin API + dashboard
Logs tab, proxy ingest) with non-live test coverage (Rust workspace +
`bridge/tests/`). The rows below are the live acceptance criteria; not yet
executed against a real TD + client session.

| # | Step | Pass? |
| --- | --- | --- |
| O1 | Bridge `print(...)` from a live TD session, with the dashboard Logs tab open, appears centrally within ~1s of the batch flush (`_BATCH_INTERVAL_S=0.5`) with `src:"bridge"` and the correct pid | |
| O2 | A slow `execute_python` call with many bridge prints during the wait still returns its normal result — no timeout/disconnect caused by the log-event interleaving (mirrors the mocked regression in `bridge_session.rs`, live this time) | |
| O3 | Logs tab: level chips (ALL/ERR/WRN) and source chips (DAEMON/BRIDGE/PROXY) filter the live list; changing a filter shows the tail under the new filter with no dropped/duplicated rows | |
| O4 | Logs tab: reconnect a TD bridge mid-session (kill tox, re-handshake) — the log list keeps scrolling through the gap, cursor resume after the GUI window is hidden/reshown loses no lines | |
| O5 | `tdmcp-daemon mcp` (stdio proxy) tool call, with the daemon up, produces at least one `src:"proxy"` line centrally (`kvs.proxyPid` matches the proxy process) | |
| O6 | Stop the daemon mid-session, then make an MCP tool call through the proxy — the call still succeeds (reconnect/respawn heals it); the proxy's own uplink POSTs fail silently in the meantime (one rate-limited stderr note, not a wall of retries) | |
| O7 | Face LOGS mirror: `print` from an unrelated node/DAT appears in face LOGS within ~1s; a broken node's traceback shows as `error` in both face LOGS and the central sink | |

## Client quirks (out of scope)

- Cursor MCP client may reject parallel tool calls with
  `Invalid arguments: server: Required`. This is client-side stdio plumbing,
  not a `td-mcp-rs` daemon bug — serialize calls when driving from Cursor.

## Notes

- This daemon is addressed by **pid**, not sticky ports.
- Do not claim the core gate green without rows 1–9 at minimum.
- Idle liveness: the daemon heartbeats with wire `ping` every 5s; either side
  treats the bridge as dead after **20s** inbound silence (see CONTRACT
  Disconnect / resurrection). After loss, `fleet` eviction is a separate
  **15s** TTL (worst-case idle path ≈ detection + TTL). Row **5b** verifies
  detection without an intervening tool call.

## v2 — Project I/O / Lifecycle / Dialogs

| # | Check | Result |
| --- | --- | --- |
| V1 | `td_installs` — deduped rows; stub `33070` complete=false; `32460` default=true | PASS |
| V2 | `project_unpack` r1_live.toe → 115 entries, canonical `.toe.toc`, exit-1 downgraded to warning | PASS |
| V3 | `project_pack` dir → 15,770 B packed; build-skew guard satisfied (32460↔32460) | PASS |
| V4 | `project_install_bridge` force on fresh copy → 3 DATs rewritten (bootstrap/callbacks/tdmcp_exec), targeted verify pass, backup written | PASS |
| V5 | `spawn_td` installed project → handshake ~27 s, identity exact, fleet spawn provenance + windowStatus flows | PASS |
| V6 | `kill_td graceful` → clean exit <8 s (no prompt on saved project); force path exercised earlier in session | PASS |
| V7 | Real modal forced via ctypes thread → `execute_python` fails fast `tdmcp.dialog.blocking`; `dialogs dismiss` OK clears it; bridged call recovers (result 42) | PASS |
| V8 | Helper-window false positives (ConsoleWindowClass/IME/MSCTFIME UI) filtered after live observation | PASS |
| V9 | Final handoff smokes over `POST /mcp/rpc`: all 8 v2 tools listed; `td_installs` 3-row dedup; `dialogs list` per-pid on a non-TD process returns popups + `windowStatus`; `project_lint` on packed `.toe` — exposed missing auto-unpack wiring, fixed (staged temp-dir expand) and re-verified: `ok:true, targetKind:"packed"`, staging cleaned | PASS |
| V10 | `project_install_bridge` create-from-scratch: bridge-less `.toe` (4,970 B) → `created:true`, 15,498 B, lint clean, real TD spawn handshakes, `inspect /project1/tdmcp_rs` shows all 10 children with correct opTypes; same loop green for `.tox` (update + create) after fixing `.toe`-hardcoded staging names; half-present bridge (`tdmcp_rs.n` listed, subtree gone) repaired to the same 15,498 B with no duplicate `.toc` entries, spawn + inspect green | PASS |

Artifacts: `fixtures/v2-probes/r0/`.

## macOS — Project I/O / Lifecycle / Dialogs (port)

Shell probes: [`scripts/probes/v2-macos/`](../scripts/probes/v2-macos/). Run
`run-smoke.sh` with daemon up; full V1–V10 need local TouchDesigner + sample
`.toe`.

| # | Check | Result |
| --- | --- | --- |
| V1 | `td_installs` — discovers `/Applications/TouchDesigner*.app` with tool files | unit + probe |
| V2 | `project_unpack` sample `.toe` → canonical toc | probe (manual) |
| V3 | `project_pack` dir → packed; build-skew guard | probe (manual) |
| V4 | `project_install_bridge` force on copy | probe (manual) |
| V5 | `project_lint` on packed `.toe` (staged expand) | probe (manual) |
| V6 | `spawn_td` → handshake + fleet provenance | probe (manual) |
| V7 | `kill_td graceful` / force | probe (manual) |
| V8 | `dialogs list` returns popups + `accessibilityGranted` | **PASS** (2026-08-30) |
| V9 | Intercept `tdmcp.dialog.blocking` during modal + dismiss recovery | **PASS** (2026-08-30, see D1-D7) |
| V10 | `fleet` + `describe_tools` lists the full tool set (palette tools included) | probe |

### macOS dialogs — live records (2026-08-30, TD 2025.33070, macOS 26.1)

Run from a terminal holding the Accessibility TCC grant; the grant follows the
terminal (the responsible process), so rebuilt test binaries inherit it and no
per-binary grant is needed. Screen Recording is **not** required.

| # | Check | Result |
| --- | --- | --- |
| D1 | AX enumeration returns the real window title with **no** Screen Recording grant; id is a `CGWindowID`; editor window classifies `is_dialog=Some(false)` | PASS — `crates/tdmcp-dialogs/tests/live_permissions.rs` |
| D2 | Idle TD yields zero popups (interception gate stays quiet) and `windowStatus=responsive` | PASS |
| D3 | Snapshot cost inside budget | PASS — worst uncached 10.4 ms vs 150 ms `SNAPSHOT_BUDGET` |
| D4 | Full round trip on an AX-exposing dialog: real labels, `AXDefaultButton` honored, dismissal via an explicit **non-default** button reports `via="button:Cancel"` | PASS — `tests/live_native_dialog.rs` (self-contained, spawns its own `osascript` dialog; no TD needed) |
| D5 | Genuine TD `THREAD CONFLICT` dialogs classify `severity=Hard`, `kind=MessageBox`, `windowStatus=blocked_by_modal_window` | PASS — 10 real dialogs, `tests/live_td_thread_conflict.rs` |
| D6 | Wedged TD (main thread deadlocked, AX unresponsive) still detected via the CGWindowList fallback | PASS — `popups=0/windowStatus=None` before the fallback became `popups=3/blocked_by_modal_window` after |
| D7 | TD-drawn dialogs expose no readable buttons; `THREAD CONFLICT` cannot be dismissed and returns `tdmcp.dialog.dismiss_failed` rather than faking success | PASS (recorded limitation — `tests/live_td_thread_conflict.rs`) |

Caution recorded with D5/D6: the trigger used to produce those dialogs
(`ui.messageBox` from a non-main Python thread) **deadlocks TouchDesigner and
stacks dialogs that cannot be dismissed programmatically**. It required a TD
restart. Do not re-run it; the evidence above is the record. A plain `op(...)`
access from a worker thread raises a clean `tdError` and is the safe way to
demonstrate the guard.

## Palette awareness

Run against a live TouchDesigner spawned on a throwaway project
(`/tmp/tdmcp-palette-e2e/palette_probe.toe`), driven over
`POST /mcp/tools/call`. Rebuild loop between rounds: `tdmcp-daemon stop` →
`cargo build --workspace` → `tdmcp-daemon install --force` → `ensure`.

| # | Check | Result |
| --- | --- | --- |
| P1 | `palette_index {action:"scan"}` indexed **281** builtin `.tox` across 17 categories from `/Applications/TouchDesigner.app/Contents/Resources/tfs/Samples/Palette`; 78 pre-blacklisted from `[palette].ignore` | PASS |
| P2 | `list {category:"Tools", limit:5}` → 5 one-line rows, `total: 49`, `truncation.nextOffset: 5` | PASS |
| P3 | `spawn_td` throwaway → `palette_probe` returned real interfaces: `particlesGpu` 6 par pages (31/43/18/4/34/3), 5 inputs, 3 outputs, 185 children, 154-char help; `bloom` and `audioAnalysis` likewise | PASS |
| P4 | `inspect ["/"]` after probing → root children are `ui/sys/local/perform/project1` only; **no `tdmcp_probe`**, no loaded components | PASS |
| P5 | `describe` → `get` returns the card body and `cardStatus:"described"`; `stats` `undescribed` drops by the batch size | PASS |
| P6 | One batch: create zone → `place builtin:Tools/particlesGpu` (`unwrapped:true`, `containerCOMP`, `Particles:2000`) → create null → `connect` — all 6 steps applied; `inspect` shows the wire resolving to `parts/out1`, zero errors, `Particles=2000`; `capture` non-black, and pulsing `Create` visibly increased particles (5859 → 6415 B). Repeated with `builtin:ImageFilters/bloom` in a noise→bloom→null chain; setting `Threshold`/`Intensity`/`Blursize`/`Glowcolor*` changed the capture (2240 → 14656 B) | PASS |
| P7 | `place` with an unknown `paletteId` → `tdmcp.palette.unknown_id`, span `steps[0].paletteId`, catalog mitigation attached, **zero nodes created**; both-fields → `tdmcp.args.unknown_field` on `steps[0].toxPath`; neither → `tdmcp.args.missing_field` | PASS |
| P8 | Blacklist: an all-ignored selection returns `skipped[]` + `skippedTotal:20` + a `note`. Corrupt `.tox` → `tdmcp.palette.load_failed` as **one error row in an `ok:true` batch**; second failure auto-blacklisted it (`ignoredAuto:true`) and the next bulk run skipped it with a reason; no debris at `/`. A stranded breadcrumb is surfaced by `scan` as `suspect` + `suspectHint`, and the entry lists under `status:"failed"`. A dispatch to a dead pid returns `tdmcp.bridge.lost` and **clears** the breadcrumb rather than blaming its components | PASS |
| P9 | Own component: `COMP.save` into `[palette].user_root` → `scan` added it as `user:MyRig/myWidget` (total 282) with the builtin roster intact; probe reported `wrapped` absent and surfaced its own `Widget`/`Gain` page and `out1` pin; `place` landed it with `unwrapped:false` | PASS |
| P10 | `/mcp/tools/list` → **18** tools including `palette_index` and `palette_probe` | PASS |

Findings from this run (wrapper unwrapping, per-entry skip reporting,
breadcrumb clearing policy, extension-slot dropping) are documented as
shipped behavior in [`CONTRACT.md`](CONTRACT.md) § `palette_index` /
`palette_probe`.

## Palette GUI

Run against the live daemon with a spawned throwaway TouchDesigner
(pid-scoped, never the user's project), plus the preview harness with no
daemon at all. Same rebuild loop as the palette table.

| # | Check | Result |
| --- | --- | --- |
| P11 | `palette_probe {thumbnails: true}` → 256px PNGs in `{store}/thumbs/<slug>.png` (76 stored from wrapper icon art); `list` / `get` expose the stored path as `thumb`; `thumbnailBase64` is stripped from the reply; a black/uniform viewer frame is reported (`thumbnailNote`) and **not** stored; a digest is never downgraded by a thumbnail failure | PASS |
| P12 | Dashboard Palette tab against the live daemon: roster renders across the categories with the stored thumbnails, card-state filters drive the same `select` the tools use; preview scenes `palette-tree` / `palette-empty` / `palette-analyse` verified with no daemon (`TDMCP_PREVIEW_SCENE`) | PASS |
| P13 | Analyse run (rescan → probe → thumbnails → agent handoff) driven from the modal, plus "Copy reference" → agent runs the `place` step verbatim | manual — the bulk loop mechanics were exercised over `POST /mcp/tools/call` (batch of 3, blacklist filtered, `remaining`-driven stop); the modal run and the paste-into-agent step remain human checks |

Findings from this run: the bulk loop must drive **explicit ids** from the
roster — `select {status: "all"}` never advances past its first page, so the
GUI computes its target list instead of re-issuing the same selector.

## Operational caveats

- `tdmcp-daemon install` skips re-extraction when `install.version` matches,
  so an edited `bridge/` package silently does **not** reach `{data_dir}`.
  Use `install --force` after any bridge edit, and clear
  `{data_dir}/bridge/tdmcp_bridge/__pycache__`.
- `install` also **resets `config.toml`**, dropping local edits (e.g.
  `[palette]` ignore lists) — back it up across a forced install.
