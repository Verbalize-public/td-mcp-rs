"""td-mcp-rs bridge package — loaded by TD after handshake FS path.

Entry: connect over local IPC is owned by the bootstrap tox; this package
owns the session + RPC (execute_python, capture, inspect helpers).
"""

from __future__ import annotations

import ctypes
import importlib
import io
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
BRIDGE_METHODS: tuple[str, ...] = (
    "execute_python",
    "capture",
    "inspect",
    "mutate_nodes",
    "ping",
)


class ExecutePythonParams(TypedDict):
    script: str
    contextPath: NotRequired[str | None]
    includeLogs: NotRequired[bool]


# execute_python log capture — MCP payload / DAT ring sizes.
_LOGS_RETURN_MAX = 32 * 1024
_DEBUG_DAT_RING_MAX = 64 * 1024
_TRUNC_MARK = "\n…[truncated]\n"
_capture_depth = 0
_bridge_host_path: str | None = None


class CaptureParams(TypedDict):
    path: str
    mode: NotRequired[str]
    contextPath: NotRequired[str | None]
    maxSize: NotRequired[int | None]


class InspectParams(TypedDict):
    path: str
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


def set_bridge_host(comp) -> None:
    """Record the bootstrap COMP path so ``./debug`` can be resolved without op.Debug."""
    global _bridge_host_path
    try:
        path = getattr(comp, "path", None)
        if isinstance(path, str) and path:
            _bridge_host_path = path
    except Exception:  # noqa: BLE001
        pass


def _truncate_logs(text: str, limit: int = _LOGS_RETURN_MAX) -> str:
    if len(text) <= limit:
        return text
    keep = max(0, limit - len(_TRUNC_MARK))
    return _TRUNC_MARK + text[-keep:]


def _ring_append_text(existing: str, chunk: str, limit: int = _DEBUG_DAT_RING_MAX) -> str:
    if not chunk:
        return existing
    merged = (existing or "") + chunk
    if len(merged) <= limit:
        return merged
    return merged[-limit:]


class _TeeStream:
    """Write to a buffer and the previous stream (Textport / TD StdoutCatcher)."""

    def __init__(self, buf: io.StringIO, previous: Any) -> None:
        self._buf = buf
        self._previous = previous

    def write(self, s: Any) -> int:
        text = s if isinstance(s, str) else str(s)
        self._buf.write(text)
        try:
            self._previous.write(text)
        except Exception:  # noqa: BLE001 — never fail the script for textport
            pass
        return len(text)

    def flush(self) -> None:
        try:
            self._buf.flush()
        except Exception:  # noqa: BLE001
            pass
        flush = getattr(self._previous, "flush", None)
        if callable(flush):
            try:
                flush()
            except Exception:  # noqa: BLE001
                pass

    def __getattr__(self, name: str) -> Any:
        return getattr(self._previous, name)


def _resolve_debug_dat():
    """Best-effort Text DAT ``debug`` under the bridge COMP (relative preferred)."""
    try:
        import td  # type: ignore
    except Exception:  # noqa: BLE001
        return None
    if _bridge_host_path:
        try:
            host = td.op(_bridge_host_path)
            if host is not None:
                dat = host.op("debug")
                if dat is not None:
                    return dat
        except Exception:  # noqa: BLE001
            pass
    try:
        host = td.op.Debug
        if host is not None:
            return host.op("debug")
    except Exception:  # noqa: BLE001
        pass
    return None


def _append_debug_dat(logs: str) -> None:
    if not logs:
        return
    dat = _resolve_debug_dat()
    if dat is None:
        return
    try:
        existing = dat.text or ""
    except Exception:  # noqa: BLE001
        existing = ""
    try:
        dat.text = _ring_append_text(existing, logs)
    except Exception:  # noqa: BLE001
        try:
            dat.write(logs)
        except Exception:  # noqa: BLE001
            pass


def handle_execute_python(params: dict[str, Any]) -> dict[str, Any]:
    global _capture_depth
    script = params.get("script") or ""
    context_path = params.get("contextPath")
    include_logs = params.get("includeLogs")
    if include_logs is None:
        include_logs = True
    else:
        include_logs = bool(include_logs)

    local_vars: dict[str, Any] = {
        "__tdmcp_context_path__": context_path,
        "tdmcp_resolve": lambda p: tdmcp_resolve(p, context_path),
        "result": None,
    }

    buf: io.StringIO | None = None
    prev_out = prev_err = None
    installed = False
    if include_logs:
        if _capture_depth == 0:
            buf = io.StringIO()
            prev_out, prev_err = sys.stdout, sys.stderr
            sys.stdout = _TeeStream(buf, prev_out)
            sys.stderr = _TeeStream(buf, prev_err)
            installed = True
        _capture_depth += 1

    try:
        try:
            exec(script, local_vars, local_vars)  # noqa: S102 — intentional TD script surface
            out: dict[str, Any] = {
                "result": local_vars.get("result"),
                "ok": True,
            }
        except Exception as exc:  # noqa: BLE001 — surface to diagnostics
            out = {
                "ok": False,
                "error": str(exc),
                "traceback": traceback.format_exc(),
            }
    finally:
        if include_logs:
            logs = ""
            if installed and buf is not None:
                try:
                    logs = _truncate_logs(buf.getvalue())
                except Exception:  # noqa: BLE001
                    logs = ""
                sys.stdout = prev_out
                sys.stderr = prev_err
                try:
                    _append_debug_dat(logs)
                except Exception:  # noqa: BLE001
                    pass
            _capture_depth = max(0, _capture_depth - 1)
            out["logs"] = logs

    return out


def handle_capture(params: dict[str, Any]) -> dict[str, Any]:
    """P0 capture: top / preview — requires live TD.

    Returns JPEG as ``jpegBase64`` so the MCP layer can emit an image content
    block. Optional ``maxSize`` (default 256) downscales via a temp
    ``resolutionTOP`` that is always destroyed.
    """
    import base64
    import td  # type: ignore

    path = params.get("path") or ""
    mode = params.get("mode") or "auto"
    context_path = params.get("contextPath")
    max_size = params.get("maxSize", 256)
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

    tmp_top = None
    source = target
    try:
        if max_size is not None:
            source, tmp_top = _maybe_downscale_top(td, target, int(max_size))
        data = source.saveByteArray(".jpg")
        raw = bytes(data) if data is not None else b""
        black = _is_black_top(source, raw)
        return {
            "ok": not black,
            "code": "tdmcp.perception.black_frame" if black else None,
            "bytes": len(raw),
            "path": getattr(target, "path", path),
            "mimeType": "image/jpeg",
            "jpegBase64": base64.b64encode(raw).decode("ascii") if raw else None,
            "maxSize": max_size,
        }
    except Exception as exc:  # noqa: BLE001
        return {"ok": False, "error": str(exc), "traceback": traceback.format_exc()}
    finally:
        if tmp_top is not None:
            try:
                tmp_top.destroy()
            except Exception:  # noqa: BLE001
                pass


_BLACK_MEAN_THRESHOLD = 1.0 / 255.0


def _maybe_downscale_top(td_mod, target, max_size: int):
    """Return ``(source_top, tmp_top|None)``; tmp must be destroyed by caller."""
    width = int(getattr(target, "width", 0) or 0)
    height = int(getattr(target, "height", 0) or 0)
    longest = max(width, height)
    if longest <= 0 or longest <= max_size:
        return target, None

    if width >= height:
        new_w = max_size
        new_h = max(1, round(height * max_size / width))
    else:
        new_h = max_size
        new_w = max(1, round(width * max_size / height))

    parent = target.parent()
    tmp_name = "__tdmcp_tmp_res__" + target.name
    existing = parent.op(tmp_name) if parent is not None else None
    if existing is not None:
        existing.destroy()
    tmp_top = parent.create(td_mod.resolutionTOP, tmp_name)
    tmp_top.inputConnectors[0].connect(target)
    tmp_top.par.outputresolution = "custom"
    tmp_top.par.resolutionw = new_w
    tmp_top.par.resolutionh = new_h
    return tmp_top, tmp_top


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


# Direct-child roster cap for inspect (summary and detailed).
CHILDREN_ROSTER_LIMIT = 64


def _child_name(child: Any) -> str:
    """Best-effort operator name; fall back to last path segment."""
    name = getattr(child, "name", None)
    if name:
        return str(name)
    path = getattr(child, "path", "") or ""
    return str(path).rsplit("/", 1)[-1]


def _op_messages(fn: Any) -> list[str]:
    """Normalize TD OP.errors()/warnings() (str) or list-like fakes to string[]."""
    try:
        raw = fn()
    except Exception:  # noqa: BLE001
        return []
    if raw is None:
        return []
    if isinstance(raw, str):
        return [line for line in raw.splitlines() if line.strip()]
    out: list[str] = []
    try:
        items = list(raw)
    except TypeError:
        s = str(raw).strip()
        return [s] if s else []
    for item in items:
        s = str(item).strip()
        if s:
            out.append(s)
    return out


def _force_cook(node: Any) -> None:
    """Best-effort ``OP.cook(force=True)`` so inspect sees post-cook errors.

    Missing ``cook``, TypeError on kwargs, and cook failures are swallowed —
    inspect still returns structure. Positional ``cook(True)`` is a fallback
    for fakes / older signatures.
    """
    cook = getattr(node, "cook", None)
    if not callable(cook):
        return
    try:
        cook(force=True)
    except TypeError:
        try:
            cook(True)
        except Exception:  # noqa: BLE001
            return
    except Exception:  # noqa: BLE001
        return


def build_inspect_node(
    n: Any,
    *,
    detail_level: str = "summary",
    want_nodes: bool = True,
    want_params: bool = False,
    want_errors: bool = False,
    want_warnings: bool = False,
) -> dict[str, Any]:
    """Shape one inspect node payload (pure enough for unit tests without TD)."""
    children: list[dict[str, Any]] = []
    child_count = 0
    if want_nodes:
        raw_children = list(n.children)  # TD OP.children is a list property
        child_count = len(raw_children)
        detailed = detail_level == "detailed"
        for child in raw_children[:CHILDREN_ROSTER_LIMIT]:
            if detailed:
                children.append({
                    "path": getattr(child, "path", None),
                    "family": getattr(child, "family", None),
                    "opType": getattr(child, "opType", None),
                })
            else:
                children.append({
                    "name": _child_name(child),
                    "opType": getattr(child, "opType", None),
                })

    out: dict[str, Any] = {
        "path": getattr(n, "path", None),
        "family": getattr(n, "family", None),
        "opType": getattr(n, "opType", None),
        "childCount": child_count,
        "childrenReturned": len(children),
        "children": children,
    }
    if want_nodes and len(children) < child_count:
        out["childrenTruncated"] = True
        out["truncation"] = {
            "field": "children",
            "limit": CHILDREN_ROSTER_LIMIT,
            "code": "tdmcp.op.children_truncated",
            "message": (
                f"Direct-child roster capped at {CHILDREN_ROSTER_LIMIT} of {child_count}"
            ),
            "mitigation": [
                "Inspect a child COMP path for nested overview",
                "detailLevel does not raise this cap",
                "Use execute_python if you need the full name list",
            ],
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
        out["errors"] = _op_messages(getattr(n, "errors", lambda: ""))
    if want_warnings:
        out["warnings"] = _op_messages(getattr(n, "warnings", lambda: ""))
    return out


def handle_inspect(params: dict[str, Any]) -> dict[str, Any]:
    """Structural subtree read (nodes/params/errors/warnings). Requires live TD."""
    import td  # type: ignore  # noqa: F401 — ensure TD runtime is importable

    path = params.get("path") or ""
    context_path = params.get("contextPath")
    include = params.get("include") or []
    detail_level = params.get("detailLevel") or "summary"

    node = tdmcp_resolve(path, context_path)
    if node is None or not getattr(node, "valid", False):
        return {"ok": False, "code": "tdmcp.op.not_found", "path": path}

    if not include:
        want_nodes = want_errors = want_warnings = True
        want_params = False
    else:
        want_nodes = "nodes" in include
        want_params = "params" in include
        want_errors = "errors" in include
        want_warnings = "warnings" in include

    _force_cook(node)

    try:
        return {
            "ok": True,
            "node": build_inspect_node(
                node,
                detail_level=detail_level,
                want_nodes=want_nodes,
                want_params=want_params,
                want_errors=want_errors,
                want_warnings=want_warnings,
            ),
        }
    except Exception as exc:  # noqa: BLE001
        return {"ok": False, "error": str(exc), "traceback": traceback.format_exc()}


# --- mutate_nodes (pure apply_step seam + live handle_mutate wrapper) --------


def _absolutize_path(path: str, context_path: str | None) -> str:
    """Join relative path against contextPath (default /project1)."""
    path = (path or "").strip()
    if path.startswith("/"):
        return path.rstrip("/") or "/"
    base = (context_path or "/project1").rstrip("/") or "/project1"
    if not path:
        return base
    return f"{base}/{path}"


def _parent_and_name(full_path: str) -> tuple[str, str]:
    """Split an absolute path into (parent_path, leaf_name)."""
    full = (full_path or "").rstrip("/") or "/"
    if full == "/":
        return "/", ""
    parent, name = full.rsplit("/", 1)
    return parent or "/", name


def _get_par(node: Any, name: str) -> Any | None:
    """Best-effort parameter lookup; None if missing."""
    try:
        pars = getattr(node, "par", None)
        if pars is None:
            return None
        return getattr(pars, name, None)
    except Exception:  # noqa: BLE001
        return None


# Operate-relevant subset of TD OP_Class "Common Flags" (docs.derivative.ca).
# Editor/UI-only flags (current, selected, expose, showCustomOnly, showDocked,
# python) are intentionally omitted.
_FLAG_NAMES = frozenset(
    {
        "activeViewer",
        "allowCooking",
        "bypass",
        "cloneImmune",
        "display",
        "lock",
        "render",
        "viewer",
    }
)


def _with_collection_hint(
    err: dict[str, Any], node: Any, *, as_param: bool
) -> dict[str, Any]:
    """Best-effort cross-collection lint. Never raises; never changes code/ok.

    When a name is missing from the requested bag but exists in the other
    collection, attach a nested lint and a short message suffix. Any enrich
    failure returns ``err`` unchanged (simplified base diagnostic only).
    Builds on a shallow copy so a mid-enrich failure cannot leave a partial
    message/lint on the caller's base dict.
    """
    try:
        name = err.get("field")
        if not isinstance(name, str) or not name:
            return err
        if as_param and name in _FLAG_NAMES:
            out = dict(err)
            out["message"] = f"unknown parameter: {name} (exists as flag — use flags)"
            out["lints"] = [
                {
                    "severity": "lint",
                    "code": "tdmcp.par.wrong_collection",
                    "message": f"'{name}' is an OP flag; use flags, not values",
                    "confidence": "high",
                    "suggestion": {"replace": f"flags.{name}"},
                }
            ]
            return out
        if not as_param and _get_par(node, name) is not None:
            out = dict(err)
            out["message"] = f"unknown flag: {name} (exists as parameter — use values)"
            out["lints"] = [
                {
                    "severity": "lint",
                    "code": "tdmcp.flag.wrong_collection",
                    "message": f"'{name}' is a .par parameter; use values, not flags",
                    "confidence": "high",
                    "suggestion": {"replace": f"values.{name}"},
                }
            ]
            return out
        return err
    except Exception:  # noqa: BLE001
        return err


def _apply_values(node: Any, values: dict[str, Any]) -> dict[str, Any] | None:
    """Assign plain parameter values. Returns an error step dict, or None on ok."""
    for name, val in values.items():
        par = _get_par(node, name)
        if par is None:
            return _with_collection_hint(
                {
                    "ok": False,
                    "code": "tdmcp.par.unknown",
                    "path": getattr(node, "path", None),
                    "message": f"unknown parameter: {name}",
                    "field": name,
                },
                node,
                as_param=True,
            )
        try:
            if hasattr(par, "val"):
                par.val = val
            else:
                setattr(node.par, name, val)
        except Exception as exc:  # noqa: BLE001
            return {
                "ok": False,
                "code": "tdmcp.mutate.step_failed",
                "path": getattr(node, "path", None),
                "message": str(exc),
                "field": name,
            }
    return None


def _apply_flags(node: Any, flags: dict[str, Any]) -> dict[str, Any] | None:
    """Assign direct OP attributes (flags). Returns an error step dict, or None on ok."""
    for name, val in flags.items():
        if name not in _FLAG_NAMES:
            return _with_collection_hint(
                {
                    "ok": False,
                    "code": "tdmcp.flag.unknown",
                    "path": getattr(node, "path", None),
                    "message": f"unknown flag: {name}",
                    "field": name,
                },
                node,
                as_param=False,
            )
        try:
            setattr(node, name, val)
        except Exception as exc:  # noqa: BLE001
            return {
                "ok": False,
                "code": "tdmcp.mutate.step_failed",
                "path": getattr(node, "path", None),
                "message": str(exc),
                "field": name,
            }
    return None


def _apply_expressions(
    node: Any, expressions: dict[str, Any], expression_mode: Any
) -> dict[str, Any] | None:
    """Set expression mode explicitly, then assign .expr."""
    for name, expr in expressions.items():
        par = _get_par(node, name)
        if par is None:
            return _with_collection_hint(
                {
                    "ok": False,
                    "code": "tdmcp.par.unknown",
                    "path": getattr(node, "path", None),
                    "message": f"unknown parameter: {name}",
                    "field": name,
                },
                node,
                as_param=True,
            )
        try:
            par.mode = expression_mode
            par.expr = expr
        except Exception as exc:  # noqa: BLE001
            return {
                "ok": False,
                "code": "tdmcp.mutate.step_failed",
                "path": getattr(node, "path", None),
                "message": str(exc),
                "field": name,
            }
    return None


def _apply_pulse(node: Any, pulse: list[str]) -> dict[str, Any] | None:
    """Pulse named parameters."""
    for name in pulse:
        par = _get_par(node, name)
        if par is None:
            return _with_collection_hint(
                {
                    "ok": False,
                    "code": "tdmcp.par.unknown",
                    "path": getattr(node, "path", None),
                    "message": f"unknown parameter: {name}",
                    "field": name,
                },
                node,
                as_param=True,
            )
        try:
            pulse_fn = getattr(par, "pulse", None)
            if not callable(pulse_fn):
                return {
                    "ok": False,
                    "code": "tdmcp.mutate.step_failed",
                    "path": getattr(node, "path", None),
                    "message": f"parameter {name} has no pulse()",
                    "field": name,
                }
            pulse_fn()
        except Exception as exc:  # noqa: BLE001
            return {
                "ok": False,
                "code": "tdmcp.mutate.step_failed",
                "path": getattr(node, "path", None),
                "message": str(exc),
                "field": name,
            }
    return None


class MutateContext:
    """Resolution hooks for apply_step — no ``td`` import in the pure seam.

    Live TD supplies :class:`_TdMutateContext`; unit tests supply fakes.
    """

    def resolve(self, path: str) -> Any | None:  # noqa: D401
        raise NotImplementedError

    def get_op_type(self, op_type: str) -> Any | None:  # noqa: D401
        raise NotImplementedError

    def expression_mode(self) -> Any:
        """Value assigned to ``par.mode`` before setting ``.expr``."""
        return "EXPRESSION"


class _TdMutateContext(MutateContext):
    """Live TD resolution via ``tdmcp_resolve`` / ``getattr(td, …)``."""

    def __init__(self, context_path: str | None) -> None:
        self._context_path = context_path

    def resolve(self, path: str) -> Any | None:
        node = tdmcp_resolve(path, self._context_path)
        if node is None or not getattr(node, "valid", True):
            return None
        return node

    def get_op_type(self, op_type: str) -> Any | None:
        import td  # type: ignore

        return getattr(td, op_type, None)

    def expression_mode(self) -> Any:
        import td  # type: ignore

        mode_enum = getattr(td, "ParMode", None)
        if mode_enum is not None:
            expr = getattr(mode_enum, "EXPRESSION", None)
            if expr is not None:
                return expr
        return "EXPRESSION"


def apply_step(
    ctx: MutateContext,
    step: dict[str, Any],
    *,
    context_path: str | None = None,
    detail_level: str = "summary",
) -> dict[str, Any]:
    """Apply one mutate step. Pure seam — no ``td`` import.

    Returns a per-step result dict: ``{ok, path?, code?, message?, …}``.
    """
    op = step.get("op") or ""
    try:
        if op == "create":
            return _step_create(ctx, step, context_path, detail_level)
        if op == "set":
            return _step_set(ctx, step, context_path, detail_level)
        if op == "delete":
            return _step_delete(ctx, step, context_path)
        if op == "connect":
            return _step_connect(ctx, step, context_path, detail_level)
        if op == "disconnect":
            return _step_disconnect(ctx, step, context_path, detail_level)
        return {
            "ok": False,
            "code": "tdmcp.mutate.step_failed",
            "message": f"unknown mutate op: {op}",
            "path": step.get("path") or step.get("dst"),
        }
    except Exception as exc:  # noqa: BLE001 — never propagate raw
        return {
            "ok": False,
            "code": "tdmcp.mutate.step_failed",
            "message": str(exc),
            "path": step.get("path"),
        }


def _step_create(
    ctx: MutateContext,
    step: dict[str, Any],
    context_path: str | None,
    detail_level: str,
) -> dict[str, Any]:
    path = step.get("path") or ""
    op_type = step.get("opType") or ""
    full = _absolutize_path(path, context_path)
    parent_path, name = _parent_and_name(full)
    if not name:
        return {
            "ok": False,
            "code": "tdmcp.mutate.step_failed",
            "message": "create path has no leaf name",
            "path": full,
        }
    parent = ctx.resolve(parent_path)
    if parent is None:
        return {
            "ok": False,
            "code": "tdmcp.op.not_found",
            "message": f"parent not found: {parent_path}",
            "path": full,
        }
    op_cls = ctx.get_op_type(op_type)
    if op_cls is None:
        return {
            "ok": False,
            "code": "tdmcp.op.unknown_type",
            "message": f"unknown opType: {op_type}",
            "path": full,
        }
    created = parent.create(op_cls, name)
    created_path = getattr(created, "path", None) or full
    values = step.get("values")
    if values:
        err = _apply_values(created, values)
        if err is not None:
            err["path"] = created_path
            return err
    flags = step.get("flags")
    if flags:
        err = _apply_flags(created, flags)
        if err is not None:
            err["path"] = created_path
            return err
    out: dict[str, Any] = {"ok": True, "path": created_path}
    if detail_level == "detailed":
        if values:
            out["values"] = values
        if flags:
            out["flags"] = flags
    return out


def _step_set(
    ctx: MutateContext,
    step: dict[str, Any],
    context_path: str | None,
    detail_level: str,
) -> dict[str, Any]:
    path = step.get("path") or ""
    full = _absolutize_path(path, context_path)
    node = ctx.resolve(full)
    if node is None:
        return {
            "ok": False,
            "code": "tdmcp.op.not_found",
            "message": f"node not found: {full}",
            "path": full,
        }
    node_path = getattr(node, "path", None) or full
    values = step.get("values")
    expressions = step.get("expressions")
    pulse = step.get("pulse")
    flags = step.get("flags")
    if values:
        err = _apply_values(node, values)
        if err is not None:
            return err
    if expressions:
        err = _apply_expressions(node, expressions, ctx.expression_mode())
        if err is not None:
            return err
    if pulse:
        err = _apply_pulse(node, list(pulse))
        if err is not None:
            return err
    if flags:
        err = _apply_flags(node, flags)
        if err is not None:
            return err
    out: dict[str, Any] = {"ok": True, "path": node_path}
    if detail_level == "detailed":
        if values:
            out["values"] = values
        if expressions:
            out["expressions"] = expressions
        if pulse:
            out["pulse"] = list(pulse)
        if flags:
            out["flags"] = flags
    return out


def _step_delete(
    ctx: MutateContext,
    step: dict[str, Any],
    context_path: str | None,
) -> dict[str, Any]:
    path = step.get("path") or ""
    full = _absolutize_path(path, context_path)
    node = ctx.resolve(full)
    if node is None:
        return {
            "ok": False,
            "code": "tdmcp.op.not_found",
            "message": f"node not found: {full}",
            "path": full,
        }
    node_path = getattr(node, "path", None) or full
    try:
        node.destroy()
    except Exception as exc:  # noqa: BLE001
        return {
            "ok": False,
            "code": "tdmcp.mutate.step_failed",
            "message": str(exc),
            "path": node_path,
        }
    return {"ok": True, "path": node_path}


def _connector_index(step: dict[str, Any], key: str, default: int = 0) -> int:
    """Coerce a connector index from a step field; default when missing/null."""
    raw = step.get(key, default)
    if raw is None:
        return default
    try:
        return int(raw)
    except (TypeError, ValueError):
        return default


def _step_connect(
    ctx: MutateContext,
    step: dict[str, Any],
    context_path: str | None,
    detail_level: str,
) -> dict[str, Any]:
    """Wire ``src.outputConnectors[i]`` to ``dst.inputConnectors[j]``."""
    src_path = _absolutize_path(step.get("src") or "", context_path)
    dst_path = _absolutize_path(step.get("dst") or "", context_path)
    src_output = _connector_index(step, "srcOutput", 0)
    dst_input = _connector_index(step, "dstInput", 0)

    src = ctx.resolve(src_path)
    if src is None:
        return {
            "ok": False,
            "code": "tdmcp.op.not_found",
            "message": f"node not found: {src_path}",
            "path": src_path,
        }
    dst = ctx.resolve(dst_path)
    if dst is None:
        return {
            "ok": False,
            "code": "tdmcp.op.not_found",
            "message": f"node not found: {dst_path}",
            "path": dst_path,
        }

    dst_canon = getattr(dst, "path", None) or dst_path
    src_canon = getattr(src, "path", None) or src_path
    try:
        out_cons = getattr(src, "outputConnectors", None)
        in_cons = getattr(dst, "inputConnectors", None)
        if out_cons is None or in_cons is None:
            return {
                "ok": False,
                "code": "tdmcp.wire.connect_failed",
                "message": "node missing inputConnectors/outputConnectors",
                "path": dst_canon,
            }
        try:
            out_c = out_cons[src_output]
            in_c = in_cons[dst_input]
        except (IndexError, KeyError, TypeError) as exc:
            return {
                "ok": False,
                "code": "tdmcp.wire.bad_index",
                "message": (
                    f"bad connector index srcOutput={src_output} "
                    f"dstInput={dst_input}: {exc}"
                ),
                "path": dst_canon,
            }
        out_c.connect(in_c)
    except IndexError as exc:
        return {
            "ok": False,
            "code": "tdmcp.wire.bad_index",
            "message": str(exc),
            "path": dst_canon,
        }
    except Exception as exc:  # noqa: BLE001
        return {
            "ok": False,
            "code": "tdmcp.wire.connect_failed",
            "message": str(exc),
            "path": dst_canon,
        }

    out: dict[str, Any] = {"ok": True, "path": dst_canon}
    if detail_level == "detailed":
        out["src"] = src_canon
        out["srcOutput"] = src_output
        out["dstInput"] = dst_input
    return out


def _step_disconnect(
    ctx: MutateContext,
    step: dict[str, Any],
    context_path: str | None,
    detail_level: str,
) -> dict[str, Any]:
    """Clear ``path.inputConnectors[input]``."""
    full = _absolutize_path(step.get("path") or "", context_path)
    input_idx = _connector_index(step, "input", 0)
    node = ctx.resolve(full)
    if node is None:
        return {
            "ok": False,
            "code": "tdmcp.op.not_found",
            "message": f"node not found: {full}",
            "path": full,
        }
    node_path = getattr(node, "path", None) or full
    try:
        in_cons = getattr(node, "inputConnectors", None)
        if in_cons is None:
            return {
                "ok": False,
                "code": "tdmcp.wire.connect_failed",
                "message": "node missing inputConnectors",
                "path": node_path,
            }
        try:
            in_c = in_cons[input_idx]
        except (IndexError, KeyError, TypeError) as exc:
            return {
                "ok": False,
                "code": "tdmcp.wire.bad_index",
                "message": f"bad input connector index {input_idx}: {exc}",
                "path": node_path,
            }
        in_c.disconnect()
    except IndexError as exc:
        return {
            "ok": False,
            "code": "tdmcp.wire.bad_index",
            "message": str(exc),
            "path": node_path,
        }
    except Exception as exc:  # noqa: BLE001
        return {
            "ok": False,
            "code": "tdmcp.wire.connect_failed",
            "message": str(exc),
            "path": node_path,
        }

    out: dict[str, Any] = {"ok": True, "path": node_path}
    if detail_level == "detailed":
        out["input"] = input_idx
    return out


def run_mutate_steps(
    ctx: MutateContext,
    steps: list[dict[str, Any]],
    *,
    context_path: str | None = None,
    detail_level: str = "summary",
) -> dict[str, Any]:
    """Sequential apply; stop on first hard error; mark rest skipped."""
    results: list[dict[str, Any]] = []
    applied = 0
    failed_at: int | None = None
    for i, step in enumerate(steps):
        if failed_at is not None:
            results.append(
                {
                    "ok": False,
                    "skipped": True,
                    "code": "tdmcp.batch.skipped_dependent",
                    "path": step.get("path") or step.get("dst"),
                }
            )
            continue
        result = apply_step(
            ctx, step, context_path=context_path, detail_level=detail_level
        )
        results.append(result)
        if result.get("ok"):
            applied += 1
        else:
            failed_at = i
    return {
        "ok": failed_at is None,
        "applied": applied,
        "failedAt": failed_at,
        "steps": results,
    }


def handle_mutate(params: dict[str, Any]) -> dict[str, Any]:
    """Live TD wrapper for mutate_nodes — resolves via ``td.op()``."""
    steps = params.get("steps") or []
    if not isinstance(steps, list):
        return {
            "ok": False,
            "applied": 0,
            "failedAt": 0,
            "steps": [
                {
                    "ok": False,
                    "code": "tdmcp.mutate.step_failed",
                    "message": "steps must be a list",
                }
            ],
        }
    context_path = params.get("contextPath")
    detail_level = params.get("detailLevel") or "summary"
    ctx = _TdMutateContext(context_path)
    return run_mutate_steps(
        ctx, steps, context_path=context_path, detail_level=detail_level
    )


HANDLERS: dict[str, Callable[[dict[str, Any]], dict[str, Any]]] = {
    "execute_python": handle_execute_python,
    "capture": handle_capture,
    "inspect": handle_inspect,
    "mutate_nodes": handle_mutate,
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
