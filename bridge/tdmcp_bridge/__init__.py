"""td-mcp-rs bridge package — loaded by TD after handshake FS path.

Entry: connect over local IPC is owned by the bootstrap tox; this package
owns the session + RPC (execute_python, capture, inspect helpers).
"""

from __future__ import annotations

import json
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


HANDLERS: dict[str, Callable[[dict[str, Any]], dict[str, Any]]] = {
    "execute_python": handle_execute_python,
    "capture": handle_capture,
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


if __name__ == "__main__":
    main()
