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
primitives. Implementation fits within existing crate boundaries with no new
dependencies.

---

## Design decisions (settled)

| # | Decision | Rationale |
| --- | --- | --- |
| D1 | Streamable HTTP, not WebSocket | `rmcp` 3.0.1 has no WS server transport. Reusing `/mcp/rpc` requires zero new transport code. |
| D2 | `bind_address` on `[server]`, not a separate `[remote]` section | "Enable remote" is implicit: `bind_address != "127.0.0.1"`. Flatter config, one fewer table. |
| D3 | Auth via `Authorization: Bearer <psk>` header, not query param | No URL leakage in logs/proxies/history. `rmcp` Streamable HTTP client supports custom headers. |
| D4 | Loopback-guard middleware on `/admin/*` | Changing bind to `0.0.0.0` must not expose shutdown/restart. One axum middleware, not a second listener. |
| D5 | Persistent `daemon_id` (UUID) generated on first start | Pid-only breaks with multiple daemons. Daemon-scoped identity stored in config, never changes. |
| D6 | Slave registration via POST to master's `/admin/federation/register` | Reuses existing admin HTTP surface. Slave pushes fleet every 2s. No new protocol. |
| D7 | Master-scanned discovery (scan subnet by hitting `/admin/status`) | No UDP/mDNS for v1. `/24` scan completes in <3s with 2s timeout. Manual entry available. |
| D8 | Every daemon is a full daemon | "Slave" is just a daemon that pushes fleet to a master. It still serves its own MCP endpoint and GUI. No reduced build. |
| D9 | Single-level federation (master → slaves) | No multi-master or daisy-chain in v1. A slave can become master by changing config. |
| D10 | Fleet is ephemeral on master | Master does not persist slave state to disk. Slaves re-register on reconnect. No stale-config problem. |

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
master_psk = ""               # PSK the master uses to authenticate to this slave
```

### Crate impact

| Crate | Changes |
| --- | --- |
| `tdmcp-config` | `bind_address` on `ServerSection`, new `AuthSection`, new `FederationSection` (role, daemon_id, master_url, master_psk). Updated `default.toml` + `save()` round-trip + `FIELD_DESCS`. |
| `tdmcp-daemon` | Bind-address resolution in `Config`, auth middleware, loopback guard, `/admin/federation/*` endpoints (register, fleet-push, status), slave registry, fleet aggregation, tool proxy dispatch, daemon_id generation + `[federation]` bootstrap. |
| `tdmcp-gui` | Role-aware header (master/slave/standalone badges), master fleet groups (collapsible), slave self-view (master connection block), add-slave dialog + subnet scan, slave settings panel, "Go standalone" action, role-switch confirmation. |
| `tdmcp-mcp` | Optional `daemonId` field on tool schemas with `pid`. Fleet response gains `daemonId` + `hostname`. Ambiguous-pid error (`tdmcp.federation.ambiguous_pid`). Proxy dispatch handler. |
| `tdmcp-core` | `DaemonId` newtype. `SlaveRegistry` types (connected-slaves tracking, fleet-aggregate model). |
| `tdmcp-diagnostics` | +4 catalog codes. |
| `tdmcp-ipc` | **None.** |
| New deps | **None.** (`uuid` is already a transitive dep via `rmcp`.) |

### Catalog codes

| Code | Severity | Meaning |
| --- | --- | --- |
| `tdmcp.remote.unauthorized` | Error | Auth token missing or wrong |
| `tdmcp.federation.slave_unreachable` | Error | Master cannot reach a slave for a proxied tool call |
| `tdmcp.federation.ambiguous_pid` | Error | `pid` matches multiple daemons; response includes candidate list with `daemonId` |
| `tdmcp.federation.auth_rejected` | Error | Slave registration rejected by master (PSK mismatch) |

---

## Phased delivery

### Phase Fa — Config + auth + loopback guard (no federation yet)

| Step | What | Test gate |
| --- | --- | --- |
| Fa1 | `ServerSection` gains `bind_address: String` (default `"127.0.0.1"`). `ConfigFile` gains `AuthSection`. Updated `default.toml`, `save()`, `FIELD_DESCS`. | `tdmcp-config` unit: default parses, save round-trips, missing sections → defaults. |
| Fa2 | Daemon `Config` resolves `bind_address`. `bind_with_retry` binds the resolved address when not loopback. Admin `/admin/status` gains `bindAddress`. | Integration: daemon binds `0.0.0.0` on ephemeral port; `/admin/status` returns `bindAddress: "0.0.0.0"`. Default stays `127.0.0.1`. |
| Fa3 | Auth middleware: axum layer on `/mcp/rpc` + `/admin/federation/*`. Reads `Authorization: Bearer <psk>`. Skips when `auth.mode = "none"`. | Integration: unauthenticated request → `tdmcp.remote.unauthorized`. Correct token → tools pass. `mode = "none"` → all pass. |
| Fa4 | Loopback guard: axum middleware on `/admin/*` (except `/admin/status` and `/admin/federation/*`). Rejects non-loopback source IPs. | Integration: `GET /admin/shutdown` from `127.0.0.1` → 200; same from remote IP → 403. |
| Fa5 | GUI: Settings gains "REMOTE ACCESS" section (bind-address text field, auth mode radio, PSK password field with show/hide + regenerate). Toggle is implicit (bind != 127.0.0.1). | Manual: Settings → change bind to `0.0.0.0` → save → restart → daemon bound to `0.0.0.0`. |
| Fa6 | GUI: Fleet header shows 🔒 (loopback) or 🌐 (remote) badge. Remote badge shows resolved URL. | Manual: loopback bind → 🔒 badge. `0.0.0.0` bind → 🌐 badge with URL. |

Tests at Fa gate: `cargo test -p tdmcp-config` + `cargo test -p tdmcp-daemon --test mcp_tools` (auth variants) + new `admin_auth.rs` integration test.

### Phase Fb — Federation: identity + slave registration

| Step | What | Test gate |
| --- | --- | --- |
| Fb1 | `FederationSection`: role, daemon_id, master_url, master_psk. `Config::load` generates `daemon_id` (UUIDv4) on first start if missing, writes back. | `tdmcp-config` unit: daemon_id generated once, survives re-load. Missing section → `role = "standalone"`. |
| Fb2 | Daemon exposes `/admin/federation/status`: returns `{daemonId, hostname, version, bindAddress, port, role}`. | Integration: GET returns populated struct. Authenticated when `auth.mode = "psk"`. |
| Fb3 | When `role = "slave"` on start: spawn background task. Connect to `POST <master_url>/admin/federation/register` with `Authorization: Bearer <master_psk>`. Body: `{daemonId, hostname, version, port}`. Retry with 1s/2s/4s/8s backoff (cap 30s). | New integration test: `federation_registration.rs`. Master + slave daemon processes on different ports. Slave registers, master accepts. Kill master → slave retries. Restart master → slave reconnects. |
| Fb4 | When `role = "master"`: expose `POST /admin/federation/register`. Auth middleware applies. Adds slave to in-memory `SlaveRegistry`. Returns `{ok: true, masterDaemonId: "..."}`. Rejects duplicate `daemonId` (re-register = overwrite). | Same test. Register → master shows 1 slave. Re-register same daemonId → overwrites, no duplicate. Duplicate port → still accepted (two daemons can share a host). |
| Fb5 | Slave pushes fleet: `POST <master_url>/admin/federation/fleet-push` every 2s. Body: full fleet JSON (same shape as `/admin/fleet`). Master stores in registry. | Integration: fake TD peer on slave → slave fleet has 1 connected → master's aggregated fleet shows it. Slave TD disconnects → next push updates master. |
| Fb6 | Master `/admin/fleet` aggregates local + all slave fleets. Each process gains `daemonId` + `hostname`. | Integration: master + 2 slaves each with 1 TD → `/admin/fleet` returns 3 processes across 3 daemonIds. |
| Fb7 | Slave stale detection: if no fleet-push for 3 intervals (6s), master marks slave's TDs as disconnected. If no fleet-push for 5 intervals (10s), master marks slave unreachable. | Integration: pause slave fleet push → master fleet shows disconnected after 6s, unreachable after 10s. Resume → reconnected. |

Tests at Fb gate: new `crates/tdmcp-daemon/tests/federation_registration.rs` (spawns 2–3 real daemon binaries on ephemeral ports). New `crates/tdmcp-core/tests/` for `SlaveRegistry` unit tests.

### Phase Fc — Tool proxy

| Step | What | Test gate |
| --- | --- | --- |
| Fc1 | `McpHandler` tool schema: `pid` stays required. `daemonId` added as optional string param on all tools that take `pid` (`inspect`, `capture`, `execute_python`, `mutate_nodes`). | Schema test: `list_tools` returns updated schemas. |
| Fc2 | Proxy dispatch: if `daemonId` is present and not local, master forwards the tool call to the slave's `/mcp/rpc` as a Streamable HTTP client call. Strips `daemonId` from params. Adds `routed: true` to response. | Integration: master with no local TD. Slave with 1 fake TD. `inspect {pid: 42, daemonId: "slave-1"}` → forwarded → slave's inspect response + `routed: true`. |
| Fc3 | Ambiguous pid: if `daemonId` is absent and `pid` matches multiple daemons, return `tdmcp.federation.ambiguous_pid` with candidate list `[{daemonId, hostname}]`. | Unit: `SlaveRegistry::resolve(pid)` → `Ambiguous([...])`. Integration: 2 slaves both with pid 42 → call without daemonId → error with 2 candidates. |
| Fc4 | Slave unreachable: if proxy call to slave fails, return `tdmcp.federation.slave_unreachable` with slave daemonId and hostname. Master does NOT retry (the IDE/slave handles retry). | Integration: kill slave daemon mid-call → `slave_unreachable`. |

Tests at Fc gate: new `crates/tdmcp-daemon/tests/federation_proxy.rs` (spawns master + slave, fake TDs on slave, proxy dispatch exercised).

### Phase Fd — GUI federation

| Step | What | Test gate |
| --- | --- | --- |
| Fd1 | GUI: role detection from `/admin/status` (new `role` + `daemonId` fields). Header shows role badge: "td-mcp-rs (master)" / "td-mcp-rs (slave)" / "td-mcp-rs". | Manual: start master → header shows "(master)". Start slave → header shows "(slave)". |
| Fd2 | GUI: slave self-view. Master connection block below header: hostname, IP, connection dot (green/amber/red), uptime, daemon ID. "Go standalone" button → confirmation dialog → changes config role to "standalone" → saves → restarts. | Manual: slave startup → master block visible with connection status. Click "Go standalone" → confirm → daemon restarts as standalone. |
| Fd3 | GUI: master fleet view. Collapsible per-daemon groups: "THIS MACHINE" + one per slave. Each slave section: hostname/IP header, TD rows, status bar (version, connection dot, uptime, ⚙ gear, ✕ remove). | Manual: master + 1 slave each with 1 TD → two groups in fleet view. Collapse/expand works. Slave disconnect → amber status. |
| Fd4 | GUI: Add-slave dialog. Text fields for host/IP + port + PSK. "Test connection" button hits `/admin/federation/status` on target. "Scan network" button scans /24 subnet (default). Scan results show discovered daemons with version + hostname. "Add" button posts to master's register endpoint. | Manual: enter slave IP + PSK → test → green. Add → slave appears in fleet. Scan → sees both daemons. |
| Fd5 | GUI: slave settings panel. When clicking ⚙ on a slave row, fetch slave config via `GET <slave_url>/admin/config` (new endpoint). Render same settings UI. Save → `POST <slave_url>/admin/config` + "Save & Restart" button. | Manual: edit slave's bridge timeout → save → verify on slave machine. |
| Fd6 | GUI: Settings → "FEDERATION" section. Role dropdown (standalone/master/slave). Role switch → confirmation dialog. Slave fields (master URL + PSK) appear only when role = slave. | Manual: switch standalone → slave → fields appear. Fill → save → restart. Daemon starts as slave. |
| Fd7 | GUI: "/admin/config" endpoint on every daemon (GET + POST). Returns the current `ConfigFile` as JSON. Accepts partial updates. Loopback-guarded (master accesses it via direct HTTP with auth, not through the loopback guard). | Integration: `GET /admin/config` → full config JSON. `POST /admin/config` with `{"bridge": {"call_timeout_secs": 90}}` → merged + saved. |

Tests at Fd gate: manual E2E rows (see § E2E checklist). No automated GUI tests (egui testing is impractical).

---

## Test suite (per layer)

### Unit tests (`cargo test --workspace`)

| Crate | New tests |
| --- | --- |
| `tdmcp-config` | `AuthSection` defaults, `FederationSection` defaults, daemon_id generation + persistence, `bind_address` on `ServerSection`, round-trip save/load with all new sections, `[auth]` / `[federation]` absent → defaults |
| `tdmcp-core` | `DaemonId` newtype (Display, Eq, Hash), `SlaveRegistry` insert/lookup/remove, `resolve_pid` (unique, ambiguous, not-found), fleet aggregate merge (local + N slaves) |
| `tdmcp-diagnostics` | Catalog completeness: all 4 new codes have catalog entries |

### Integration tests (no live TD, spawn real daemon binaries)

| Test file | What it covers |
| --- | --- |
| `federation_registration.rs` | Spawn 1 master + 2 slave daemons on ephemeral ports. Slave registration + retry + re-register. Fleet-push with fake TD. Slave disconnect detection (stale timestamp). Master `/admin/fleet` aggregation. Persistent `daemon_id` across restarts. |
| `federation_proxy.rs` | Spawn master + slave. Fake TD on slave. Tool proxy dispatch (`inspect`, `execute_python`, `capture`). `daemonId` optional param. Ambiguous pid error. Slave unreachable error. Proxy latency budget (<500ms per call). |
| `admin_auth.rs` | Auth middleware: correct token → 200, wrong token → 401 with `tdmcp.remote.unauthorized`, no token → 401, `mode = "none"` → bypass. Loopback guard: remote IP → `/admin/shutdown` → 403, local → 200. `/admin/config` GET/POST with auth. |

Pattern: follow `multi_client_freeze.rs`. Spawn daemon child processes with `--no-gui`, `TDMCP_IDLE_EXIT_SECS=0`, ephemeral ports (`pick_free_port`), `TestDaemon` struct with `Drop` kill. Wait for health before testing. Log tails on failure.

### Bridge pytest (no change)

Federation does not touch `bridge/` or the IPC protocol. Existing bridge tests (`bridge/tests/`) remain unchanged and must stay green.

### Manual E2E (real TD, real network)

See § E2E checklist below.

---

## E2E checklist (F-phase gates)

### Prerequisites

1. Two machines on the same LAN, or two TD instances on one machine (different projects).
2. Release build: `cargo build -p tdmcp-daemon --release`.
3. Both machines running the daemon.

### Fa gate (remote access)

| # | Step | |
| --- | --- | --- |
| Fa-E1 | Daemon binds `0.0.0.0`. `/mcp/health` reachable from another machine. | |
| Fa-E2 | Auth mode `psk`. Tool call from another machine without token → `tdmcp.remote.unauthorized`. | |
| Fa-E3 | Same call with correct `Authorization: Bearer <psk>` → success. | |
| Fa-E4 | Auth mode `none`. Same call without token → success. | |
| Fa-E5 | Loopback guard: `POST /admin/shutdown` from remote IP → 403. Same from local → 200. | |
| Fa-E6 | GUI: Remote section visible in Settings. Bind address + auth fields editable. Save + restart → changes take effect. PSK show/hide + regenerate work. | |
| Fa-E7 | GUI: Fleet header shows 🌐 badge when remote enabled. URL displayed. | |

### Fb gate (slave registration + fleet)

| # | Step | |
| --- | --- | --- |
| Fb-E1 | Machine A: master. Machine B: slave. Slave starts → registers with master. Master GUI shows slave in fleet view. | |
| Fb-E2 | Slave has 1 connected TD. Master fleet shows it with correct `daemonId` and `hostname`. | |
| Fb-E3 | Slave TD disconnects (close project). Within 6s, master fleet shows it as disconnected. Reconnect → reconnected. | |
| Fb-E4 | Kill slave daemon. Master fleet shows slave unreachable within 10s. TDs greyed. Restart slave → reconnects, TDs green. | |
| Fb-E5 | Two slaves, both with pid collisions (e.g. both have pid 100 from different machines). Master fleet shows both with distinct `daemonId`. | |
| Fb-E6 | Slave GUI: header shows "(slave)". Master connection block visible with live status dot. "Go standalone" → confirm → daemon restarts as standalone → header shows no badge. | |

### Fc gate (tool proxy)

| # | Step | |
| --- | --- | --- |
| Fc-E1 | IDE connected to master. `inspect {pid: X, daemonId: "slave-1"}` → proxy → correct result with `routed: true`. | |
| Fc-E2 | `capture top {pid: X, daemonId: "slave-1"}` → PNG image proxied successfully. | |
| Fc-E3 | `execute_python {pid: X, daemonId: "slave-1", script: "result = 1"}` → `{result: 1}`. | |
| Fc-E4 | Same pid on two slaves, call without `daemonId` → `tdmcp.federation.ambiguous_pid` with candidate list. | |
| Fc-E5 | Kill slave daemon mid-proxy-call → `tdmcp.federation.slave_unreachable`. | |

### Fd gate (GUI completeness)

| # | Step | |
| --- | --- | --- |
| Fd-E1 | Master GUI: fleet view shows collapsible per-daemon groups. Expand/collapse works. | |
| Fd-E2 | Master GUI: Add-slave dialog. Enter slave IP + PSK → "Test connection" → green. "Add" → slave appears. | |
| Fd-E3 | Master GUI: "Scan network" discovers running daemons on LAN. Results show version + hostname. | |
| Fd-E4 | Master GUI: click ⚙ on slave row → slave settings panel loads config from slave. Edit → save → slave config updated. | |
| Fd-E5 | Slave GUI: Settings → FEDERATION section. Role dropdown works. Slave fields appear/disappear based on role. | |
| Fd-E6 | Slave GUI: "Go standalone" works without data loss (TD stays connected, local fleet unchanged). | |

---

## Stability design (federation-specific)

| Failure | Behavior | Test coverage |
| --- | --- | --- |
| Master down | Slaves keep serving. IDE can connect to any slave directly. Master restart → slaves re-register (backoff). | `federation_registration.rs`: kill → retry → restart → reconnect |
| Slave down | Master marks slave's TDs grey. Proxied calls fail with `slave_unreachable`. | `federation_proxy.rs`: kill slave mid-call |
| Network partition | Both sides see "unreachable." No split-brain (master doesn't own slave state). | Same as slave-down |
| Slave backoff saturation | Backoff caps at 30s. Slave GUI shows "retrying in Ns." | `federation_registration.rs`: assert backoff ceiling |
| Master overload (many slaves) | Fleet-push is a lightweight POST every 2s per slave. Master aggregates in-memory. 100 slaves = 50 req/s. | Load test in `federation_registration.rs`: spawn 10 slaves, all fleet-pushes land within budget |
| pid collision | Resolver returns `ambiguous_pid` with candidate list. IDE retries with `daemonId`. | `federation_proxy.rs`: two slaves, same pid, no daemonId → error |
| Auth mismatch (master ↔ slave) | Registration rejected. Slave GUI shows "auth rejected." Master logs attempt. | `federation_registration.rs`: wrong master_psk → rejected |
| Slave version mismatch | Master shows version in fleet. No refusal — backward-compatible protocol. | `federation_registration.rs`: different daemon versions still interoperate |
| Tool proxy timeout | Master applies same bridge call budgets to proxied calls. Configurable via `[bridge]`. | `federation_proxy.rs`: slow slave response → timeout + error |
| Slave restarts while master is down | Slave keeps trying master_url with backoff. No data loss — fleet is ephemeral. | `federation_registration.rs`: restart both in sequence |

---

## RISKS.md entries

| Id | Risk | Mitigation | 
| --- | --- | --- |
| RF1 | `0.0.0.0` bind exposes `/mcp/rpc` to LAN before auth middleware is applied. Window between bind and axum route attachment is zero (same `Router::new()` call), but a theoretical race exists between `bind_with_retry` returning and `axum::serve` starting. | Auth middleware is composed into the Router before `bind_with_retry`. No window. |
| RF2 | Slave registration backoff could create thundering-herd on master restart (N slaves all retry at 1s). | Initial backoff starts at 1s, not 0. Each slave's backoff timer is independent (no clock sync). Acceptable for N≤100. |
| RF3 | `daemon_id` generation on first start uses `uuid::Uuid::new_v4()`. UUIDv4 is random, not deterministic. If a user copies the config file to another machine, both machines share the same `daemon_id`. | Document that `daemon_id` must be unique per machine. Master rejects duplicate `daemon_id` on register. GUI shows "daemon_id conflict" warning. |
| RF4 | Tool proxy dispatch adds latency (~RTT to slave). Bridged calls are already bounded by `[bridge]` timeouts (45s/120s). Proxy adds ~5–50ms on LAN but could be more on WAN. | Proxy timeout = slave's `[bridge]` timeout + 5s margin. Configurable. Not a v1 WAN use case. |

---

## Rollback / compatibility

- Old config files (no `[auth]` or `[federation]` sections) parse with `ConfigFile::default()` — all new sections have serde `#[serde(default)]`.
- Old daemons (pre-P3) ignore unknown TOML sections. Upgrading is additive.
- `daemon_id` is generated on first P3 start. Missing → generated + saved back to config.
- `role = "standalone"` is behaviorally identical to today. Zero change for existing users who don't touch federation settings.
- `bind_address = "127.0.0.1"` is behaviorally identical to today's hardcoded bind. Zero change for existing users.

---

## Open: not decided

| # | Question | Candidate |
| --- | --- | --- |
| Q1 | Slave pids when slave is disconnected: show greyed or omit? | Show greyed with "unreachable" badge — more informative, less surprising than disappearing entries. |
| Q2 | Allow master to restart a slave via ⚙ menu? | Yes — gear menu on slave row: "View config" / "Restart slave". Restart calls `/admin/restart` on slave (authenticated). Confirmation required. |
| Q3 | Should `bind_address` honor interface name (e.g. "Ethernet") or IP only? | IP only for v1. Interface-name resolution is platform-specific and a point release. |
| Q4 | Auto-generate PSK when user switches auth mode to "psk" and PSK is empty? | Yes — avoid blank-PSK footgun. Generate on toggle, display in confirmation dialog. |