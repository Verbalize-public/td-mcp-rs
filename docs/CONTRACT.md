# td-mcp-rs contract

Durable v1 technical contract. Status markers:

| Marker | Meaning |
| --- | --- |
| **Shipped** | Implemented and covered by tests / live E2E |
| **Planned** | Specified; not yet in the tool surface or incomplete |

Crate layout: [`ARCHITECTURE.md`](../ARCHITECTURE.md). Engineering law: [`CONSTITUTION.md`](../CONSTITUTION.md).

---

## Goals (v1) — Shipped intent

1. **Multi-instance first** — every mutating call names the target by OS `pid`. No session “current target,” no generated peer ids.
2. **Live operate only** — inspect, mutate, script, verify on a connected bridge. No `.toe` / `.tox` binary editing.
3. **Agent-shaped surface** — small tool set; **`fleet` → `inspect` → `capture`** three-layer read model; perception is explicit (`capture`); uniform diagnostics; summary-by-default; timeouts fail the *wait* (not claim TD cancelled).
4. **Connected ⇒ usable** — `bridge: "connected"` ⇒ any MCP caller may address that `pid`. Coordination via visible tasks + exclusive requests that fail when the queue is busy.
5. **One local control plane** — long-lived daemon owns pid→bridge map, per-pid queues, MCP surface, bridge sessions.
6. **Self-contained delivery** — one binary + one drop-in `.tox` bootstrap.
7. **Resurrection on reconnect** — on IPC loss the daemon states the disconnect and stacks cancelled tasks until the first successful task afterward (then erase).

### Non-goals (v1)

| Item | Why |
| --- | --- |
| Sticky / `select_target` / session peer | Replaced by per-call `pid` + `fleet` |
| Generated `targetId` / UUID / path-hash | **`pid` is the only id** |
| Offline ToeDigest / `.toe` write / inject | Separate MCP; v1 adopt path = drop tox |
| Remote / WAN TD control | After local contract is boring |
| Multiple bridge protocols | One local IPC |
| Silent auto-reconnect | Explicit resurrection policy |
| Lifecycle create/start/stop | **P2+** |

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

| Direction | Transport | Role | Status |
| --- | --- | --- | --- |
| IDE → daemon | Streamable HTTP MCP (`/mcp/rpc`) + JSON fallback (`/mcp/tools/*`) | Tool calls (direct HTTP clients) | **Shipped** |
| IDE → daemon | stdio (`tdmcp-daemon mcp` → stdio proxy → HTTP `/mcp/rpc`) | Cursor entrypoint | **Shipped** |
| TD → daemon | Local IPC | Bridge | **Shipped** |
| Operator → daemon | In-process tray + admin HTTP + OS toasts (`gui` feature, default on) | Human monitor | **Shipped** |
| IDE → daemon | WebSocket | — | **Planned** |

**Singleton:** one owner per listen port. Exclusivity = `daemon.lock` (pid) + TCP bind on `127.0.0.1:{port}`. Stale locks (dead pid) are reclaimed on `start` / `ensure`. A second `start` while healthy refuses with a clear error. `/admin/restart` clears the lock then spawn-then-exit; the replacement retries bind briefly. No distributed leader election — localhost only.

**Idle auto-exit:** after **30s** with zero connected bridges and zero live Streamable HTTP MCP session leases (stdio proxy counts), the daemon toasts and exits (`process::exit`). Admin/health polls and JSON `/mcp/tools/*` do not keep it alive. Assumes session-mode MCP (not per-request handlers). Override: `TDMCP_IDLE_EXIT_SECS` (`0` disables). `ensure` / `mcp` respawn on next use. Tests may set `TDMCP_IPC_PIPE` so a live TD on the production pipe cannot attach.

**Listen:** `127.0.0.1:9860` (override via CLI / env / RC); loopback only; no auth.

**Stdio proxy (v1):** forwards tools request/response only (`list_tools` /
`call_tool`). Server-initiated notifications are **not** forwarded. The HTTP
daemon is the control plane; the stdio process is a short-lived shim per MCP
client session.

### Bridge transport — Shipped

| OS | Mechanism | Address |
| --- | --- | --- |
| Windows | Named pipe | `\\.\pipe\tdmcp-rs` |
| macOS / Linux | Unix domain socket | `{dataDir}/bridge.sock` |

Handshake returns a local FS path to the bridge package directory. TD reloads from disk on every handshake. Package: `bridge/` + `manifest.json` (`protocolVersion`, `minDaemon`, entry).

### Addressing — Shipped

| Rule | Detail |
| --- | --- |
| Ground truth | OS `pid` only |
| Discovery | `fleet` lists by `pid` + **title** (`project.name`) + **toePath** (`folder`+`name`), bridge, tasks, traces |
| Identity source | Filled at bridge handshake from a main-thread snapshot; stale until reconnect (no mid-session refresh) |
| Fingerprint | Best-effort `image` (process exe path) + `startTime` (opaque OS start); used for pid-reuse |
| Deferred | `windowStatus` and `fleet` `popups` — empty until P1 `dialogs` (Win) |
| Usable | `bridge: "connected"` ⇒ any caller may address that `pid` |
| Addressing | Process-scoped tools require `pid` |
| IPC loss | Mark disconnected; cancel waits; stack cancelled-task trace; remove from fleet after **15s** TTL or when **any** handshake succeeds |
| Resurrection | Same `pid` re-handshakes within the grace window ⇒ connected; stack cleared on first successful task |
| Pid reuse | Best-effort fingerprint; mismatch ⇒ clear that pid’s state only |

### Task queue — Shipped

| Mode | Behavior |
| --- | --- |
| **Shared (default)** | Enqueue / run; visible in `fleet` when requested |
| **Exclusive** | Fails if queue is non-empty (any shared or exclusive) |

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
tasks). Defaults: interval **5s**, pong wait **5s**, idle-dead **15s** (no
inbound framed traffic). Any successful request/response (including ping/pong)
resets the inactivity clock. Missed pong or idle-dead → same teardown as IPC
loss. The bridge answers `ping` on the IPC worker thread (no main-thread
`process_pending`) and exits its serve loop after **15s** inbound silence when
read timeouts are available. Idle detection and fleet eviction are separate
clocks (each up to **15s**). `GET /mcp/health` remains daemon-process liveness
only — not bridge peer health.

---

## MCP tools

### Tool template

| Field | Content |
| --- | --- |
| Name | stable snake_case |
| Params | typed; process-scoped tools require `pid`; paths use `OpPath` |
| Diagnostics | uniform `diagnostics` envelope; stable `tdmcp.*` codes |
| Detail flags | `detailLevel` (structure), `diagnosticLevel` (error payload), `resultRef` (large payloads) |

### Catalogue

| Tool | Job | Status |
| --- | --- | --- |
| `fleet` | Fleet view — processes by pid, bridge, tasks, cancelled traces | **Shipped** |
| `execute_python` | Run Python in TD; `result = …`; optional `logs` (stdio capture) | **Shipped** |
| `inspect` | Structural subtree read (nodes / params / errors / warnings); summary = direct-child roster | **Shipped** |
| `capture` | Perception — `top` / `preview` / `auto` | **Shipped** (P0 modes) |
| `describe_tools` | Manifest of available tools | **Shipped** |
| `mutate_nodes` | Ordered create / set / delete / connect / disconnect steps; sequential apply, stop on first hard error | **Shipped** |
| `dialogs` | List / dismiss OS dialogs | **Planned** (P1, Win) |
| `api_help` | Live TD Python API introspection | **Planned** (P1) |
| `capture` `chop_data` / `pop` / `chop_image` | Extra perception modes | **Planned** (P1 / P1.x) |
| Lifecycle create/start/stop | Return new `pid` | **Planned** (P2) |

**Not planned (v1):** sticky / `select_target` / `targetId` / ToeDigest / inject / `call_node` (use `execute_python` for other node method calls; connector wiring is `mutate_nodes` `connect` / `disconnect`).

### Three layers

| Layer | Tool | Answers |
| --- | --- | --- |
| Fleet | `fleet` | Which pid? Connected? Busy? Resurrection traces? |
| Structure | `inspect` | Nodes, params, errors, warnings — no perception by default |
| Perception | `capture` | Pixels / basic signal — trigger keyword **perception** |

Typical loop: `fleet` → pick connected `pid` → `inspect` → mutate → `inspect` errors/warnings → `capture` (when look is the claim) → perception-critic for look PASS/FAIL.

### MCP tool result shapes (stdio / rmcp)

Structured success is **flat tool fields** with a single outer `ok` (bridge may still return a mini-envelope; mappers pass it through without nesting under the tool name):

| Tool | Structured success |
| --- | --- |
| `execute_python` | `{ ok: true, result, logs? }` |
| `mutate_nodes` | `{ ok: true, applied, failedAt, steps }` |
| `inspect` | `{ ok: true, node }` |
| `capture` | `{ ok: true, path, bytes, mimeType, jpegBase64?, maxSize? }` + MCP image content when JPEG present (`jpegBase64` stripped from structured after promotion) |
| `fleet` | tool-specific fleet object (no shared shell) |
| `describe_tools` | tool-specific catalog object (no shared shell) |

Failures (all bridge-backed tools): `{ ok: false, summary, items, … }` via diagnostics flatten. Mutate soft-fail splices `applied` / `failedAt` / `steps` **flat** (not under `data`). Soft perception fails (black/uniform frame) use `isError` + diagnostics + image content — that path is separate from success nesting.

**Transport note:** Streamable HTTP JSON fallback (`/mcp/tools/call`) still wraps success as `{ ok: true, data: <above> }`; stdio/rmcp does not. Failures are not double-wrapped.

### `fleet` / `include`

| `include` | Behavior |
| --- | --- |
| default / omit | identity (`pid`, `title?`, `toePath?`) + `bridge`; also `cancelledTasks` / `resurrected` / `lastDisconnectAt` only when non-default |
| `tasks` | queue snapshot; **omitted when empty** |
| `cancelled` | accepted for forward-compat; cancelled stack still emits whenever non-empty (not gated today) |
| `popups` | deferred / empty until P1 |

Unknown `include` values are **rejected** at MCP arg deserialize (invalid params) — not silently ignored.

### `inspect` / `include`

| `include` | Sections |
| --- | --- |
| `[]` / omitted | `nodes` + `errors` + `warnings` |
| non-empty | allowlist only (`nodes` / `params` / `errors` / `warnings`) |

`params` is opt-in. `errors` / `warnings` are string arrays from `OP.errors()` / `OP.warnings()` (no recurse). Not gated by `detailLevel`. When those sections are included (including via default empty `include`), empty arrays are still emitted as “section loaded” — they never flip MCP tool failure. Tool-level failure is only bridge soft-fail (`ok: false`), queue busy, or transport. Unknown `include` values are **rejected** at MCP arg deserialize.

Child roster fields (`childCount`, `childrenReturned`, `children`, and when truncated `childrenTruncated` / `truncation`) are emitted **only when `nodes` is in the effective include set** (default empty `include` includes `nodes`). When a non-empty `include` omits `nodes`, those keys are absent entirely — same "section not loaded" signal as omitted `params` / `errors` / `warnings`.

Many TD cook problems (invalid select path, missing movie file, …) surface as **warnings**, not `errors` — live DoD for structural messages should not assume a non-empty `errors` array.

When `params` is included, each entry is `{ name, mode, val, expr? }`:

| Field | Content |
| --- | --- |
| `name` | Parameter name |
| `mode` | `ParMode` name string (`CONSTANT`, `EXPRESSION`, …) |
| `val` | Evaluated value, JSON-safe (live `OP` → path string; eval failure → `null`) |
| `expr` | Present only when `mode == "EXPRESSION"` — the expression string |

**Cook before read (rule):** `inspect` always calls `OP.cook(force=True)` on the resolved target before reading nodes/params/errors/warnings. Inspecting a COMP cooks that network. Errors/warnings remain non-recursive (target only). Cook failures are best-effort and do not fail the inspect.

### `inspect` / `detailLevel`

Applies **only when `nodes` is included** (see `inspect` / `include` — default empty `include` includes `nodes`). When `nodes` is excluded, no child-roster fields are present.

| Level | Direct `children` entries | Counts / truncation |
| --- | --- | --- |
| `summary` (default) | `{name, opType}` | `childCount` + `childrenReturned`; roster capped at **64** |
| `detailed` | `{path, family, opType}` | Same cap — **does not** raise the limit |

When `childrenReturned < childCount`, the node includes `childrenTruncated: true` and a `truncation` object (`field`, `limit`, `code: tdmcp.op.children_truncated`, `message`, `mitigation`). Soft limit — MCP success stays `{ ok: true, node }` (see result shapes). Mitigation: inspect a child COMP, or `execute_python` for a full name list — not `detailLevel: detailed`.

When the roster is loaded, `children` is always an array (never a bare count).

### `capture` modes

| Mode | Status | Behavior |
| --- | --- | --- |
| `top` | **Shipped** | TOP → JPEG (flat success fields + MCP image content; see result shapes); optional `maxSize` (default 256); black or uniform solid frame = perception fail (`tdmcp.perception.black_frame` / `tdmcp.perception.uniform_frame`; image still returned) |
| `preview` | **Shipped** | COMP face: `opviewer` → `./out1` → TOP child → error |
| `auto` | **Shipped** | TOP → `top`; COMP → `preview` |
| `chop_data` | **Planned** | CHOP → capped JSON |
| `pop` / `chop_image` | **Planned** | Temp converter → TOP → `top` |

### `mutate_nodes` — Shipped (P1)

One tool. Ordered `steps[]`. **Sequential apply, stop on first hard error, never roll back.** No separate preflight pass — the live network can change between passes and a single-caller local daemon does not need two-phase commit. "Aggregate bad paths" is met by *returning* every path/param error seen up to the stop point, not by a resolve-all-then-apply phase.

| Field | Content |
| --- | --- |
| `pid` | Target process (process-scoped) |
| `steps[]` | Ordered; each is `{op, ...}` below |
| `contextPath?` | Anchor for relative `path` (default `/project1`) |
| `exclusive?` | Exclusive enqueue (default false) |
| `detailLevel` | `summary` (default) = per-step `{ok, path?}`; `detailed` adds echoed params |

Step shapes:

| `op` | Fields | Notes |
| --- | --- | --- |
| `create` | `path`, `opType`, `values?`, `flags?` | Parent derived from path resolution; `values` is a convenience (agent may follow with `set`). If `values`/`flags` fail after the node is materialized, the just-created node is destroyed (best-effort) — no orphan is left. When TD auto-renames (requested leaf already occupied), the step still **succeeds** with the actual canonical `path` and a nested lint `tdmcp.op.renamed` (`suggestion.opPath` / `replace` = actual). Within the same `mutate_nodes` batch, later steps whose `path`/`src`/`dst` absolutize to the **requested** create path are remapped to the actual created op (create-intent wins). To target the pre-existing occupant instead, inspect and use its path, or split batches. Nested success lints live under `steps[]` — `ok:true` responses have no top-level `items` |
| `set` | `path`, `values?`, `expressions?`, `pulse?`, `flags?` | Explicit modes — no silent guessing. `values: {name: val}`, `expressions: {name: expr}`, `pulse: [name]` |
| `delete` | `path` | |
| `connect` | `src`, `dst`, `srcOutput?` (default 0), `dstInput?` (default 0) | `src.outputConnectors[i].connect(dst.inputConnectors[j])`. Echoes canonical `path` = dst |
| `disconnect` | `path`, `input?` (default 0) | `path.inputConnectors[input].disconnect()` |

`values` = `.par.*` only. `flags` = direct OP attributes (`node.<name> = val`); allowlist = operate-relevant TD Common Flags subset: `activeViewer`, `allowCooking`, `bypass`, `cloneImmune`, `display`, `lock`, `render`, `viewer`. Unknown flag names → `tdmcp.flag.unknown`. When a name is in the wrong bag (flag under `values` / param under `flags`), the hard code stays (`tdmcp.par.unknown` / `tdmcp.flag.unknown`) and a best-effort nested lint (`tdmcp.par.wrong_collection` / `tdmcp.flag.wrong_collection`) may be attached — hints never auto-redirect and never change the hard outcome. Same-collection near-misses (typo / case) may attach `tdmcp.par.similar_name` or `tdmcp.op.similar_type` with `suggestion.replace` — also best-effort, never changing the hard code. Wire errors: `tdmcp.wire.bad_index` (connector index OOB), `tdmcp.wire.connect_failed` (TD connector exception); missing ops reuse `tdmcp.op.not_found`.

Result (summary):

```json
{ "ok": true|false, "applied": N, "failedAt": <index|null>,
  "steps": [{"ok": true, "path": "/project1/..."} | {"ok": false, "code": "tdmcp.*", "path": "..."}],
  "summary"?: "...", "items"?: [/* diagnostics */] }
```

- Success: `{ok: true, applied, failedAt: null, steps}`.
- Soft failure (any transport): flat `{ok: false, summary, items, applied, failedAt, steps}` — mutate fields are **not** nested under `data`.
- `applied` = count of steps that succeeded before any stop.
- `failedAt` = index of the first hard failure, or `null` if all applied.
- Steps after `failedAt` are marked `skipped` with `tdmcp.batch.skipped_dependent` — they are **not** replayed; the agent fixes from `failedAt` only. Skipped-step `path` is absolutized against `contextPath`.
- Canonical absolute `path` is echoed per step so the agent can re-`inspect` without re-resolving.
- `diagnosticLevel` (default `summary`) on bridge-backed tools gates `rawTraceback` inclusion (`detailed` only).

**Mutation zones are not enforced by the daemon in v1.** Zone discipline lives in the agent layer (`creative-operator` → `cop-touchdesigner-mcp` → `reference/mutation-zones.md`): the agent only passes paths under a self-created named COMP or an authorized subtree. `tdmcp.op.outside_zone` stays **reserved** in the catalog, not emitted by the daemon. A future P2 may add per-pid zone registration if operate experience demands it.

**Testability seam:** the bridge exposes a pure `apply_step(node, step) -> StepResult` function (no `td` import at the seam) so `bridge/tests/test_mutate.py` mirrors `test_inspect_summary.py` — no live TD required for shaping/parity. The `handle_mutate` wrapper does the `td.op()` resolution + calls `apply_step` per step.

---

## Global harnesses

### OpPath — Shipped (resolution on bridge)

All network-scoped tools share one reference system resolved by TouchDesigner via `td.op()`:

| Field | Role |
| --- | --- |
| `OpPath` | Absolute or relative path string |
| `contextPath?` | Anchor for relative paths; default base = project root (`/project1`) |

Canonical output echoes TD’s absolute `node.path`. `execute_python` is OpPath-exempt by default; `contextPath` is exposed as `__tdmcp_context_path__` + optional `tdmcp_resolve()` helper. Scripts also receive convenience globals `td` and `op` (TD’s module and `td.op`) when running inside TouchDesigner — `me` / `parent` are not provided (no script-owner OP context).

### `execute_python` logs / Debug DAT — Shipped

| Piece | Behavior |
| --- | --- |
| Global OP Shortcut | Bridge COMP claims **`Debug`** (`ensure_ui`; skipped if already taken) |
| Text DAT | `./debug` under the bridge → `op.Debug.op('debug')` when the shortcut is ours |
| Face | Operator Viewer ASCII panel includes a **LOGS** section (tail of `./debug`) |
| `includeLogs` | Default **true**. When true, stdout/stderr during `exec` are teed (Textport still receives them), ring-appended to `./debug` (64 KiB), and returned as `logs` (capped 32 KiB) |
| Success | `{ ok: true, result, logs? }` — `logs` omitted when `includeLogs: false` |
| Failure | `diagnostics.context.logs` carries the same capture; `rawTraceback` unchanged |
| Scope | Only stdio (`print` / writes to stdout/stderr). TD `debug()` may bypass stdio |

### Diagnostics — Shipped

Every tool failure carries a structured `diagnostics` block:

- Severities: `error` | `lint` | `note` | `help`
- `layer` (coarse): `fleet` | `structure` | `perception` | `mutate` | `script`
- `span` (exact tool + step)
- Stable `code` strings from [`diagnostics/catalog.yaml`](../diagnostics/catalog.yaml)
- Mitigation + optional corpus / doc references

Code families in use today: `tdmcp.bridge.*`, `tdmcp.script.*`, `tdmcp.perception.*`, `tdmcp.op.*` (catalog also reserves mutate/batch codes for P1).

`tdmcp.bridge.timeout` = daemon wait ended (TD may still run). `tdmcp.bridge.lost` = IPC died (cancel + resurrection stack).

---

## Storage and delivery

| OS | Data dir |
| --- | --- |
| Windows | `%LOCALAPPDATA%/tdmcp-rs/` |
| macOS | `~/Library/Application Support/tdmcp-rs/` |
| Linux | `$XDG_DATA_HOME/tdmcp-rs/` or `~/.local/share/tdmcp-rs/` |

Config precedence: **CLI args > env (`TDMCP_*`) > RC file > defaults**.

Artifacts: `tdmcp-daemon` (embeds tray UI when built with default `gui` feature), `bridge/`, `diagnostics/catalog.yaml`, bootstrap `.tox`. Bridge, catalog, and bootstrap ship embedded in the daemon binary; `install` / `ensure` / `start` / `mcp` extract into the data dir. Same semver stamp skips re-extract — use `tdmcp-daemon install --force` or `ensure --force` to refresh embedded assets without bumping the package version. `mcp` upsert stays non-force (does not re-extract on every Cursor reconnect). Packaging via `cargo xtask dist` is **Planned** (P2); until then build with `cargo build --release -p tdmcp-daemon`. Headless: `cargo build --release -p tdmcp-daemon --no-default-features`, or runtime `--no-gui` / `TDMCP_NO_GUI=1`.

Daemon CLI: `start` (foreground; tray + toast by default, dashboard hidden until opened), `stop`, `status`, `install` (`--force` re-extract), `ensure` (`--force` re-extract then upsert), `mcp` (Cursor entrypoint — `ensure` then stdio proxy). Manual `start` for debugging; Cursor uses `mcp`.

---

## Phased delivery

| Phase | Ship | Exit green | Status |
| --- | --- | --- | --- |
| **P0** | Daemon + IPC + bootstrap + Streamable HTTP: `fleet` + script/errors + `capture` (`top`/`preview`) + diagnostics + per-pid queue + exclusive fail + resurrection | Two connected pids; exclusive fails while busy; perception non-black; structured script failure | **Shipped** (see [`E2E_CHECKLIST.md`](E2E_CHECKLIST.md)) |
| **P1** | `mutate_nodes` (incl. connect/disconnect), `capture` `chop_data`, dialogs (Win), op lint engine | `mutate_nodes` sequential apply stops at first bad path with `failedAt`; later steps emit `tdmcp.batch.skipped_dependent`; pure `apply_step` seam unit-covered without TD | Partial (`mutate_nodes` **Shipped** + live E2E M1–M17; remaining P1 items **Planned**) |
| **P1.x** | `capture` `pop`, `chop_image` | Non-TOP heroes via temp converters | **Planned** |
| **P2** | Lifecycle create/start/stop (tray already shipped) | Operator create/start/stop; new project by pid | Partial (tray **Shipped**; lifecycle **Planned**) |
| **P3** | WebSocket / remote RFC | Separate design review | **Planned** |

---

## Decided contract (summary)

- TD↔daemon: local IPC (named pipe / UDS); handshake returns FS path to bridge package.
- Cursor↔daemon: `tdmcp-daemon mcp` (stdio proxy → Streamable HTTP at `/mcp/rpc`; v1 tools only, no notification forward). Direct HTTP clients may use `http://127.0.0.1:9860/mcp` (JSON fallback on `/mcp/tools/*`).
- Identity: `pid` only; exclusive fails iff queue non-empty; resurrection stacks until first success.
- Perception: `capture` only; builders never self-grade look.
- Paths: `OpPath` + optional `contextPath`; TD resolves; default base `/project1`.
- Diagnostics: catalog-backed codes; free-string-only failures forbidden.
- MCP success: flat tool fields (`node` / `path` / `result` / `steps` at top level); bridge mini-envelopes are passed through by mappers, not nested under the tool name. HTTP JSON fallback still wraps success in `{ ok, data }`.
