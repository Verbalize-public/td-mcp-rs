# Architecture

Control-plane layout for **td-mcp-rs**. Contract source of truth for agent
behaviour remains [`docs/CONTRACT.md`](docs/CONTRACT.md). This document owns
crate boundaries and process topology.

## Topology

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

## Crate graph

```text
tdmcp-core          domain: PidRegistry, TaskQueue, ResurrectionState (zero I/O)
tdmcp-diagnostics   catalog types, YAML loader, envelope builders
tdmcp-ipc           named pipe / UDS + framing + handshake
tdmcp-mcp           rmcp tool handlers → core calls → diagnostics envelope
tdmcp-daemon        bin: clap, tracing, axum wiring, admin API (composition root)
tdmcp-gui           bin: egui + eframe + tray-icon → admin client
tdmcp-test-support  fake TD bridge peer (dev / tests)
xtask               release / packaging helpers
bridge/             Python package (not a crate) loaded by TD after handshake
```

### Boundary rule

`tdmcp-core` never imports `rmcp`, `axum`, or IPC types — pure domain logic,
testable without network or process mocking.

`tdmcp-mcp` is the only crate that knows MCP tool schemas; it translates JSON
args into core calls and core results into the `diagnostics` envelope.

`tdmcp-daemon` is a **composition root** only (wiring, no business logic).

## Surfaces

| Surface | Bind | Role |
| --- | --- | --- |
| MCP Streamable HTTP | `127.0.0.1:9860/mcp/rpc` | Agent tools (`rmcp` Streamable HTTP); JSON fallback at `/mcp/tools/list` + `/mcp/tools/call` |
| Admin HTTP | `127.0.0.1:9860/admin/*` | GUI status / kill / restart |
| Bridge IPC | `\\.\pipe\tdmcp-rs` or `{dataDir}/bridge.sock` | TD peer |

## Identity and queues

- **Only id:** OS `pid`.
- Per-pid task queue: shared (default) or exclusive (fail if queue non-empty).
- Resurrection: stack cancelled tasks on IPC loss; clear on first successful task.

## Global harnesses

See [`docs/CONTRACT.md`](docs/CONTRACT.md):

- **`OpPath` + `contextPath`** — TD-native path resolution on the bridge.
- **`diagnostics`** — rustc-style envelope + `diagnostics/catalog.yaml`.

## Testing layers

| Layer | Where | Needs live TD? |
| --- | --- | --- |
| Unit | per crate | no |
| Integration | daemon + `tdmcp-test-support` | no |
| Bridge pytest | `bridge/tests` | no (fake `td`) |
| Manual E2E | [`docs/E2E_CHECKLIST.md`](docs/E2E_CHECKLIST.md) | yes |

## Related docs

| Doc | Role |
| --- | --- |
| [`README.md`](README.md) | Install / quickstart |
| [`docs/CONTRACT.md`](docs/CONTRACT.md) | v1 contract, tool catalogue, phases |
| [`CONSTITUTION.md`](CONSTITUTION.md) | Rust engineering law |
| [`AGENTS.md`](AGENTS.md) | Agent route-first entry |
| [`RISKS.md`](RISKS.md) | Accepted exceptions |
| [`docs/TESTING.md`](docs/TESTING.md) | Test strategy |
| [`docs/DELIVERY.md`](docs/DELIVERY.md) | Packaging / release |
| [`docs/E2E_CHECKLIST.md`](docs/E2E_CHECKLIST.md) | Live TD verification |
