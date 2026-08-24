"""Main-thread task queue + serve_queued IPC worker loop."""
from __future__ import annotations

import queue
import sys
import threading
import time
import traceback
from dataclasses import dataclass, field
from typing import Any

from .constants import (
    DEFAULT_MAX_CALL_WAIT_S,
    IDLE_DEAD_S,
    _READ_POLL_S,
)
from .identity import (
    _identity_snapshot,
    _td_pid,
    handshake,
    idle_dead_from_handshake,
    max_call_wait_from_handshake,
)
from . import logtap as _logtap
from .transport import (
    MidFrameTimeout,
    _apply_read_timeout,
    _read_frame,
    _write_frame,
    dial,
)


def _dispatch(msg: dict[str, Any]) -> dict[str, Any]:
    """Lazy import avoids package import cycles (HANDLERS live in ``__init__``)."""
    from . import dispatch

    return dispatch(msg)

def serve(stream) -> None:
    """Framed dispatch loop over a connected IPC stream — direct dispatch.

    Runs `dispatch()` (and therefore any `td.*` call inside a handler)
    **on the calling thread**. Only safe when the caller either *is* TD's
    main thread and is fine blocking it (short-lived manual smoke tests), or
    is guaranteed to never touch `op`/`td.project` (never true for our
    handlers). Live TD sessions must use [`serve_queued`] instead.
    """
    while True:
        try:
            msg = _read_frame(stream)
        except EOFError:
            break
        resp = _dispatch(msg)
        _write_frame(stream, resp)


# --- Main-thread-safe serving (worker thread enqueues, Execute DAT drains) --
#
# TD's Python API is only safe to call from the main/cook thread. The IPC
# read is blocking, so it must live on a worker thread — but the worker must
# never call `dispatch()` itself (that would run `td.*` off-thread). Instead
# it enqueues a pending item and blocks on the response slot; a per-frame pump
# on the main thread drains the queue, calls `dispatch()`, and unblocks the
# worker. Only plain dicts and `queue.Queue` objects cross the thread
# boundary — never an `OP`.
#
# The pending list is inspectable for the bootstrap Operator Viewer face
# (`task_snapshot` / `pending_count` / `cancel_queued`).


_SUMMARY_MAX = 36
_SNAPSHOT_MAX = 12


@dataclass
class _PendingItem:
    msg: dict[str, Any]
    response_slot: "queue.Queue[dict[str, Any]]"
    method: str
    summary: str
    enqueued_at: float = field(default_factory=time.monotonic)
    req_id: Any = None
    abandoned: bool = False


_pending_lock = threading.Lock()
_pending: list[_PendingItem] = []
_running: _PendingItem | None = None

# Outbound events (M2 log uplink) for the active connection. Only
# `serve_queued`'s own worker thread ever writes to the stream — other
# threads (e.g. the main-thread pump) enqueue here instead of writing
# directly, so two threads never race `_write_frame` on the same stream.
_event_queue: "queue.Queue[dict[str, Any]] | None" = None


def enqueue_event(msg: dict[str, Any]) -> bool:
    """Queue an outbound event frame for the active connection to send.

    Returns False (silently) when no connection is currently serving —
    callers (e.g. the log uplink) should just drop in that case.
    """
    q = _event_queue
    if q is None:
        return False
    try:
        q.put_nowait(msg)
        return True
    except queue.Full:
        return False


def _drain_outbound(stream, max_items: int = 64) -> bool:
    """Write queued outbound events onto ``stream``. Returns False on the
    first write failure (caller should treat the connection as dead)."""
    q = _event_queue
    if q is None:
        return True
    n = 0
    while n < max_items:
        try:
            msg = q.get_nowait()
        except queue.Empty:
            break
        try:
            _write_frame(stream, msg)
        except Exception:  # noqa: BLE001 — the read loop will observe the dead stream
            return False
        n += 1
    return True


def _deliver_response(item: _PendingItem, resp: dict[str, Any]) -> None:
    """Unblock the worker without hanging if it already timed out."""
    if item.abandoned:
        return
    try:
        item.response_slot.put_nowait(resp)
    except queue.Full:
        pass


def _abandon_pending(req_id: Any) -> None:
    """Remove a queued item or mark the in-flight item abandoned after worker timeout."""
    global _running
    with _pending_lock:
        for i, item in enumerate(_pending):
            if item.req_id == req_id:
                _pending.pop(i)
                item.abandoned = True
                return
        if _running is not None and _running.req_id == req_id:
            _running.abandoned = True


def _short_text(s: str, n: int = _SUMMARY_MAX) -> str:
    s = str(s or "").replace("\n", " ").strip()
    if len(s) <= n:
        return s
    return s[: n - 1] + "~"


def summarize_request(msg: dict[str, Any]) -> str:
    """Short human describe for a bridge IPC request (face / task table)."""
    method = str(msg.get("method") or "")
    params = msg.get("params") or {}
    if not isinstance(params, dict):
        params = {}
    if method == "execute_python":
        script = str(params.get("script") or "")
        for line in script.splitlines():
            line = line.strip()
            if line:
                return _short_text(line)
        return "execute_python"
    if method == "inspect":
        paths = params.get("paths")
        if isinstance(paths, list) and paths:
            if len(paths) == 1:
                return _short_text(str(paths[0]))
            return _short_text(f"inspect×{len(paths)}")
        return _short_text(str(params.get("path") or "inspect"))
    if method == "capture":
        path = str(params.get("path") or "capture")
        mode = str(params.get("mode") or "auto")
        if mode and mode != "auto":
            return _short_text(f"{path} ({mode})")
        return _short_text(path)
    if method == "mutate_nodes":
        steps = params.get("steps") or []
        n = len(steps) if isinstance(steps, list) else 0
        first_op = ""
        if isinstance(steps, list) and steps:
            first = steps[0] if isinstance(steps[0], dict) else {}
            first_op = str(first.get("op") or "")
        if first_op:
            return _short_text(f"{n}× {first_op}")
        return _short_text(f"mutate×{n}")
    if method == "ping":
        return "ping"
    return _short_text(method or "unknown")


def _enqueue_pending(
    msg: dict[str, Any], response_slot: "queue.Queue[dict[str, Any]]"
) -> _PendingItem:
    item = _PendingItem(
        msg=msg,
        response_slot=response_slot,
        method=str(msg.get("method") or ""),
        summary=summarize_request(msg),
        req_id=msg.get("id"),
    )
    with _pending_lock:
        _pending.append(item)
    return item


# Pause-resilient pump state (functions defined after process_pending).
_pump_scheduled: bool = False
_pump_lock = threading.Lock()
_last_schedule: float = 0.0


def _set_pump_scheduled(value: bool) -> None:
    """Update task_queue + package re-export (bools do not share identity)."""
    global _pump_scheduled
    _pump_scheduled = value
    pkg = sys.modules.get("tdmcp_bridge")
    if pkg is not None:
        pkg._pump_scheduled = value


def _reset_pending_for_tests() -> None:
    """Clear pending/running/pump state — test harness only."""
    global _running, _last_schedule
    with _pending_lock:
        _pending.clear()
        _running = None
    with _pump_lock:
        _set_pump_scheduled(False)
        _last_schedule = 0.0
        pkg = sys.modules.get("tdmcp_bridge")
        if pkg is not None:
            pkg._last_schedule = 0.0


def pending_count() -> int:
    """Queued + in-flight items awaiting / receiving main-thread dispatch."""
    with _pending_lock:
        return len(_pending) + (1 if _running is not None else 0)


def task_snapshot() -> list[dict[str, Any]]:
    """Running (0..1) then FIFO queued rows for the Operator Viewer face.

    Caps at ``_SNAPSHOT_MAX`` rows. Age is seconds since enqueue.
    """
    now = time.monotonic()
    rows: list[dict[str, Any]] = []
    with _pending_lock:
        if _running is not None:
            rows.append(
                {
                    "state": "running",
                    "method": _running.method,
                    "summarize": _running.summary,
                    "age_s": round(max(0.0, now - _running.enqueued_at), 1),
                    "id": _running.req_id,
                }
            )
        for item in _pending:
            rows.append(
                {
                    "state": "queued",
                    "method": item.method,
                    "summarize": item.summary,
                    "age_s": round(max(0.0, now - item.enqueued_at), 1),
                    "id": item.req_id,
                }
            )
    return rows[:_SNAPSHOT_MAX]


def cancel_queued() -> int:
    """Fail every **queued** pending item; does not abort in-flight dispatch.

    Returns the number of cancelled items. Each worker blocked on its
    response slot receives an error response so the IPC loop can continue.
    """
    with _pending_lock:
        items = list(_pending)
        _pending.clear()
    for item in items:
        _deliver_response(
            item,
            {
                "type": "response",
                "id": item.req_id,
                "error": {
                    "message": "cancelled",
                    "code": "tdmcp.bridge.cancelled",
                },
            },
        )
    return len(items)


def process_pending(max_items: int = 64) -> int:
    """Drain the request queue and dispatch on the calling thread.

    Call **only** from TD's main thread (an Execute DAT's `onFrameStart`).
    Bounded per call so a burst of requests can't stall a frame indefinitely;
    remaining items are picked up next frame.
    """
    global _running
    n = 0
    while n < max_items:
        with _pending_lock:
            if not _pending:
                break
            item = _pending.pop(0)
            _running = item
        try:
            _deliver_response(item, _dispatch(item.msg))
        except Exception as exc:  # noqa: BLE001 — never let the pump die
            _deliver_response(
                item,
                {
                    "type": "response",
                    "id": item.req_id,
                    "error": {"message": str(exc)},
                },
            )
        finally:
            with _pending_lock:
                if _running is item:
                    _running = None
        n += 1
    return n


# ── Pause-resilient main-thread pump ──────────────────────────────────
#
# When the TD project is paused, onFrameStart does not fire, so
# process_pending is never called — all bridge methods time out at
# maxCallWaitSecs.  This pump uses td.run(delayMilliSeconds=…) with
# delayRef=op.TDResources so the delay follows independent time and
# keeps firing while the root timeline is paused (plain delayMilliSeconds
# alone is rooted at / and does NOT advance while paused).
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

def _td_delay_ref():
    """Independent time COMP so run() delays advance while paused.

    Derivative docs require ``delayRef=op.TDResources`` (Global OP Shortcut).
    ``td.op.TDResources`` is not always exposed the same way as bare ``op``, so
    try several lookups and never silently omit the ref.
    """
    import td  # noqa: F811

    op_fn = getattr(td, "op", None)
    if op_fn is None:
        return None

    # 1) Shortcut attribute on the op finder (same as bare op.TDResources).
    try:
        ref = getattr(op_fn, "TDResources", None)
        if ref is not None:
            return ref
    except Exception:  # noqa: BLE001
        pass

    # 2) Shortcut via op() call patterns used on some builds.
    for getter in (
        lambda: op_fn.TDResources,  # type: ignore[attr-defined]
        lambda: op_fn("/local"),  # may host resources on some installs
    ):
        try:
            ref = getter()
            if ref is not None and getattr(ref, "time", None) is not None:
                return ref
        except Exception:  # noqa: BLE001
            continue

    return None


def _schedule_pump(delay_ms: int) -> None:
    """Schedule ``_pump`` via ``td.run`` with pause-safe delayRef + wallTime."""
    import td  # noqa: F811

    # Import package binding so delayed callable sees the live ``_pump``.
    import tdmcp_bridge as _pkg

    ref = _td_delay_ref()
    kwargs: dict[str, Any] = {
        "delayMilliSeconds": delay_ms,
        # Elapsed-time delay (not frame counting) — pairs with delayRef for pause.
        "wallTime": True,
    }
    if ref is None:
        print(
            "tdmcp-rs: pause pump: op.TDResources unavailable; "
            "run() delay will not advance while paused"
        )
    else:
        kwargs["delayRef"] = ref

    # Callable form (not a string) so we keep the in-process function object.
    td.run(_pkg._pump, **kwargs)


def _pump() -> None:
    """Self-rescheduling main-thread dispatch pump — callable from run()."""
    global _last_schedule

    try:
        process_pending(max_items=4)
    except Exception:  # noqa: BLE001 — one bad dispatch must not kill the pump
        pass

    try:
        _logtap.maybe_flush()
    except Exception:  # noqa: BLE001 — uplink must never kill the pump
        pass

    with _pump_lock:
        import tdmcp_bridge as _pkg

        if not _pkg.is_connected():
            _set_pump_scheduled(False)
            return  # clean stop — do not re-schedule

        now = time.monotonic()
        if now - _last_schedule < 0.050:
            return  # rate-limited — another pump invocation already scheduled
        _last_schedule = now
        try:
            _schedule_pump(50)
        except Exception as exc:  # noqa: BLE001
            print("tdmcp-rs: pause pump reschedule failed:", exc)
            _set_pump_scheduled(False)


def start_pump() -> None:
    """Idempotent start.  Called from bootstrap_threaded after connection."""
    with _pump_lock:
        if _pump_scheduled:
            return
        _set_pump_scheduled(True)
    try:
        _schedule_pump(0)
    except Exception as exc:  # noqa: BLE001 — pre-TD 088 or non-TD environment
        print("tdmcp-rs: start_pump failed:", exc)
        with _pump_lock:
            _set_pump_scheduled(False)


def _close_serve_stream(stream) -> None:
    """Best-effort close so the daemon sees I/O failure instead of idle silence."""
    try:
        if hasattr(stream, "close"):
            stream.close()
    except Exception:  # noqa: BLE001 — teardown must not raise
        pass


def serve_queued(
    stream,
    *,
    idle_dead_s: float = IDLE_DEAD_S,
    max_call_wait_s: float = DEFAULT_MAX_CALL_WAIT_S,
) -> None:
    """Framed dispatch loop, worker-thread-safe for TD API methods.

    ``ping`` is answered on this worker thread (daemon idle heartbeat) so a
    paused timeline cannot look like a dead bridge. Other methods enqueue for
    [`process_pending`] on the main thread.

    Worker waits for the main-thread response up to ``max_call_wait_s``; on
    timeout it writes a structured error and continues so the IPC stream is
    not wedged after the daemon has already timed out the wait.

    Exits on EOF, mid-frame stall (``idle_dead_s`` without byte progress), or
    when no inbound frame arrives for ``idle_dead_s`` (when the stream
    supports read timeouts). Always closes ``stream`` on exit so the peer
    observes an immediate disconnect rather than waiting out heartbeat silence.
    """
    poll = min(_READ_POLL_S, idle_dead_s) if idle_dead_s > 0 else _READ_POLL_S
    wait_s = max_call_wait_s if max_call_wait_s > 0 else DEFAULT_MAX_CALL_WAIT_S
    try:
        stream._mid_frame_dead_s = idle_dead_s if idle_dead_s > 0 else IDLE_DEAD_S
    except Exception:  # noqa: BLE001 — stubs / frozen objects
        pass
    try:
        _apply_read_timeout(stream, poll)
    except Exception:  # noqa: BLE001 — makefile / test stubs may not support it
        pass

    global _event_queue
    my_event_queue: "queue.Queue[dict[str, Any]]" = queue.Queue(maxsize=1024)
    _event_queue = my_event_queue

    last_recv = time.monotonic()
    try:
        while True:
            # Flush any queued outbound events (M2 log uplink) before the next
            # blocking read — keeps all stream writes on this one thread.
            if not _drain_outbound(stream):
                break
            try:
                msg = _read_frame(stream, idle_dead_s=idle_dead_s)
            except MidFrameTimeout:
                # No progress for idle_dead_s mid-frame — stream is stuck/desynced.
                break
            except TimeoutError:
                if idle_dead_s > 0 and (time.monotonic() - last_recv) >= idle_dead_s:
                    break
                continue
            except EOFError:
                break
            except Exception:  # noqa: BLE001 — never kill the daemon thread silently
                # Decode / unexpected stream errors: close cleanly with a trace.
                sys.stderr.write(
                    "tdmcp_bridge: serve_queued stopping after unexpected read error\n"
                )
                traceback.print_exc(file=sys.stderr)
                break
            last_recv = time.monotonic()
            try:
                if msg.get("type") != "request":
                    continue
                # Fast-path liveness — never touch the main-thread queue.
                if msg.get("method") == "ping":
                    _write_frame(stream, _dispatch(msg))
                    continue
                response_slot: "queue.Queue[dict[str, Any]]" = queue.Queue(maxsize=1)
                _enqueue_pending(msg, response_slot)
                try:
                    resp = response_slot.get(timeout=wait_s)
                except queue.Empty:
                    req_id = msg.get("id")
                    _abandon_pending(req_id)
                    resp = {
                        "type": "response",
                        "id": req_id,
                        "error": {
                            "message": (
                                f"main-thread dispatch did not complete within {wait_s:.0f}s "
                                "(paused timeline or hung script); IPC unwedged"
                            ),
                            "code": "tdmcp.bridge.main_thread_timeout",
                        },
                    }
                _write_frame(stream, resp)
            except Exception:  # noqa: BLE001 — never kill the daemon thread silently
                sys.stderr.write(
                    "tdmcp_bridge: serve_queued stopping after dispatch/write error\n"
                )
                traceback.print_exc(file=sys.stderr)
                break
    finally:
        if _event_queue is my_event_queue:
            _event_queue = None
        _close_serve_stream(stream)

