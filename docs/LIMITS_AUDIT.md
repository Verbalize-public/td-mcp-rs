# Limits Audit — hard caps vs soft caps

Audit of every size / count / time limit in the MCP stack, the margins between
them, and the recommended generous values. Motivation: current limits are too
tight on large TD projects; reliability is preferred over latency; hard limits
of low-level layers must sit far above the soft caps agents see (margin).

Status: **audit, resolution plan, and Phases 0/1/3(partial)/2(partial)/4
implemented — see §6.** §4 has live-reproduced findings from a 2026-08-25
stability pass (real daemon/TD kills and restarts, including one new bug not
previously known: §4.1, session-reinit swallows a clean timeout response).
§5 is the curated, sequenced resolution plan covering both the static audit
(§2) and the live findings (§4). §6 records what actually landed and what's
still open.

## 1. Limit inventory

### 1.1 Wire / transport (hard, invisible to agents)

| Limit | Value | Enforced at | Failure mode |
| --- | --- | --- | --- |
| IPC frame cap (`MAX_FRAME`) | 16 MiB | `crates/tdmcp-ipc/src/framing.rs:25` | `FrameError::TooLarge` — call dies |
| rmcp server request body | 4 MiB default, not overridden | `StreamableHttpServerConfig::default()` at `crates/tdmcp-daemon/src/main.rs:805`; rmcp 3.0.1 `streamable_http_server/tower.rs:55` (`DEFAULT_MAX_REQUEST_BODY_BYTES`) | request rejected before tool dispatch |
| axum JSON extractor bodies (fallback `/mcp/tools/call`, federation register/fleet-push, admin annotate/ingest) | 2 MiB axum default; no `DefaultBodyLimit` override anywhere | `crates/tdmcp-mcp/src/server.rs:143`, `crates/tdmcp-daemon/src/federation.rs:163,233`, `crates/tdmcp-daemon/src/admin.rs:210,365` | rejected |
| rmcp client SSE event size | 16 MiB, fixed by rmcp (`pub(crate)`, not configurable) | stdio-proxy & federation clients reading daemon responses (`rmcp-3.0.1 client_side_sse.rs:18` `DEFAULT_MAX_SSE_EVENT_SIZE`) | oversized response event dropped → silent hang until proxy ceiling |
| MCP stdio line framing | unbounded in our code | — | — |

### 1.2 Bridge payload caps (hard-fail)

| Limit | Value | Where |
| --- | --- | --- |
| `SCRIPT_MAX_BYTES` | 1 MiB | `bridge/tdmcp_bridge/constants.py:18` → `tdmcp.script.too_large` |
| `RESULT_MAX_BYTES` | 1 MiB | `constants.py:19`; enforced `bridge/tdmcp_bridge/execute.py:211`. Rejects **after** the script ran → executed work discarded (`tdmcp.script.result_too_large`) |
| capture `maxSize` | default 256 px, `null` = native, **no upper clamp** | `crates/tdmcp-mcp/src/tools.rs:354`, `bridge/tdmcp_bridge/capture.py:351`. Native-res noisy PNG + base64 can blow the 16 MiB frame |

### 1.3 Agent-facing batch soft caps

All soft: truncate first N + emit `truncation{field, limit, code, message,
mitigation}` metadata; tool stays `ok:true`. Mirrored Rust↔Python via
`bridge/fixtures/limits.json` + `bridge/tests/test_limits_parity.py`.

| Cap | Value | Rust const (`tools.rs`) / Python (`constants.py`) |
| --- | --- | --- |
| inspect `paths[]` | 96 | `INSPECT_PATHS_LIMIT` |
| inspect child roster per node | 96 | `CHILDREN_ROSTER_LIMIT` |
| api_help `queries[]` | 32 | `API_HELP_QUERIES_LIMIT` |
| api_help classes index | 1024 | `API_HELP_CLASSES_LIMIT` |
| api_help members summary / detailed | 40 / 512 | `API_HELP_MEMBERS_SUMMARY` / `_DETAILED` |
| api_help module sample | 32 | `API_HELP_MODULE_SAMPLE` |
| editor selection per pane / panes | 96 / 32 | `EDITOR_SELECTION_LIMIT` / `EDITOR_PANES_LIMIT` |
| shader lint scan ops / consumers | 512 / 16 | `SHADER_SCAN_LIMIT` / `SHADER_CONSUMER_LIMIT` |
| CHOP channels × samples (scalars total) | 32 × 256 = 4096 | `CHOP_DATA_MAX_CHANNELS/SAMPLES/SCALARS` |
| enableExpr evals | 32 | `ENABLE_EXPR_EVAL_LIMIT` |
| execute_python logs return / debug DAT ring | 32 KiB / 64 KiB | `_LOGS_RETURN_MAX` / `_DEBUG_DAT_RING_MAX` |
| capture default longer-side px | 256 | `CAPTURE_DEFAULT_MAX_SIZE` |

Internal diagnostics caps (fine as-is): `args_diag.rs:277 CAP=10`,
`template.rs:235 CAP=8`, logring ring 2048 entries / 64 KiB msg / 256 batch,
proxy log channel 1024, GUI render caps.

### 1.4 Time budgets

| Budget | Value | Where |
| --- | --- | --- |
| `[bridge].call_timeout_secs` (ping/inspect/capture/api_help/editor_context) | 45 s | config, `bridge.rs:47` |
| `[bridge].script_timeout_secs` (execute_python/mutate_nodes) | 120 s | config, `bridge.rs:49` |
| MCP oneshot safety net `BRIDGE_TIMEOUT` | 180 s **const** | `crates/tdmcp-mcp/src/tools.rs:43` |
| Master→slave proxy timeout `PROXY_TIMEOUT` | 130 s const | `tools.rs:46` |
| stdio proxy ceilings short/script/list | 105 s / 180 s / 30 s, **env-only** | `crates/tdmcp-mcp/src/daemon_link.rs:44-50`, `TDMCP_PROXY_*_TIMEOUT_MS` |
| TD-side worker wait `DEFAULT_MAX_CALL_WAIT_S` | 180 s | `task_queue.py:16` |
| Handshake frame IO | 5 s | `crates/tdmcp-ipc/src/listener.rs:17` |
| Heartbeat interval / pong timeout / idle-dead | 5 s / 8 s / 20 s | config ↔ `HeartbeatConfig` (`bridge.rs:51-55`) |
| Fleet probe max / health / connect | 5 s / 800 ms / 2 s | `daemon_link.rs:40,52,54` |
| Stdio-proxy initial connect attempts | 3 | `crates/tdmcp-daemon/src/main.rs:327` |
| Per-pid job queue depth | 32 | `bridge.rs:40` (`JOB_CHANNEL_CAPACITY`) |

Liveness design note: heartbeat pings are answered on the bridge worker thread
without the main-thread pump (`task_queue.py:491`, test
"Idle heartbeat ping must not depend on the main-thread pump"), so long scripts
do not trip idle-dead by themselves. Idle-dead bites when the whole TD process
stalls (modal dialogs, GC pauses).

## 2. Broken margins (hard limits NOT far above soft caps)

1. **Request path capped by accident.** Effective ingress is 2 MiB (axum
   routes) or 4 MiB (rmcp streamable) — both below the intended 16 MiB wire
   design, and inconsistent with each other: the same payload succeeds on one
   endpoint and fails on the other.
2. **Response path has zero top margin.** IPC frame 16 MiB == rmcp SSE event
   limit 16 MiB (fixed). Additionally `rmcp_handler.rs:216-228` serializes
   every result **twice** (structured content + identical JSON text block), so
   a ~8 MiB result already risks a dropped SSE event → silent hang until the
   proxy ceiling fires.
3. **Oversized results discard completed work.** `RESULT_MAX_BYTES` rejects
   after execution; a long script returning >1 MiB loses everything.
4. **Hidden glass ceilings in the timeout chain.** Raising
   `script_timeout_secs` beyond 120 s silently hits `BRIDGE_TIMEOUT` (180 s
   const), then the env-only proxy ceiling (180 s). The config knob pretends to
   go higher than the stack allows.
5. **20 s idle-dead tears down on process-wide stalls** (modal dialog, GC) —
   exactly the large-project case. Reconnect exists (resurrection) but costs a
   failed call.
6. **capture native resolution unbounded** — a large noisy TOP can produce a
   PNG+base64 payload that dies at the frame cap mid-call.

## 3. Recommended ladder ("hard ≫ soft")

Rule of thumb: agent-visible soft response ≤ ~1 MiB typical → payload hard caps
≥ 4× that → transport ≥ 2× payload → everything stays under rmcp's immovable
16 MiB SSE event budget (which also bounds responses given double-serialization).

These are the **target values**; §5 sequences *when* it's safe to move each
one (several are gated on a correctness fix landing first, so the cliff they
guard against doesn't just move further out).

### 3.1 Wire / transport

| Knob | Now | Proposed | Why |
| --- | --- | --- | --- |
| rmcp server body (`main.rs:805`) | 4 MiB | `.with_max_request_body_bytes(16 MiB)` | one line; matches frame |
| axum routes (fallback/federation/admin) | 2 MiB | `DefaultBodyLimit::max(16 MiB)` layer | removes 2<4 inconsistency |
| `MAX_FRAME` | 16 MiB | **32 MiB** | ≥2× largest legal request; headroom for deliberately-uncapped DAT content reads |

### 3.2 Bridge payloads

| Knob | Now | Proposed |
| --- | --- | --- |
| `SCRIPT_MAX_BYTES` | 1 MiB | **4 MiB** |
| `RESULT_MAX_BYTES` | 1 MiB reject-after-run | **4 MiB + truncate-and-return-with-marker** (ok:false with truncated partial result + metadata, or ok:true + truncated flag — never discard work) |
| capture clamp | none, default 256 | clamp `maxSize` ≤ 1536; default 256 → **512** |

### 3.3 Agent-facing soft caps

| Cap | Now | Proposed |
| --- | --- | --- |
| inspect paths / child roster | 96 / 96 | **256 / 256** |
| api_help queries / classes index | 32 / 1024 | **64 / 2048** |
| api_help members summary / detailed | 40 / 512 | **128 / 1024** |
| editor selection / panes | 96 / 32 | **256 / 64** |
| shader scan / consumers | 512 / 16 | **2048 / 64** |
| CHOP channels / samples / scalars | 32 / 256 / 4096 | **64 / 1024 / 32768** |
| enableExpr evals | 32 | **64** |
| exec logs return / debug ring | 32 KiB / 64 KiB | **128 KiB / 256 KiB** |
| capture default px | 256 | **512** |

Soft caps only ever trade tokens-per-response for fewer round-trips — aligned
with reliability-over-latency.

### 3.4 Time budgets

| Knob | Now | Proposed |
| --- | --- | --- |
| `call_timeout_secs` | 45 s | **90 s** |
| `script_timeout_secs` | 120 s | **600 s** |
| `BRIDGE_TIMEOUT` / `PROXY_TIMEOUT` consts | 180 / 130 s fixed | derive from script budget (+60 s margin, floor 180 s); pass budgets into `McpHandler` state |
| proxy ceilings | env-only 105/180/30 s | promote to `[proxy]` config keys; defaults scaled 150/720/60 s |
| pong / idle_dead / handshake IO | 8 / 20 / 5 s | **15 / 60 / 10 s** |
| health / connect / fleet probe | 800 ms / 2 s / 5 s | **2 s / 5 s / 10 s** |
| `JOB_CHANNEL_CAPACITY` | 32 | **128** |

## 4. Live audit findings (2026-08-25)

Reproduced against a running daemon (`tdmcp-daemon start --port 9860`) and two
real TouchDesigner processes (`td-sandbox/toe/_agent_tdmcprs_dev`), including
deliberate kills/restarts of both TD and the daemon. Timestamps from
`daemon.2026-08-25.log`. Ranked by how much blast radius each has beyond the
one call that triggered it.

### 4.1 A clean server-side timeout can still reach the client as a raw `-32603` (new)

Repro: `execute_python` with `time.sleep(130)` against `script_timeout_secs =
120`. While it was in flight (backgrounded past 120s by the client harness),
two unrelated tool calls (`api_help`, `mutate_nodes`) were sent on the same
stdio-proxy connection.

Log trace:

```
16:09:15.170 tool call failed code=tdmcp.bridge.timeout elapsed_ms=120002  # daemon computed the correct, curated error
16:09:20.172 discarding stale bridge response (prior timeout)             # TD's late reply correctly ignored
16:09:23.406 create new session <uuid>                                    # a second rmcp streamable-http session opens
16:09:23.459 call forward failed: Mcp error -32603: streamable HTTP session was re-initialized before the response arrived
```

The daemon did its job — it resolved the call to `tdmcp.bridge.timeout` at
exactly the configured 120s budget. But the *client* never saw that payload:
issuing another tool call on the same connection while the long one is still
open causes the stdio-proxy/rmcp streamable-http session to reinitialize,
orphaning the first request's pending SSE response. The agent gets a bare
JSON-RPC `-32603` with no `code`, no `mitigation`, nothing — the one failure
shape the curated-error contract (`crates/tdmcp-daemon/tests/error_surface.rs`)
was built to eliminate, reappearing at the transport layer instead of the
application layer.

This is strictly worse than a plain timeout: a caller cannot distinguish "the
script is still running" from "the script's result is gone forever," and any
agent that polls `fleet` (or retries) while a slow script runs will reproduce
this on every long call. It also means the `[bridge].script_timeout_secs`
knob this doc proposes raising to 600s makes the window for hitting this
*larger*, not smaller — the fix here is a prerequisite for §3.4, not optional
polish.

**Recommendation:** the stdio-proxy should keep one streamable-http session
alive per daemon connection and pipeline/queue client requests onto it rather
than opening a second session while the first response is outstanding; at
minimum, a session reinit should flush a synthetic curated error for any
orphaned in-flight request instead of surfacing the rmcp transport error.

### 4.2 `capture maxSize:null` on a large TOP kills the whole bridge connection, not just the call (confirms §2.6)

Repro: created a `noiseTOP` under `/project1/e2e_kit/zone`, resized it up.

| Resolution | PNG bytes | Result |
| --- | --- | --- |
| 4096×4096 | 3.10 MB | ok |
| 8192×8192 | 8.45 MB | ok |
| 16384×16384 | (never returned) | **bridge dies** |

```
15:58:14.329 tool call complete tool=capture elapsed_ms=10074    # 8192 case, slow but fine
15:58:56.435 bridge session ended — disconnected, cancelled tasks stacked   # 16384 case, 30.4s in
15:58:56.501 pid handshake — resurrected                          # TD's own reconnect, ~66ms later
```

The failure isn't a clean per-call rejection (no `tdmcp.script.result_too_large`
equivalent for capture) — the IPC frame overflow tears down the *entire*
bridge session for that pid, cancelling anything else queued behind it. In
this instance TD's main thread was healthy, so resurrection was near-instant
(66ms); §4.3 shows that isn't guaranteed. Confirms the doc's existing
recommendation (§3.2, clamp `maxSize` ≤ 1536) — this repro shows the clamp
needs to live in the tool itself (reject before capture, like `SCRIPT_MAX_BYTES`
does), not just as a lower default, since `maxSize:null` is explicitly
offered as "native resolution" in the schema with no upper bound.

### 4.3 A stalled TD process degrades to minutes of flaky bridge death with no way to tell "busy" from "dead" (confirms §2.5)

The pre-existing dev TD process (pid 29660, up ~4h, growing working set) went
through repeated cycles of: `inspect /project1` (a single-node, no-recursion
call) → 45s `tdmcp.bridge.timeout` → 8s later, heartbeat pong timeout →
`bridge session ended — disconnected` → 15s TTL eviction from `fleet` → empty
`fleet` for minutes → silent resurrection → same thing again on the next
call. Confirmed reproducible three times against the same process; killing it
and starting a fresh TD process made the identical `inspect /project1` call
return instantly (single-digit ms), so the problem was state accumulated in
that one long-lived process, not `inspect` itself or project size (11
top-level children on the clean instance).

Nothing in the agent-facing surface distinguishes "TD's main thread is
legitimately busy, wait" from "TD is wedged, kill it" — `fleet` just shows
the pid vanish and reappear. An agent hitting this has no signal to act on
besides "keep polling `fleet` for a few minutes, then give up and restart
TD," which is exactly what this audit ended up doing. Confirms §2.5's
existing note; the actionable gap is diagnostic, not a timeout value: `fleet`
has no notion of "TD process is alive but its main thread hasn't pumped in
N seconds" separate from "bridge socket is gone."

**Recommendation:** surface the bridge's last-successful-heartbeat age (or
last main-thread-pump timestamp from the bootstrap tox) in `fleet` even while
`bridge:"connected"`, so an agent can tell a slow-but-alive process from one
about to time out, instead of finding out via a 45s hang.

### 4.4 Confirmed: `RESULT_MAX_BYTES` silently discards completed writes (repro for §2.3)

```python
op('/project1').store('tdmcp_audit_marker', 'work_was_done')
result = 'x' * (2 * 1024 * 1024)
```

→ `ok:false`, `tdmcp.script.result_too_large`, message says only "return a
smaller result." A follow-up call confirmed `op('/project1').fetch('tdmcp_audit_marker')
== 'work_was_done'` — the store executed and persisted; the error gives the
caller zero indication anything ran. Exactly the failure mode §2.3 predicted,
now with a concrete repro. By contrast `SCRIPT_MAX_BYTES` (oversized script
*input*) rejects cleanly pre-execution with no side effects — confirms the
fix belongs specifically on the result path, not the whole tool.

### 4.5 Confirmed: transport size limits are inconsistent and the smaller one raises a raw HTTP error (repro for §2.1)

Same 3 MiB JSON body:

- `POST /mcp/tools/call` (axum) → raw `413 Payload Too Large`,
  `content-type: text/plain`, body `Failed to buffer the request body: length
  limit exceeded` — no `ok`/`items`/JSON at all, breaking the curated-error
  contract at the transport edge.
- `POST /mcp/rpc` (rmcp streamable) with the same byte count passes the size
  check and gets to protocol-level processing (rejected instead for missing
  session init, which is a separate, expected error) — confirming the 2 MiB
  vs 4 MiB split lets the same payload behave differently by endpoint.

### 4.6 Minor: unknown pid reuses the "lost bridge" message

`inspect` against a pid that was never registered (`999999`) returns the same
`tdmcp.bridge.lost` / "wait for resurrection" mitigation as a pid that
genuinely disconnected. For a pid that never existed there is nothing to
resurrect; the mitigation actively wastes a retry. Cheap fix: `PidRegistry`
already knows the difference (present-and-disconnected vs never-seen) —
thread that into the error code/message.

### 4.7 Verified working as designed (no finding, recorded so it isn't re-litigated)

- Same-pid overlapping tool calls (a 3s `execute_python` fired alongside an
  `inspect`) queued and completed correctly — no corruption, no crossed
  results, softening how sharp the "never parallel" warning needs to be for
  short calls (it's real for the long-call case, see §4.1).
- `paths[]` soft-cap truncation (96) returns `ok:true` + `pathsTruncated` +
  `truncation{...}` exactly as documented, for a 104-entry batch.
- Daemon kill → restart: proxy and both TD bridges self-healed within
  seconds with zero manual reconnect steps once the daemon process came back
  on the same port. The proxy's own "may still be starting" message during
  the outage is optimistic wording (it does *not* auto-spawn the daemon, per
  §2 note 5) but doesn't block correct recovery once the daemon is
  restarted externally.

## 5. Curated resolution plan

Phased so that **no phase widens the blast radius of a bug a later phase still
has to fix**. Concretely: don't raise `script_timeout_secs` (§3.4) before
fixing the session-reinit bug (§4.1) — that only gives the orphaning window
more time to trigger. Don't raise `RESULT_MAX_BYTES` (§3.2) before it stops
discarding work — that only moves the silent-loss cliff further out. Phase 0
is entirely correctness fixes at **today's** limit values; phases 1–3 are the
already-fully-specified §3 ladder, safe to apply once Phase 0 lands; Phase 4
is the mechanical parity sweep every change must carry.

### Phase 0 — Stop the silent-loss bugs (correctness, no limit changes)

Blocking, do these first; each one is a bug independent of any proposed
number in §3.

| # | Fix | Files | Verifies against |
| --- | --- | --- | --- |
| 0.1 | Stdio-proxy: never open a second streamable-http session while a request is still outstanding on the first. Queue/pipeline subsequent client calls onto the live session instead of reinitializing. If a reinit is unavoidable (daemon-side session eviction), synthesize a curated `tdmcp.proxy.session_reinit` error for the orphaned in-flight request instead of surfacing rmcp's raw `-32603`. | `crates/tdmcp-mcp/src/daemon_link.rs`, stdio-proxy session handling in `crates/tdmcp-daemon/src/main.rs` | §4.1 repro: long `execute_python` + an unrelated call on the same connection while it's outstanding must not lose the timeout response |
| 0.2 | `RESULT_MAX_BYTES`: stop rejecting after execution. Truncate the result to the cap, return `ok:true` (or `ok:false` with the truncated partial payload attached, decision already recorded at the top of this doc) plus `truncation{...}` metadata — never a bare "return a smaller result" with the real result thrown away. | `bridge/tdmcp_bridge/execute.py:211` | §4.4 repro: `store()` a marker, return an oversized result, confirm the response now carries truncated output instead of just an error, no extra round-trip needed to learn the write happened |
| 0.3 | `capture`: reject `maxSize` above a hard ceiling (mirror `SCRIPT_MAX_BYTES`'s pre-flight rejection) instead of attempting the capture and letting the IPC frame overflow tear down the bridge session. Applies even before the §3.2 clamp value (1536) is chosen — enforce *some* ceiling now. | `crates/tdmcp-mcp/src/tools.rs:354`, `bridge/tdmcp_bridge/capture.py:351` | §4.2 repro: `maxSize:null` (or any value producing >~12 MiB PNG) on a large TOP must return a curated size-rejection error, not kill the bridge session |

### Phase 1 — Transport consistency (safe once Phase 0 lands)

| # | Fix | Files |
| --- | --- | --- |
| 1.1 | `DefaultBodyLimit::max(16 MiB)` layer on the axum routers (fallback `/mcp/tools/call`, federation register/fleet-push, admin annotate/ingest) | `crates/tdmcp-mcp/src/server.rs:143`, `crates/tdmcp-daemon/src/federation.rs:163,233`, `crates/tdmcp-daemon/src/admin.rs:210,365` |
| 1.2 | `.with_max_request_body_bytes(16 MiB)` on the rmcp streamable server config, so it matches 1.1 instead of the current 4 MiB default | `crates/tdmcp-daemon/src/main.rs:805` |
| 1.3 | Wrap the axum body-limit rejection (currently a bare `413`/`text/plain` from `DefaultBodyLimit`, reproduced live in §4.5) in a JSON error handler so oversized requests get the same curated `{ok:false, items[]}` envelope as every other failure | same axum routers as 1.1 |
| 1.4 | `MAX_FRAME` 16 MiB → 32 MiB, so the transport bump in 1.1/1.2 doesn't immediately reintroduce the "response has zero top margin" problem (§2.2) | `crates/tdmcp-ipc/src/framing.rs:25` |
| 1.5 | Stop double-serializing results as text + structured content in `rmcp_handler.rs` for large payloads — halves response wire size, buys back SSE-event headroom under the still-fixed 16 MiB rmcp SSE cap | `crates/tdmcp-mcp/src/rmcp_handler.rs:216-228` |

### Phase 2 — Timeout chain + diagnosability (safe once Phase 0 lands; independent of Phase 1)

| # | Fix | Files |
| --- | --- | --- |
| 2.1 | Derive `BRIDGE_TIMEOUT` / `PROXY_TIMEOUT` from `[bridge].script_timeout_secs` (+60 s margin, floor 180 s) instead of fixed consts, so raising the config knob in Phase 3 doesn't silently hit a lower hardcoded ceiling (§2.4) | `crates/tdmcp-mcp/src/tools.rs:43,46` |
| 2.2 | Promote stdio-proxy ceilings (`TDMCP_PROXY_*_TIMEOUT_MS`) from env-only to `[proxy]` config keys | `crates/tdmcp-mcp/src/daemon_link.rs:44-50`, `docs/CONFIG.md` |
| 2.3 | `fleet`: surface bridge liveness detail beyond `connected`/`disconnected` — last-heartbeat age or TD main-thread-pump staleness, so an agent can tell "busy, still alive" from "about to time out" (§4.3) instead of discovering it via a 45 s hang | `crates/tdmcp-mcp/src/fleet.rs`, `bridge/tox_callbacks.py` (pump timestamp) |
| 2.4 | Distinguish never-registered pid from disconnected pid in the `tdmcp.bridge.lost` message/mitigation (§4.6) | `crates/tdmcp-core/src/registry.rs`, wherever `tdmcp.bridge.lost` is raised |

### Phase 3 — Raise the limits (only after Phases 0–2; values already specified in §3)

Apply §3.1–§3.4 tables as-is once the above land. Nothing new to design here —
this phase is "flip the constants," which is exactly why it's safe to do
last: every failure mode a bigger number could make worse (silent loss,
raw transport errors, glass-ceiling timeouts, unbounded capture) is already
fixed by Phase 0–2.

### Phase 4 — Mechanical lockstep sweep (part of every phase above that touches a shared value)

Every limit change must move together:

- [x] `bridge/tdmcp_bridge/constants.py` — Python caps
- [x] `crates/tdmcp-mcp/src/tools.rs` — Rust consts **and verbatim tool
      description strings** ("soft-capped at 96" etc. appear inside schemas)
- [x] `bridge/fixtures/limits.json` — parity fixture (`test_limits_parity.py`)
- [x] Docs mirror: `docs/CONTRACT.md` (§ sizes/caps rows), `docs/CONFIG.md`
- [ ] No tox repack needed for Phase 0–3 — none of it touches `bootstrap.py` /
      `tox_callbacks.py` **except** 2.3 (pump-staleness timestamp), which does
      and needs `pack_bootstrap_tox.md` re-run + live-instance reload

## 6. Implementation status (2026-08-25)

Implemented in this pass, in order, each with its own tests (`cargo test
--workspace` + `python -m pytest bridge/tests` both green throughout):

- **Phase 0 — done.** 0.1 (stdio-proxy `call_gate` single-flight, in
  `daemon_link.rs`/`stdio_proxy.rs`), 0.2 (`execute.py` truncates oversized
  `result` instead of discarding the run), 0.3 (`capture.py` hard-rejects
  `maxSize`/native resolution above `CAPTURE_MAX_SIZE`=1536 pre-flight).
- **Phase 1 — done**, with one deliberate scope cut. 1.1/1.2/1.4 (16 MiB
  `DefaultBodyLimit` + rmcp request body cap + 32 MiB `MAX_FRAME`), 1.5
  (`rmcp_handler.rs` drops the duplicate `content[].text` copy of
  `structuredContent` above 256 KiB). 1.3 (curated JSON on body-limit
  rejection) landed on `/mcp/tools/call` only — the agent-facing route
  `error_surface.rs` actually gates. Federation/admin routes (`register`,
  `fleet-push`, `/admin/config`) still return axum's raw rejection; revisit
  if agents ever hit those directly (today they don't — daemon-to-daemon /
  GUI-only).
- **Phase 2 — partial.** 2.1 done: `tools::init_bridge_timeouts` derives
  `BRIDGE_TIMEOUT`/`PROXY_TIMEOUT` from `[bridge].script_timeout_secs` (+60s
  margin, floored at the historical consts) via a process-wide `OnceLock`,
  called once from `main.rs`. 2.4 done: `BridgeRpcError::Unknown` (new
  variant) vs `NotConnected`, decided in `BridgeSessions::call` by checking
  `PidRegistry` before falling back to "no session" — the pre-existing but
  previously-unused `tdmcp.bridge.unknown_pid` catalog code now actually
  fires. **2.2 (promote proxy ceilings to `[proxy]` config) and 2.3 (fleet
  visibility into TD main-thread pump staleness) are deferred** — 2.3 touches
  the live bootstrap tox (`tox_callbacks.py` main-thread pump) and needs a
  dedicated pack/reload/live-verify pass, not a code-only change; 2.2 is
  config-schema work with its own docs/back-compat surface.
- **Phase 3 — partial, deliberately.** All of §3.2 (bridge payload caps:
  `SCRIPT_MAX_BYTES`/`RESULT_MAX_BYTES` → 4 MiB) and §3.3 (agent-facing soft
  caps) landed, plus `JOB_CHANNEL_CAPACITY` → 128 from §3.4. **Not landed:
  `call_timeout_secs`/`script_timeout_secs`/`heartbeat`/`pong`/`idle_dead`
  config defaults, and the proxy ceilings** (§3.4's remaining rows). Reason:
  §4.1's live repro showed the *concurrency* bug clearly (fixed by 0.1), but
  a single call approaching the proposed 600s `script_timeout_secs` was only
  verified live up to ~70s in this session (§4.7-adjacent finding — a lone
  70s call survives the daemon's 60s rmcp session `keep_alive` fine, so the
  600s target is plausible) — not the full 600s. Raising the config default
  without a longer live soak test would be shipping an unverified number for
  exactly the kind of long-tail timing bug this audit exists to catch. The
  values are still fully specified in §3.4; landing them is now just editing
  `tdmcp-config` defaults + `crates/tdmcp-daemon/src/bridge.rs` consts +
  `docs/CONFIG.md` + the fixture, once soak-tested.
- **Phase 4 — done for everything landed above** (see checklist).

Net effect: every *correctness* bug found live in §4 is fixed (0.1–0.3, 2.4);
every *safe-to-move-now* number in §3 moved; every number whose safety this
session couldn't actually verify live (the two script-timeout-adjacent
config defaults) was left alone rather than guessed.
