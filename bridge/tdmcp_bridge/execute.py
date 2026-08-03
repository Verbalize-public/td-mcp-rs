"""execute_python handler + stdout/stderr tee helpers."""
from __future__ import annotations

import io
import json
import sys
import traceback
from typing import Any

from . import state as _state
from .constants import (
    RESULT_MAX_BYTES,
    SCRIPT_MAX_BYTES,
    _DEBUG_DAT_RING_MAX,
    _LOGS_RETURN_MAX,
    _TRUNC_MARK,
)
from .paths import resolve_op, tdmcp_resolve

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
    host_path = _state.get_bridge_host_path()
    if host_path:
        try:
            host = td.op(host_path)
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


def _json_utf8_size(value: Any) -> int:
    """UTF-8 byte length of ``value`` as JSON (compact separators)."""
    return len(json.dumps(value, separators=(",", ":"), default=str).encode("utf-8"))


def handle_execute_python(params: dict[str, Any]) -> dict[str, Any]:
    from .exception_report import build_exception_report

    script = params.get("script") or ""
    if not isinstance(script, str):
        script = str(script)
    script_bytes = len(script.encode("utf-8"))
    if script_bytes > SCRIPT_MAX_BYTES:
        return {
            "ok": False,
            "error": (
                f"script exceeds {SCRIPT_MAX_BYTES} bytes "
                f"(got {script_bytes}); split the batch or prefer mutate_nodes"
            ),
            "code": "tdmcp.script.too_large",
            "message": (
                f"script exceeds {SCRIPT_MAX_BYTES} bytes "
                f"(got {script_bytes}); split the batch or prefer mutate_nodes"
            ),
        }

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
        "tdmcp_resolve": lambda p: resolve_op(p, context_path),
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
        if _state.get_capture_depth() == 0:
            buf = io.StringIO()
            prev_out, prev_err = sys.stdout, sys.stderr
            sys.stdout = _TeeStream(buf, prev_out)
            sys.stderr = _TeeStream(buf, prev_err)
            installed = True
        _state.set_capture_depth(_state.get_capture_depth() + 1)

    try:
        try:
            exec(script, local_vars, local_vars)  # noqa: S102 — intentional TD script surface
            result_value = local_vars.get("result")
            result_bytes = _json_utf8_size(result_value)
            if result_bytes > RESULT_MAX_BYTES:
                out: dict[str, Any] = {
                    "ok": False,
                    "error": (
                        f"result JSON exceeds {RESULT_MAX_BYTES} bytes "
                        f"(got {result_bytes}); return a smaller result"
                    ),
                    "code": "tdmcp.script.result_too_large",
                    "message": (
                        f"result JSON exceeds {RESULT_MAX_BYTES} bytes "
                        f"(got {result_bytes}); return a smaller result"
                    ),
                }
            else:
                out = {
                    "result": result_value,
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
            _state.set_capture_depth(max(0, _state.get_capture_depth() - 1))
            out["logs"] = logs

    return out


