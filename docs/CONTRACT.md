# td-mcp-rs contract

Durable v1 technical contract. Status markers:


| Marker      | Meaning                                              |
| ----------- | ---------------------------------------------------- |
| **Shipped** | Implemented and covered by tests / live E2E          |
| **Planned** | Specified; not yet in the tool surface or incomplete |


Crate layout: `[ARCHITECTURE.md](../ARCHITECTURE.md)`. Engineering law: `[CONSTITUTION.md](../CONSTITUTION.md)`.

---

## Goals (v1) — Shipped intent

1. **Multi-instance first** — every mutating call names the target by OS `pid`. No session “current target,” no generated peer ids.
2. **Live operate only** — inspect, mutate, script, verify on a connected bridge. No `.toe` / `.tox` binary editing.
3. **Agent-shaped surface** — small tool set; `**fleet` → `inspect` → `capture`** three-layer read model; perception is explicit (`capture`); uniform diagnostics; summary-by-default; timeouts fail the *wait* (not claim TD cancelled).
4. **Connected ⇒ usable** — `bridge: "connected"` ⇒ any MCP caller may address that `pid`. Coordination via visible tasks + **hard sequential gates** (session chill + per-pid exclusive enqueue); overload fails fast and never tears down the bridge.
5. **One local control plane** — long-lived daemon owns pid→bridge map, per-pid queues, MCP surface, bridge sessions.
6. **Self-contained delivery** — one binary + one drop-in `.tox` bootstrap.
7. **Resurrection on reconnect** — on IPC loss the daemon states the disconnect and stacks cancelled tasks until the first successful task afterward (then erase).

### Non-goals (v1)


| Item                                      | Why                                                                                                     |
| ----------------------------------------- | ------------------------------------------------------------------------------------------------------- |
| Sticky / `select_target` / session peer   | Replaced by per-call `pid` + `fleet`                                                                    |
| Generated `targetId` / UUID / path-hash   | `**pid` is the only id**                                                                                |
| Offline ToeDigest / `.toe` write / inject | Separate MCP; v1 adopt path = drop tox                                                                  |
| Remote / WAN TD control                   | After local contract is boring                                                                          |
| Multiple bridge protocols                 | One local IPC                                                                                           |
| Silent auto-reconnect (TD↔daemon IPC)     | Explicit resurrection policy (see Goals §7). Distinct from stdio↔daemon HTTP reconnect-only heal below. |
| Lifecycle create/start/stop               | **P2+**                                                                                                 |


---

## Architecture — Shipped

```text
 IDE (Cursor)  ── stdio ──► tdmcp-daemon mcp ──► HTTP MCP proxy ──┐
 Other MCP callers ── Streamable HTTP http://127.0.0.1:9860/mcp ──┤
                                                                  ▼
 ┌────────────── Daemon process (tdmcp-daemon, single binary) ──────────┐
 │  background: axum + rmcp │ admin │ PidRegistry │ per-pid queues      │
 │  main thread (default): tray + toast; dashboard on demand (gui)      │
 └──────────┬───────────────────────────────────────────────────────────┘
            │  local IPC (Win named pipe / Unix UDS)
            ▼
   TD process(es)  ←── bootstrap .tox → handshake → FS load bridge/
```

### Surfaces


| Direction         | Transport                                                            | Role                             | Status      |
| ----------------- | -------------------------------------------------------------------- | -------------------------------- | ----------- |
| IDE → daemon      | Streamable HTTP MCP (`/mcp/rpc`) + JSON fallback (`/mcp/tools/*`)    | Tool calls (direct HTTP clients) | **Shipped** |
| IDE → daemon      | stdio (`tdmcp-daemon mcp` → stdio proxy → HTTP `/mcp/rpc`)           | Cursor entrypoint                | **Shipped** |
| TD → daemon       | Local IPC                                                            | Bridge                           | **Shipped** |
| Operator → daemon | In-process tray + admin HTTP + OS toasts (`gui` feature, default on) | Human monitor                    | **Shipped** |
| IDE → daemon      | Streamable HTTP remote (`bind_address` + optional Bearer PSK)        | LAN / non-loopback MCP           | **P3**      |
| Master → slave    | Streamable HTTP tool proxy (`daemonId`) + `/admin/federation/*`      | Single-level federation          | **P3**      |
| Bridge/proxy → daemon | Bridge `log` events (IPC) + `POST /admin/logs/ingest` (proxy)   | Log uplink into the central JSONL sink | **Shipped** |
| Operator → daemon | `GET /admin/logs*` + tray `View::Logs`                               | Central log tail, filter, follow | **Shipped** (TD-side textport mirror: **Planned**, needs live-TD verification) |


**Singleton:** one owner per listen port. Exclusivity = `daemon.lock` (pid) + TCP bind on `{bind_address}:{port}` (default `127.0.0.1`). Stale locks (dead pid) are reclaimed on `start` / `ensure`. A second `start` while healthy refuses with a clear error. `/admin/restart` clears the lock then spawn-then-exit; the replacement retries bind briefly. No distributed leader election — single-host by default; P3 federation is single-level master→slave, not multi-master.

**Idle auto-exit:** after **30s** with zero connected bridges and zero live Streamable HTTP MCP session leases (stdio proxy counts), the daemon toasts and cancels the serve loop (same path as `/admin/shutdown` / ctrl_c). A **5s startup grace** after the idle watcher starts prevents a freshly-(re)started daemon from exiting before the stdio proxy re-acquires a Streamable HTTP lease. Drain is deadline-bounded (~2s); the process then ends on the main thread — never via `process::exit` from a background tokio task. Admin/health polls and JSON `/mcp/tools/`* do not keep it alive. Assumes session-mode MCP (not per-request handlers); production Streamable HTTP disables SSE keepalive and wires the daemon shutdown token (same as integration tests). Override: `TDMCP_IDLE_EXIT_SECS` (`0` disables). `ensure` / `mcp` respawn on next use. Tests may set `TDMCP_IPC_PIPE` so a live TD on the production pipe cannot attach.

**Listen:** `{bind_address}:{port}` default `127.0.0.1:9860` (override via CLI / env / RC / `[server]`). Non-loopback bind requires `[auth] mode = "psk"` with a non-empty PSK (`Authorization: Bearer`). Admin surfaces: loopback-only for shutdown/restart/sessions; auth-gated remote allowlist for `/admin/federation/*` and `/admin/config`; minimal unauth probe at `/admin/federation/status` for LAN discovery. See [`CONFIG.md`](CONFIG.md) § Federation auth & admin surface.

**Stdio proxy (v1):** forwards tools request/response only (`list_tools` /
`call_tool`). Server-initiated notifications are **not** forwarded. The HTTP
daemon is the control plane; Cursor invokes `tdmcp-daemon mcp`, which
`ensure`s once at cold start then runs the stdio shim for the MCP client
session.

**Stdio proxy resilience:** if the HTTP link to the daemon is lost (e.g.
`/admin/restart`, crash, idle-exit), the shim attempts a **reconnect-only**
heal — it never spawns / upserts a daemon mid-session. Heal is single-flight
with **waiters** (bounded gate wait; concurrent callers share the in-flight
outcome instead of failing open) and debounced for the gate holder; a
background watcher keeps probing while unhealthy so a freshly-restarted
daemon does not idle-exit again before the next tool call (paired with the
idle startup grace above). The failed call always returns
`tdmcp.daemon.unreachable` with downtime and a suggestion (no silent retry of
the tool). Thresholds:
`TDMCP_RECONNECT_RECENT_MS` (default 3000), `TDMCP_RECONNECT_STALE_MS` (15000),
`TDMCP_RECONNECT_DEBOUNCE_MS` (250), `TDMCP_RECONNECT_PROBE_INTERVAL_MS` (500),
`TDMCP_RECONNECT_PROBE_MAX_MS` (5000). This is **not** the TD↔daemon IPC
resurrection policy (Goals §7 / non-goal “Silent auto-reconnect”).

### Bridge transport — Shipped


| OS            | Mechanism          | Address                 |
| ------------- | ------------------ | ----------------------- |
| Windows       | Named pipe         | `\\.\pipe\tdmcp-rs`     |
| macOS / Linux | Unix domain socket | `{dataDir}/bridge.sock` |


Handshake returns a local FS path to the bridge package directory. TD reloads from disk on every handshake. Package: `bridge/` + `manifest.json` (`protocolVersion`, `minDaemon`, entry). Post-connect handshake frame I/O is bounded at **5s** (`HANDSHAKE_IO_TIMEOUT`); a peer that connects then stalls is dropped so the accept loop can take the next connection. Handshake field `minDaemon` is **unused in v1** (always omitted by the daemon); `tdmcp.bridge.version` stays **reserved** in the catalog, not emitted.

### Addressing — Shipped


| Rule            | Detail                                                                                                                              |
| --------------- | ----------------------------------------------------------------------------------------------------------------------------------- |
| Ground truth    | OS `pid` only                                                                                                                       |
| Discovery       | `fleet` lists by `pid` + **title** (`project.name`) + **toePath** (`folder`+`name`), bridge, tasks, traces                          |
| Identity source | Filled at bridge handshake from a main-thread snapshot; stale until reconnect (no mid-session refresh)                              |
| Fingerprint     | Best-effort `image` + `startTime`; when **both** sides omit `startTime`, require a shared `title` or `image` (empty/empty ≠ match) |
| Deferred        | `windowStatus` and `fleet` `popups` — empty until P1 `dialogs` (Win)                                                                |
| Usable          | `bridge: "connected"` ⇒ any caller may address that `pid`                                                                           |
| Addressing      | Process-scoped tools require `pid`                                                                                                  |
| IPC loss        | Mark disconnected; cancel waits; stack cancelled-task trace; remove from fleet after **15s** TTL or when **any** handshake succeeds |
| Resurrection    | Same `pid` re-handshakes within the grace window ⇒ connected; stack cleared on first successful task                                |
| Pid reuse       | Best-effort fingerprint; mismatch ⇒ clear that pid’s state only                                                                     |


### Task queue — Shipped

**Goals**

1. Bridge IPC stays up under agent abuse — overload returns typed errors; never tears down the peer.
2. Hard sequential use of TD for bridged tools: a second concurrent call fails fast.

**Non-goals**

1. Concurrent / pipelined tool execution as a supported mode.
2. Pre-empting or cancelling in-flight TD main-thread work (timeout ≠ cancel; see RISKS R4).
3. Throughput features (multi-flight wire, async inspect jobs).

**Dual gates (bridged tools only)**

| Gate | Scope | On conflict |
| --- | --- | --- |
| **Session chill** | `(mcp_session_id, pid)` — at most one in-flight bridged tool | `tdmcp.mcp.session_busy` |
| **Pid exclusive** | Per-pid `TaskQueue` — always exclusive enqueue | `tdmcp.bridge.queue_busy` |

Bridged tools (`execute_python`, `inspect`, `capture`, `mutate_nodes`, `api_help`, `editor_context`) always enqueue **exclusive**. Client `exclusive` is accepted for wire compat and **ignored**. Shared multi-enqueue is not a supported mode.

**Exempt** (no session chill, no task-queue enqueue): `fleet`, `describe_tools`, wire heartbeat `ping`.

JSON `/mcp/tools/call` has no session lease — session chill is skipped; pid exclusive still applies.


| Mode | Behavior |
| --- | --- |
| **Exclusive (always for bridged tools)** | Fails if the per-pid queue is non-empty |
| **Shared** | Reserved / unused by MCP bridged tools |


### Disconnect / resurrection — Shipped

1. State the loss (`bridge` disconnected, `lastDisconnectAt`).
2. Cancel waits; stack cancelled tasks (`reason: bridge_lost`).
3. Same `pid` re-handshake within the grace window ⇒ usable again; stack stays until first success.
4. First successful task clears the stack. First failure after resurrection keeps it.
5. **Fleet eviction:** while still `disconnected`, the pid is removed from the
  registry (and thus `fleet`) when either **any** successful handshake occurs
   (other ghosts purged; the connecting pid stays / resurrects) or **15s**
   elapses since this loss (`DISCONNECTED_TTL`).

**Idle heartbeat (liveness):** after handshake, the daemon session actor probes
the peer with wire `ping` outside the task queue (not visible in `fleet`
tasks). Defaults (configurable via `[bridge]` in `config.toml`): interval
**5s**, pong wait **8s**, idle-dead **20s** (no inbound framed traffic). Any
completed tool wait (ok, bridge error, or call timeout) and any successful
ping/pong reset the inactivity clock — a call budget longer than idle-dead must
not immediately tear the session down after `tdmcp.bridge.timeout`.
Missed pong or idle-dead → same teardown as IPC loss. The bridge answers `ping`
on the IPC worker thread (no main-thread `process_pending`) and exits its serve
loop after the handshake-forwarded idle-dead budget (default **20s**) when read
timeouts are available. Mid-frame reads tolerate short poll stalls; the bridge
only treats a transfer as dead after idle-dead with **no byte progress** (then
disconnects and closes the stream). Idle detection and fleet eviction are
separate clocks (eviction TTL remains **15s**). IPC frames are hard-capped at
**32 MiB**. `GET /mcp/health` remains daemon-process liveness only — not bridge
peer health.

**Per-call wait budgets:** `ping` / `inspect` / `capture` / `api_help` / `editor_context` default to **45s**;
`execute_python` / `mutate_nodes` default to **120s** (`[bridge].call_timeout_secs`
/ `script_timeout_secs`). A timeout fails the wait (`tdmcp.bridge.timeout`) and
does **not** tear down the session. Stale late responses from a timed-out call
are discarded on the next receive so they cannot surface as `tdmcp.bridge.lost`.
The MCP dispatch layer keeps a separate **180s** oneshot ceiling as a hang
safety net only — the daemon owns the real per-method budgets.

---

## MCP tools

### Tool template


| Field        | Content                                                                                    |
| ------------ | ------------------------------------------------------------------------------------------ |
| Name         | stable snake_case                                                                          |
| Params       | typed; process-scoped tools require `pid`; paths use `OpPath`                              |
| Diagnostics  | uniform `diagnostics` envelope; stable `tdmcp.*` codes                                     |
| Detail flags | `detailLevel` (structure), `diagnosticLevel` (error payload), `resultRef` (large payloads) |


### Catalogue


| Tool                        | Job                                                                                                    | Status                |
| --------------------------- | ------------------------------------------------------------------------------------------------------ | --------------------- |
| `fleet`                     | Fleet view — processes by pid, bridge, tasks, cancelled traces                                         | **Shipped**           |
| `execute_python`            | Run Python in TD; `result = …`; optional `logs`; structured `exception` on failure                     | **Shipped**           |
| `inspect`                   | Structural read for explicit `paths[]` batch (nodes + wires / params / errors / warnings / content); no auto-recursion | **Shipped**           |
| `capture`                   | Perception — `top` / `preview` / `auto` / `chop_data` / `chop_image`† / `pop`† († aliases of preview)  | **Shipped**           |
| `describe_tools`            | Manifest of available tools                                                                            | **Shipped**           |
| `mutate_nodes`              | Ordered create / set / delete / connect / disconnect steps; sequential apply, stop on first hard error | **Shipped**           |
| `api_help`                  | Live TD Python API cards (class / classes index / thin module) — not wiki/help dumps                   | **Shipped**           |
| `editor_context`            | Live editor panes + per-pane selection (`ownerPath`, `focused`, `selection`)                           | **Shipped**           |
| `dialogs`                   | List / dismiss OS dialogs                                                                              | **Planned** (P1, Win) |
| Lifecycle create/start/stop | Return new `pid`                                                                                       | **Planned** (P2)      |


**Not planned (v1):** sticky / `select_target` / `targetId` / ToeDigest / inject / `call_node` (use `execute_python` for other node method calls; connector wiring is `mutate_nodes` `connect` / `disconnect`).

### Three layers


| Layer      | Tool             | Answers                                                    |
| ---------- | ---------------- | ---------------------------------------------------------- |
| Fleet      | `fleet`          | Which pid? Connected? Busy? Resurrection traces?           |
| Editor     | `editor_context` | Which panes / owner COMPs / selection is the user looking at? |
| Structure  | `inspect`        | Nodes, wires, params, errors, warnings — no perception by default |
| Perception | `capture`        | Pixels / basic signal — trigger keyword **perception**     |


Typical loop: `fleet` → pick connected `pid` → `editor_context` (optional hint) → `inspect` → mutate → `inspect` errors/warnings → `capture` (when look is the claim) → perception-critic for look PASS/FAIL.

### MCP tool result shapes (stdio / rmcp)

Structured success is **flat tool fields** with a single outer `ok` (bridge may still return a mini-envelope; mappers pass it through without nesting under the tool name):


| Tool             | Structured success                                                                                                                                                                                                                                                                                                               |
| ---------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `execute_python` | `{ ok: true, result, logs? }`                                                                                                                                                                                                                                                                                                    |
| `mutate_nodes`   | `{ ok: true, applied, failedAt, steps }`                                                                                                                                                                                                                                                                                         |
| `inspect`        | `{ ok: true, nodes: [{ ok, path, …node fields or error }], pathsTruncated?, truncation? }`                                                                                                                                                                                                                                         |
| `api_help`       | `{ ok: true, results: [{ ok, kind, …card or error }], queriesTruncated?, truncation? }` — structured API cards / names index (no `helpText` / wiki body; parameter names via `inspect` include params)                                                                                                                              |
| `editor_context` | `{ ok: true, panes: [{ ok, id, name, type, focused, ownerPath, selection? }], panesTruncated?, truncation? }` — `selection` omitted when empty; per-pane soft errors inline                                                                                                                                                       |
| `capture`        | Image modes: `{ ok: true, path, bytes, mimeType, imageBase64?, maxSize?, mode?, family? }` + MCP image content when PNG present (`imageBase64` stripped from structured after promotion). `chop_data`: `{ ok: true, path, mode, family, numChans, numSamples, rate?, channels:[{name, samples}], truncation? }` (structured only; no image) |
| `fleet`          | tool-specific fleet object (no shared shell)                                                                                                                                                                                                                                                                                     |
| `describe_tools` | tool-specific catalog object (no shared shell)                                                                                                                                                                                                                                                                                   |


Failures (all bridge-backed tools): `{ ok: false, summary, items, … }` via diagnostics flatten. Mutate soft-fail splices `applied` / `failedAt` / `steps` **flat** (not under `data`). Soft perception fails (black/uniform frame) use `isError` + diagnostics + image content — that path is separate from success nesting.

**Argument-shape failures** (missing / unknown field, bad enum value, wrong type) are **not** protocol errors: every tool returns the same `{ ok: false, summary, items }` shape with catalog-backed `tdmcp.args.*` codes (`missing_field`, `unknown_field`, `unknown_variant`, `wrong_type`; `tdmcp.args.similar_field` lint carries a did-you-mean suggestion). Spans point at the exact JSON reference (`steps[0].op`). `-32602 invalid_params` is reserved for unknown tool names and malformed requests; expected fields/values are derived from each tool's advertised schema. See [`TOOL_ERROR_PLAN.md`](TOOL_ERROR_PLAN.md).

**Transport note:** Streamable HTTP JSON fallback (`/mcp/tools/call`) still wraps success as `{ ok: true, data: <above> }`; stdio/rmcp does not. Failures are not double-wrapped.

### `fleet` / `include`


| `include`      | Behavior                                                                                                                            |
| -------------- | ----------------------------------------------------------------------------------------------------------------------------------- |
| default / omit | identity (`pid`, `title?`, `toePath?`) + `bridge`; also `cancelledTasks` / `resurrected` / `lastDisconnectAt` only when non-default |
| `tasks`        | queue snapshot; **omitted when empty**                                                                                              |
| `cancelled`    | accepted for forward-compat; cancelled stack still emits whenever non-empty (not gated today)                                       |
| `popups`       | deferred / empty until P1                                                                                                           |


Unknown `include` values are **rejected** at arg validation — structured failure with `tdmcp.args.unknown_variant` listing allowed values (not a protocol error).

### `inspect` / `paths`

`inspect` requires a non-empty `paths: OpPath[]` array. There is **no single-`path` param** and **no auto-recursion** — the caller chooses exactly which nodes to fetch. Soft-capped at **256** paths per call (`tdmcp.op.paths_truncated` when exceeded; first 256 processed). Empty / missing `paths` → structured `tdmcp.op.paths_required` failure (daemon pre-check; bridge emits the same code).

**Partial success:** a bad path does not fail the whole batch. That entry is `{ ok: false, path, code: "tdmcp.op.not_found", message }` (or `tdmcp.op.inspect_failed`); siblings still return data. Top-level success stays `{ ok: true, nodes: [...] }`.

### `inspect` / `include`


| `include`      | Sections                                                              |
| -------------- | --------------------------------------------------------------------- |
| `[]` / omitted | `nodes` + `errors` + `warnings`                                       |
| non-empty      | allowlist only (`nodes` / `params` / `errors` / `warnings` / `content`) |


`params` and `content` are opt-in. `errors` / `warnings` are string arrays from `OP.errors()` / `OP.warnings()` (no recurse). Not gated by `detailLevel`. When those sections are included (including via default empty `include`), empty arrays are still emitted as “section loaded” — they never flip MCP tool failure. Tool-level failure is only bridge soft-fail (`ok: false`), queue busy, or transport. Unknown `include` values are **rejected** at arg validation with `tdmcp.args.unknown_variant`.

Child roster fields (`childCount`, `childrenReturned`, `children`, and when truncated `childrenTruncated` / `truncation`) and positional wire peers (`inputs`, `outputs`) are emitted **only when `nodes` is in the effective include set** (default empty `include` includes `nodes`). When a non-empty `include` omits `nodes`, those keys are absent entirely — same "section not loaded" signal as omitted `params` / `errors` / `warnings`.

When `nodes` is loaded, each ok node always includes:

| Field | Content |
| ----- | ------- |
| `inputs` | Positional peers from `OP.inputs` — each entry is `{path, name, opType}` or `null` (empty connector slot); fully empty TD list → `[]` |
| `outputs` | Same shape from `OP.outputs` |

Index order matches connector order (OpSketch `<- a, b`). Wire reads are best-effort: an access/iteration failure yields `[]` for that side and never flips node or top-level `ok`. `detailLevel` does not change wire peer shape.

Many TD cook problems (invalid select path, missing movie file, …) surface as **warnings**, not `errors` — live DoD for structural messages should not assume a non-empty `errors` array.

**Enable-expr enrichment:** when the `warnings` section is loaded and any warning matches TD’s enable-parm wording (case-insensitive substring: `enable parm expressions` / `enable expression`), the bridge may attach structured siblings on that node:

| Field | Content |
| ----- | ------- |
| `parmExprIssues` | `[{ kind: "enableExpr", par, label, expr, errorType, message }, …]` — failures from `OP.evalExpression(enableExpr)` over unique non-empty custom `enableExpr`s (`customParGroups`, else `customPars`), capped at **64** evals |
| `diagnostics` | Soft catalog-shaped entries (`code: tdmcp.par.enable_expr_failed`, `severity: "warning"`, mitigation, `context: { par, expr }`) — **not** a tool-failure envelope |

Both keys are **omitted when empty**. Independent of `include: params`. Never flips node or top-level `ok`. Coarse `warnings[]` strings are always kept.

When `params` is included, each entry is `{ name, mode, val, expr? }`:


| Field  | Content                                                                     |
| ------ | --------------------------------------------------------------------------- |
| `name` | Parameter name                                                              |
| `mode` | `ParMode` name string (`CONSTANT`, `EXPRESSION`, …)                         |
| `val`  | Evaluated value, JSON-safe (live `OP` → path string; eval failure → `null`) |
| `expr` | Present only when `mode == "EXPRESSION"` — the expression string            |



When `content` is included, eligible nodes gain a `content` object (omitted on non-DAT / non-GLSL ops). Independent of `detailLevel` (roster shape only). **No size cap** — full `.text` / followed shader bodies. Content read/follow failures never flip node or top-level `ok`.

**DAT** (`family == "DAT"` / `isDAT`):

| Field | Content |
| ----- | ------- |
| `kind` | `"dat"` |
| `isText` | `OP.isText` |
| `isTable` | `OP.isTable` (tables included; body is still `.text` TSV) |
| `bytes` | UTF-8 byte length of `text` |
| `text` | Full `OP.text` |
| `consumers?` | Shader-consumer diagnostics for this DAT — same item shape as mutate `shaderDiagnostics[]`; caps 2048 ops scanned / 64 consumers (`consumersTruncated` + standard `truncation` on overflow). Reading consumer `compileResult` forces a synchronous recompile of that consumer |

**GLSL** (`opType` in `glslTOP` / `glslmultiTOP` / `glslMAT` / `glslPOP`):

| Field | Content |
| ----- | ------- |
| `kind` | `"shader"` |
| `compileResult` | `OP.compileResult` string (may be empty) |
| `compileState?` | `"compiled"` \| `"error"` — classified from the same `compileResult` read (no extra reads); omitted on `glslPOP` (no verified compile surface) |
| `stages` | Followed DAT refs — see role map below |

Each stage is `{ role, path, opType, bytes, text }` when the DAT resolves; broken/invalid follow yields `{ role, path, opType?, error }` with no `text`. Unset/null DAT pars omit that stage.

| Op | Pars → `role` |
| -- | ------------- |
| `glslTOP` / `glslmultiTOP` | `pixeldat`→`pixel`, `vertexdat`→`vertex`, `computedat`→`compute`, `predat`→`pre` |
| `glslMAT` | `pdat`→`pixel`, `vdat`→`vertex`, `gdat`→`geometry`, `predat`→`pre` |
| `glslPOP` | `computedat`→`compute` |

Info DAT compile dumps are ordinary DAT `content` (not merged into the GLSL node). Prefer `inspect` + `include: content` over `execute_python` for DAT/GLSL body reads.

**Cooking:** `inspect` does **not** force-cook. TD cooks on demand when operators are read; agents that need a forced cook use `execute_python` (`op('…').cook(force=True)`). Errors/warnings remain non-recursive (target only).

### `inspect` / `detailLevel`

Applies **only when `nodes` is included** (see `inspect` / `include` — default empty `include` includes `nodes`). When `nodes` is excluded, no child-roster fields are present.


| Level               | Direct `children` entries | Counts / truncation                                        |
| ------------------- | ------------------------- | ---------------------------------------------------------- |
| `summary` (default) | `{name, opType}`          | `childCount` + `childrenReturned`; roster capped at **256** |
| `detailed`          | `{path, family, opType}`  | Same cap — **does not** raise the limit                    |


When `childrenReturned < childCount`, the node includes `childrenTruncated: true` and a `truncation` object (`field`, `limit`, `code: tdmcp.op.children_truncated`, `message`, `mitigation`). Soft limit — MCP success stays `{ ok: true, nodes }` (see result shapes). Mitigation: add the child COMP path to a follow-up `paths` batch, or `execute_python` for a full name list — not `detailLevel: detailed`.

When the roster is loaded, `children` is always an array (never a bare count).

### `editor_context` — Shipped

Live multi-pane snapshot of TouchDesigner’s UI via `td.ui.panes` (bridge method — not `execute_python`). Requires `pid` only.

| Field | Rule |
| --- | --- |
| `panes[]` | All panes (soft-cap **64**; `panesTruncated` + top-level `truncation` when exceeded) |
| `panes[].type` | `PaneType` name (`NETWORKEDITOR`, `PANEL`, …) |
| `panes[].focused` | `true` when `pane.id == ui.panes.current.id`; all `false` when there is no current pane |
| `panes[].ownerPath` | `pane.owner.path`, or `null` when unresolved |
| `panes[].selection` | `[{ path, current }]` for COMP owners — **omitted when empty**; soft-capped at **256** with per-pane `selectionTruncated` / `truncation` |
| `panes[].current` | Exactly one selection entry may have `current: true` (the green current child) |

**Partial success:** a bad pane is `{ ok: false, id?, name?, code: "tdmcp.editor.pane_failed", message }`; siblings still succeed. Top-level handler failure → `tdmcp.editor.context_failed`.

**Semantics:** editor context is a **hint** for where the user is looking — not authorization. Still resolve / verify a mutation zone with `inspect` before mutating.

### `api_help` — Shipped

Live TD Python API **cards** via bridge introspection (`getattr(td,…)`, `dir`, short `__doc__`, class `opType`/`family`/`mro`). **Not** a documentation fetcher: no raw `help()` dumps, no wiki HTML body, no bundled OP parameter corpus.

Requires `pid` and a non-empty `queries[]` (soft-cap **64**; `queriesTruncated` + `truncation` when exceeded). Partial success: a bad query is `{ ok: false, code: "tdmcp.api_help.not_found", … }`; siblings still succeed; top-level stays `{ ok: true, results: [...] }`.

| `queries[].kind` | Shape |
| --- | --- |
| `class` | Requires `name` (exact, case-sensitive). Card: `doc`, `opType?`, `family?`, capped `members` + `memberCount`, `mro`; `detailed` adds fuller members + `wikiUrl` (best-effort string only) |
| `classes` | Optional `family` (TOP/CHOP/SOP/DAT/MAT/COMP/POP) + `prefix` (casefold). Returns op-like type **names** index |
| `module` | `name: "td"` only in v1 — thin `{ doc, publicCount, typeCount, sample }` |

**Parameter names** are **not** listed by `api_help` (class `.par` is not enumerable). Use `inspect` with `include: ["params"]` on an existing node. Conceptual “when to use X” lives in the operate pack (`tdmcp://docs/operator-families`, primers) and Derivative wiki.

Diagnostic references may include `{ kind: "api_help", query: "<opType>" }` on `tdmcp.op.unknown_type` / `tdmcp.par.unknown` (params mitigation still points at `inspect`).

### `capture` modes

Capture does **not** force-cook. TD cooks on read / `saveByteArray`; shared-viewer modes retarget `./capture_viewer` then encode. Optional `maxSize` uses a temp `resolutionTOP` (always destroyed). If a TOP should have content but capture is black, ensure it is cooking (see `black_frame` mitigation).


| Mode         | Status      | Behavior                                                                                                                                                                                                                                                                                            |
| ------------ | ----------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `top`        | **Shipped** | TOP → PNG (native `saveByteArray(".png")`; retains alpha; flat success fields + MCP image content); optional `maxSize` (default 512, hard-capped at 1536 — `tdmcp.perception.max_size_too_large` above the cap, including `null`/native when native resolution itself exceeds it); black or uniform solid frame = perception fail (`tdmcp.perception.black_frame` / `tdmcp.perception.uniform_frame`; image still returned); wrong family → `tdmcp.perception.wrong_family` |
| `preview`    | **Shipped** | Any family → retarget bridge `./capture_viewer` (OP Viewer TOP) at the source → PNG; missing/unbound viewer → `tdmcp.perception.no_path`. Safe under per-pid FIFO (one capture at a time).                                                                                                          |
| `auto`       | **Shipped** | TOP → `top`; CHOP → `chop_data`; everything else (COMP/POP/SOP/MAT/DAT/…) → `preview`                                                                                                                                                                                                                |
| `chop_data`  | **Shipped** | CHOP → capped JSON (64 channels, 1024 samples/channel, 32768 scalars); soft `truncation` + `tdmcp.perception.chop_truncated` when capped; empty → `tdmcp.perception.empty_chop`; wrong family → `tdmcp.perception.wrong_family`; all-zero non-empty = success; ignores `maxSize`                      |
| `chop_image` | **Shipped** | Alias of `preview` (shared OP Viewer); kept for existing callers                                                                                                                                                                                                                                    |
| `pop`        | **Shipped** | Alias of `preview` (shared OP Viewer); kept for existing callers                                                                                                                                                                                                                                    |


### `mutate_nodes` — Shipped (P1)

One tool. Ordered `steps[]`. **Sequential apply, stop on first hard error, never roll back.** No separate preflight pass — the live network can change between passes and a single-caller local daemon does not need two-phase commit. "Aggregate bad paths" is met by *returning* every path/param error seen up to the stop point, not by a resolve-all-then-apply phase.


| Field          | Content                                                                     |
| -------------- | --------------------------------------------------------------------------- |
| `pid`          | Target process (process-scoped)                                             |
| `steps[]`      | Ordered; each is `{op, ...}` below                                          |
| `contextPath?` | Anchor for relative `path` (default `/project1`)                            |
| `exclusive?`   | Ignored (bridged tools always exclusive-enqueue)                            |
| `detailLevel`  | `summary` (default) = per-step `{ok, path?}`; `detailed` adds echoed params |


Step shapes:


| `op`         | Fields                                                          | Notes                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                    |
| ------------ | --------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `create`     | `path`, `opType`, `text?`, `values?`, `flags?` | Parent derived from path resolution; `values` is a convenience (agent may follow with `set`). If `values`/`flags` fail after the node is materialized, the just-created node is destroyed (best-effort) — no orphan is left. When TD auto-renames (requested leaf already occupied), the step still **succeeds** with the actual canonical `path` and a nested lint `tdmcp.op.renamed` (`suggestion.opPath` / `replace` = actual). Within the same `mutate_nodes` batch, later steps whose `path`/`src`/`dst` absolutize to the **requested** create path are remapped to the actual created op (create-intent wins). To target the pre-existing occupant instead, inspect and use its path, or split batches. Nested success lints live under `steps[]` — `ok:true` responses have no top-level `items` |
| `set`        | `path`, `text?`, `values?`, `expressions?`, `pulse?`, `flags?` | Explicit modes — no silent guessing. `values: {name: val}`, `expressions: {name: expr}`, `pulse: [name]`                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                 |
| `delete`     | `path`                                                          |                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                          |
| `connect`    | `src`, `dst`, `srcOutput?` (default 0), `dstInput?` (default 0) | `src.outputConnectors[i].connect(dst.inputConnectors[j])`. Echoes canonical `path` = dst                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                 |
| `disconnect` | `path`, `input?` (default 0)                                    | `path.inputConnectors[input].disconnect()`                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                               |


`values` = `.par.*` only. `flags` = direct OP attributes (`node.<name> = val`); allowlist = operate-relevant TD Common Flags subset: `activeViewer`, `allowCooking`, `bypass`, `cloneImmune`, `display`, `lock`, `render`, `viewer`. Unknown flag names → `tdmcp.flag.unknown`. When a name is in the wrong bag (flag under `values` / param under `flags`), the hard code stays (`tdmcp.par.unknown` / `tdmcp.flag.unknown`) and a best-effort nested lint (`tdmcp.par.wrong_collection` / `tdmcp.flag.wrong_collection`) may be attached — hints never auto-redirect and never change the hard outcome. Same-collection near-misses (typo / case) may attach `tdmcp.par.similar_name` or `tdmcp.op.similar_type` with `suggestion.replace` — also best-effort, never changing the hard code. Wire errors: `tdmcp.wire.bad_index` (connector index OOB), `tdmcp.wire.connect_failed` (TD connector exception); missing ops reuse `tdmcp.op.not_found`.

**Text writes & shader lint.** `create`/`set` accept optional `text` (string) — applied **before** `values`. The target must be a DAT: otherwise hard step error `tdmcp.mutate.not_dat` (on `create`, the usual rollback destroys the node). After every successful `text` write, consuming GLSL ops are linted: stage-reference pars (`pixeldat`/`vertexdat`/`computedat`/`predat` on glslTOP/glslmultiTOP; `pdat`/`vdat`/`gdat`/`predat` on glslMAT; `computedat` on glslPOP) are scanned within the `contextPath` subtree (default `/project1`; ≤2048 ops scanned), each consumer's `OP.compileResult` is classified (reading forces a synchronous recompile of that consumer), and results attach as `steps[i].shaderDiagnostics[]`: `{severity: "note"|"error", code: "tdmcp.shader.compiled" | "tdmcp.shader.compile_failed" | "tdmcp.shader.unsupported_consumer", consumer, consumerOpType, role, message, lines[]}` — `lines[]` carries verbatim `ERROR:` lines, errors only; `glslPOP` consumers report `unsupported_consumer` (no verified compile surface). Batch summary adds `shaderNotes` / `shaderErrors` counts when nonzero. Consumers cap at 64 per DAT (`tdmcp.shader.consumers_truncated` on overflow in inspect content). Lint is best-effort enrichment: it never flips step/tool `ok`. `detailLevel: detailed` echoes `steps[i].textLength`, never the body. Full verified patterns: [SHADER_LINT.md](SHADER_LINT.md).

Result (summary):

```json
{ "ok": true|false, "applied": N, "failedAt": <index|null>,
  "steps": [{"ok": true, "path": "/project1/...", "shaderDiagnostics"?: [...]} | {"ok": false, "code": "tdmcp.*", "path": "..."}],
  "shaderNotes"?: N, "shaderErrors"?: N,
  "summary"?: "...", "items"?: [/* diagnostics */] }
```

- Success: `{ok: true, applied, failedAt: null, steps}`.
- Soft failure (any transport): flat `{ok: false, summary, items, applied, failedAt, steps}` — mutate fields are **not** nested under `data`.
- `applied` = count of steps that succeeded before any stop.
- `failedAt` = index of the first hard failure, or `null` if all applied.
- Steps after `failedAt` are marked `skipped` with `tdmcp.batch.skipped_dependent` — they are **not** replayed; the agent fixes from `failedAt` only. Skipped-step `path` is absolutized against `contextPath`.
- Canonical absolute `path` is echoed per step so the agent can re-`inspect` without re-resolving.
- `diagnosticLevel` (default `summary` on most bridge-backed tools) gates `rawTraceback` inclusion (`detailed` only). `**execute_python` defaults to `detailed**` (tool-local only — the global `DiagnosticLevel` default remains `summary`).

**Mutation zones are not enforced by the daemon in v1.** Zone discipline lives in the agent layer (`tdmcp://docs/mutation-zones`): the agent only passes paths under a self-created named COMP or an authorized subtree. `tdmcp.op.outside_zone` stays **reserved** in the catalog, not emitted by the daemon. A future P2 may add per-pid zone registration if operate experience demands it.

**Bridge package version checks are not enforced in v1.** `tdmcp.bridge.version` stays **reserved** in the catalog (not emitted). Handshake `minDaemon` is unused until a future compat gate lands.

**Testability seam:** the bridge exposes a pure `apply_step(node, step) -> StepResult` function (no `td` import at the seam) so `bridge/tests/test_mutate.py` mirrors `test_inspect_summary.py` — no live TD required for shaping/parity. The `handle_mutate` wrapper does the `td.op()` resolution + calls `apply_step` per step.

---

## Global harnesses

### OpPath — Shipped (resolution on bridge)

All network-scoped tools share one reference system resolved by TouchDesigner via `td.op()`:


| Field          | Role                                                                 |
| -------------- | -------------------------------------------------------------------- |
| `OpPath`       | Absolute or relative path string                                     |
| `contextPath?` | Anchor for relative paths; default base = project root (`/project1`) |


Canonical output echoes TD’s absolute `node.path`. `execute_python` is OpPath-exempt by default; `contextPath` is exposed as `__tdmcp_context_path__` + optional `tdmcp_resolve()` helper. Scripts also receive convenience globals when running inside TouchDesigner:

- Always: `td` (TD module), `op` (`td.op`), `result` (pre-seeded `None`; assign to return)
- Closed-set aliases bound only if present on `td`: `root`, `ui`, `project`, `absTime`, `tdu`, `run`, `ops`, `opex`, `passive`, `mod`
- **Not** injected: `me`, `parent` (no script-owner OP context), bare opTypes (`noiseTOP`, … — use `td.noiseTOP` or the string form), `debug`

### `execute_python` logs / Debug DAT — Shipped


| Piece              | Behavior                                                                                                                                                                      |
| ------------------ | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Global OP Shortcut | Bridge COMP claims `**Debug**` (`ensure_ui`; skipped if already taken)                                                                                                        |
| Text DAT           | `./debug` under the bridge → `op.Debug.op('debug')` when the shortcut is ours                                                                                                 |
| Face               | Operator Viewer ASCII panel includes a **LOGS** section (tail of `./debug`)                                                                                                   |
| `includeLogs`      | Default **true**. When true, stdout/stderr during `exec` are teed (Textport still receives them), ring-appended to `./debug` (256 KiB), and returned as `logs` (capped 128 KiB) |
| Success            | `{ ok: true, result, logs? }` — `logs` omitted when `includeLogs: false`                                                                                                      |
| Failure            | `diagnostics.context.logs` carries the same capture; see structured `exception` below                                                                                         |
| Scope              | Only stdio (`print` / writes to stdout/stderr). TD `debug()` may bypass stdio                                                                                                 |
| Size caps          | Script UTF-8 ≤ **4 MiB** (`tdmcp.script.too_large`, rejected pre-execution); JSON-encoded `result` ≤ **4 MiB** (over that, `result` is truncated and `resultTruncated`/`truncation` metadata attached — the script already ran, so its effect is never discarded, only the returned value; code `tdmcp.script.result_too_large`). Caps keep framed IPC under the 32 MiB hard frame limit. Prefer `mutate_nodes` for create/wire/set batches. |


### `execute_python` structured exception — Shipped

On soft-fail the bridge returns additive `{ error, traceback, exception }` (strings kept for compatibility).

`exception` shape:

```json
{
  "type": "AttributeError",
  "message": "'NoneType' object has no attribute 'name'",
  "frames": [
    { "filename": "<string>", "lineno": 2, "name": "<module>", "line": "…" }
  ],
  "syntax": null,
  "raw": "Traceback (most recent call last):\n…"
}
```


| Piece             | Behavior                                                                                                                                                                            |
| ----------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `diagnosticLevel` | **Default `detailed`** on this tool only. `summary` omits `rawTraceback` but **keeps** structured `exception`.                                                                      |
| `formatMode`      | `normal` (default) | `debug`. `debug` asks the bridge for capped locals on `<string>` frames; daemon strips `locals` unless `debug`.                                                |
| MCP item          | Hard code is bridge `code` when set (`tdmcp.script.too_large` / `tdmcp.script.result_too_large`), else `tdmcp.script.execution_failed`. `items[0].exception` carries the reduced report. `span.line` / `column` / `snippet` filled from `syntax` or last `<string>` frame. |
| Nested lint       | `tdmcp.script.none_op` when `type == AttributeError` and message contains `NoneType` (hints never change the hard code).                                                            |
| Locals policy     | Max 8 names / frame, repr ≤ 120 chars, skip callables/modules; never fails the report.                                                                                              |


### Diagnostics — Shipped

Every tool failure carries a structured `diagnostics` block:

- Severities: `error` | `lint` | `note` | `help`
- `layer` (coarse): `fleet` | `structure` | `perception` | `mutate` | `script` | `editor`
- `span` (exact tool + step)
- Stable `code` strings from [`diagnostics/catalog.yaml`](../diagnostics/catalog.yaml)
- Mitigation steps (short imperatives; often include `` `resources/read` `tdmcp://docs/<id>` ``)
- `references[]` kinds:
  - `doc` — operate skill card; `id` matches `skills/MANIFEST.yaml`; `uri` is `tdmcp://docs/<id>` (call MCP `resources/read`)
  - `api_help` — call tool `api_help` with `queries: [{kind: class, name: <query>}]` (`query` may be spliced at runtime)
  - `tool` — named MCP tool hint (`fleet`, `inspect`, `mutate_nodes`, …); see mitigation for args
- Optional `exception` (execute_python structured report) and `rawTraceback` (`detailed` only)
- `exception.frames` / trimmed `exception.raw` include **only** user `<string>` frames — bridge `execute.py` / `exec` wrappers are stripped

Code families: `tdmcp.bridge.*`, `tdmcp.mcp.*`, `tdmcp.daemon.*`, `tdmcp.script.*`, `tdmcp.perception.*`, `tdmcp.op.*`, `tdmcp.par.*`, `tdmcp.flag.*`, `tdmcp.wire.*`, `tdmcp.mutate.*`, `tdmcp.batch.*`, `tdmcp.editor.*`, `tdmcp.api_help.*`, `tdmcp.shader.*`. Reserved (catalogued, not emitted in v1): `tdmcp.bridge.version`, `tdmcp.op.outside_zone`, `tdmcp.td.glsl_compile`, `tdmcp.bridge.unknown_pid` (NotConnected maps to `lost` today).

`tdmcp.bridge.timeout` = daemon wait ended (TD may still run — **timeout ≠ cancel**).
`tdmcp.bridge.lost` = IPC died (cancel + resurrection stack).
`tdmcp.bridge.cancelled` = queued/in-flight bridge work cancelled (preserved from bridge IPC, not remapped to `lost`).
`tdmcp.bridge.main_thread_timeout` = bridge worker gave up waiting for
`process_pending` (paused timeline / hung script) and unwedged the IPC loop;
late main-thread results are dropped. Same-pid reconnect **aborts** the prior
daemon session actor (cancel token) so dual actors cannot serve one pid when
Python `disconnect()` join fails.
`tdmcp.mcp.session_busy` / `tdmcp.bridge.queue_busy` → sequential bridged tools; see `tdmcp://docs/tooling-concurrency`.

---

## Storage and delivery


| OS      | Data dir                                                 |
| ------- | -------------------------------------------------------- |
| Windows | `%LOCALAPPDATA%/tdmcp-rs/`                               |
| macOS   | `~/Library/Application Support/tdmcp-rs/`                |
| Linux   | `$XDG_DATA_HOME/tdmcp-rs/` or `~/.local/share/tdmcp-rs/` |


Config precedence: **CLI args > env (`TDMCP_`*) > RC file > defaults**.

Artifacts: `tdmcp-daemon` (embeds tray UI when built with default `gui` feature), `bridge/`, `diagnostics/catalog.yaml`, bootstrap `.tox`. Bridge, catalog, and bootstrap ship embedded in the daemon binary; `install` / `ensure` / `start` / `mcp` extract into the data dir. Same semver stamp skips re-extract — use `tdmcp-daemon install --force` or `ensure --force` to refresh embedded assets without bumping the package version. `mcp` upsert stays non-force (does not re-extract on every Cursor reconnect). Packaging via `cargo xtask dist` is **Planned** (P2); until then build with `cargo build --release -p tdmcp-daemon`. Headless: `cargo build --release -p tdmcp-daemon --no-default-features`, or runtime `--no-gui` / `TDMCP_NO_GUI=1`.

Daemon CLI: `start` (foreground; tray + toast by default, dashboard hidden until opened), `stop`, `status`, `install` (`--force` re-extract), `ensure` (`--force` re-extract then upsert), `mcp` (Cursor entrypoint — `ensure` then stdio proxy). Manual `start` for debugging; Cursor uses `mcp`.

---

## Phased delivery


| Phase    | Ship                                                                                                                                                            | Exit green                                                                                                                                                                | Status                                                                                      |
| -------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------- |
| **P0**   | Daemon + IPC + bootstrap + Streamable HTTP: `fleet` + script/errors + `capture` (`top`/`preview`) + diagnostics + per-pid queue + exclusive fail + resurrection | Two connected pids; exclusive fails while busy; perception non-black; structured script failure                                                                           | **Shipped** (see `[E2E_CHECKLIST.md](E2E_CHECKLIST.md)`)                                    |
| **P1**   | `mutate_nodes` (incl. connect/disconnect), `capture` `chop_data`, dialogs (Win), op lint engine                                                                 | `mutate_nodes` sequential apply stops at first bad path with `failedAt`; later steps emit `tdmcp.batch.skipped_dependent`; pure `apply_step` seam unit-covered without TD | Partial (`mutate_nodes` + `capture` `chop_data` **Shipped**; dialogs / op lint **Planned**) |
| **P1.x** | Universal `capture` via shared OP Viewer; `inspect` explicit `paths[]`; `api_help` live API cards                                                              | Any-family preview; chop_image/pop aliases; inspect batch + partial success; api_help class/classes/module                                                                 | **Shipped** (unit + FakeTdPeer; E2E rows 17–19 for inspect/capture)                         |
| **P2**   | Lifecycle create/start/stop (tray already shipped)                                                                                                              | Operator create/start/stop; new project by pid                                                                                                                            | Partial (tray **Shipped**; lifecycle **Planned**)                                           |
| **P3**   | Streamable HTTP remote + single-level federation (`bind_address`, PSK auth, register/fleet-push, `daemonId` tool proxy)                                           | Automated: `admin_auth` + `federation_registration` + `federation_proxy` (inspect/capture/ambiguous/unreachable); see [`CONFIG.md`](CONFIG.md) § Federation auth & admin surface | **Shipped**                                                                                 |


---

## Decided contract (summary)

- TD↔daemon: local IPC (named pipe / UDS); handshake returns FS path to bridge package.
- Cursor↔daemon: `tdmcp-daemon mcp` (stdio proxy → Streamable HTTP at `/mcp/rpc`; v1 tools only, no notification forward). Direct HTTP clients may use `http://127.0.0.1:9860/mcp` (JSON fallback on `/mcp/tools/*`).
- Identity: `pid` required on bridged tools; optional `daemonId` when federated (ambiguous pid → `tdmcp.federation.ambiguous_pid`). Bridged tools always exclusive-enqueue (fail iff queue non-empty); session chill on `(mcp_session, pid)` locally and `(mcp_session, daemonId, pid)` when proxied; resurrection stacks until first success.
- Perception: `capture` only; builders never self-grade look.
- Paths: `OpPath` + optional `contextPath`; TD resolves; default base `/project1`.
- Diagnostics: catalog-backed codes; free-string-only failures forbidden. Argument-shape failures use `tdmcp.args.*` codes as structured `isError` results; `-32602` reserved for unknown tool / malformed request (see [`TOOL_ERROR_PLAN.md`](TOOL_ERROR_PLAN.md)).
- MCP success: flat tool fields (`node` / `path` / `result` / `steps` at top level); bridge mini-envelopes are passed through by mappers, not nested under the tool name. HTTP JSON fallback still wraps success in `{ ok, data }`.

