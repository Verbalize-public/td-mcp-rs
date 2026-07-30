"""td-mcp-rs bridge package — loaded by TD after handshake FS path.

Entry: connect over local IPC is owned by the bootstrap tox; this package
owns the session + RPC (execute_python, capture, inspect helpers).
"""

from __future__ import annotations

import ctypes
import importlib
import json
import os
import queue
import struct
import sys
import threading
import time
import traceback
from dataclasses import dataclass, field
from typing import Any, Callable, NotRequired, TypedDict

__protocol_version__ = "1"
__min_daemon__ = "0.1.0"

# Idle liveness — must match tdmcp-daemon HeartbeatConfig::production.
HEARTBEAT_INTERVAL_S = 5.0
PONG_TIMEOUT_S = 5.0
IDLE_DEAD_S = 15.0
# Short poll so serve_queued can notice IDLE_DEAD without blocking forever.
_READ_POLL_S = 1.0

# Wire method names — must match tdmcp_core::BridgeMethod::wire_str() exactly.
# Parity gated by bridge/tests/test_bridge_methods.py + fixtures/bridge_methods.json.
BRIDGE_METHODS: tuple[str, ...] = ("execute_python", "capture", "inspect", "ping")


class ExecutePythonParams(TypedDict):
    script: str
    contextPath: NotRequired[str | None]


class CaptureParams(TypedDict):
    path: str
    mode: NotRequired[str]
    contextPath: NotRequired[str | None]


class InspectParams(TypedDict):
    path: str
    contextPath: NotRequired[str | None]
    include: NotRequired[list[str]]
    detailLevel: NotRequired[str]


class BridgeOkResult(TypedDict):
    ok: bool
    result: NotRequired[Any]


class BridgeErrResult(TypedDict):
    ok: bool
    error: NotRequired[str]
    message: NotRequired[str]
    code: NotRequired[str]
    traceback: NotRequired[str]


def _read_frame(stream) -> dict[str, Any]:
    """Read one length-prefixed JSON frame.

    Raises:
        EOFError: peer closed / short read that is not a timeout.
        TimeoutError: underlying stream read timed out (idle poll).
    """
    try:
        header = stream.read(4)
    except TimeoutError:
        raise
    if len(header) < 4:
        if len(header) == 0:
            raise EOFError("short header")
        raise EOFError("short header")
    (length,) = struct.unpack("<I", header)
    try:
        body = stream.read(length)
    except TimeoutError as exc:
        raise TimeoutError("timed out mid-frame") from exc
    if len(body) < length:
        raise EOFError("short body")
    return json.loads(body.decode("utf-8"))


def _apply_read_timeout(stream, seconds: float) -> None:
    """Best-effort read timeout for idle polling (UDS / named pipe wrappers)."""
    setter = getattr(stream, "set_read_timeout", None)
    if callable(setter):
        setter(seconds)


def _write_frame(stream, msg: dict[str, Any]) -> None:
    body = json.dumps(msg).encode("utf-8")
    stream.write(struct.pack("<I", len(body)))
    stream.write(body)
    stream.flush()


def tdmcp_resolve(path: str, context_path: str | None = None):
    """Optional OpPath helper for execute_python scripts."""
    import td  # type: ignore

    if path.startswith("/"):
        return td.op(path)
    base = context_path or "/project1"
    return td.op(base).op(path) if td.op(base) is not None else td.op(path)


def handle_execute_python(params: dict[str, Any]) -> dict[str, Any]:
    script = params.get("script") or ""
    context_path = params.get("contextPath")
    local_vars: dict[str, Any] = {
        "__tdmcp_context_path__": context_path,
        "tdmcp_resolve": lambda p: tdmcp_resolve(p, context_path),
        "result": None,
    }
    try:
        exec(script, local_vars, local_vars)  # noqa: S102 — intentional TD script surface
        return {"result": local_vars.get("result"), "ok": True}
    except Exception as exc:  # noqa: BLE001 — surface to diagnostics
        return {
            "ok": False,
            "error": str(exc),
            "traceback": traceback.format_exc(),
        }


def handle_capture(params: dict[str, Any]) -> dict[str, Any]:
    """P0 capture: top / preview — requires live TD."""
    import td  # type: ignore

    path = params.get("path") or ""
    mode = params.get("mode") or "auto"
    context_path = params.get("contextPath")
    node = tdmcp_resolve(path, context_path)
    if node is None or not getattr(node, "valid", False):
        return {"ok": False, "code": "tdmcp.op.not_found", "path": path}

    target = node
    if mode in ("preview", "auto") and hasattr(node, "par"):
        # Fallback chain: opviewer → ./out1 → first TOP child
        opviewer = getattr(node.par, "opviewer", None)
        if opviewer is not None and getattr(opviewer, "eval", None):
            try:
                ref = opviewer.eval()
                if ref:
                    target = td.op(ref) or target
            except Exception:  # noqa: BLE001
                pass
        if target is node:
            child = node.op("out1")
            if child is not None:
                target = child

    # Pixel capture is TD-version specific; return a structured stub when
    # saveByteArray is unavailable so daemon diagnostics can classify.
    if not hasattr(target, "saveByteArray"):
        return {
            "ok": False,
            "code": "tdmcp.perception.no_path",
            "message": "no TOP saveByteArray on resolved path",
            "path": getattr(target, "path", path),
        }

    try:
        data = target.saveByteArray(".jpg")
        black = _is_black_top(target, data)
        return {
            "ok": not black,
            "code": "tdmcp.perception.black_frame" if black else None,
            "bytes": len(data) if data is not None else 0,
            "path": getattr(target, "path", path),
        }
    except Exception as exc:  # noqa: BLE001
        return {"ok": False, "error": str(exc), "traceback": traceback.format_exc()}


_BLACK_MEAN_THRESHOLD = 1.0 / 255.0


def _is_black_top(target, data: bytes | None) -> bool:
    """Real pixel-based black check via `TOP.numpyArray`, byte-size as fallback.

    `saveByteArray`'s encoded JPEG size is not a reliable black-frame signal —
    solid colors of *any* value compress to a similarly tiny size (verified:
    a solid-white and solid-black 256x256 Constant TOP both encode to the same
    byte count). `numpyArray()` gives real RGB(A) samples; mean over the RGB
    channels near zero is the actual "black" signal. Falls back to the old
    tiny-file heuristic only when `numpyArray` isn't available on this target
    (e.g. very old TD builds) or genuinely produced no bytes.
    """
    if hasattr(target, "numpyArray"):
        try:
            arr = target.numpyArray(delayed=False)
            if arr is not None and arr.size:
                channels = min(3, arr.shape[-1]) if arr.ndim >= 3 else 1
                sample = arr[..., :channels] if arr.ndim >= 3 else arr
                return bool(sample.mean() <= _BLACK_MEAN_THRESHOLD)
        except Exception:  # noqa: BLE001 — fall through to byte-size heuristic
            pass
    if not data:
        return True
    return len(data) < 200


def handle_inspect(params: dict[str, Any]) -> dict[str, Any]:
    """Structural subtree read (nodes/params/errors). Requires live TD."""
    import td  # type: ignore

    path = params.get("path") or ""
    context_path = params.get("contextPath")
    include = params.get("include") or []
    detail_level = params.get("detailLevel") or "summary"

    node = tdmcp_resolve(path, context_path)
    if node is None or not getattr(node, "valid", False):
        return {"ok": False, "code": "tdmcp.op.not_found", "path": path}

    want_nodes = "nodes" in include or not include
    want_params = "params" in include
    want_errors = "errors" in include

    def summarize(n) -> dict[str, Any]:
        children = []
        if want_nodes:
            for child in n.children:  # TD OP.children is a list property
                children.append({
                    "path": child.path,
                    "family": getattr(child, "family", None),
                    "opType": getattr(child, "opType", None),
                })
        out: dict[str, Any] = {
            "path": n.path,
            "family": getattr(n, "family", None),
            "opType": getattr(n, "opType", None),
            "children": children if detail_level == "detailed" else len(children),
        }
        if want_params:
            pars = []
            for p in n.pars():
                try:
                    pars.append({"name": p.name, "val": p.eval()})
                except Exception:  # noqa: BLE001
                    pars.append({"name": p.name, "val": None})
            out["params"] = pars
        if want_errors:
            errs = []
            try:
                errs = list(n.errors())
            except Exception:  # noqa: BLE001
                errs = []
            out["errors"] = errs
        return out

    try:
        return {"ok": True, "node": summarize(node)}
    except Exception as exc:  # noqa: BLE001
        return {"ok": False, "error": str(exc), "traceback": traceback.format_exc()}


HANDLERS: dict[str, Callable[[dict[str, Any]], dict[str, Any]]] = {
    "execute_python": handle_execute_python,
    "capture": handle_capture,
    "inspect": handle_inspect,
    "ping": lambda _p: {"ok": True, "pong": True},
}


def dispatch(msg: dict[str, Any]) -> dict[str, Any]:
    if msg.get("type") != "request":
        return {"type": "response", "id": msg.get("id"), "error": {"message": "not a request"}}
    method = msg.get("method") or ""
    params = msg.get("params") or {}
    handler = HANDLERS.get(method)
    if handler is None:
        return {
            "type": "response",
            "id": msg.get("id"),
            "error": {"message": f"unknown method: {method}"},
        }
    result = handler(params)
    return {"type": "response", "id": msg.get("id"), "result": result}


def main() -> None:
    """Stdio framed loop for local debugging (tox uses named pipe / UDS)."""
    while True:
        try:
            msg = _read_frame(sys.stdin.buffer)
        except EOFError:
            break
        resp = dispatch(msg)
        _write_frame(sys.stdout.buffer, resp)


# --- Live IPC client (named pipe on Windows, UDS on Unix) -------------------


def default_endpoint() -> str:
    """Return the default local IPC endpoint path for this platform."""
    if sys.platform.startswith("win"):
        return r"\\.\pipe\tdmcp-rs"
    import os

    data_dir = os.environ.get("TDMCP_DATA_DIR") or os.path.join(
        os.path.expanduser("~"), ".local", "share", "tdmcp-rs"
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
        while len(out) < n:
            try:
                chunk = self._sock.recv(n - len(out))
            except _socket.timeout as exc:
                if not out:
                    raise TimeoutError("uds read timed out") from exc
                raise TimeoutError("uds read timed out mid-frame") from exc
            if not chunk:
                break
            out += chunk
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
                    raise TimeoutError("named pipe read timed out mid-frame")
                break
            if read.value == 0:
                # Timeout with zero bytes can also surface as success+0.
                if self._read_timeout_ms is not None and not out:
                    raise TimeoutError("named pipe read timed out")
                break
            out += bytes(self._buf[: read.value])
        return bytes(out)

    def write(self, data: bytes) -> int:
        written = self._wintypes.DWORD(0)
        ok = self._kernel32.WriteFile(
            self._handle,
            data,
            len(data),
            ctypes.byref(written),
            None,
        )
        if not ok:
            raise OSError("WriteFile failed")
        return written.value

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


def _td_pid() -> int:
    try:
        import td  # type: ignore

        return int(td.project.pid)
    except Exception:  # noqa: BLE001
        return os.getpid()


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
        resp = dispatch(msg)
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


_pending_lock = threading.Lock()
_pending: list[_PendingItem] = []
_running: _PendingItem | None = None


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
        return _short_text(str(params.get("path") or "inspect"))
    if method == "capture":
        path = str(params.get("path") or "capture")
        mode = str(params.get("mode") or "auto")
        if mode and mode != "auto":
            return _short_text(f"{path} ({mode})")
        return _short_text(path)
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


def _reset_pending_for_tests() -> None:
    """Clear pending/running state — test harness only."""
    global _running
    with _pending_lock:
        _pending.clear()
        _running = None


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
        try:
            item.response_slot.put(
                {
                    "type": "response",
                    "id": item.req_id,
                    "error": {
                        "message": "cancelled",
                        "code": "tdmcp.bridge.cancelled",
                    },
                }
            )
        except Exception:  # noqa: BLE001 — best-effort unblock
            pass
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
            item.response_slot.put(dispatch(item.msg))
        except Exception as exc:  # noqa: BLE001 — never let the pump die
            item.response_slot.put(
                {
                    "type": "response",
                    "id": item.req_id,
                    "error": {"message": str(exc)},
                }
            )
        finally:
            with _pending_lock:
                if _running is item:
                    _running = None
        n += 1
    return n


def serve_queued(stream, *, idle_dead_s: float = IDLE_DEAD_S) -> None:
    """Framed dispatch loop, worker-thread-safe for TD API methods.

    ``ping`` is answered on this worker thread (daemon idle heartbeat) so a
    paused timeline cannot look like a dead bridge. Other methods enqueue for
    [`process_pending`] on the main thread.

    Exits on EOF, or when no inbound frame arrives for ``idle_dead_s`` (when
    the stream supports read timeouts).
    """
    poll = min(_READ_POLL_S, idle_dead_s) if idle_dead_s > 0 else _READ_POLL_S
    try:
        _apply_read_timeout(stream, poll)
    except Exception:  # noqa: BLE001 — makefile / test stubs may not support it
        pass

    last_recv = time.monotonic()
    while True:
        try:
            msg = _read_frame(stream)
        except TimeoutError:
            if idle_dead_s > 0 and (time.monotonic() - last_recv) >= idle_dead_s:
                break
            continue
        except EOFError:
            break
        last_recv = time.monotonic()
        if msg.get("type") != "request":
            continue
        # Fast-path liveness — never touch the main-thread queue.
        if msg.get("method") == "ping":
            _write_frame(stream, dispatch(msg))
            continue
        response_slot: "queue.Queue[dict[str, Any]]" = queue.Queue(maxsize=1)
        _enqueue_pending(msg, response_slot)
        resp = response_slot.get()
        _write_frame(stream, resp)


def _env_bridge_dir() -> str | None:
    env = os.environ.get("TDMCP_BRIDGE_DIR")
    if env and os.path.isfile(os.path.join(env, "tdmcp_bridge", "__init__.py")):
        return env
    return None


def _conventional_bridge_dir() -> str | None:
    if sys.platform.startswith("win"):
        base = os.environ.get("LOCALAPPDATA") or os.path.expanduser("~")
        data = os.path.join(base, "tdmcp-rs")
    elif sys.platform == "darwin":
        data = os.path.join(
            os.path.expanduser("~"), "Library", "Application Support", "tdmcp-rs"
        )
    else:
        base = os.environ.get("XDG_DATA_HOME") or os.path.join(
            os.path.expanduser("~"), ".local", "share"
        )
        data = os.path.join(base, "tdmcp-rs")
    candidate = os.path.join(data, "bridge")
    if os.path.isfile(os.path.join(candidate, "tdmcp_bridge", "__init__.py")):
        return candidate
    return None


def _resolve_bridge_package_dir(
    bridge_dir: str | None, handshake_resp: dict[str, Any]
) -> str | None:
    """Path order: explicit/env override → handshake → conventional data dir."""
    if bridge_dir and os.path.isfile(
        os.path.join(bridge_dir, "tdmcp_bridge", "__init__.py")
    ):
        return bridge_dir
    env = _env_bridge_dir()
    if env:
        return env
    hs = handshake_resp.get("bridgePackageDir")
    if isinstance(hs, str) and os.path.isfile(
        os.path.join(hs, "tdmcp_bridge", "__init__.py")
    ):
        return hs
    return _conventional_bridge_dir()


def _load_bridge_package(pkg_dir: str | None) -> None:
    """Put ``pkg_dir`` on ``sys.path`` and reload ``tdmcp_bridge`` from disk."""
    if not pkg_dir:
        return
    if pkg_dir not in sys.path:
        sys.path.insert(0, pkg_dir)
    mod = sys.modules.get("tdmcp_bridge")
    if mod is not None:
        importlib.reload(mod)


def bootstrap(bridge_dir: str | None = None) -> dict[str, Any]:
    """Dial the daemon, handshake, load the bridge package, and serve.

    Blocks the calling thread for the lifetime of the connection and
    dispatches directly (see [`serve`]) — do **not** call this from a live TD
    session (use [`bootstrap_threaded`] there). Useful for manual/non-TD
    smoke tests (e.g. a plain Python REPL, or a script talking to a stub
    peer in tests).

    Path resolution: ``bridge_dir`` / ``TDMCP_BRIDGE_DIR`` → handshake
    ``bridgePackageDir`` → conventional data-dir ``bridge/``. Reloads the
    package from disk on every connect.
    Returns the handshake response.
    """
    snap = _identity_snapshot()
    stream = dial()
    resp = handshake(
        stream,
        title=snap["title"],
        toe_path=snap["toe_path"],
        image=snap["image"],
        start_time=snap["start_time"],
    )
    pkg_dir = _resolve_bridge_package_dir(bridge_dir, resp)
    _load_bridge_package(pkg_dir)
    serve(stream)
    return resp


_active_stream: Any = None
_active_thread: threading.Thread | None = None


def is_connected() -> bool:
    """True while the IPC worker thread is alive after a successful bootstrap."""
    return _active_thread is not None and _active_thread.is_alive()


def bootstrap_threaded(bridge_dir: str | None = None) -> dict[str, Any]:
    """Non-blocking variant of [`bootstrap`] for a live TD session.

    Dials and handshakes synchronously (fast — a couple of IPC round trips)
    so callers get an immediate handshake result / error, then hands the
    framed read loop to a worker thread running [`serve_queued`] — the
    worker only ever touches the stream and a `queue.Queue`, never `td.*`.

    Captures project identity (``project.name`` / folder → title + toePath)
    on the main thread **before** dialing — never from the IPC worker.

    Path resolution: ``bridge_dir`` / ``TDMCP_BRIDGE_DIR`` → handshake
    ``bridgePackageDir`` → conventional data-dir ``bridge/``. Reloads the
    package from disk on every connect (version bumps without re-baking the tox).

    The caller (the bootstrap Text DAT's owning Execute DAT) **must** also
    enable `Frame Start` and call [`process_pending`] from `onFrameStart`,
    or requests will queue forever without a response. Safe to call again
    after disconnect / dead worker (explicit resurrection).
    """
    global _active_stream, _active_thread
    if _active_stream is not None or _active_thread is not None:
        disconnect()
    snap = _identity_snapshot()
    stream = dial()
    resp = handshake(
        stream,
        title=snap["title"],
        toe_path=snap["toe_path"],
        image=snap["image"],
        start_time=snap["start_time"],
    )
    pkg_dir = _resolve_bridge_package_dir(bridge_dir, resp)
    _load_bridge_package(pkg_dir)
    # Bind serve_queued from the (possibly reloaded) module object.
    serve_fn = sys.modules[__name__].serve_queued
    thread = threading.Thread(target=serve_fn, args=(stream,), daemon=True)
    thread.start()
    _active_stream = stream
    _active_thread = thread
    return resp


def disconnect() -> bool:
    """Close the active bridge connection.

    Main-thread only. Lets the daemon observe a disconnect without quitting
    TD — e.g. to exercise the resurrection path, or to force a clean
    reconnect after changing the bridge package on disk.

    The worker thread ([`serve_queued`]) is normally blocked in a
    synchronous, non-overlapped read on the stream. Closing the handle out
    from under that pending read from a different thread is undefined
    behavior on Windows (and unsafe on POSIX) — it can hang the *closing*
    thread instead of erroring the reader, which freezes TD if this is
    called from a script running on TD's main thread. So: cancel the
    worker's pending I/O first ([`_NamedPipeStream.cancel_pending_io`] /
    [`_UdsStream.cancel_pending_io`]), join it, *then* close.
    """
    global _active_stream, _active_thread
    if _active_stream is None:
        return False
    stream = _active_stream
    thread = _active_thread
    _active_stream = None
    _active_thread = None

    thread_id = getattr(thread, "native_id", None) if thread is not None else None
    try:
        if hasattr(stream, "cancel_pending_io"):
            stream.cancel_pending_io(thread_id)
    except Exception:  # noqa: BLE001 — best-effort; still try to join + close
        pass

    if thread is not None:
        thread.join(timeout=2.0)

    try:
        stream.close()
    except Exception:  # noqa: BLE001 — best-effort teardown
        pass
    return True


if __name__ == "__main__":
    main()
