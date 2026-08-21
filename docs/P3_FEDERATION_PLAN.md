# P3 — Federation implementation plan

Curated build-and-test gate for the remote/federation delivery.
Design settled in this document; no separate RFC.

Related: [`CONTRACT.md`](CONTRACT.md) § Phased delivery (P3 row),
[`ARCHITECTURE.md`](../ARCHITECTURE.md), [`TESTING.md`](TESTING.md),
[`CONFIG.md`](CONFIG.md).

---

## Verdict

The daemon's existing transport-agnostic tool dispatch and `reqwest`-based admin
HTTP client make federation tractable. The main risk is in the slave-registration
state machine and pid disambiguation — the rest is thin wiring over proven
primitives. Implementation fits within existing crate boundaries; `uuid` is
already a workspace dep (add as direct on crates that generate/store
`daemon_id`).

---

## Design decisions (settled)

| # | Decision | Rationale |
| --- | --- | --- |
| D1 | Streamable HTTP, not WebSocket | `rmcp` 3.0.1 has no WS server transport. Reusing `/mcp/rpc` requires zero new transport code. |
| D2 | `bind_address` on `[server]`, not a separate `[remote]` section | "Enable remote" is implicit: `bind_address != "127.0.0.1"`. Flatter config, one fewer table. |
| D3 | Auth via `Authorization: Bearer <psk>` header, not query param | No URL leakage in logs/proxies/history. `rmcp` Streamable HTTP client supports custom headers. |
| D4 | Loopback-guard middleware on `/admin/*` with auth-gated allowlist | Changing bind to `0.0.0.0` must not expose shutdown/restart. See § Middleware allowlist. |
| D5 | Persistent `daemon_id` (UUID) generated on first start | Pid-only breaks with multiple daemons. Daemon-scoped identity stored in config, never changes (preserve across `install` template rewrite). |
| D6 | Slave registration via POST to master's `/admin/federation/register` | Reuses existing admin HTTP surface. Slave pushes fleet every 2s. No new protocol. |
| D7 | Master-scanned discovery via `/admin/federation/status` (minimal unauth probe) | No UDP/mDNS for v1. Full `/admin/status` is **not** the LAN oracle. Manual entry available. |
| D8 | Every daemon is a full daemon | "Slave" is just a daemon that pushes fleet to a master. It still serves its own MCP endpoint and GUI. No reduced build. |
| D9 | Single-level federation (master → slaves) | No multi-master or daisy-chain in v1. A slave can become master by changing config. |
| D10 | Fleet is ephemeral on master | Master does not persist slave state to disk. Slaves re-register on reconnect. No stale-config problem. |
| D11 | Register-token auth model | Slave→master uses master’s `auth.psk` as `master_psk`. Master→slave proxy/config uses `authToken` from register body (slave’s `auth.psk`, or empty if `mode=none`). |
| D12 | `role=slave` disables idle exit | Same effect as `keep_alive`; slave must stay up without a local MCP lease. |
| D13 | Non-loopback bind requires PSK | Config validation rejects `bind_address` non-loopback when `auth.mode=none` or empty `psk`. |

### Locked former open questions

| # | Decision |
| --- | --- |
| Q1 | Disconnected slave pids: show greyed with `unreachable` badge (omit only after optional later purge). |
| Q2 | Remote slave restart: **deferred** post-MVP. Fd = view/edit config only. |
| Q3 | `bind_address`: IP only (no interface-name resolution). |
| Q4 | Auto-generate PSK when switching to `psk` if empty; reject save if non-loopback + none/empty. |

### Auth matrix (locked)

| Hop | Bearer |
| --- | --- |
| Slave → master register / fleet-push | Master’s `auth.psk` (slave config field `master_psk`) |
| Client → any daemon `/mcp/rpc` | That daemon’s `auth.psk` (skip if `mode=none`) |
| Master → slave tool proxy | Slave’s `auth.psk` as register body `authToken` (empty if slave `mode=none`); stored in `SlaveRegistry` |
| Master → slave `/admin/config` | Same stored `authToken` |

`master_psk` meaning: PSK to present to the master (the master’s `auth.psk`).

### Middleware allowlist (locked)

| Path | Remote (`0.0.0.0`) | Auth |
| --- | --- | --- |
| `/mcp/rpc`, `/mcp/health`, `/mcp/tools/*` | Allowed | Bearer if `auth.mode=psk` |
| `/admin/federation/*` (except status probe) | Allowed | Bearer if psk |
| `/admin/config` GET/POST | Allowed | Bearer if psk |
| `/admin/federation/status` | Allowed | **Unauth minimal probe** `{ok,version,role,hostname,daemonId,port}` |
| `/admin/status` | Loopback only (or auth) | Not the LAN scan oracle |
| `/admin/shutdown`, `/admin/restart`, `/admin/mcp-sessions*` | **Loopback only** | N/A remote |

### `daemon_id` conflict (locked)

- Re-register same `daemonId` from the **same** advertised base URL → overwrite.
- Same `daemonId` from a **different** host/port → reject + diagnostic.

---

## Config changes

### New sections

```toml
[server]
port = 9860
bind_address = "127.0.0.1"    # "0.0.0.0" to allow remote connections

[auth]
mode = "none"                 # "none" | "psk"
psk = ""                      # used to authenticate incoming MCP + slave registration

[federation]
role = "standalone"           # "standalone" | "master" | "slave"
daemon_id = "a1b2c3d4-..."    # auto-generated on first start, never changes

# Only valid when role = "slave":
master_url = "http://192.168.1.100:9860"
master_psk = ""               # PSK to present to the master (master's auth.psk)
```

### Crate impact

| Crate | Changes |
| --- | --- |
| `tdmcp-config` | `bind_address` on `ServerSection`, new `AuthSection`, new `FederationSection`. Updated `default.toml` + `save()` round-trip + `FIELD_DESCS`. Non-loopback⇒psk validation. |
| `tdmcp-daemon` | Bind-address resolution, auth middleware, loopback guard, `/admin/federation/*`, `/admin/config`, slave registry wiring, fleet aggregation, tool proxy dispatch, daemon_id bootstrap, slave idle-exit disable. |
| `tdmcp-gui` | Role-aware header, master fleet groups, slave self-view, add-slave + subnet scan, slave settings panel, "Go standalone", role-switch confirmation. **No remote restart (Q2 deferred).** |
| `tdmcp-mcp` | Optional `daemonId` on tools with `pid`. Fleet gains `daemonId` + `hostname`. Ambiguous-pid error. Proxy dispatch. |
| `tdmcp-core` | `DaemonId` newtype. `SlaveRegistry` types. |
| `tdmcp-diagnostics` | +4 catalog codes. |
| `tdmcp-ipc` | **None.** |
| Deps | Workspace `uuid` as direct on config/core as needed (not a new package). |

### Catalog codes

| Code | Severity | Meaning |
| --- | --- | --- |
| `tdmcp.remote.unauthorized` | Error | Auth token missing or wrong |
| `tdmcp.federation.slave_unreachable` | Error | Master cannot reach a slave for a proxied tool call |
| `tdmcp.federation.ambiguous_pid` | Error | `pid` matches multiple daemons; response includes candidate list with `daemonId` |
| `tdmcp.federation.auth_rejected` | Error | Slave registration rejected by master (PSK mismatch) |

---

## Agent loop — gates

| Gate | Objective | Exit green (observable) | Fail / stop |
| --- | --- | --- | --- |
| G0 | Doc lock | This file has auth matrix, allowlist, Q1–Q4 locked, PR waves; CONTRACT P3 ≠ WebSocket | `handoff` |
| G1 | Fa: bind + auth + loopback | `cargo test -p tdmcp-config`; `admin_auth.rs` green; non-loopback+none rejected | `error`/`budget` |
| G2 | Fb: identity + registry + fleet + `/admin/config` | `federation_registration.rs` green; `SlaveRegistry` unit tests | `error`/`budget` |
| G3 | Fc: `daemonId` proxy | `federation_proxy.rs` green (inspect, ambiguous, unreachable, capture budget) | `error`/`budget` |
| G4 | Document operate surfaces | CONTRACT/CONFIG/AGENTS + 4 catalog codes + skill/operate note | `error` |
| G5 | Fd GUI | Manual Fd-E* after G3; backend already automated | `handoff` |

Budgets: `max_gate_retries=3`, `stagnation_window=4`. Manual GUI is **not** Exit green for G1–G3.

---

## Phased delivery (PR waves)

### Wave 1 — Fa: Config + auth + loopback guard (G1)

| Step | What | Test gate |
| --- | --- | --- |
| Fa1 | `ServerSection.bind_address`; `AuthSection`; `default.toml`, `save()`, `FIELD_DESCS`; non-loopback⇒psk validation | `tdmcp-config` unit |
| Fa2 | Daemon resolves `bind_address`; `/admin/status` gains `bindAddress` | Integration: bind `0.0.0.0` on ephemeral port |
| Fa3 | Auth middleware on `/mcp/rpc` + `/admin/federation/*` (except status probe) + `/admin/config` | `admin_auth.rs`: wrong/missing → `tdmcp.remote.unauthorized` |
| Fa4 | Loopback guard per § Middleware allowlist | Remote `/admin/shutdown` → 403; local → 200 |
| Fa5/Fa6 | GUI Remote section + header badge | **Optional PR1b** — not G1 Exit green |

Tests: `cargo test -p tdmcp-config` + `cargo test -p tdmcp-daemon --test admin_auth`.

### Wave 2 — Fb: Identity + registration + `/admin/config` (G2)

| Step | What | Test gate |
| --- | --- | --- |
| Fb1 | `FederationSection`; `daemon_id` generate-once; preserve across install | Config unit |
| Fb2 | `/admin/federation/status` minimal unauth probe | Integration |
| Fb3 | Slave register task + backoff; body includes `authToken`; idle exit disabled | `federation_registration.rs` |
| Fb4 | Master register → `SlaveRegistry`; same URL overwrite; different URL reject | Same |
| Fb5 | Fleet-push every 2s | Same |
| Fb6 | `/admin/fleet` aggregates local + slaves (`daemonId`, `hostname`) | Same |
| Fb7 | Stale: 6s disconnected / 10s unreachable | Same |
| Fb8 | `/admin/config` GET/POST partial merge (auth-gated remote) | Same + `admin_auth.rs` |

### Wave 3 — Fc: Tool proxy (G3 — value gate)

| Step | What | Test gate |
| --- | --- | --- |
| Fc1 | Optional `daemonId` on pid tools; schema golden | `schema_golden` |
| Fc2 | Proxy via pooled Streamable HTTP client per slave; strip `daemonId`; `routed: true` | `federation_proxy.rs` |
| Fc3 | Ambiguous pid → `tdmcp.federation.ambiguous_pid` | Same |
| Fc4 | Unreachable → `tdmcp.federation.slave_unreachable` (no master retry) | Same |
| Fc5 | Per `(master_session, daemonId, pid)` one-in-flight; federated capture store-first / size budget | Same |

### Wave 4 — Fd: GUI federation (G5)

| Step | What | Test gate |
| --- | --- | --- |
| Fd1–Fd6 | Role badges, slave self-view, master fleet groups, add-slave + scan, slave settings, Settings FEDERATION | Manual E2E only |
| — | Remote restart | **Deferred (Q2)** |

---

## Test suite (per layer)

### Unit

| Crate | New tests |
| --- | --- |
| `tdmcp-config` | Auth/Federation defaults, daemon_id persistence, bind_address, round-trip, non-loopback validation |
| `tdmcp-core` | `DaemonId`, `SlaveRegistry` insert/resolve/aggregate |
| `tdmcp-diagnostics` | 4 new codes in catalog |

### Integration (spawn real daemon binaries)

| Test file | Covers |
| --- | --- |
| `admin_auth.rs` | Auth middleware + loopback guard + `/admin/config` auth |
| `federation_registration.rs` | Master + 2 slaves, register/retry, fleet-push, stale, aggregate |
| `federation_proxy.rs` | Proxy inspect/execute/capture, ambiguous, unreachable, latency/size budget |

Pattern: follow `multi_client_freeze.rs` (`TestDaemon`, `pick_free_port`, pipe fake TD, `TDMCP_IDLE_EXIT_SECS=0`).

### Bridge pytest

No change. Federation does not touch `bridge/` or IPC.

---

## E2E checklist (F-phase gates)

### Prerequisites

1. Two machines on the same LAN, or two TD instances on one machine.
2. Release build: `cargo build -p tdmcp-daemon --release`.
3. Both machines running the daemon.

### Fa gate (remote access) — automated preferred

| # | Step |
| --- | --- |
| Fa-E1 | Daemon binds `0.0.0.0`. `/mcp/health` reachable from another machine. |
| Fa-E2 | Auth mode `psk`. Tool call without token → `tdmcp.remote.unauthorized`. |
| Fa-E3 | Same call with correct Bearer → success. |
| Fa-E4 | Auth mode `none` (loopback only). Call without token → success. |
| Fa-E5 | Loopback guard: remote `/admin/shutdown` → 403; local → 200. |
| Fa-E6 | GUI Remote section (manual / PR1b). |
| Fa-E7 | GUI remote badge (manual / PR1b). |

### Fb gate (registration + fleet)

| # | Step |
| --- | --- |
| Fb-E1 | Slave registers; master `/admin/fleet` shows slave (automated). GUI optional. |
| Fb-E2 | Slave TD appears with `daemonId` + `hostname`. |
| Fb-E3 | Disconnect → greyed within 6s; reconnect restores. |
| Fb-E4 | Kill slave → unreachable within 10s; restart reconnects. |
| Fb-E5 | Pid collision across slaves → distinct `daemonId`s. |
| Fb-E6 | Slave GUI header / Go standalone (manual Fd). |

### Fc gate (tool proxy)

| # | Step |
| --- | --- |
| Fc-E1 | `inspect` with `daemonId` → `routed: true`. |
| Fc-E2 | `capture` proxied; payload under budget / store-first. |
| Fc-E3 | `execute_python` proxied. |
| Fc-E4 | Ambiguous pid without `daemonId`. |
| Fc-E5 | Mid-call slave kill → `slave_unreachable`. |

### Fd gate (GUI — manual)

| # | Step |
| --- | --- |
| Fd-E1 | Collapsible per-daemon fleet groups. |
| Fd-E2 | Add-slave dialog test + add. |
| Fd-E3 | Scan network via `/admin/federation/status`. |
| Fd-E4 | Slave settings via `/admin/config`. |
| Fd-E5 | Settings FEDERATION role dropdown. |
| Fd-E6 | Go standalone without local TD loss. |

---

## Stability design (federation-specific)

| Failure | Behavior | Test coverage |
| --- | --- | --- |
| Master down | Slaves keep serving; re-register with backoff | `federation_registration.rs` |
| Slave down | Grey TDs; proxy → `slave_unreachable` | `federation_proxy.rs` |
| Network partition | Both sides unreachable; no split-brain | Same as slave-down |
| Backoff saturation | Cap 30s | `federation_registration.rs` |
| pid collision | `ambiguous_pid` + candidates | `federation_proxy.rs` |
| Auth mismatch | Registration rejected; `auth_rejected` | `federation_registration.rs` |
| Version mismatch | Show version; no refusal | Interop accepted |
| Proxy timeout | Bridge budgets + 5s margin; pooled HTTP client | `federation_proxy.rs` |

---

## RISKS.md entries

| Id | Risk | Mitigation |
| --- | --- | --- |
| RF1 | Bind before middleware window | Auth/loopback composed into Router before serve |
| RF2 | Thundering-herd on master restart | Backoff starts at 1s; independent timers; N≤100 OK |
| RF3 | Copied config shares `daemon_id` | Same URL overwrite; different URL reject + GUI warning |
| RF4 | Proxy latency | Timeout = bridge + 5s; LAN-only v1 |
| RF5 | Slave idle-exit without local MCP | `role=slave` disables idle exit |
| RF6 | Unauth federation status probe on LAN | Minimal fields only; full `/admin/status` not the oracle; non-loopback requires PSK |

---

## Rollback / compatibility

- Old config files (no `[auth]` / `[federation]`) parse with `#[serde(default)]`.
- `role = "standalone"` and `bind_address = "127.0.0.1"` are behaviorally identical to today.
- `daemon_id` generated on first P3 start and preserved across install when possible.
- Identity: `pid` required; `daemonId` optional when federated.

---

## Verification commands

```text
cargo test -p tdmcp-config
cargo test -p tdmcp-core
cargo test -p tdmcp-daemon --test admin_auth
cargo test -p tdmcp-daemon --test federation_registration
cargo test -p tdmcp-daemon --test federation_proxy
cargo test -p tdmcp-mcp --test schema_golden
scripts/check.ps1
```
