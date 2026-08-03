# Curated Review — td-mcp-rs

Fact-checked audit across hardcoding, schema duplication, compile-time
enforcement, architecture, complexity, and stability. Citations are
path + line against the tree as of this writing. Findings labeled
**code-path verified** were traced in source; they are not live-TD
MCP claims unless noted.

Related: [`CONTRACT.md`](CONTRACT.md), [`../ARCHITECTURE.md`](../ARCHITECTURE.md),
[`../RISKS.md`](../RISKS.md), [`../CONSTITUTION.md`](../CONSTITUTION.md).

## Verdict

The repo is disciplined: workspace `unwrap_used` / `expect_used` deny,
catalog ↔ `codes.rs` parity tests, `BridgeMethod` wire parity, typed
diagnostics. Problems are concentrated — session supersede vs task-queue
honesty, serial handshake without read timeout, stringly MCP tool names,
and cross-language timing/limits without parity fixtures. Not systemic
rot: scoped debt with a few real stability bugs.

```text
IDE MCP → daemon → run_ipc_accept → accept_handshake/read_msg
                 → BridgeSessions → PidRegistry/TaskQueue
                 → supersede cancel → old run_session
                 → teardown "skip loss" (no cancel_all)
```

---

## High severity (stability)

### H1 — Supersede-while-in-flight can leave zombie `TaskQueue` slots

**Status: fixed (Wave A).** Originally code-path verified; now covered by
`superseding_while_in_flight_clears_queue_for_exclusive`.

On disconnect/cancel mid-job, `run_tool_job` returns `JobLoop::Disconnect`
**before** `complete_task`:

```430:449:crates/tdmcp-daemon/src/bridge.rs
            RecvOutcome::Disconnected => {
                let _ = job.reply.send(Err(BridgeRpcError::Disconnected { pid }));
                return JobLoop::Disconnect;
            }
        },
        Err(_e) => {
            let _ = job.reply.send(Err(BridgeRpcError::Disconnected { pid }));
            return JobLoop::Disconnect;
        }
    };

    let success = matches!(&outcome, Ok(v) if !is_bridge_error(v));
    {
        let mut reg = registry.lock().await;
        // ...
        let _ = reg.complete_task(pid, result);
    }
```

Superseded teardown intentionally skips `on_bridge_lost` / `cancel_all`
when generation mismatches:

```575:586:crates/tdmcp-daemon/src/bridge.rs
        match s.get(&pid) {
            Some(handle) if handle.generation == generation => {
                s.remove(&pid);
            }
            _ => {
                // Superseded by a newer session for this pid — do not touch registry.
                warn!(
                    pid,
                    generation, "bridge session ended — superseded, skip loss"
                );
                return;
            }
        }
```

Same-fingerprint `handshake` does **not** reset the queue
(`crates/tdmcp-core/src/registry.rs` ~99–118). Exclusive enqueue then
fails while the queue is non-empty
(`crates/tdmcp-core/src/task_queue.rs` ~91–105) — callers can see
permanent false `tdmcp.bridge.queue_busy`.

[`RISKS.md`](../RISKS.md) R5 covers dual pipes/actors, **not** this queue
honesty gap. Existing supersede tests only hit **idle** old sessions.

**Fix direction:** On superseded teardown, still `queue.cancel_all()`
(without marking Disconnected / starting eviction TTL); add a
supersede-while-in-flight integration test.

### H2 — Serial IPC accept + unbounded handshake read can wedge all bridges

**Status: fixed (Wave A).** Handshake I/O timeout 5s; accept loop spawns
post-accept work.

`run_ipc_accept` awaits one `accept_handshake` at a time
(`crates/tdmcp-daemon/src/bridge.rs` ~638–670). `read_msg` uses bare
`read_exact` with **no timeout** (`crates/tdmcp-ipc/src/listener.rs`
~250–261). A peer that connects then stalls blocks every other TD
reconnect.

**Fix direction:** Per-connection handshake task +
`tokio::time::timeout` around handshake I/O.

---

## Medium severity

### M1 — `min_daemon` / `tdmcp.bridge.version` are dead protocol surface

Handshake always sets `min_daemon: None`
(`crates/tdmcp-ipc/src/listener.rs` ~115, ~217, ~236). Bridge still
declares `__min_daemon__ = "0.1.0"` and `bridge/manifest.json` carries
`minDaemon`. `codes::BRIDGE_VERSION` is in `ALL` +
`diagnostics/catalog.yaml` but **never emitted**. Contrast:
`tdmcp.op.outside_zone` is explicitly **reserved** in CONTRACT; this
code is not.

**Fix:** Wire version check + emit, **or** mark reserved in CONTRACT
like `outside_zone`.

### M2 — MCP tool names are stringly typed; schema catch-all hides drift

Three independent string matches: `tool_descriptors`, `dispatch_tool`,
`input_schema_for` (`crates/tdmcp-mcp/src/tools.rs`,
`crates/tdmcp-mcp/src/schema.rs` `_ => empty_object_schema()` at line 27).
A new or typo’d tool can get `{}` schema with no compile error.
`BridgeMethod` already shows the right pattern in core.

**Fix:** `ToolName` enum + exhaustive matches (no `_`).

### M3 — Outcome code sanitization inconsistent across tools

`execute_python` / mutate whitelist bridge codes; `capture` / `inspect`
pass raw codes into `build_diag`, whose fallback hardcodes
`layer: Fleet` (`crates/tdmcp-mcp/src/outcomes.rs`). Mis-layer breaks
CONTRACT’s agent re-entry signal.

**Fix:** Same whitelist (or typed code enum) for all outcome mappers;
fallback takes expected layer.

### M4 — Daemon panic join reported as success

```371:374:crates/tdmcp-daemon/src/main.rs
        Ok(Err(_)) => {
            warn!("daemon thread panicked");
            Ok(())
        }
```

Control-plane panic looks like clean exit to supervisors.

**Fix:** Non-zero / `Err` on panic join.

### M5 — Restart pipe handoff race (Windows)

TCP has `bind_with_retry`; named pipe `create()` can succeed while the
draining daemon still has an instance. TD may briefly attach to the
dying process. Secondary nit: `first_pipe_instance` is spent even if
the first `create` fails (`crates/tdmcp-ipc/src/listener.rs` ~202–212).

### M6 — Stdio session annotate is “latest matching name” only

Proxy never learns the daemon session UUID; annotate uses
`matchClientName: tdmcp-stdio-proxy`
(`crates/tdmcp-mcp/src/stdio_proxy.rs`,
`crates/tdmcp-daemon/src/admin.rs` ~139–162). Two concurrent stdio
clients can cross-label `/admin/mcp-sessions`.

### M7 — Dual MCP response shaping + hand-rolled HTTP

`server.rs` (JSON fallback) vs `rmcp_handler.rs` (image promotion) must
stay hand-synced. Daemon `ensure` / `main` use raw TCP HTTP while mcp
already uses `reqwest` — fragile body parsing (`rfind('{')` in
`crates/tdmcp-daemon/src/ensure.rs` ~151).

---

## Hardcoded / duplicate schema (curated)

| Item | Sources | Risk | Enforcement gap |
| --- | --- | --- | --- |
| Port `9860` | `tdmcp-config` default + `ensure.rs` + `main.rs:594` + gui | Drift on default change | Need `DEFAULT_PORT` const |
| `default_data_dir()` | Duplicate bodies in `config.rs` ~99–103 and `install.rs` ~132–136 | Same-crate copy-paste | Delete private copy; use `install::default_data_dir` |
| `MAX_FRAME` 16 MiB | `framing.rs` const vs literal in `listener.rs:256` | Cap drift | `pub(crate)` reuse |
| Heartbeat / timeouts | `BridgeSection::default`, `bridge.rs` consts / `production()`, Python `__init__.py` | Pre-handshake + test drift | Derive from config + limits fixture |
| `INSPECT_PATHS_LIMIT=32` | Rust + Python named; **no parity test** | Soft-cap drift | Extend `bridge_methods` fixture pattern |
| Children roster `64` | Python `CHILDREN_ROSTER_LIMIT` only; Rust docs only | Doc/behavior drift | Shared const + fixture |
| Pipe `\\.\pipe\tdmcp-rs` | Rust listener + Python dial | Cross-lang drift | Fixture or env-only SSOT |
| Bridge methods | Enum + Python HANDLERS + fixture | **Already guarded** by parity tests | Keep; model other constants after this |
| `/admin/history` | Same `fleet_summary` as `/admin/fleet` minus ipc depths | Misleading API | Delete or implement real history |

`AGENTS.md` still points at `TODO_ENFORCE_TYPE.md` — **file missing**
(doc drift).

---

## Architecture / complexity (not all bugs)

| Finding | Category | Notes |
| --- | --- | --- |
| ARCHITECTURE says daemon is “composition root only” | Architecture drift | `bridge.rs` owns session actor, timeouts, teardown — real business logic. Either move toward `tdmcp-mcp` / `tdmcp-core` or rewrite the claim. |
| `bridge/tdmcp_bridge/__init__.py` (~3180 lines) | Overcomplex / spaghetti | Wire + capture + mutate + fuzzy suggest + Win/Unix dial + queue in one module. Split by concern; keep `__init__` re-exports. |
| `daemon_link.rs` (~691 lines) | Overcomplex but tested | 5 reconnect knobs + generation + single-flight gate + watcher. Document state machine or collapse knobs; do not rewrite as P0. |
| Stdio vs HTTP dual surface | By design | CONTRACT transport note — keep; factor shared shaping only. |

---

## Explicitly not defects (verified WAD)

- Stdio + Streamable HTTP dual MCP surface
- `mutate_nodes` sequential no-rollback
- Resurrection / `DISCONNECTED_TTL` eviction
- Per-call vs per-method timeout layering (RISKS R4)
- Same-pid supersede cancel of prior actor (RISKS R5) — **queue side-effect is the real gap (H1)**
- Never-panic discipline outside listed RISKS exceptions

---

## Compile-time ROI ranking

1. Delete duplicate `default_data_dir` (trivial)
2. `ToolName` enum + exhaustive schema/dispatch (high ROI)
3. Share `MAX_FRAME` (trivial, security-relevant)
4. Limits/version parity fixtures (mirror `bridge_method_parity`)
5. Derive heartbeat production defaults from `BridgeSection::default()`
6. Wire or reserve `min_daemon` / `BRIDGE_VERSION`
7. Optional: codegen `codes.rs` from catalog YAML (nice-to-have; tests already bidirectional)

---

## Suggested remediation waves

- **Wave A (stability):** H1 queue cancel on supersede + test; H2 handshake timeout + concurrent accept
- **Wave B (types/SSOT):** ToolName enum; `DEFAULT_PORT` / `MAX_FRAME` / `data_dir`; limits fixture; wire-or-reserve `min_daemon`
- **Wave C (cleanup):** history endpoint; HTTP via reqwest; outcome whitelist; panic exit code; bridge module split

---

## Remediation status (Waves A–C)

| Wave | Item | Status |
| --- | --- | --- |
| A | H1 — `cancel_queue_keep_connected` on supersede teardown + in-flight test | **Fixed** |
| A | H2 — 5s handshake I/O timeout, shared `MAX_FRAME`, post-accept spawn | **Fixed** |
| B | `ToolName` enum + exhaustive schema/dispatch | **Fixed** |
| B | `DEFAULT_PORT` / `APP_DIR_NAME` / single `default_data_dir` | **Fixed** |
| B | `bridge/fixtures/limits.json` + Rust/Python parity | **Fixed** |
| B | Heartbeat/timeouts `From<&BridgeSection>` | **Fixed** |
| B | Reserve `tdmcp.bridge.version` (not wired rejection) | **Fixed** (reserved) |
| C | Remove `/admin/history` | **Fixed** |
| C | Daemon HTTP via shared `reqwest` helpers | **Fixed** |
| C | Capture/inspect outcome code whitelist + `build_diag` layer fallback | **Fixed** |
| C | Panic join → `Err` (non-success) | **Fixed** |
| C | Bridge package split (`constants`/`paths`/`execute`/…/`task_queue`) | **Fixed** (`bridge/tests` green) |

Still open from this review (out of A–C scope): M5 pipe handoff race, M6 stdio annotate, M7 dual MCP shaping, optional `codes.rs` codegen, missing `TODO_ENFORCE_TYPE.md` pointer.
