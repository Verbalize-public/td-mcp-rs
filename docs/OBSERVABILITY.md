# Observability & Logging — Spec

Status: **M1/M2/M4/M5/M6 shipped; M3 planned** (spec v2 — challenged & revised;
implementation plan: [`OBSERVABILITY_PLAN.md`](OBSERVABILITY_PLAN.md)). M3 (TD
textport mirror / face LOGS upgrade) needs its own T3.1 live-verify gate
against a real TD instance before implementation starts — not yet run.
Owner: daemon/GUI.
Cross-refs: [`CONTRACT.md`](CONTRACT.md) (diagnostics catalog, `execute_python`
logs), [`CONFIG.md`](CONFIG.md) (config surface — gains `[logging]`),
[`E2E_CHECKLIST.md`](E2E_CHECKLIST.md).

---

## 1. Problem statement

| # | Problem | Evidence (current state) |
| --- | --- | --- |
| P1 | **TD-side logs are weak and spread.** The bridge COMP's face LOGS panel almost never shows anything | The face LOGS section is a 14-line tail of the `./debug` Text DAT (`bridge/tox_callbacks.py:36-37`, `_debug_log_lines` at `:139-148`). That DAT has **exactly one writer**: `execute_python`'s stream capture (`bridge/tdmcp_bridge/execute.py:92` `_append_debug_dat`). Nothing mirrors the Textport, node tracebacks, or `debug()` calls — so unless the agent happens to run scripts, the panel reads `( no logs )` |
| P2 | **Log sources are fragmented with no central store.** Daemon, MCP/proxy layer, and TD bridge each log somewhere else and nothing lands on disk reliably on the primary platform (Windows) | One subscriber, stderr only: `crates/tdmcp-daemon/src/tracing_init.rs`. File persistence exists **Unix-only** via detached-spawn fd attach to `{data_dir}/daemon.log` (`crates/tdmcp-daemon/src/ensure.rs:269-286`); Windows nulls both streams (`ensure.rs:248`). Python bridge just `print(...)`s into the Textport. The stdio proxy (`mcp` subcommand) logs to a stderr owned by Cursor |
| P3 | **Logs are not visible in the GUI.** The tray GUI surfaces status/fleet/settings but zero log content; diagnosing a bad bridge means hunting stderr that may not exist | Admin API exposes `/admin/status`, `/admin/fleet`, `/admin/mcp-sessions`, `/admin/shutdown`, `/admin/restart` — no log endpoint (`crates/tdmcp-daemon/src/admin.rs:67-76`). GUI `View` enum is `Fleet | Settings` only (`crates/tdmcp-gui/src/lib.rs:34`) |
| P4 | **Retention is unbounded or absent.** `daemon.log` appends forever when it exists; there is no rotation, size cap, or sweep anywhere | No rotation code in workspace; `attach_unix_daemon_log` opens append-only (`ensure.rs:273-276`) |
| P5 | **Message quality is inconsistent.** Coverage is uneven (heavy in `bridge.rs`/`ensure.rs`, silent crates elsewhere), some messages duplicate or under-specify errors, and stale claims exist (doc comment says "`RUST_LOG` / `TDMCP_LOG` control filter" but `TDMCP_LOG` is read nowhere — `tracing_init.rs:8`) | Macro census: dense in `tdmcp-daemon`, moderate in `tdmcp-mcp`/`tdmcp-gui`/`tdmcp-ipc`; `tdmcp-core`, `tdmcp-config`, `tdmcp-diagnostics`, `xtask` have **no tracing dependency at all** |

End goal: production-ready logging/error handling — one place on disk, every
component contributing, visible live in the GUI, bounded disk usage, and
consistent messages.

## 2. Goals

1. **G1 — Centralized store in the install folder.** All components write into
   `{data_dir}/logs/` (the data dir *is* the install folder:
   `%LOCALAPPDATA%\tdmcp-rs\` on Windows per
   [`CONFIG.md`](CONFIG.md), created by `install::default_data_dir`,
   `crates/tdmcp-daemon/src/install.rs:148-152`).
2. **G2 — Every source flows there**: Rust daemon + mcp/ipc layers, TD bridge,
   out-of-process emitters (stdio proxy).
3. **G3 — GUI visibility**: a Logs view in the tray GUI with follow mode.
4. **G4 — Bounded persistence**: rotation + retention sweep; disk use provably capped.
5. **G5 — TD textport mirroring**: the bridge face LOGS panel reflects what an
   operator would see in the Textport, not only agent-run scripts.
6. **G6 — Message hygiene**: level/target/field conventions, audit pass over all
   crates, delete noise.

## 3. Non-goals

- Structured telemetry export (OTLP/metrics/traces). JSON-lines files are the
  ceiling for v1; ingestion by external stacks stays out of scope.
- Federation slave→master log shipping (P2 candidate; the ingest seam below
  makes it additive later).
- Changing the diagnostics catalog contract (`tdmcp.*` codes returned inside
  MCP responses) — logging complements it, never replaces it.
- A standalone log-viewer binary or web UI.

## 4. Current state inventory (kept as reference)

| Surface | Today | Fate |
| --- | --- | --- |
| Subscriber | Single `fmt` → stderr, EnvFilter (`RUST_LOG`, fallback hardcoded target list, fallback `info`) at `crates/tdmcp-daemon/src/tracing_init.rs` | Rewritten into layered `Registry` (console + file), see §5.1 |
| Detached-spawn capture | Unix-only `{data_dir}/daemon.log` fd attach (`ensure.rs:269-286`); Windows null | Superseded by in-process file sink; fd attach deleted |
| TD bridge | Face LOGS = tail of `./debug` DAT; single writer = `execute_python` capture; plain `print("tdmcp-rs: ...")` elsewhere | `./debug` becomes a real ring buffer fed by textport tee + IPC downlink (§5.4); face panel upgraded |
| Stdio proxy | tracing events → its own stderr (owned by MCP client) | Forwards via loopback ingest endpoint (§5.3) |
| Tests | Spawn daemons with `RUST_LOG=warn,tdmcp_daemon=info`, redirect stderr to per-test files (`tests/admin_auth.rs:110-133` etc.) | Keep working unchanged; add assertions against `{data_dir}/logs/` |

## 5. Design

### 5.0 Record schema (uniform across sources)

Every stored line is JSON with this shape (superset; unknown fields preserved):

```json
{"ts":"2026-01-01T12:00:00.123Z","level":"warn","src":"bridge","pid":12345,
 "target":"bridge::tox_callbacks","msg":"heartbeat pong timeout","code":null,"kvs":{}}
```

| Field | Meaning |
| --- | --- |
| `ts` | RFC3339 UTC, ms precision |
| `level` | `trace/debug/info/warn/error` |
| `src` | Emitter class: `daemon`, `ipc`, `mcp`, `proxy`, `gui`, `bridge` |
| `pid` | TouchDesigner pid for bridge-sourced lines; process pid otherwise |
| `target` | Tracing target or Python logger name (`bridge::<module>`) |
| `msg` | Human message (conventions §5.7) |
| `code` | Optional diagnostics catalog code when the event maps to one |
| `kvs` | Structured extras (flat stringly-typed map) |

### 5.1 Daemon sink & subscriber rewrite

- New crate dependency: `tracing-appender` (≥ 0.2.3 for `max_log_files`);
  `tracing-subscriber` gains the `registry` feature (workspace pins features,
  which disables defaults). No `json` feature: a custom **SinkLayer** owns
  serialization once (`serde_json`) and feeds both the file writer and the
  ring — single formatter, no double-format drift (plan T1.1/T1.4).
- `tracing_init::init(&cfg)` builds a `Registry` with two layers:
  - **File layer**: `RollingFileAppender` — daily rotation,
    filename prefix `daemon`, suffix date, writer into `{cfg.logging.dir}`,
    `max_log_files(cfg.logging.max_files)`; JSON format, filter
    `EnvFilter::from(cfg.logging.filter)`.
  - **Console layer**: existing `fmt().with_target(true).with_writer(stderr)`
    kept verbatim so terminal runs behave as today.
- Filter resolution order (fixes the stale-doc bug): explicit config
  `[logging].filter` → `RUST_LOG` env → built-in default
  `"info,tdmcp_daemon=debug"` (file layer gets ≥ debug for tdmcp targets;
  console keeps current defaults). `TDMCP_LOG` is either implemented as an
  alias of the file-layer filter or removed from the doc comment — decide in
  M1, no third option.
- In-memory tail for the GUI: one shared `Mutex<VecDeque<Record>>` ring
  (capacity 2048) with a monotonic `seq` stamped at insert; the file layer and
  the ring are fed by the same custom `Layer`. No broadcast channel — the GUI
  is poll-based (existing 250 ms repaint cadence), so a seq-cursor read is
  strictly simpler with identical behavior. Ring lives in daemon memory only;
  crash safety comes from the files.
- Ordering guarantee (explicit): **per-source strict order, cross-source
  best-effort** by arrival seq. Proxy-ingested lines can interleave with local
  ones by up to network/flush latency.
- Multi-process safety: only the lock-holding daemon writes files (existing
  `daemon.lock` guarantees a single writer). `ensure`'s fd-attach path is
  deleted along with its doc comment (`ensure.rs:227-231`).

### 5.2 Retention (bounded disk)

Three independent bounds, all enforced:

| Mechanism | Setting | Default | Notes |
| --- | --- | --- | --- |
| Rotation | `rotation = "daily"` | daily | One file/day; restarts append to same-day file |
| File count | `[logging] max_files` | `14` | `max_log_files` prunes oldest at rollover |
| Startup sweep | — | — | On init, delete any `*.log*` older than `retention_days` (default `30`) — catches pre-upgrade strays like legacy `daemon.log` |
| Periodic sweep | — | every 24 h | Same predicate on a tokio timer so long-lived daemons prune without restarts |

Legacy cleanup: M1 deletes `{data_dir}/daemon.log` handling entirely and the
startup sweep removes leftover copies. Worst-case disk ≈
`max_files × per-file size`; a soft per-file size guard (stop writing new lines
past N MiB until next rotation) is a P1 stretch goal, not required if daily
volumes stay < ~10 MiB in practice (measure in M1 acceptance).

### 5.3 Out-of-process emitters (stdio proxy)

The `mcp` subcommand cannot share the rotating file safely (separate process,
may outlive/restart around the daemon). It forwards instead:

- `POST /admin/logs/ingest` — body: `{lines: [Record…]}` (≤ 64 KiB/call).
  Loopback-only unless `[auth] psk` set; reuses admin auth exactly like other
  admin routes. Handler stamps `src:"proxy"` and feeds the same sink/ring.
- Fire-and-forget with drop-on-full policy; a proxy that can't reach the
  daemon still prints to stderr (never blocks tool calls on logging).
- Same endpoint is the future seam for federation slave shipping (non-goal now).

### 5.4 TD bridge: textport mirroring + forwarding

Root cause of P1 is fixed by making the bridge observe everything the
Textport sees, then fan out to two sinks (local `./debug` DAT and daemon):

- **Stdout/stderr tee** installed once at bootstrap: replace `sys.stdout` /
  `sys.stderr` with tee objects that (a) call through to the original streams —
  Textport behavior unchanged — and (b) hand each line to the local ring +
  outbound queue. This captures `print` from *any* node/script, uncaught
  traceback tails, and `debug()` output routed through stdout. Re-asserted on
  heartbeat if TD restores the originals (TD can reset `sys.stdout` on save).
- **Local ring**: `./debug` Text DAT keeps its role but stops being
  execute_python-exclusive: timestamp+level prefix per line, same 64 KiB ring
  trim logic (reuse `_ring_append_text`), best-effort writes only (existing
  "never fail for textport" rule, `execute.py:47`).
- **Face LOGS upgrade** (`tox_callbacks.py:_debug_log_lines:139`): raise
  `_LOG_PANEL_LINES` (`tox_callbacks.py:37`) 14 → up to ~22, final value pinned
  by the live fit probe (plan V5: 560×560 face @ font 10); each tail line
  renders `HH:MM:SS <glyph> msg` (`!` warn, `!!` error, plain info); keep the
  `( no logs )` idle marker (`tox_callbacks.py:401`). Face stays a tail view;
  full history remains in `./debug`.
- **Forwarding**: batched IPC `Message::Event { name: "log", payload }`
  (`crates/tdmcp-ipc/src/framing.rs:84-89` — envelope already exists).
  Payload = array of records per §5.0 (`src:"bridge"`, `pid` stamped by the
  daemon from the connection's handshake identity, not trusted from Python).
  Batch trigger: 500 ms timer or 32 lines, whichever first; queue cap 256
  lines with drop-oldest + one-shot `dropped=N` marker line (same discipline
  as fleet budgets). Flush piggybacks the existing pump tick
  (`task_queue.py:_schedule_pump`, 50 ms cadence) — no dedicated thread.
- **Session-safety constraint (hard requirement)**: today any non-`Response`
  frame received while awaiting a tool reply tears the session down
  (`crates/tdmcp-daemon/src/bridge.rs:517-523` — `Ok(Ok(_other))` → warn +
  `Disconnected`). M2 must add an explicit arm routing
  `Message::Event{name:"log"}` to the log sink and *continuing* the await — in
  `await_matching_response` and every other frame reader. Without this, a log
  burst during a long `execute_python` kills live bridges. Log events also
  count as inbound traffic for idle-dead accounting (they prove liveness).
- **TD global errors** (`td.errors`): poll on the heartbeat cadence (5 s,
  `default.toml:64`), **not** the 50 ms pump tick (main-thread cost); forward
  new entries as `level:error` records with `target:"td_errors"`, deduped via
  an LRU keyed `(op_path, text)` size 500. Cadence re-checked in M3 against
  live TD (see `touchdesigner` skill before implementing).

### 5.5 Admin API + GUI Logs view

- `GET /admin/logs?after=<seq>&limit=&level=&src=` →
  `{records:[…], next:<seq>}` served from the in-memory ring (§5.1);
  cursor-based poll, no SSE/websocket in v1 (GUI already repaints at 250 ms
  and throttles fetches to 2 s).
- `GET /admin/logs/path` → absolute log dir; GUI gets an "Open logs folder"
  button next to the existing reveal actions.
- **Auth**: `/admin/logs` and `/admin/logs/path` are added to the
  PSK-required path list (`crates/tdmcp-daemon/src/middleware.rs:55-57`,
  `requires_psk_auth`) so LAN + psk deployments never expose logs unauthed;
  loopback behavior unchanged. `/admin/logs/ingest` joins the same list.
- GUI: new `View::Logs` variant (`crates/tdmcp-gui/src/lib.rs:34`) entered via
  a ghost button in the existing top-bar RTL action row (`.tox` ⚙ row,
  `lib.rs:913-936`). **Hard layout constraint: the tray window is 380 px wide
  × 600 max tall** (`crates/tdmcp-gui/src/theme.rs:39-41`) — the Logs view is
  designed as a narrow single-column scrollback with level dots, tap-to-expand
  detail, compact filter chips, and follow-pinned-to-bottom; heavy analysis is
  delegated to "Open logs folder" in the real editor. Full UX spec:
  [`OBSERVABILITY_PLAN.md`](OBSERVABILITY_PLAN.md) §M4. Headless (`--no-gui`)
  unaffected.
- MCP-facing convenience (P1, optional): reuse `fleet include=logs` style — a
  `logs {tail}` tool row is deliberately **not** added in v1; agents read
  `execute_python` results and diagnostics as today.

### 5.6 Config surface (`[logging]`, template `crates/tdmcp-config/assets/default.toml`)

| Key | Default | Meaning |
| --- | --- | --- |
| `dir` | *(unset → `{data_dir}/logs`)* | Override for tests/portable installs |
| `filter` | *(unset)* | EnvFilter string for the file layer (beats `RUST_LOG`) |
| `max_files` | `14` | Rotated files kept |
| `retention_days` | `30` | Sweep threshold at startup |
| `console_level` | *(unset)* | Optional separate console filter; unset = current behavior |

Precedence follows [`CONFIG.md`](CONFIG.md): CLI/env > TOML > defaults. Seven-
touchpoint rule applies when adding keys (config struct, template, docs, GUI
draft fields, validation, tests, CHANGELOG note).

### 5.7 Message hygiene conventions (enforced by review checklist + clippy where possible)

Levels: `error` = user-actionable fault needing attention; `warn` = degraded
but self-healed or retrying; `info` = lifecycle milestones only (bind, spawn,
connect, disconnect, install); `debug` = protocol/state-machine detail;
`trace` reserved, unused by default.

Rules:

1. One fact per record; imperative lowercase message (`"bridge idle-dead — no inbound traffic"` style retained).
2. Error records carry `error = %e` (or `code` when catalog-backed) — never a bare message restating the type.
3. Log at the boundary once: a caller that returns an error does not also log the same error its caller will log.
4. Targets are module paths; no ad-hoc prefixes duplicated inside messages (drop the `"stdio_proxy:"`-style message prefixes once `src` exists).
5. No secrets ever logged: PSK, auth tokens, session ids beyond short prefixes.
6. Silent crates get baseline coverage: `tdmcp-core` registry connect/drop, `tdmcp-config` load/override application (debug level), `xtask` stays console-only.
7. Audit pass deletes: duplicate near-identical warns in retry loops (keep first + final), success-path info noise, stale doc comments (`TDMCP_LOG`).
8. Every emitted `code` field value must exist in `diagnostics/catalog.yaml`
   (completeness test extended to scan log statements, mirroring the existing
   response-code test in [`TESTING.md`](TESTING.md)).
9. Argument-shape tool failures carry `tdmcp.args.*` codes as structured
   `isError` results — raw serde strings never reach agents; mapping in
   [`TOOL_ERROR_PLAN.md`](TOOL_ERROR_PLAN.md).

## 6. Milestones

| M | Scope | Acceptance |
| --- | --- | --- |
| **M1 Sink** | §5.1 + §5.2: layered subscriber, JSON file layer, rotation, sweeps, delete fd-attach, resolve `TDMCP_LOG` question | Fresh `start` creates `{data_dir}/logs/daemon.<date>.log` with valid JSON lines on **Windows and Unix**; killing + restarting appends; 15th-old file swept |
| **M2 Bridge uplink** | §5.4 forwarding half: Python sender, daemon ingest into sink/ring, batching + drop policy | Fake TD peer sends 1000 events; daemon file contains them in order with `src:"bridge"`, correct `pid`; flood drops oldest without backpressure stall |
| **M3 TD mirror** | §5.4 local half: stdout/stderr tee, `./debug` ring, face LOGS upgrade, `td.errors` polling | Live TD: `print` from an unrelated node appears in face LOGS within ~1 s; traceback from a broken node shows as `error`; E2E_CHECKLIST rows written and pass |
| **M4 GUI + API** | §5.5: admin endpoints, `View::Logs`, follow mode, open-folder | GUI shows live lines while a bridge connects/disconnects; cursor resume after reconnect loses no lines; headless build unaffected |
| **M5 Proxy ingest** | §5.3 | With daemon up, `mcp` subcommand tool call produces `src:"proxy"` lines centrally; daemon-down call still succeeds |
| **M6 Hygiene** | §5.7 audit across all crates, silent-crate baselines, docs updates ([`CONFIG.md`](CONFIG.md) `[logging]`, [`CONTRACT.md`](CONTRACT.md) observability row, README) | Census: every `error!` carries structured cause; grep finds no bare-prefix messages; completeness test green |

M1–M2 are the critical path; M3/M4/M5 parallelizable after M2; M6 last.

## 7. Testing plan

- Unit: ring/snapshot semantics, sweep age math, batch/drop policy, schema serializer.
- Integration (fake bridge peer, pattern of `tests/admin_auth.rs`): uplink ordering, pid stamping, ingest auth rejection when non-loopback without psk.
- Retention: temp-dir test forcing old mtimes; assert prune.
- GUI: manual rows appended to [`E2E_CHECKLIST.md`](E2E_CHECKLIST.md) (live-TD mirror + follow mode).
- Constitution: no `unwrap`/`expect` in new lib paths; Python side best-effort everywhere ("never fail the script for logging").

## 8. Risks & mitigations

| Risk | Mitigation |
| --- | --- |
| Log event frames kill live bridge sessions (current frame-reader behavior, `bridge.rs:517-523`) | Design-level fix in M2: explicit `Message::Event` arm routes to sink and continues; covered by a regression test that interleaves events during an awaited tool reply |
| `sys.stdout` replacement fights TD (reset on save/reload) | Re-assert on heartbeat + before each execute; original streams always called through. **TD `debug()` may bypass `sys.stdout` entirely** — M3 must verify live; if uncapturable, document as known limitation and rely on `td.errors` polling for op errors |
| Per-line forwarding floods slow links | Batch + drop-oldest caps (§5.4); local `./debug` keeps full fidelity even when uplink drops |
| Two writers on same-day file after admin-restart race | Real window: restart is a spawn-then-die handoff that deletes `daemon.lock` before spawning (`admin.rs:238-243`). Accepted: file layer is append-only with per-write line records (no in-file offsets/index), so brief interleave degrades to shuffled lines, never corruption; M1 may additionally have the replacement wait for old-pid exit if interleave shows up in practice |
| JSON verbosity inflates disk vs plain fmt | Measure in M1; if > ~10 MiB/day at default filter, switch file layer to compact fmt + sidecar index rather than raising limits |
| GUI poll jank with large backlog | Server-side `limit` clamp (512/req); GUI fetches pages on scroll-up only |

## 9. Open questions

1. `TDMCP_LOG`: implement as alias (which layer?) or purge the mention? (M1 decision)
2. Should `fleet` gain `include=logs` tail for remote slaves in P2, or is GUI-only viewing enough?
3. `td.errors` polling cadence vs TD main-thread cost — needs a live-TD measurement session before M3 lands.

## 10. Revision log

**v2 (this revision)** — challenge pass over v1; outcomes:

- **C1 (correctness, blocking):** v1 missed that non-`Response` IPC frames
  disconnect the session mid-await (`bridge.rs:517-523`). Uplink now carries an
  explicit hard requirement + regression test. §5.4.
- **C2 (simplification):** broadcast channel dropped for a seq-stamped
  `Mutex<VecDeque>` ring — the GUI is poll-based, broadcast bought nothing.
  §5.1.
- **C3 (UX grounding):** tray window is 380×600; Logs view scoped to a narrow
  single-column design with external-folder escape hatch. §5.5, plan §M4.
- **C4 (human factor):** JSONL stays canonical (no format toggle); a
  `tdmcp-daemon logs [n]` CLI renderer covers human tailing instead. Plan §M1.
- **C5–C7:** pin `tracing-appender >= 0.2.3` (`max_log_files`), per-layer
  filters confirmed feasible via `Registry`, new `/admin/logs*` +
  `/admin/logs/ingest` added to the PSK-required path list (`middleware.rs`).
- **C8:** face LOGS line budget re-checked against 560×560 face at font 10 —
  target ≤ 22 lines incl. timestamp prefix, measured live in M3.
- **C9:** `debug()` capture uncertainty made an explicit M3 live-verify item.
- **C11:** retention sweep runs periodically (24 h), not only at startup.
  §5.2.
- **C12:** GUI Settings gains no logging controls in v1 (reveal button only);
  TOML remains the sole source of truth.
