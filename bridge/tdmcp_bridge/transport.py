"""Wire framing + local IPC transport (named pipe / UDS)."""
from __future__ import annotations

import ctypes
import json
import os
import struct
import sys
import time
from typing import Any

from .constants import IDLE_DEAD_S

class MidFrameTimeout(TimeoutError):
    """No byte progress while a frame was already partially consumed.

    Distinct from a clean idle ``TimeoutError`` (zero bytes at a frame
    boundary). A single short read-poll stall mid-transfer is **not** fatal —
    streams retry until ``IDLE_DEAD_S`` of silence since the last progress.
    After that, the byte stream is assumed stuck/desynced and
    ``serve_queued`` must disconnect rather than ``continue``.
    """

def _read_frame(stream, *, idle_dead_s: float = IDLE_DEAD_S) -> dict[str, Any]:
    """Read one length-prefixed JSON frame.

    Raises:
        EOFError: peer closed / short read that is not a timeout.
        TimeoutError: underlying stream read timed out (idle poll at frame boundary).
        MidFrameTimeout: no byte progress for ``idle_dead_s`` after the header
            (or mid-body) — stream is stuck/desynced.
    """
    try:
        header = stream.read(4)
    except MidFrameTimeout:
        raise
    except TimeoutError:
        raise
    if len(header) < 4:
        if len(header) == 0:
            raise EOFError("short header")
        raise EOFError("short header")
    (length,) = struct.unpack("<I", header)
    # Header consumed ⇒ mid-frame even before any body bytes arrive. Tolerate
    # short poll stalls; only die after idle_dead_s with no progress.
    body = bytearray()
    last_progress = time.monotonic()
    while len(body) < length:
        remaining = length - len(body)
        try:
            chunk = stream.read(remaining)
        except MidFrameTimeout:
            raise
        except TimeoutError as exc:
            if idle_dead_s > 0 and (time.monotonic() - last_progress) >= idle_dead_s:
                raise MidFrameTimeout("timed out mid-frame") from exc
            continue
        if not chunk:
            raise EOFError("short body")
        body += chunk
        last_progress = time.monotonic()
    return json.loads(bytes(body).decode("utf-8"))


def _mid_frame_dead_s(stream, default: float = IDLE_DEAD_S) -> float:
    """Per-stream mid-frame stall budget (set by ``serve_queued``)."""
    value = getattr(stream, "_mid_frame_dead_s", default)
    try:
        return float(value)
    except (TypeError, ValueError):
        return default


def _apply_read_timeout(stream, seconds: float) -> None:
    """Best-effort read timeout for idle polling (UDS / named pipe wrappers)."""
    setter = getattr(stream, "set_read_timeout", None)
    if callable(setter):
        setter(seconds)


def _write_frame(stream, msg: dict[str, Any]) -> None:
    body = json.dumps(msg).encode("utf-8")
    header = struct.pack("<I", len(body))
    n = stream.write(header)
    if n != len(header):
        raise OSError(f"short write: header {n}/{len(header)}")
    n = stream.write(body)
    if n != len(body):
        raise OSError(f"short write: body {n}/{len(body)}")
    stream.flush()

def default_endpoint() -> str:
    """Return the default local IPC endpoint path for this platform."""
    if sys.platform.startswith("win"):
        return r"\\.\pipe\tdmcp-rs"
    import os

    env = os.environ.get("TDMCP_DATA_DIR")
    if env:
        data_dir = env
    elif sys.platform == "darwin":
        # Match daemon `dirs::data_local_dir()` → ~/Library/Application Support.
        data_dir = os.path.join(
            os.path.expanduser("~"), "Library", "Application Support", "tdmcp-rs"
        )
    else:
        data_dir = os.path.join(
            os.environ.get("XDG_DATA_HOME")
            or os.path.join(os.path.expanduser("~"), ".local", "share"),
            "tdmcp-rs",
        )
    return os.path.join(data_dir, "bridge.sock")


def dial(endpoint: str | None = None):
    """Connect to the daemon IPC endpoint. Returns a file-like stream."""
    endpoint = endpoint or default_endpoint()
    if sys.platform.startswith("win"):
        return _dial_named_pipe(endpoint)
    return _dial_uds(endpoint)


def _dial_uds(path: str):
    import socket

    sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    sock.connect(path)
    return _UdsStream(sock)


class _UdsStream:
    """Minimal file-like wrapper over a UDS socket (mirrors `_NamedPipeStream`).

    Unbuffered by design (manual length-prefixed framing already batches
    reads/writes); avoids `makefile()`'s buffering surprises and gives us a
    real socket reference for [`shutdown`], which is the POSIX-supported way
    to unblock a concurrent `recv()` on another thread.
    """

    def __init__(self, sock) -> None:
        self._sock = sock

    def set_read_timeout(self, seconds: float | None) -> None:
        """Socket-level recv timeout for idle polling (`None` = block forever)."""
        self._sock.settimeout(seconds)

    def read(self, n: int) -> bytes:
        import socket as _socket

        out = bytearray()
        last_progress = time.monotonic()
        while len(out) < n:
            try:
                chunk = self._sock.recv(n - len(out))
            except _socket.timeout as exc:
                if not out:
                    raise TimeoutError("uds read timed out") from exc
                if (time.monotonic() - last_progress) >= _mid_frame_dead_s(self):
                    raise MidFrameTimeout(
                        "uds read stalled mid-frame with no progress"
                    ) from exc
                continue
            if not chunk:
                break
            out += chunk
            last_progress = time.monotonic()
        return bytes(out)

    def write(self, data: bytes) -> int:
        self._sock.sendall(data)
        return len(data)

    def flush(self) -> None:  # noqa: D401
        return None

    def close(self) -> None:
        try:
            self._sock.close()
        except OSError:
            pass

    def cancel_pending_io(self, _thread_id: int | None) -> None:
        """Unblock a concurrent `recv()` on another thread before `close()`.

        The thread id is irrelevant on POSIX — `shutdown()` unblocks *any*
        thread reading this socket, unlike Windows `CancelSynchronousIo`
        which must target a specific thread.
        """
        import socket as _socket

        try:
            self._sock.shutdown(_socket.SHUT_RDWR)
        except OSError:
            pass


def _dial_named_pipe(name: str):
    kernel32 = ctypes.windll.kernel32  # type: ignore[attr-defined]
    GENERIC_READ = 0x80000000
    GENERIC_WRITE = 0x40000000
    OPEN_EXISTING = 3
    INVALID = ctypes.c_void_p(-1).value

    handle = kernel32.CreateFileW(
        name,
        GENERIC_READ | GENERIC_WRITE,
        0,
        None,
        OPEN_EXISTING,
        0,
        None,
    )
    if handle in (INVALID, None):
        raise OSError(f"could not open named pipe {name}")
    return _NamedPipeStream(handle)


class _NamedPipeStream:
    """Minimal file-like wrapper over a named pipe handle."""

    _ERROR_TIMEOUT = 1460
    _ERROR_SEM_TIMEOUT = 121
    _ERROR_IO_INCOMPLETE = 996

    def __init__(self, handle: int) -> None:
        from ctypes import wintypes

        self._handle = handle
        self._kernel32 = ctypes.windll.kernel32  # type: ignore[attr-defined]
        self._buf = (ctypes.c_ubyte * 65536)()
        self._wintypes = wintypes
        self._read_timeout_ms: int | None = None

    def set_read_timeout(self, seconds: float | None) -> None:
        """Apply ``SetCommTimeouts`` so ``ReadFile`` can return on idle polls."""
        from ctypes import wintypes

        class _CommTimeouts(ctypes.Structure):
            _fields_ = [
                ("ReadIntervalTimeout", wintypes.DWORD),
                ("ReadTotalTimeoutMultiplier", wintypes.DWORD),
                ("ReadTotalTimeoutConstant", wintypes.DWORD),
                ("WriteTotalTimeoutMultiplier", wintypes.DWORD),
                ("WriteTotalTimeoutConstant", wintypes.DWORD),
            ]

        timeouts = _CommTimeouts()
        if seconds is None:
            self._read_timeout_ms = None
            # Restore blocking defaults (all zero).
            timeouts.ReadIntervalTimeout = 0
            timeouts.ReadTotalTimeoutMultiplier = 0
            timeouts.ReadTotalTimeoutConstant = 0
        else:
            ms = max(1, int(seconds * 1000))
            self._read_timeout_ms = ms
            # Total timeout only (no per-byte multiplier).
            timeouts.ReadIntervalTimeout = 0
            timeouts.ReadTotalTimeoutMultiplier = 0
            timeouts.ReadTotalTimeoutConstant = ms
        timeouts.WriteTotalTimeoutMultiplier = 0
        timeouts.WriteTotalTimeoutConstant = 0
        ok = self._kernel32.SetCommTimeouts(self._handle, ctypes.byref(timeouts))
        if not ok:
            raise OSError("SetCommTimeouts failed")

    def read(self, n: int) -> bytes:
        out = bytearray()
        last_progress = time.monotonic()
        while len(out) < n:
            want = min(n - len(out), len(self._buf))
            read = self._wintypes.DWORD(0)
            ok = self._kernel32.ReadFile(
                self._handle,
                self._buf,
                want,
                ctypes.byref(read),
                None,
            )
            if not ok:
                err = self._kernel32.GetLastError()
                if err in (
                    self._ERROR_TIMEOUT,
                    self._ERROR_SEM_TIMEOUT,
                    self._ERROR_IO_INCOMPLETE,
                ):
                    if not out:
                        raise TimeoutError("named pipe read timed out")
                    if (time.monotonic() - last_progress) >= _mid_frame_dead_s(self):
                        raise MidFrameTimeout(
                            "named pipe read stalled mid-frame with no progress"
                        )
                    continue
                break
            if read.value == 0:
                # Timeout with zero bytes can also surface as success+0.
                if self._read_timeout_ms is not None and not out:
                    raise TimeoutError("named pipe read timed out")
                if self._read_timeout_ms is not None and out:
                    if (time.monotonic() - last_progress) >= _mid_frame_dead_s(self):
                        raise MidFrameTimeout(
                            "named pipe read stalled mid-frame with no progress"
                        )
                    continue
                break
            out += bytes(self._buf[: read.value])
            last_progress = time.monotonic()
        return bytes(out)

    def write(self, data: bytes) -> int:
        """Write all of ``data`` (loop WriteFile — partial writes are normal)."""
        total = 0
        while total < len(data):
            written = self._wintypes.DWORD(0)
            chunk = data[total:]
            ok = self._kernel32.WriteFile(
                self._handle,
                chunk,
                len(chunk),
                ctypes.byref(written),
                None,
            )
            if not ok:
                raise OSError("WriteFile failed")
            if written.value == 0:
                raise OSError("WriteFile wrote 0 bytes")
            total += written.value
        return total

    def flush(self) -> None:  # noqa: D401
        return None

    def close(self) -> None:
        self._kernel32.CloseHandle(self._handle)

    def cancel_pending_io(self, thread_id: int | None) -> None:
        """Unblock a concurrent synchronous `ReadFile` on `thread_id`.

        **Must** be called before [`close`] whenever another thread might be
        mid-blocking-read on this handle: closing a handle out from under a
        pending synchronous `ReadFile` on a *different* thread is undefined
        behavior on Windows (observed: freezes the whole caller thread,
        which — if that thread is TD's main/cook thread reached indirectly
        via a script waiting on `join()` — freezes TD itself). Targets the
        specific thread via `OpenThread` + `CancelSynchronousIo`, which is
        the documented, safe cross-thread cancellation primitive for
        synchronous (non-overlapped) I/O.
        """
        if not thread_id:
            return
        thread_terminate = 0x0001
        handle = self._kernel32.OpenThread(thread_terminate, False, thread_id)
        if not handle:
            return
        try:
            self._kernel32.CancelSynchronousIo(handle)
        except OSError:  # noqa: BLE001 — best-effort; read loop will retry/exit
            pass
        finally:
            self._kernel32.CloseHandle(handle)

