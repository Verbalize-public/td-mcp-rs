"""Helpers to read/write package-level mutable state (monkeypatch-friendly)."""

from __future__ import annotations

import sys
import threading
from types import ModuleType


def _pkg() -> ModuleType:
    return sys.modules[__package__]


def get_bridge_host_path() -> str | None:
    return getattr(_pkg(), "_bridge_host_path", None)


def set_bridge_host_path(path: str | None) -> None:
    _pkg()._bridge_host_path = path


def get_capture_depth() -> int:
    return int(getattr(_pkg(), "_capture_depth", 0))


def set_capture_depth(value: int) -> None:
    _pkg()._capture_depth = value


def mark_main_thread() -> None:
    """Record the calling thread as TD's main thread (session anchor).

    Only call from a path that provably runs on TD's main thread —
    ``bootstrap_threaded`` (driven by onStart / onFrameStart / the td.run
    watchdog). ``is_main_thread`` prefers this mark over
    ``threading.main_thread()`` in case the embedded interpreter's notion of
    the main thread ever diverges from TD's cook thread.
    """
    _pkg()._main_thread_ident = threading.get_ident()


def is_main_thread() -> bool:
    """True on the marked TD main thread (fallback: Python's own main thread).

    An unset mark (tests, non-TD hosts, pre-bootstrap) falls back to
    ``threading.current_thread() is threading.main_thread()`` so plain-Python
    callers keep working.
    """
    ident = getattr(_pkg(), "_main_thread_ident", None)
    if ident is not None:
        return threading.get_ident() == ident
    return threading.current_thread() is threading.main_thread()


def require_main_thread() -> None:
    """Reject unsafe entry before accessing TD objects or native UI streams."""
    if not is_main_thread():
        raise RuntimeError('TD API access requires the main thread; enqueue work for process_pending')
