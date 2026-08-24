"""Global stdout/stderr tee → batched log uplink to the daemon (M2).

Installs a write-through ``Tee`` over ``sys.stdout`` / ``sys.stderr`` so every
``print`` anywhere in the process (bridge internals, user scripts, whatever
prints "tdmcp-rs: ...") is captured as a structured record *and* still reaches
the Textport exactly as before. Records batch in memory and flush via a
caller-supplied callback — wired to :func:`tdmcp_bridge.task_queue.enqueue_event`
so the actual wire write happens on the connection's own thread (see
``task_queue.py`` — never write the IPC stream from two threads at once).

Mirrors the ``execute.py:47`` discipline: a failing capture/flush never
propagates and the original stream is always written first.
"""

from __future__ import annotations

import contextlib
import io
import sys
import threading
import time
from typing import Any, Callable

_LOG_QUEUE_MAX = 256  # lines before drop-oldest
_BATCH_LINES = 32
_BATCH_INTERVAL_S = 0.5

_lock = threading.Lock()
_buffer: list[dict[str, Any]] = []
_dropped = 0
_last_flush = 0.0
_on_flush: Callable[[list[dict[str, Any]]], None] | None = None
# M3: fires synchronously on every captured record (uplink-buffered or not,
# via append_local) — the face LOGS mirror hooks in here. Kept optional and
# TD-agnostic: this module never imports `td`; the TD-aware caller (M3 wires
# it in tdmcp_bridge/__init__.py) decides what "local" means.
_on_local: Callable[[dict[str, Any]], None] | None = None
_orig_stdout: Any = None
_orig_stderr: Any = None
_suppress_depth = 0


class Tee(io.TextIOBase):
    """Write-through wrapper: writes go to ``original`` first, then get
    buffered as a log record. Never raises."""

    # Duck-typed marker, not `isinstance` — TD reloads this module on every
    # reconnect, which rebuilds the `Tee` class as a new object; an identity
    # check would then treat a still-installed tee from the prior connection
    # as "not ours" and wrap it again, nesting one layer per reconnect.
    _is_tdmcp_logtap_tee = True

    def __init__(self, original: Any, level: str, target: str) -> None:
        self._original = original
        self._level = level
        self._target = target

    def write(self, s: str) -> int:
        try:
            self._original.write(s)
        except Exception:  # noqa: BLE001 — original stream must not break capture
            pass
        try:
            if _suppress_depth == 0 and s and s.strip():
                _append(s.rstrip("\n"), self._level, self._target)
        except Exception:  # noqa: BLE001 — capture must not break the caller's print
            pass
        return len(s)

    def flush(self) -> None:
        flush = getattr(self._original, "flush", None)
        if callable(flush):
            try:
                flush()
            except Exception:  # noqa: BLE001
                pass

    def isatty(self) -> bool:
        try:
            return bool(self._original.isatty())
        except Exception:  # noqa: BLE001
            return False

    def __getattr__(self, name: str) -> Any:
        return getattr(self._original, name)


def _append(msg: str, level: str, target: str) -> None:
    global _dropped
    record = {"level": level, "target": target, "msg": msg}
    with _lock:
        if len(_buffer) >= _LOG_QUEUE_MAX:
            _buffer.pop(0)
            _dropped += 1
        _buffer.append(record)
    on_local = _on_local
    if on_local is not None:
        try:
            on_local(record)
        except Exception:  # noqa: BLE001 — a local-mirror failure must not break capture
            pass


def append_local(msg: str, level: str = "info", target: str = "bridge") -> None:
    """Direct entry for internal bridge prints ('tdmcp-rs: ...').

    Keeps the Textport print at the call site (this only feeds the uplink
    buffer) — callers should still ``print()`` themselves.
    """
    _append(msg, level, target)


def _unwrap_stale_tee(stream: Any) -> Any:
    """Peel off a tee left by an *earlier* reload generation.

    TD reloads this module on every reconnect (``_load_bridge_package``),
    which re-executes the module body and rebinds ``_buffer``/``_on_flush``/
    ``_append`` to fresh objects — but a ``Tee`` instance already sitting in
    ``sys.stdout`` from a *previous* generation keeps its old bound method
    referencing the *old* globals dict (reload reuses the module object, but
    only for modules still reached through ``sys.modules`` at reload time;
    an instance holds its class's original ``__globals__` regardless). Left
    in place, it silently writes into an orphaned buffer nothing ever
    flushes — the tee looks installed but the uplink is dead. Unwrap down to
    the true non-tee original so a fresh, correctly-bound ``Tee`` can be
    installed on every call.
    """
    seen = 0
    while getattr(stream, "_is_tdmcp_logtap_tee", False) and seen < 8:
        inner = getattr(stream, "_original", None)
        if inner is None:
            break
        stream = inner
        seen += 1
    return stream


def install(
    on_flush: Callable[[list[dict[str, Any]]], None],
    on_local: Callable[[dict[str, Any]], None] | None = None,
) -> bool:
    """Replace ``sys.stdout`` / ``sys.stderr`` with fresh :class:`Tee`
    instances, unwrapping any stale tee from an earlier reload generation
    first (see :func:`_unwrap_stale_tee`) — always safe to call again, never
    nests, and always rebinds to *this* generation's buffer/flush state.
    """
    global _orig_stdout, _orig_stderr, _on_flush, _on_local
    _on_flush = on_flush
    _on_local = on_local
    _orig_stdout = _unwrap_stale_tee(sys.stdout)
    sys.stdout = Tee(_orig_stdout, "info", "bridge::stdout")
    _orig_stderr = _unwrap_stale_tee(sys.stderr)
    sys.stderr = Tee(_orig_stderr, "error", "bridge::stderr")
    return True


def is_installed() -> bool:
    """Whether :func:`install` has run (a flush target is bound)."""
    return _on_flush is not None


@contextlib.contextmanager
def suppress():
    """Stand the tee down for the block (execute_python's own capture window
    swaps streams itself; avoids double-capturing the same lines)."""
    global _suppress_depth
    _suppress_depth += 1
    try:
        yield
    finally:
        _suppress_depth -= 1


def maybe_flush(force: bool = False) -> None:
    """Drain the buffer to ``on_flush`` when due (batch size / interval / forced)."""
    global _last_flush, _dropped
    now = time.monotonic()
    with _lock:
        if not _buffer:
            return
        due = force or len(_buffer) >= _BATCH_LINES or (now - _last_flush) >= _BATCH_INTERVAL_S
        if not due:
            return
        records = _buffer[:]
        _buffer.clear()
        dropped = _dropped
        _dropped = 0
        _last_flush = now
    if dropped:
        records.append(
            {
                "level": "warn",
                "target": "bridge::logtap",
                "msg": f"dropped {dropped} log lines (uplink queue full)",
            }
        )
    on_flush = _on_flush
    if on_flush is None:
        return
    try:
        on_flush(records)
    except Exception:  # noqa: BLE001 — uplink failure must never propagate
        pass


def _reset_for_tests() -> None:
    """Clear all module state — test harness only."""
    global _dropped, _last_flush, _on_flush, _on_local, _orig_stdout, _orig_stderr
    global _suppress_depth
    with _lock:
        _buffer.clear()
        _dropped = 0
        # "Just flushed" rather than epoch 0 — the latter makes the very
        # next `maybe_flush()` due immediately regardless of batch size,
        # which is fine for a fresh process but would make interval tests
        # here nondeterministic against real wall-clock timing.
        _last_flush = time.monotonic()
    _on_flush = None
    _on_local = None
    _orig_stdout = None
    _orig_stderr = None
    _suppress_depth = 0
