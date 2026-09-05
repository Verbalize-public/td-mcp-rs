# Architecture

Control-plane layout for **td-mcp-rs**. Contract source of truth for agent
behaviour remains [`docs/CONTRACT.md`](docs/CONTRACT.md). This document owns
crate boundaries and process topology.

## Topology

```text
 IDE (Cursor / …)  ──┐
                     │  MCP Streamable HTTP  http://{bind_address}:9860/mcp
 Other MCP callers ──┤   (default bind 127.0.0.1; LAN sharing uses a configured bind)
                     ▼
  ┌────────── Daemon process (tdmcp-daemon) ─────────────────────────────┐
  │  bg thread: axum + rmcp │ admin │ PidRegistry │ SlaveRegistry │ queues │
  │  main thread (gui feature, default): tray + egui → /admin/*          │
  └──────────┬───────────────────────────────────────────────────────────┘
             │  TCP loopback 127.0.0.1:9861 (configurable via [bridge] host/port)
             ▼
    TD process(es)  ←── bootstrap .tox → handshake → FS load bridge/

 Federation: joined daemons register and push fleet updates to a coordinator;
 the coordinator proxies tools by daemonId (see docs/FEDERATION.md).
```

## Crate graph

```text
tdmcp-core          domain: PidRegistry, SlaveRegistry, TaskQueue, ResurrectionState (zero I/O)
tdmcp-config        TOML schema, atomic save, validation, partial patches and restart diff
tdmcp-diagnostics   catalog types, YAML loader, envelope builders
tdmcp-ipc           TCP loopback + framing + handshake
tdmcp-mcp           rmcp tool handlers → core calls → diagnostics envelope
tdmcp-projectio     official tools (toeexpand/toecollapse), toc/sidecar
tdmcp-dialogs       OS dialogs (Win32 + macOS adapters)
tdmcp-daemon        bin: clap, tracing, axum wiring, admin API (composition root)
                    optional dep on tdmcp-gui via default `gui` feature
tdmcp-gui           lib: egui + eframe + native tray → background admin HTTP + Settings
tdmcp-test-support  fake TD bridge peer (dev / tests)
xtask               release / packaging helpers
bridge/             Python package (not a crate) loaded by TD after handshake
```

### Boundary rule

`tdmcp-core` never imports `rmcp`, `axum`, or IPC types — pure domain logic,
testable without network or process mocking.

`tdmcp-mcp` is the only crate that knows MCP tool schemas; it translates JSON
args into core calls and core results into the `diagnostics` envelope.

`tdmcp-daemon` owns bridge session runtime (accept loop, per-pid actors,
timeouts, teardown) plus process wiring (axum, admin, GUI spawn).

## Surfaces

| Surface | Bind | Role |
| --- | --- | --- |
| MCP Streamable HTTP | `{bind_address}:9860/mcp/rpc` | Agent tools; JSON fallback `/mcp/tools/*`. Optional Bearer PSK when `[auth] mode=psk`. |
| Admin HTTP | `{bind_address}:9860/admin/*` | GUI + status; loopback-only for shutdown/restart/sessions; auth-gated remote for `/admin/federation/*` + `/admin/config`. |
| Federation | master↔slave HTTP | Register, fleet-push, tool proxy (`daemonId`). See [`docs/CONFIG.md`](docs/CONFIG.md) § Federation auth & admin surface. |
| Bridge IPC | `127.0.0.1:9861` (TCP loopback, `[bridge] host`/`port`) | TD peer |

## Identity and queues

The daemon's Settings service serializes partial writes, validates and persists
them before notifying live consumers. A startup snapshot identifies values
that still require restart. Federation reconnects only when its link settings
change; per-call bridge budgets update without replacing TD connections.

- **Ids:** OS `pid` (required on bridged tools); optional `daemonId` when federated.
- Ambiguous pid across daemons → `tdmcp.federation.ambiguous_pid`.
- Per-pid task queue on the owning daemon; session chill on `(mcp_session, daemon_scope, pid)`.
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
| [`README.md`](README.md) | Product overview + quickstart |
| [`docs/INSTALL.md`](docs/INSTALL.md) | Per-harness MCP setup + troubleshooting |
| [`docs/FEDERATION.md`](docs/FEDERATION.md) | Federation user guide (setup, security model, limits) |
| [`docs/RECIPES.md`](docs/RECIPES.md) | Prompt cookbook for end users |
| [`docs/CONFIG.md`](docs/CONFIG.md) | TOML config + Settings GUI |
| [`docs/CONTRACT.md`](docs/CONTRACT.md) | Tool shapes and diagnostics |
| [`CONSTITUTION.md`](CONSTITUTION.md) | Rust engineering law |
| [`AGENTS.md`](AGENTS.md) | Agent route-first entry |
| [`RISKS.md`](RISKS.md) | Accepted exceptions |
| [`docs/TESTING.md`](docs/TESTING.md) | Test strategy |
| [`docs/DELIVERY.md`](docs/DELIVERY.md) | Packaging / release |
| [`docs/E2E_CHECKLIST.md`](docs/E2E_CHECKLIST.md) | Live TD verification |
