# Pause-Resilient Bridge

Status: **Planned** — not yet implemented.

## Problem

The td-mcp-rs bridge stops responding to daemon tool calls when the TD project is
not playing. Every tool method — `inspect`, `capture`, `mutate_nodes`,
`execute_python`, `api_help`, `editor_context` — times out at
`maxCallWaitSecs` (120 s by default) and returns `BridgeRpcError::Timeout` or
`tdmcp.bridge.main_thread_timeout`. The root cause is **not** that these methods
require cooking (they don't) — it is that the main-thread dispatch pump
(`process_pending`) is gated behind the tox Execute DAT's `onFrameStart`,
which TD only fires when the timeline advances.

### Anatomy of the failure

```text
Daemon                            TD process
──────                            ──────────
BridgeSessions.call()             Worker thread: serve_queued (always alive)
  │                                 │
  │  framed request ────────────▶   │ _read_frame()
  │                                 │ _enqueue_pending()
  │                                 │ block on response_slot (maxCallWaitSecs)
  │                                 │
  │  ◀── timeout ─────────────────  │   NEVER REACHED: onFrameStart
  │                                 │   (fires only when playing)
  │                                 │
  BridgeRpcError::Timeout           │   slot.get() raises queue.Empty
  (session stays up)                │   → "main_thread dispatch did not
                                    │      complete within {wait_s}s"
```

The worker thread is alive and reading. `ping` is answered on the worker
(fast-path at `task_queue.py:347`). Every other method enqueues and blocks on
the response slot. `process_pending()` drains that slot — but only when
`onFrameStart` calls it. **When paused, `onFrameStart` never fires.**

### Scope of the fix

All bridge methods except `ping` require `td.*` access → must run on TD's main
thread. There is no subset that can run off-thread or without `td.*`. The fix
adds a main-thread pump independent of the timeline, plus watchdog reconnection.

---

## Design

### Pump: `run()`-based self-rescheduling main-thread dispatch

`td.run(code, delayMilliSeconds=N)` schedules deferred execution on the main
thread using wall-clock time — it fires regardless of play/pause state.

```text
Main thread (always)
══════════════════════════════════════════════
 onFrameStart ──▶ process_pending()   ◀── still works when playing
 run() pump   ──▶ process_pending()   ◀── covers pause state
```

**Lifecycle:**

1. **Start:** `bootstrap_threaded()` spawns the pump immediately after the
   worker thread starts.
2. **Tick:** Each invocation processes up to 4 queued items, then re-schedules
   itself via `run()` with 50 ms delay (~20 Hz). The low batch-per-tick
   prevents frame stalls.
3. **Idle:** Empty queue → one uncontested lock acquire per tick. Effectively
   zero CPU.
4. **Stop:** When `is_connected()` → False, the chain intentionally does not
   re-schedule. No explicit teardown needed.
5. **Dual-pumping when playing:** `onFrameStart` does the heavy lift (64 items
   per frame). The `run()` pump finds an empty queue and returns instantly.
   Both paths are idempotent and harmless.

**Rate-limit guard against backed-up `run()` bursts:**

After a long `execute_python` blocks the main thread, multiple queued `run()`
calls fire in rapid succession. A `_last_schedule` timestamp with 50 ms minimum
interval prevents each one from scheduling yet another — at most one schedule
per 50 ms.

### Watchdog: paused-mode reconnection

A second `run()`-based function in `tox_callbacks.py` polls connection state
every 2 s. When disconnected with `Connect` and `Autoconnect` enabled, it
triggers `_run_bootstrap()` — the same reconnection logic already proven in
`onFrameStart`. This is a separate, slower loop from the dispatch pump so
reconnection polling doesn't compete with dispatch.

### Four resilience layers

| Layer | Mechanism | Guards against |
|-------|-----------|----------------|
| L1 — Exception guard | `try`/`except` wraps `process_pending()` in `_pump()` | Single bad dispatch method kills the pump |
| L2 — Rate limiting | `_last_schedule` minimum 50 ms interval | Backed-up `run()` burst from main-thread block |
| L3 — Watchdog | `onFrameStart` checks `_pump_scheduled` and restarts if dead | Pump death during play; self-heals on next frame |
| L4 — Daemon unwedge | Worker `response_slot` timeout at `maxCallWaitSecs`; writes structured error; IPC continues | All three bridge-side layers fail simultaneously |

L4 already exists and is tested (`MainThreadWaitTimeoutTest` in
`test_bridge_queue.py`, `call_timeout_does_not_idle_dead_session` in
`bridge_session.rs`). This change adds L1–L3.

---

## Implementation plan

### Phase 1 — `task_queue.py` pump functions

**File:** `bridge/tdmcp_bridge/task_queue.py`

Add after the existing `process_pending()` function (line 278):

```python
# ── Pause-resilient main-thread pump ──────────────────────────────────
#
# When the TD project is paused, onFrameStart does not fire, so
# process_pending is never called — all bridge methods time out at
# maxCallWaitSecs.  This pump uses td.run(delayMilliSeconds=…) to
# self-schedule on wall-clock time, independent of the timeline.
#
# Lifecycle:
#   start_pump()  → bootstrap_threaded() calls it after the worker starts.
#   _pump()       → processes up to 4 items per tick, re-schedules itself
#                    at ~20 Hz (50 ms) as long as is_connected().
#   Stop           → when is_connected() → False, the chain intentionally
#                    does not re-schedule.  No explicit teardown.
#
# Resilience:
#   - Each invocation wraps process_pending() in try/except so a single
#     bad dispatch never kills the pump.
#   - _last_schedule with 50 ms minimum prevents backed-up run() bursts
#     after a long main-thread block (e.g. 30 s execute_python).
#   - _pump_scheduled flag prevents double-scheduling via start_pump().

_pump_scheduled: bool = False
_pump_lock = threading.Lock()
_last_schedule: float = 0.0


def _pump() -> None:
    """Self-rescheduling main-thread dispatch pump — callable from run()."""
    global _pump_scheduled, _last_schedule

    try:
        process_pending(max_items=4)
    except Exception:  # noqa: BLE001 — one bad dispatch must not kill the pump
        pass

    with _pump_lock:
        if not is_connected():
            _pump_scheduled = False
            return  # clean stop — do not re-schedule

        now = time.monotonic()
        if now - _last_schedule < 0.050:
            return  # rate-limited — another pump invocation already scheduled
        _last_schedule = now
        try:
            import td  # noqa: F811
            td.run(
                "import tdmcp_bridge; tdmcp_bridge._pump()",
                delayMilliSeconds=50,
            )
        except Exception:  # noqa: BLE001
            _pump_scheduled = False


def start_pump() -> None:
    """Idempotent start.  Called from bootstrap_threaded after connection."""
    global _pump_scheduled
    with _pump_lock:
        if _pump_scheduled:
            return
        _pump_scheduled = True
    try:
        import td  # noqa: F811
        td.run(
            "import tdmcp_bridge; tdmcp_bridge._pump()",
            delayMilliSeconds=0,
        )
    except Exception:  # noqa: BLE001 — pre-TD 088 or non-TD environment
        with _pump_lock:
            _pump_scheduled = False
```

### Phase 2 — `__init__.py` integration

**File:** `bridge/tdmcp_bridge/__init__.py`

In `bootstrap_threaded()`, after `thread.start()` (line 332), add:

```python
    pkg._active_stream = stream
    pkg._active_thread = thread
    # Start the timeline-independent dispatch pump so the bridge works
    # even when the TD project is paused.
    pkg.start_pump()
    return resp
```

### Phase 3 — `tox_callbacks.py` watchdog

**File:** `bridge/tox_callbacks.py`

**3a — Pump watchdog in `onFrameStart` Connected path (line 651):**

After `_phase = "Connected"` and before the existing `mod.process_pending()`, add:

```python
    # Watchdog: if the paused-mode pump died (e.g. exception in _pump),
    # resurrect it while playing.  When paused this check never runs, but
    # L1 (exception guard in _pump) handles that case.
    try:
        if mod is not None and not getattr(mod, '_pump_scheduled', True):
            fn = getattr(mod, 'start_pump', None)
            if callable(fn):
                fn()
    except Exception:  # noqa: BLE001
        pass
```

**3b — Paused-mode reconnection watchdog (new function, after `onExit`):**

```python
_reconnect_watchdog_scheduled = False

def _reconnect_watchdog() -> None:
    """Wall-clock reconnection poll — works when the timeline is paused.

    When the project is not playing, onFrameStart never fires and the
    bridge can neither reconnect nor detect disconnection.  This run()-
    based loop polls every 2 s and triggers the same _run_bootstrap()
    path that onFrameStart uses.
    """
    global _reconnect_watchdog_scheduled
    comp = _comp()
    want = _par_bool(comp, "Connect", True)
    auto = _par_bool(comp, "Autoconnect", True)
    mod = _bridge_mod()
    connected = mod is not None and _bridge_connected(mod)

    if not connected and want and auto:
        _run_bootstrap()

    if want:  # keep polling while Connect is on
        try:
            import td
            td.run(
                "__import__('tdmcp_bridge')._reconnect_watchdog()",
                delayMilliSeconds=2000,
            )
        except Exception:  # noqa: BLE001
            _reconnect_watchdog_scheduled = False
        else:
            _reconnect_watchdog_scheduled = True
    else:
        _reconnect_watchdog_scheduled = False
```

**3c — Start the reconnection watchdog in `onStart` (line 565):**

After `ensure_ui(comp)`, add:

```python
    if not _reconnect_watchdog_scheduled:
        _reconnect_watchdog()
```

### Phase 4 — `RISKS.md` entry

**File:** `RISKS.md`

Add row:

```markdown
| R6 | `bridge/tdmcp_bridge/task_queue.py` (`_pump`) | `run()`-based self-rescheduling main-thread pump for pause resilience. If `_pump()` dies (uncaught exception in `_pump` itself, not in `process_pending` which has its own guard), the pump stops — identical to the current pause bug. `onFrameStart` watchdog (L3) catches this when playing. When paused, a pump-stopping exception in `_pump`'s own non-`process_pending` code (e.g. a crash in the `import td` / `td.run` calls) leaves the pump dead. | `_pump`'s non-dispatch code is trivial (import + run). `td.run()` has existed since TD 088 and does not throw in normal operation. The daemon-side unwedge (L4) remains the ultimate safety net. | 2026-10-15 |
```

### Phase 5 — Tests

**File:** `bridge/tests/test_bridge_queue.py`

New test class: `PumpTest` (pure Python, no TD needed — mocks `td.run`).

| Test | What it validates |
|------|-------------------|
| `test_pump_processes_and_reschedules` | `_pump()` drains the queue, calls `td.run` with 50 ms delay while connected |
| `test_pump_stops_when_disconnected` | `_pump()` unsets `_pump_scheduled` and stops calling `td.run` when `is_connected()` → False |
| `test_pump_survives_dispatch_exception` | A `ValueError` from inside `process_pending()` does not kill `_pump()` or prevent re-schedule |
| `test_start_pump_idempotent` | Double `start_pump()` → one pump chain, not two |
| `test_pump_rate_limits_burst` | 100 rapid `_pump()` calls → `td.run` called at most once per 50 ms |
| `test_start_pump_no_td_module` | `start_pump()` with no `td` module → sets `_pump_scheduled = False`, no crash |

Test helper: inject a fake `td` module onto `tdmcp_bridge`:

```python
class _FakeTd:
    run_calls: list[tuple[str, int]] = []

    @staticmethod
    def run(code: str, delayMilliSeconds: int = 0) -> None:
        _FakeTd.run_calls.append((code, delayMilliSeconds))


def setUp(self) -> None:
    tdmcp_bridge._reset_pending_for_tests()
    tdmcp_bridge._pump_scheduled = False
    tdmcp_bridge._last_schedule = 0.0
    _FakeTd.run_calls.clear()
```

**File:** `crates/tdmcp-daemon/tests/bridge_session.rs`

Existing tests already cover the daemon-side timeout + unwedge behavior
(`call_timeout_does_not_idle_dead_session`, `timeout_does_not_desync_next_call`,
`superseding_while_in_flight_clears_queue_for_exclusive`). No new daemon tests
needed — this change is purely bridge-side.

### Manual integration tests (needs live TD)

| Test | Procedure | Expected |
|------|-----------|----------|
| Pause then inspect | Pause TD, send `inspect` via MCP | Response < 1 s |
| Pause burst | Pause TD, fire 20 MCP calls | All complete |
| Pause→play | Pause, queue calls, hit play | No double-processing; no lost calls |
| Long execute_python | 30 s `execute_python` while paused | Pump resumes after, no run-burst |
| Pump death + watchdog | Inject exception in `_pump`, hit play | `onFrameStart` restarts pump |
| Reconnect while paused | Kill daemon, restart daemon | Bridge reconnects within ~2 s while paused |
| 1-hour paused idle | Leave paused overnight | Pump alive, no CPU creep |

---

## Alternatives considered

### Timer CHOP + CHOP Execute DAT

A Timer CHOP with `timetype="System Time"` cooks independent of the timeline.
A CHOP Execute DAT triggered by its output fires every cook.

**Pro:** TD-native delivery guarantee — not a Python chain that can break.
Survives exceptions — TD re-invokes per cook.

**Con:** Requires tox changes (new operators, wiring to Null CHOP → Execute DAT
to ensure evaluation). Two code paths to maintain (frame Execute DAT + CHOP
Execute DAT). Harder to test outside TD. More surface area.

**Verdict:** `run()` is simpler and more testable. The Timer CHOP is a clean
fallback if `run()` chain death proves to be a real concern in practice.

### Worker-side push (each item → `run()`)

Instead of polling, the worker pushes each item to the main thread via
`td.run(code, delayMilliSeconds=0)`.

**Con:** No batching — 100 items = 100 `run()` calls. Competes with
`onFrameStart` batching when playing. Needs global request-id registry on the
bridge side.

**Verdict:** Poll is the natural shape — batch processing is already what
`process_pending(max_items=N)` does. Polling at 20 Hz is effectively
zero-overhead at idle.

### Daemon-side auto-retry

If the daemon detected `main_thread_timeout`, it could increase tolerance and
retry with backoff.

**Verdict:** Complementary to the pump fix, not an alternative. Improves MCP
client experience but doesn't make the bridge functional while paused. Can be
added later as a daemon-side enhancement.

---

## Non-goals (out of scope)

- Splitting methods into main-thread vs non-main-thread queues. Every useful
  bridge method requires `td.*` access; there is no safe off-thread subset
  beyond the existing `ping` fast-path.
- Auto-starting a paused project. The pump makes the bridge work while paused;
  it does not change TD's play state.
- Changing the daemon's call-timeout budget. The 45 s / 120 s split remains.

---

## Changelog

| Date | Note |
|------|------|
| 2026-10-15 | Initial plan — architecture, resilience model, implementation phases |