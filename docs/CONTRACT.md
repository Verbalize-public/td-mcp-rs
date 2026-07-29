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
 IDE (Cursor / …)  ──┐
                     │  MCP Streamable HTTP  http://127.0.0.1:9860/mcp
 Other MCP callers ──┤
                     ▼
 ┌────────────────── Daemon (tdmcp-daemon) ─────────────────────────────┐
 │  axum + rmcp  │  admin /admin/*  │  PidRegistry  │  per-pid queues   │
 └──────────┬────────────────────────────────────────────────────────────┘
            │  local IPC (Win named pipe / Unix UDS)
            ▼
   TD process(es)  ←── bootstrap .tox → handshake → FS load bridge/

 GUI (tdmcp-gui) ──► admin HTTP (loopback) ──► daemon
```

### Surfaces

| Direction | Transport | Role | Status |
| --- | --- | --- | --- |
| IDE → daemon | Streamable HTTP MCP (`/mcp/rpc`) + JSON fallback (`/mcp/tools/*`) | Tool calls | **Shipped** |
| TD → daemon | Local IPC | Bridge | **Shipped** |
| Operator → daemon | Tray + admin HTTP + OS toasts | Human monitor | **Shipped** |
| IDE → daemon | WebSocket / stdio | — | **Planned** / not used |

**Listen:** `127.0.0.1:9860` (override via CLI / env / RC); loopback only; no auth.

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
| Discovery | `fleet` lists by `pid` + title, bridge, tasks, traces |
| Usable | `bridge: "connected"` ⇒ any caller may address that `pid` |
| Addressing | Process-scoped tools require `pid` |
| IPC loss | Mark disconnected; cancel waits; stack cancelled-task trace |
| Resurrection | Same `pid` re-handshakes ⇒ connected; stack cleared on first successful task |
| Pid reuse | Best-effort fingerprint; mismatch ⇒ clear that pid’s state only |

### Task queue — Shipped

| Mode | Behavior |
| --- | --- |
| **Shared (default)** | Enqueue / run; visible in `fleet` when requested |
| **Exclusive** | Fails if queue is non-empty (any shared or exclusive) |

### Disconnect / resurrection — Shipped

1. State the loss (`bridge` disconnected, `lastDisconnectAt`).
2. Cancel waits; stack cancelled tasks (`reason: bridge_lost`).
3. Same `pid` re-handshake ⇒ usable again; stack stays until first success.
4. First successful task clears the stack. First failure after resurrection keeps it.

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
| `execute_python` | Run Python in TD; `result = …` | **Shipped** |
| `inspect` | Structural subtree read (nodes / params / errors) | **Shipped** |
| `capture` | Perception — `top` / `preview` / `auto` | **Shipped** (P0 modes) |
| `describe_tools` | Manifest of available tools | **Shipped** |
| `mutate_nodes` | Batched create / set / delete | **Planned** (P1) |
| `call_node` | Call a method on a node | **Planned** (P1) |
| `dialogs` | List / dismiss OS dialogs | **Planned** (P1, Win) |
| `api_help` | Live TD Python API introspection | **Planned** (P1) |
| `capture` `chop_data` / `pop` / `chop_image` | Extra perception modes | **Planned** (P1 / P1.x) |
| Lifecycle create/start/stop | Return new `pid` | **Planned** (P2) |

**Not planned (v1):** sticky / `select_target` / `targetId` / ToeDigest / inject.

### Three layers

| Layer | Tool | Answers |
| --- | --- | --- |
| Fleet | `fleet` | Which pid? Connected? Busy? Resurrection traces? |
| Structure | `inspect` | Nodes, params, errors — no perception by default |
| Perception | `capture` | Pixels / basic signal — trigger keyword **perception** |

Typical loop: `fleet` → pick connected `pid` → `inspect` → mutate → `inspect` errors → `capture` (when look is the claim) → perception-critic for look PASS/FAIL.

### `capture` modes

| Mode | Status | Behavior |
| --- | --- | --- |
| `top` | **Shipped** | TOP → JPEG; black frame = perception fail |
| `preview` | **Shipped** | COMP face: `opviewer` → `./out1` → TOP child → error |
| `auto` | **Shipped** | TOP → `top`; COMP → `preview` |
| `chop_data` | **Planned** | CHOP → capped JSON |
| `pop` / `chop_image` | **Planned** | Temp converter → TOP → `top` |

---

## Global harnesses

### OpPath — Shipped (resolution on bridge)

All network-scoped tools share one reference system resolved by TouchDesigner via `td.op()`:

| Field | Role |
| --- | --- |
| `OpPath` | Absolute or relative path string |
| `contextPath?` | Anchor for relative paths; default base = project root (`/project1`) |

Canonical output echoes TD’s absolute `node.path`. `execute_python` is OpPath-exempt by default; `contextPath` is exposed as `__tdmcp_context_path__` + optional `tdmcp_resolve()` helper.

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

Artifacts: `tdmcp-daemon`, `tdmcp-gui`, `bridge/`, `diagnostics/catalog.yaml`, bootstrap `.tox`. Packaging via `cargo xtask dist` is **Planned** (P2); until then build with `cargo build --release -p tdmcp-daemon -p tdmcp-gui`.

Daemon start: `tdmcp-daemon start [--port 9860] [--data-dir …]`. MCP clients may auto-spawn on connection refused. Manual start for debugging.

---

## Phased delivery

| Phase | Ship | Exit green | Status |
| --- | --- | --- | --- |
| **P0** | Daemon + IPC + bootstrap + Streamable HTTP: `fleet` + script/errors + `capture` (`top`/`preview`) + diagnostics + per-pid queue + exclusive fail + resurrection | Two connected pids; exclusive fails while busy; perception non-black; structured script failure | **Shipped** (see [`E2E_CHECKLIST.md`](E2E_CHECKLIST.md)) |
| **P1** | `mutate_nodes`, `capture` `chop_data`, dialogs (Win), op lint engine | Preflight aggregates bad paths + lint; partial apply emits `skipped_dependent` | **Planned** |
| **P1.x** | `capture` `pop`, `chop_image` | Non-TOP heroes via temp converters | **Planned** |
| **P2** | Lifecycle create/start/stop (tray already shipped) | Operator create/start/stop; new project by pid | Partial (tray **Shipped**; lifecycle **Planned**) |
| **P3** | WebSocket / remote RFC | Separate design review | **Planned** |

---

## Decided contract (summary)

- TD↔daemon: local IPC (named pipe / UDS); handshake returns FS path to bridge package.
- Cursor↔daemon: Streamable HTTP on `http://127.0.0.1:9860/mcp` (JSON fallback also on `/mcp/tools/*`; rmcp at `/mcp/rpc`).
- Identity: `pid` only; exclusive fails iff queue non-empty; resurrection stacks until first success.
- Perception: `capture` only; builders never self-grade look.
- Paths: `OpPath` + optional `contextPath`; TD resolves; default base `/project1`.
- Diagnostics: catalog-backed codes; free-string-only failures forbidden.
