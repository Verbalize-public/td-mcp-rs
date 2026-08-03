"""td-mcp-rs bridge package — loaded by TD after handshake FS path.

Entry: connect over local IPC is owned by the bootstrap tox; this package
owns the session + RPC (execute_python, capture, inspect helpers).
"""

from __future__ import annotations

import ctypes
import difflib
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


class MidFrameTimeout(TimeoutError):
    """Read timed out after partial frame bytes were already consumed.

    Distinct from a clean idle ``TimeoutError`` (zero bytes). Continuing to
    read after this leaves the byte stream desynced — ``serve_queued`` must
    disconnect rather than ``continue``.
    """

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
    formatMode: NotRequired[str]


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
    paths: list[str]
    contextPath: NotRequired[str | None]
    include: NotRequired[list[str]]
    detailLevel: NotRequired[str]


# Soft cap on inspect batch size.
INSPECT_PATHS_LIMIT = 32
CAPTURE_VIEWER_NAME = "capture_viewer"


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


def _read_frame(stream) -> dict[str, Any]:
    """Read one length-prefixed JSON frame.

    Raises:
        EOFError: peer closed / short read that is not a timeout.
        TimeoutError: underlying stream read timed out (idle poll).
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
    try:
        body = stream.read(length)
    except MidFrameTimeout:
        raise
    except TimeoutError as exc:
        raise MidFrameTimeout("timed out mid-frame") from exc
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
    from .exception_report import build_exception_report

    script = params.get("script") or ""
    context_path = params.get("contextPath")
    include_logs = params.get("includeLogs")
    if include_logs is None:
        include_logs = True
    else:
        include_logs = bool(include_logs)
    format_mode = params.get("formatMode") or "normal"
    if format_mode not in ("normal", "debug"):
        format_mode = "normal"

    # Convenience globals for agent scripts. ``td`` / ``op`` are safe here
    # because handle_execute_python only runs on TD's main/cook thread via
    # process_pending → dispatch. ``me`` / ``parent`` are intentionally
    # omitted — execute_python has no script-owner OP context.
    local_vars: dict[str, Any] = {
        "__tdmcp_context_path__": context_path,
        "tdmcp_resolve": lambda p: tdmcp_resolve(p, context_path),
        "result": None,
    }
    try:
        import td  # type: ignore

        local_vars["td"] = td
        op_fn = getattr(td, "op", None)
        if callable(op_fn):
            local_vars["op"] = op_fn
    except Exception:  # noqa: BLE001 — unit tests / non-TD hosts
        pass

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
            raw_tb = traceback.format_exc()
            out = {
                "ok": False,
                "error": str(exc),
                "traceback": raw_tb,
                "exception": build_exception_report(
                    exc, script, format_mode=format_mode
                ),
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


# chop_data caps (token / wire discipline; not MCP knobs).
CHOP_DATA_MAX_CHANNELS = 32
CHOP_DATA_MAX_SAMPLES = 256
CHOP_DATA_MAX_SCALARS = 4096

_BLACK_MEAN_THRESHOLD = 1.0 / 255.0
_UNIFORM_RANGE_THRESHOLD = 2.0 / 255.0


def _op_family(node: Any) -> str | None:
    """Best-effort TD family string (``TOP`` / ``CHOP`` / ``COMP`` / ``POP`` / …)."""
    fam = getattr(node, "family", None)
    if fam is None:
        return None
    return str(fam)


def _effective_capture_mode(mode: str, node: Any) -> str:
    """Resolve ``auto`` (and pass-through explicit modes) from operator family.

    ``TOP`` → ``top``; ``CHOP`` → ``chop_data`` (numeric default); everything
    else (COMP/POP/SOP/MAT/DAT/unrecognized) → ``preview`` via shared OP Viewer.
    """
    if mode != "auto":
        return mode
    family = _op_family(node)
    if family == "CHOP":
        return "chop_data"
    if family == "TOP" or hasattr(node, "saveByteArray"):
        return "top"
    return "preview"


def _chan_samples(chan: Any, n_samples: int) -> list[float]:
    """Read up to ``n_samples`` floats from a TD Channel (or test double)."""
    vals = getattr(chan, "vals", None)
    if vals is not None:
        try:
            return [float(vals[i]) for i in range(n_samples)]
        except Exception:  # noqa: BLE001
            pass
    out: list[float] = []
    for i in range(n_samples):
        try:
            out.append(float(chan[i]))
        except Exception:  # noqa: BLE001
            out.append(0.0)
    return out


def _capture_chop_data(node: Any, path: str) -> dict[str, Any]:
    """CHOP → capped JSON. Pure enough for unit tests with fake CHOPs."""
    resolved = getattr(node, "path", None) or path
    family = _op_family(node) or "CHOP"
    if family != "CHOP":
        return {
            "ok": False,
            "code": "tdmcp.perception.wrong_family",
            "message": (
                f"mode chop_data requires CHOP; resolved family is {family!r}"
            ),
            "path": resolved,
            "mode": "chop_data",
            "family": family,
        }

    num_chans = int(getattr(node, "numChans", 0) or 0)
    num_samples = int(getattr(node, "numSamples", 0) or 0)
    if num_chans <= 0 or num_samples <= 0:
        return {
            "ok": False,
            "code": "tdmcp.perception.empty_chop",
            "message": (
                f"CHOP has no channels or samples "
                f"(numChans={num_chans}, numSamples={num_samples})"
            ),
            "path": resolved,
            "mode": "chop_data",
            "family": "CHOP",
            "numChans": num_chans,
            "numSamples": num_samples,
        }

    chans_attr = getattr(node, "chans", None)
    if callable(chans_attr):
        try:
            chans_attr = chans_attr()
        except Exception:  # noqa: BLE001
            chans_attr = None
    if chans_attr is None:
        chans_list: list[Any] = []
        for i in range(num_chans):
            try:
                chans_list.append(node[i])
            except Exception:  # noqa: BLE001
                break
    else:
        chans_list = list(chans_attr)

    channels_out: list[dict[str, Any]] = []
    scalars_used = 0
    truncated_field: str | None = None
    truncated_limit = 0
    chan_limit = min(num_chans, CHOP_DATA_MAX_CHANNELS, len(chans_list))

    for ci in range(chan_limit):
        if scalars_used >= CHOP_DATA_MAX_SCALARS:
            truncated_field = "scalars"
            truncated_limit = CHOP_DATA_MAX_SCALARS
            break
        chan = chans_list[ci]
        samples_wanted = min(
            num_samples,
            CHOP_DATA_MAX_SAMPLES,
            CHOP_DATA_MAX_SCALARS - scalars_used,
        )
        samples = _chan_samples(chan, samples_wanted)
        name = getattr(chan, "name", None)
        if name is None:
            name = f"chan{ci}"
        channels_out.append({"name": str(name), "samples": samples})
        scalars_used += len(samples)
        if samples_wanted < num_samples:
            truncated_field = "samples"
            truncated_limit = CHOP_DATA_MAX_SAMPLES
            # Continue other channels only if scalar budget remains; if we
            # hit samples-per-channel cap but still have budget, mark samples
            # and keep going until channel/scalar caps.
            if scalars_used >= CHOP_DATA_MAX_SCALARS:
                truncated_field = "scalars"
                truncated_limit = CHOP_DATA_MAX_SCALARS
                break

    if truncated_field is None and (
        chan_limit < num_chans or len(chans_list) < num_chans
    ):
        truncated_field = "channels"
        truncated_limit = CHOP_DATA_MAX_CHANNELS

    out: dict[str, Any] = {
        "ok": True,
        "path": resolved,
        "mode": "chop_data",
        "family": "CHOP",
        "numChans": num_chans,
        "numSamples": num_samples,
        "channels": channels_out,
    }
    rate = getattr(node, "rate", None)
    if rate is not None:
        try:
            out["rate"] = float(rate)
        except (TypeError, ValueError):
            pass
    if truncated_field is not None:
        out["truncation"] = {
            "field": truncated_field,
            "limit": truncated_limit,
            "code": "tdmcp.perception.chop_truncated",
            "message": (
                f"CHOP capture capped at {truncated_field} "
                f"(limit={truncated_limit}; "
                f"numChans={num_chans}, numSamples={num_samples})"
            ),
            "mitigation": [
                "Narrow the CHOP window or channel count in TD before re-capture",
                "Caps are fixed: 32 channels, 256 samples/channel, 4096 scalars",
            ],
        }
    return out


def _capture_top_image(
    td_mod: Any, target: Any, path: str, max_size: Any
) -> dict[str, Any]:
    """TOP → PNG (+ black/uniform soft-fail). Temps always destroyed."""
    import base64

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
            source, tmp_top = _maybe_downscale_top(td_mod, target, int(max_size))
        data = source.saveByteArray(".png")
        raw = bytes(data) if data is not None else b""
        kind, mean_rgb = _classify_frame(source, raw)
        code = None
        message = None
        if kind == "black":
            code = "tdmcp.perception.black_frame"
            message = _perception_frame_message("black", mean_rgb)
        elif kind == "uniform":
            code = "tdmcp.perception.uniform_frame"
            message = _perception_frame_message("uniform", mean_rgb)
        return {
            "ok": kind is None,
            "code": code,
            "message": message,
            "bytes": len(raw),
            "path": getattr(target, "path", path),
            "mimeType": "image/png",
            "imageBase64": base64.b64encode(raw).decode("ascii") if raw else None,
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


def _bridge_host(td_mod: Any) -> Any:
    """Resolve the bootstrap COMP that owns ``capture_viewer`` / ``debug``."""
    if not _bridge_host_path:
        return None
    try:
        return td_mod.op(_bridge_host_path)
    except Exception:  # noqa: BLE001
        return None


def _ensure_capture_viewer(td_mod: Any) -> Any:
    """Return the shared OP Viewer TOP under the bridge host (create if missing)."""
    host = _bridge_host(td_mod)
    if host is None:
        return None
    existing = host.op(CAPTURE_VIEWER_NAME) if hasattr(host, "op") else None
    if existing is not None:
        return existing
    op_class = getattr(td_mod, "opviewerTOP", None)
    if op_class is None or not hasattr(host, "create"):
        return None
    try:
        viewer = host.create(op_class, CAPTURE_VIEWER_NAME)
        try:
            viewer.nodeX, viewer.nodeY = 400, 0
        except Exception:  # noqa: BLE001
            pass
        return viewer
    except Exception:  # noqa: BLE001
        return None


def _capture_via_shared_viewer(
    td_mod: Any,
    source: Any,
    path: str,
    max_size: Any,
    *,
    mode: str,
) -> dict[str, Any]:
    """Retarget bridge ``capture_viewer`` at ``source`` → PNG (any family).

    Safe under the per-pid FIFO: only one bridge dispatch runs at a time, so
    ``par.opviewer`` retarget + save is not raced by concurrent capture.
    """
    family = _op_family(source)
    viewer = _ensure_capture_viewer(td_mod)
    if viewer is None:
        return {
            "ok": False,
            "code": "tdmcp.perception.no_path",
            "message": (
                "shared capture_viewer missing "
                "(bridge host not registered or opviewerTOP unavailable)"
            ),
            "path": getattr(source, "path", path),
            "mode": mode,
            "family": family,
        }

    try:
        par = getattr(getattr(viewer, "par", None), "opviewer", None)
        if par is None:
            raise RuntimeError("capture_viewer has no par.opviewer")
        # Prefer source-native resolution (avoid letterboxed black frames).
        out_res = getattr(getattr(viewer, "par", None), "outputresolution", None)
        if out_res is not None:
            try:
                out_res.val = "useinput"
            except Exception:  # noqa: BLE001
                pass
        # Node Viewer must be enabled for OP Viewer TOP to rasterize content.
        try:
            source.viewer = True
        except Exception:  # noqa: BLE001
            pass
        par.val = source
    except Exception as exc:  # noqa: BLE001
        return {
            "ok": False,
            "code": "tdmcp.perception.no_path",
            "message": f"failed to bind capture_viewer: {exc}",
            "path": getattr(source, "path", path),
            "mode": mode,
            "family": family,
            "traceback": traceback.format_exc(),
        }

    result = _capture_top_image(td_mod, viewer, path, max_size)
    # Report the source path agents asked for, not the shared viewer.
    result["path"] = getattr(source, "path", path)
    result["mode"] = mode
    if family is not None:
        result["family"] = family
    if result.get("ok") is False and "error" in result and "code" not in result:
        return {
            "ok": False,
            "code": "tdmcp.perception.no_path",
            "message": str(result.get("error") or "shared viewer capture failed"),
            "path": getattr(source, "path", path),
            "mode": mode,
            "family": family,
            "traceback": result.get("traceback"),
        }
    return result


def handle_capture(params: dict[str, Any]) -> dict[str, Any]:
    """Perception capture: top / preview / auto / chop_data / chop_image / pop.

    Image modes return ``imageBase64`` (PNG) for MCP image promotion.
    ``chop_data`` returns capped channel JSON (no image). Optional ``maxSize``
    (default 256) applies to image paths only via a temp ``resolutionTOP`` that
    is always destroyed. ``preview`` (and aliases ``chop_image`` / ``pop``)
    retarget the bridge's shared ``capture_viewer`` OP Viewer TOP — any family.
    Cooking is left to TD on read / ``saveByteArray`` (no force-cook).
    """
    path = params.get("path") or ""
    mode = params.get("mode") or "auto"
    context_path = params.get("contextPath")
    max_size = params.get("maxSize", 256)
    node = tdmcp_resolve(path, context_path)
    if node is None or not getattr(node, "valid", False):
        return {"ok": False, "code": "tdmcp.op.not_found", "path": path}

    effective = _effective_capture_mode(str(mode), node)

    if effective == "chop_data":
        return _capture_chop_data(node, path)

    import td  # type: ignore  # PNG / shared-viewer paths need the TD module

    # chop_image / pop are aliases of preview (shared OP Viewer path).
    if effective in ("preview", "chop_image", "pop"):
        return _capture_via_shared_viewer(
            td, node, path, max_size, mode=effective
        )

    if effective == "top":
        # Explicit top on non-TOP → wrong_family (not no_path).
        if _op_family(node) not in (None, "TOP") and not hasattr(
            node, "saveByteArray"
        ):
            fam = _op_family(node)
            return {
                "ok": False,
                "code": "tdmcp.perception.wrong_family",
                "message": f"mode top requires TOP; resolved family is {fam!r}",
                "path": getattr(node, "path", path),
                "mode": "top",
                "family": fam,
            }
        result = _capture_top_image(td, node, path, max_size)
        if (
            result.get("code") == "tdmcp.perception.no_path"
            and _op_family(node) not in (None, "TOP")
        ):
            fam = _op_family(node)
            return {
                "ok": False,
                "code": "tdmcp.perception.wrong_family",
                "message": f"mode top requires TOP; resolved family is {fam!r}",
                "path": getattr(node, "path", path),
                "mode": "top",
                "family": fam,
            }
        return result

    # Unknown mode string (should not happen for schema-validated callers).
    fam = _op_family(node)
    return {
        "ok": False,
        "code": "tdmcp.perception.wrong_family",
        "message": f"unsupported capture mode {effective!r} for family {fam!r}",
        "path": getattr(node, "path", path),
        "mode": effective,
        "family": fam,
    }


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


def _perception_frame_message(kind: str, mean_rgb: tuple[float, ...] | None) -> str:
    """Human message for black / uniform perception failures."""
    if mean_rgb is not None and len(mean_rgb) >= 3:
        rgb = f"{mean_rgb[0]:.2f},{mean_rgb[1]:.2f},{mean_rgb[2]:.2f}"
    elif mean_rgb is not None and mean_rgb:
        rgb = ",".join(f"{c:.2f}" for c in mean_rgb)
    else:
        rgb = None
    if kind == "black":
        base = "Captured TOP frame is black"
    else:
        base = "Captured TOP frame is a uniform solid color"
    if rgb is None:
        return f"{base} — perception fail"
    return f"{base} (mean rgb≈{rgb})"


def _classify_frame(
    target, data: bytes | None
) -> tuple[str | None, tuple[float, ...] | None]:
    """Classify a captured TOP as black / uniform / ok via `TOP.numpyArray`.

    Returns ``(kind, mean_rgb)`` where ``kind`` is ``"black"``, ``"uniform"``,
    or ``None`` (ok). ``mean_rgb`` is set when pixels were sampled.

    Black = mean RGB ≤ 1/255. Uniform = not black and per-channel spatial
    range (max−min) ≤ 2/255 (solid red counts; global max−min across channels
    would not). `saveByteArray` image size is not a reliable color signal —
    solid colors of *any* value compress similarly. Falls back to the old
    tiny-file heuristic as **black** only when `numpyArray` isn't available
    or produced no bytes — size alone cannot distinguish white from black.
    """
    if hasattr(target, "numpyArray"):
        try:
            arr = target.numpyArray(delayed=False)
            if arr is not None and arr.size:
                channels = min(3, arr.shape[-1]) if arr.ndim >= 3 else 1
                sample = arr[..., :channels] if arr.ndim >= 3 else arr
                mean_val = float(sample.mean())
                if sample.ndim >= 3:
                    ch_means = sample.mean(axis=(0, 1))
                    mean_rgb = tuple(float(x) for x in ch_means[:channels])
                    # Per-channel spatial range — solid red is uniform even though
                    # R≠G (global max−min across all samples would be ~1.0).
                    ch_max = sample.max(axis=(0, 1))
                    ch_min = sample.min(axis=(0, 1))
                    rng = max(
                        float(ch_max[i] - ch_min[i]) for i in range(channels)
                    )
                else:
                    mean_rgb = (mean_val,)
                    rng = float(sample.max() - sample.min())
                if mean_val <= _BLACK_MEAN_THRESHOLD:
                    return "black", mean_rgb
                if rng <= _UNIFORM_RANGE_THRESHOLD:
                    return "uniform", mean_rgb
                return None, mean_rgb
        except Exception:  # noqa: BLE001 — fall through to byte-size heuristic
            pass
    if not data or len(data) < 200:
        return "black", None
    return None, None


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


def _par_mode_name(p: Any) -> str:
    """Stable ParMode name string (CONSTANT / EXPRESSION / …)."""
    mode = getattr(p, "mode", None)
    if mode is None:
        return "CONSTANT"
    name = getattr(mode, "name", None)
    if isinstance(name, str) and name:
        return name
    s = str(mode)
    if "." in s:
        return s.rsplit(".", 1)[-1]
    return s or "CONSTANT"


def _json_safe_par_val(val: Any) -> Any:
    """Coerce evaluated par values so json.dumps never dies on td.OP etc."""
    if val is None or isinstance(val, (bool, int, float, str)):
        return val
    path = getattr(val, "path", None)
    if isinstance(path, str):
        return path
    if callable(path):
        try:
            resolved = path()
            if isinstance(resolved, str):
                return resolved
        except Exception:  # noqa: BLE001
            pass
    try:
        json.dumps(val)
        return val
    except (TypeError, ValueError, OverflowError):
        return str(val)


def _inspect_param_entry(p: Any) -> dict[str, Any]:
    """One params[] entry: name + mode + JSON-safe val; expr when EXPRESSION."""
    mode = _par_mode_name(p)
    entry: dict[str, Any] = {"name": getattr(p, "name", None), "mode": mode}
    try:
        entry["val"] = _json_safe_par_val(p.eval())
    except Exception:  # noqa: BLE001
        entry["val"] = None
    if mode == "EXPRESSION":
        expr = getattr(p, "expr", None)
        entry["expr"] = "" if expr is None else str(expr)
    return entry


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
    }
    if want_nodes:
        out["childCount"] = child_count
        out["childrenReturned"] = len(children)
        out["children"] = children
        if len(children) < child_count:
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
        out["params"] = [_inspect_param_entry(p) for p in n.pars()]
    if want_errors:
        out["errors"] = _op_messages(getattr(n, "errors", lambda: ""))
    if want_warnings:
        out["warnings"] = _op_messages(getattr(n, "warnings", lambda: ""))
    return out


def handle_inspect(params: dict[str, Any]) -> dict[str, Any]:
    """Structural read for an explicit list of paths (no auto-recursion).

    Requires live TD. Each path is shaped independently; a bad path does not
    fail the whole batch (partial success). Soft-caps at ``INSPECT_PATHS_LIMIT``
    with ``tdmcp.op.paths_truncated``. Cooking is left to TD / the caller.
    """
    import td  # type: ignore  # noqa: F401 — ensure TD runtime is importable

    raw_paths = params.get("paths")
    if raw_paths is None and params.get("path") is not None:
        # Backward-compat: single path → one-element batch (Rust schema requires paths).
        raw_paths = [params.get("path")]
    if not isinstance(raw_paths, list) or len(raw_paths) == 0:
        return {
            "ok": False,
            "code": "tdmcp.op.paths_required",
            "message": "inspect requires a non-empty paths array",
        }

    context_path = params.get("contextPath")
    include = params.get("include") or []
    detail_level = params.get("detailLevel") or "summary"

    if not include:
        want_nodes = want_errors = want_warnings = True
        want_params = False
    else:
        want_nodes = "nodes" in include
        want_params = "params" in include
        want_errors = "errors" in include
        want_warnings = "warnings" in include

    path_list = [str(p) for p in raw_paths]
    truncated = False
    if len(path_list) > INSPECT_PATHS_LIMIT:
        path_list = path_list[:INSPECT_PATHS_LIMIT]
        truncated = True

    nodes_out: list[dict[str, Any]] = []
    for path in path_list:
        node = tdmcp_resolve(path, context_path)
        if node is None or not getattr(node, "valid", False):
            nodes_out.append({
                "ok": False,
                "path": path,
                "code": "tdmcp.op.not_found",
                "message": f"operator not found: {path}",
            })
            continue
        try:
            shaped = build_inspect_node(
                node,
                detail_level=detail_level,
                want_nodes=want_nodes,
                want_params=want_params,
                want_errors=want_errors,
                want_warnings=want_warnings,
            )
            shaped["ok"] = True
            nodes_out.append(shaped)
        except Exception as exc:  # noqa: BLE001
            nodes_out.append({
                "ok": False,
                "path": getattr(node, "path", path),
                "code": "tdmcp.op.inspect_failed",
                "message": str(exc),
                "traceback": traceback.format_exc(),
            })

    out: dict[str, Any] = {"ok": True, "nodes": nodes_out}
    if truncated:
        out["pathsTruncated"] = True
        out["truncation"] = {
            "field": "paths",
            "limit": INSPECT_PATHS_LIMIT,
            "code": "tdmcp.op.paths_truncated",
            "message": (
                f"Inspect paths batch capped at {INSPECT_PATHS_LIMIT} "
                f"of {len(raw_paths)}"
            ),
            "mitigation": [
                "Split into multiple inspect calls",
                "Keep batches small for responsive inspect",
            ],
        }
    return out


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


def _suggest_names(name: str, candidates: list[str], *, n: int = 3) -> list[str]:
    """Case-insensitive near-miss suggestions via difflib. Never raises."""
    try:
        if not isinstance(name, str) or not name or not candidates:
            return []
        lower_map: dict[str, str] = {}
        for c in candidates:
            if isinstance(c, str) and c and not c.startswith("_"):
                lower_map.setdefault(c.lower(), c)
        if not lower_map:
            return []
        key = name.lower()
        out: list[str] = []
        if key in lower_map:
            out.append(lower_map[key])
        for m in difflib.get_close_matches(key, list(lower_map.keys()), n=n, cutoff=0.5):
            cand = lower_map[m]
            if cand not in out:
                out.append(cand)
            if len(out) >= n:
                break
        return out[:n]
    except Exception:  # noqa: BLE001
        return []


def _par_names(node: Any) -> list[str]:
    """Best-effort list of .par names on ``node`` (via ``pars()``)."""
    try:
        pars_fn = getattr(node, "pars", None)
        if not callable(pars_fn):
            return []
        names: list[str] = []
        for p in pars_fn() or []:
            n = getattr(p, "name", None)
            if isinstance(n, str) and n:
                names.append(n)
        return names
    except Exception:  # noqa: BLE001
        return []


def _with_similar_param_hint(err: dict[str, Any], node: Any) -> dict[str, Any]:
    """Best-effort near-miss .par name lint. Never raises; never changes code/ok."""
    try:
        name = err.get("field")
        if not isinstance(name, str) or not name:
            return err
        suggestions = _suggest_names(name, _par_names(node))
        if not suggestions:
            return err
        top = suggestions[0]
        out = dict(err)
        out["message"] = f"unknown parameter: {name} (did you mean: {top}?)"
        out["lints"] = [
            {
                "severity": "lint",
                "code": "tdmcp.par.similar_name",
                "message": f"similar parameter '{top}' found on node",
                "confidence": "medium",
                "suggestion": {"replace": top},
            }
        ]
        return out
    except Exception:  # noqa: BLE001
        return err


def _with_similar_type_hint(err: dict[str, Any], ctx: "MutateContext") -> dict[str, Any]:
    """Best-effort near-miss opType lint. Never raises; never changes code/ok."""
    try:
        name = err.get("field")
        if not isinstance(name, str) or not name:
            return err
        list_fn = getattr(ctx, "list_op_type_names", None)
        candidates = list_fn() if callable(list_fn) else []
        if not isinstance(candidates, list):
            return err
        suggestions = _suggest_names(name, [c for c in candidates if isinstance(c, str)])
        if not suggestions:
            return err
        top = suggestions[0]
        out = dict(err)
        out["message"] = f"unknown opType: {name} (did you mean: {top}?)"
        out["lints"] = [
            {
                "severity": "lint",
                "code": "tdmcp.op.similar_type",
                "message": f"similar opType '{top}' found",
                "confidence": "medium",
                "suggestion": {"replace": top},
            }
        ]
        return out
    except Exception:  # noqa: BLE001
        return err


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
        if as_param:
            return _with_similar_param_hint(err, node)
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

    def list_op_type_names(self) -> list[str]:
        """Candidate opType names for near-miss suggestions (default: none)."""
        return []

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

    def list_op_type_names(self) -> list[str]:
        import td  # type: ignore

        try:
            return [n for n in dir(td) if isinstance(n, str) and not n.startswith("_")]
        except Exception:  # noqa: BLE001
            return []

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


def _rollback_create(created: Any) -> None:
    """Best-effort destroy of a just-created node. Never raises; never masks the step error."""
    try:
        destroy = getattr(created, "destroy", None)
        if callable(destroy):
            destroy()
    except Exception:  # noqa: BLE001
        pass


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
        return _with_similar_type_hint(
            {
                "ok": False,
                "code": "tdmcp.op.unknown_type",
                "message": f"unknown opType: {op_type}",
                "path": full,
                "field": op_type,
            },
            ctx,
        )
    created = parent.create(op_cls, name)
    raw_created = getattr(created, "path", None)
    if isinstance(raw_created, str) and raw_created.strip():
        created_path = _absolutize_path(raw_created, None)
    else:
        # Missing path → assume requested; never emit a false rename lint.
        created_path = full
    values = step.get("values")
    if values:
        err = _apply_values(created, values)
        if err is not None:
            err["path"] = created_path
            _rollback_create(created)
            return err
    flags = step.get("flags")
    if flags:
        err = _apply_flags(created, flags)
        if err is not None:
            err["path"] = created_path
            _rollback_create(created)
            return err
    out: dict[str, Any] = {"ok": True, "path": created_path}
    if detail_level == "detailed":
        if values:
            out["values"] = values
        if flags:
            out["flags"] = flags
    if created_path != full:
        try:
            out["lints"] = [
                {
                    "severity": "lint",
                    "code": "tdmcp.op.renamed",
                    "message": (
                        f"requested '{full}', created as '{created_path}'"
                    ),
                    "confidence": "high",
                    "suggestion": {
                        "opPath": created_path,
                        "replace": created_path,
                    },
                }
            ]
        except Exception:  # noqa: BLE001 — never change ok after materialization
            pass
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


def _rewrite_step_aliases(
    step: dict[str, Any],
    aliases: dict[str, str],
    context_path: str | None,
) -> dict[str, Any]:
    """Shallow-copy ``step`` and remap path/src/dst via create aliases.

    Best-effort: never raises; returns ``step`` unchanged on any failure.
    """
    if not aliases:
        return step
    try:
        out = dict(step)
        for key in ("path", "src", "dst"):
            raw = out.get(key)
            if not isinstance(raw, str) or not raw:
                continue
            abs_path = _absolutize_path(raw, context_path)
            mapped = aliases.get(abs_path)
            if mapped:
                out[key] = mapped
        return out
    except Exception:  # noqa: BLE001
        return step


def _alias_lookup(
    path: str, aliases: dict[str, str], context_path: str | None
) -> str:
    """Absolutize then apply alias; best-effort, never raises."""
    try:
        if not path:
            return path
        abs_path = _absolutize_path(path, context_path)
        return aliases.get(abs_path, abs_path)
    except Exception:  # noqa: BLE001
        return path


def run_mutate_steps(
    ctx: MutateContext,
    steps: list[dict[str, Any]],
    *,
    context_path: str | None = None,
    detail_level: str = "summary",
) -> dict[str, Any]:
    """Sequential apply; stop on first hard error; mark rest skipped.

    When a create step is auto-renamed by TD, later steps in this batch that
    still reference the requested path are remapped to the actual created path
    (create-intent wins inside one ``mutate_nodes`` call).
    """
    results: list[dict[str, Any]] = []
    applied = 0
    failed_at: int | None = None
    aliases: dict[str, str] = {}
    for i, step in enumerate(steps):
        if failed_at is not None:
            raw_path = step.get("path") or step.get("dst") or ""
            results.append(
                {
                    "ok": False,
                    "skipped": True,
                    "code": "tdmcp.batch.skipped_dependent",
                    "path": _alias_lookup(raw_path, aliases, context_path)
                    if raw_path
                    else raw_path,
                }
            )
            continue
        rewritten = _rewrite_step_aliases(step, aliases, context_path)
        result = apply_step(
            ctx, rewritten, context_path=context_path, detail_level=detail_level
        )
        results.append(result)
        if result.get("ok"):
            applied += 1
            if (step.get("op") or "") == "create":
                try:
                    requested = _absolutize_path(
                        step.get("path") or "", context_path
                    )
                    actual = result.get("path")
                    if (
                        isinstance(actual, str)
                        and actual
                        and actual != requested
                    ):
                        aliases[requested] = actual
                except Exception:  # noqa: BLE001 — never fail the batch
                    pass
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
                raise MidFrameTimeout("uds read timed out mid-frame") from exc
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
                    raise MidFrameTimeout("named pipe read timed out mid-frame")
                break
            if read.value == 0:
                # Timeout with zero bytes can also surface as success+0.
                if self._read_timeout_ms is not None and not out:
                    raise TimeoutError("named pipe read timed out")
                if self._read_timeout_ms is not None and out:
                    raise MidFrameTimeout("named pipe read timed out mid-frame")
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
        except MidFrameTimeout:
            # Partial frame already consumed — stream is desynced; disconnect.
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
                _write_frame(stream, dispatch(msg))
                continue
            response_slot: "queue.Queue[dict[str, Any]]" = queue.Queue(maxsize=1)
            _enqueue_pending(msg, response_slot)
            resp = response_slot.get()
            _write_frame(stream, resp)
        except Exception:  # noqa: BLE001 — never kill the daemon thread silently
            sys.stderr.write(
                "tdmcp_bridge: serve_queued stopping after dispatch/write error\n"
            )
            traceback.print_exc(file=sys.stderr)
            break


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
