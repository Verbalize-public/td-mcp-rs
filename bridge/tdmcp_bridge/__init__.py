"""td-mcp-rs bridge package — loaded by TD after handshake FS path.

Entry: connect over local IPC is owned by the bootstrap tox; this package
owns the session + RPC (execute_python, capture, inspect helpers).
"""

from __future__ import annotations

import importlib
import os
import sys
import threading
import time
from typing import Any, Callable, NotRequired, TypedDict

import queue as queue  # re-exported for tests (tdmcp_bridge.queue.Queue)

# Package-level mutable state (tests monkeypatch these on ``tdmcp_bridge``).
_bridge_host_path: str | None = None
_capture_depth = 0

from . import (
    api_help as _api_help,
    capture as _capture,
    constants as _constants,
    editor_context as _editor_context,
    execute as _execute,
    identity as _identity,
    inspect as _inspect,
    mutate as _mutate,
    paths as _paths,
    suggest as _suggest,
    task_queue as _task_queue,
    transport as _transport,
)


_SKIP = frozenset(
    {
        "__name__",
        "__doc__",
        "__package__",
        "__loader__",
        "__spec__",
        "__file__",
        "__cached__",
        "__builtins__",
        "__annotations__",
        "__path__",
        "__all__",
    }
)


def _reexport(mod: object) -> None:
    """Copy public **and** private names (tests poke underscore helpers)."""
    for name in dir(mod):
        if name in _SKIP:
            continue
        globals()[name] = getattr(mod, name)


for _mod in (
    _constants,
    _transport,
    _paths,
    _suggest,
    _execute,
    _capture,
    _inspect,
    _mutate,
    _api_help,
    _editor_context,
    _identity,
    _task_queue,
):
    _reexport(_mod)
del _mod, _reexport

class ExecutePythonParams(TypedDict):
    script: str
    contextPath: NotRequired[str | None]
    includeLogs: NotRequired[bool]
    formatMode: NotRequired[str]


# execute_python log capture — MCP payload / DAT ring sizes.
class CaptureParams(TypedDict):
    path: str
    mode: NotRequired[str]
    contextPath: NotRequired[str | None]
    maxSize: NotRequired[int | None]


class InspectParams(TypedDict):
    paths: list[str]
    contextPath: NotRequired[str | None]
    include: NotRequired[list[str]]
    detailLevel: NotRequired[str]


class MutateNodesParams(TypedDict):
    steps: list[dict[str, Any]]
    contextPath: NotRequired[str | None]
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
    exception: NotRequired[dict[str, Any]]


HANDLERS: dict[str, Callable[[dict[str, Any]], dict[str, Any]]] = {
    "execute_python": handle_execute_python,
    "capture": handle_capture,
    "inspect": handle_inspect,
    "mutate_nodes": handle_mutate,
    "api_help": handle_api_help,
    "editor_context": handle_editor_context,
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
    """Put ``pkg_dir`` on ``sys.path`` and reload package **and** submodules.

    ``importlib.reload(tdmcp_bridge)`` alone leaves stale ``tdmcp_bridge.*``
    submodules in ``sys.modules`` (TD keeps the interpreter across dialer
    retries). Reload children deepest-first, then the package root so
    re-exports bind to the fresh callables.
    """
    if pkg_dir:
        if pkg_dir not in sys.path:
            sys.path.insert(0, pkg_dir)
    elif "tdmcp_bridge" not in sys.modules:
        return
    submods = sorted(
        (n for n in list(sys.modules) if n.startswith("tdmcp_bridge.")),
        key=lambda n: n.count("."),
        reverse=True,
    )
    for name in submods:
        mod = sys.modules.get(name)
        if mod is None:
            continue
        try:
            importlib.reload(mod)
        except Exception:  # noqa: BLE001 — drop broken cache; next import reloads
            del sys.modules[name]
    pkg = sys.modules.get("tdmcp_bridge")
    if pkg is not None:
        importlib.reload(pkg)


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
    # Refresh before handshake — dialer retries share one TD interpreter.
    pre_dir = bridge_dir or _env_bridge_dir() or _conventional_bridge_dir()
    _load_bridge_package(pre_dir)
    pkg = sys.modules[__name__]
    snap = pkg._identity_snapshot()
    stream = pkg.dial()
    resp = pkg.handshake(
        stream,
        title=snap["title"],
        toe_path=snap["toe_path"],
        image=snap["image"],
        start_time=snap["start_time"],
    )
    pkg_dir = pkg._resolve_bridge_package_dir(bridge_dir, resp)
    pkg._load_bridge_package(pkg_dir)
    pkg.serve(stream)
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

    Starts the timeline-independent [`start_pump`] so queued requests drain
    even when the project is paused. The Execute DAT should still enable
    `Frame Start` and call [`process_pending`] from `onFrameStart` while
    playing (larger batch per frame). Safe to call again after disconnect /
    dead worker (explicit resurrection).
    """
    global _active_stream, _active_thread
    if _active_stream is not None or _active_thread is not None:
        disconnect()
    # Refresh before handshake — dialer retries share one TD interpreter.
    # Without this, a fixed identity.py on disk stays invisible while the
    # stale ``tdmcp_bridge.identity`` module remains in ``sys.modules``.
    pre_dir = bridge_dir or _env_bridge_dir() or _conventional_bridge_dir()
    _load_bridge_package(pre_dir)
    pkg = sys.modules[__name__]
    # If reload replaced this function, re-enter the fresh implementation.
    if pkg.bootstrap_threaded is not bootstrap_threaded:
        return pkg.bootstrap_threaded(bridge_dir=bridge_dir)
    snap = pkg._identity_snapshot()
    stream = pkg.dial()
    resp = pkg.handshake(
        stream,
        title=snap["title"],
        toe_path=snap["toe_path"],
        image=snap["image"],
        start_time=snap["start_time"],
    )
    pkg_dir = pkg._resolve_bridge_package_dir(bridge_dir, resp)
    pkg._load_bridge_package(pkg_dir)
    # Bind serve_queued from the (possibly reloaded) module object.
    serve_fn = sys.modules[__name__].serve_queued
    idle_dead_s = pkg.idle_dead_from_handshake(resp)
    max_call_wait_s = pkg.max_call_wait_from_handshake(resp)
    thread = threading.Thread(
        target=serve_fn,
        args=(stream,),
        kwargs={"idle_dead_s": idle_dead_s, "max_call_wait_s": max_call_wait_s},
        daemon=True,
    )
    thread.start()
    pkg._active_stream = stream
    pkg._active_thread = thread
    # Timeline-independent dispatch so the bridge works while paused.
    pkg.start_pump()
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
