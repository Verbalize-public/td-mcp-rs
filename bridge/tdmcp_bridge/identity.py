"""Handshake identity / fingerprint helpers (main-thread snapshot)."""
from __future__ import annotations

import ctypes
import os
import sys
from typing import Any, TypedDict

from .constants import (
    DEFAULT_MAX_CALL_WAIT_S,
    IDLE_DEAD_S,
    __min_daemon__,
    __protocol_version__,
)
from .transport import _read_frame, _write_frame

class IdentitySnapshot(TypedDict):
    """Plain strings for handshake discovery — capture on the main thread only."""

    title: str | None
    toe_path: str | None
    image: str | None
    start_time: str | None


def compose_toe_path(folder: str | None, name: str | None) -> str | None:
    """Join project folder + name into a toe path when both are non-empty."""
    folder_s = (folder or "").strip()
    name_s = (name or "").strip()
    if folder_s and name_s:
        return os.path.join(folder_s, name_s)
    return None


def _process_image() -> str | None:
    """Best-effort absolute path of the current process image."""
    try:
        if sys.platform.startswith("win"):
            buf = ctypes.create_unicode_buffer(32768)
            n = ctypes.windll.kernel32.GetModuleFileNameW(None, buf, len(buf))
            if n:
                return buf.value or None
    except Exception:  # noqa: BLE001 — best-effort fingerprint
        pass
    return None


def _process_start_time() -> str | None:
    """Best-effort opaque OS process start-time string for pid-reuse fingerprint."""
    try:
        if sys.platform.startswith("win"):

            class _FileTime(ctypes.Structure):
                _fields_ = [
                    ("dwLowDateTime", ctypes.c_uint32),
                    ("dwHighDateTime", ctypes.c_uint32),
                ]

            creation = _FileTime()
            exit_t = _FileTime()
            kernel = _FileTime()
            user = _FileTime()
            handle = ctypes.windll.kernel32.GetCurrentProcess()
            ok = ctypes.windll.kernel32.GetProcessTimes(
                handle,
                ctypes.byref(creation),
                ctypes.byref(exit_t),
                ctypes.byref(kernel),
                ctypes.byref(user),
            )
            if ok:
                val = (int(creation.dwHighDateTime) << 32) | int(
                    creation.dwLowDateTime
                )
                return str(val)
        else:
            # Linux /proc; macOS has no /proc — leave None on failure.
            st = os.stat("/proc/self")
            return str(int(st.st_ctime))
    except Exception:  # noqa: BLE001 — best-effort fingerprint
        pass
    return None


def identity_from_project(
    name: str | None, folder: str | None
) -> tuple[str | None, str | None]:
    """Map TD ``project.name`` / ``project.folder`` → handshake title + toePath."""
    title = (name or "").strip() or None
    toe_path = compose_toe_path(folder, title)
    return title, toe_path


def _identity_snapshot() -> IdentitySnapshot:
    """MAIN THREAD ONLY. Capture project identity + process fingerprint strings.

    Never call from the IPC worker thread. Non-TD environments (smoke/REPL)
    return null title/toePath with best-effort fingerprint fields.
    """
    title: str | None = None
    toe_path: str | None = None
    try:
        import td  # type: ignore

        title, toe_path = identity_from_project(
            str(td.project.name), str(td.project.folder)
        )
    except Exception:  # noqa: BLE001 — outside TD or project unavailable
        pass
    image = _process_image() or "TouchDesigner.exe"
    return {
        "title": title,
        "toe_path": toe_path,
        "image": image,
        "start_time": _process_start_time(),
    }


def handshake(
    stream,
    title: str | None = None,
    toe_path: str | None = None,
    image: str | None = None,
    start_time: str | None = None,
) -> dict[str, Any]:
    """Perform the client side of the IPC handshake over `stream`."""
    req = {
        "pid": _td_pid(),
        "protocolVersion": __protocol_version__,
        "title": title,
        "toePath": toe_path,
        "image": image if image is not None else "TouchDesigner.exe",
        "startTime": start_time,
    }
    _write_frame(stream, req)
    return _read_frame(stream)


def idle_dead_from_handshake(resp: dict[str, Any] | None) -> float:
    """Map handshake ``idleDeadSecs`` to a ``serve_queued`` idle-dead budget.

    Missing / invalid values fall back to [`IDLE_DEAD_S`] so older daemons
    that omit the field remain compatible.
    """
    if not isinstance(resp, dict):
        return IDLE_DEAD_S
    raw = resp.get("idleDeadSecs")
    if raw is None:
        return IDLE_DEAD_S
    try:
        value = float(raw)
    except (TypeError, ValueError):
        return IDLE_DEAD_S
    if value <= 0:
        return IDLE_DEAD_S
    return value


def max_call_wait_from_handshake(resp: dict[str, Any] | None) -> float:
    """Map handshake ``maxCallWaitSecs`` to a worker ``response_slot`` wait.

    Missing / invalid values fall back to [`DEFAULT_MAX_CALL_WAIT_S`].
    """
    if not isinstance(resp, dict):
        return DEFAULT_MAX_CALL_WAIT_S
    raw = resp.get("maxCallWaitSecs")
    if raw is None:
        return DEFAULT_MAX_CALL_WAIT_S
    try:
        value = float(raw)
    except (TypeError, ValueError):
        return DEFAULT_MAX_CALL_WAIT_S
    if value <= 0:
        return DEFAULT_MAX_CALL_WAIT_S
    return value


def _td_pid() -> int:
    try:
        import td  # type: ignore

        return int(td.project.pid)
    except Exception:  # noqa: BLE001
        return os.getpid()

