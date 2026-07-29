"""td-mcp-rs bridge package — loaded by TD after handshake FS path.

Entry: connect over local IPC is owned by the bootstrap tox; this package
owns the session + RPC (execute_python, capture, inspect helpers).
"""

from __future__ import annotations

import json
import os
import struct
import sys
import traceback
from typing import Any, Callable

__protocol_version__ = "1"
__min_daemon__ = "0.1.0"


def _read_frame(stream) -> dict[str, Any]:
    header = stream.read(4)
    if len(header) < 4:
        raise EOFError("short header")
    (length,) = struct.unpack("<I", header)
    body = stream.read(length)
    if len(body) < length:
        raise EOFError("short body")
    return json.loads(body.decode("utf-8"))


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
        black = _is_black_jpeg(data)
        return {
            "ok": not black,
            "code": "tdmcp.perception.black_frame" if black else None,
            "bytes": len(data) if data is not None else 0,
            "path": getattr(target, "path", path),
        }
    except Exception as exc:  # noqa: BLE001
        return {"ok": False, "error": str(exc), "traceback": traceback.format_exc()}


def _is_black_jpeg(data: bytes | None) -> bool:
    if not data:
        return True
    # Heuristic: tiny JPEG often means empty/black; real check needs decode.
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
            for child in n.children():
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
    return sock.makefile("rwb")


def _dial_named_pipe(name: str):
    import ctypes
    from ctypes import wintypes

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

    def __init__(self, handle: int) -> None:
        import ctypes
        from ctypes import wintypes

        self._handle = handle
        self._kernel32 = ctypes.windll.kernel32  # type: ignore[attr-defined]
        self._buf = (ctypes.c_ubyte * 65536)()
        self._wintypes = wintypes

    def read(self, n: int) -> bytes:
        out = bytearray()
        while len(out) < n:
            want = min(n - len(out), len(self._buf))
            read = self._wintypes.DWORD(0)
            ok = self._kernel32.ReadFile(
                self._handle,
                ctypes.byref(self._buf, want),
                want,
                ctypes.byref(read),
                None,
            )
            if not ok or read.value == 0:
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


def handshake(
    stream,
    title: str | None = None,
    toe_path: str | None = None,
) -> dict[str, Any]:
    """Perform the client side of the IPC handshake over `stream`."""
    req = {
        "pid": _td_pid(),
        "protocolVersion": __protocol_version__,
        "title": title,
        "toePath": toe_path,
        "image": "TouchDesigner.exe",
        "startTime": None,
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
    """Framed dispatch loop over a connected IPC stream."""
    while True:
        try:
            msg = _read_frame(stream)
        except EOFError:
            break
        resp = dispatch(msg)
        _write_frame(stream, resp)


def bootstrap(bridge_dir: str | None = None) -> None:
    """Dial the daemon, handshake, load the bridge package, and serve.

    `bridge_dir` is where `tdmcp_bridge` lives; if omitted, the daemon's
    handshake response supplies it (advisory — reload from disk each connect).
    """
    stream = dial()
    resp = handshake(stream)
    pkg_dir = bridge_dir or resp.get("bridgePackageDir")
    if pkg_dir and pkg_dir not in sys.path:
        sys.path.insert(0, pkg_dir)
    serve(stream)


if __name__ == "__main__":
    main()
