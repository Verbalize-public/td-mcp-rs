"""Curated structured exception reports for execute_python soft-fails."""

from __future__ import annotations

import traceback
import types
from types import FrameType, TracebackType
from typing import Any

_LOCALS_MAX_NAMES = 8
_LOCALS_REPR_MAX = 120


def build_exception_report(
    exc: BaseException,
    script: str,
    *,
    format_mode: str = "normal",
) -> dict[str, Any]:
    """Build a small structured exception object for daemon mapping.

    Shape: ``{type, message, frames, syntax, raw}``.
    When ``format_mode == "debug"``, ``<string>`` frames may include capped
    ``locals`` (type + truncated repr).
    """
    tb = exc.__traceback__
    raw = "".join(traceback.format_exception(type(exc), exc, tb))
    frames = _build_frames(tb, script, format_mode=format_mode)
    syntax = _syntax_block(exc)
    return {
        "type": type(exc).__name__,
        "message": str(exc),
        "frames": frames,
        "syntax": syntax,
        "raw": raw,
    }


def _build_frames(
    tb: TracebackType | None,
    script: str,
    *,
    format_mode: str,
) -> list[dict[str, Any]]:
    if tb is None:
        return []
    script_lines = script.splitlines()
    summaries = traceback.extract_tb(tb)
    walked = list(traceback.walk_tb(tb))
    want_locals = format_mode == "debug"
    out: list[dict[str, Any]] = []
    for i, fs in enumerate(summaries):
        line = fs.line
        if fs.filename == "<string>" and fs.lineno and not line:
            idx = fs.lineno - 1
            if 0 <= idx < len(script_lines):
                line = script_lines[idx]
        frame: dict[str, Any] = {
            "filename": fs.filename,
            "lineno": fs.lineno,
            "name": fs.name,
            "line": line,
        }
        if want_locals and fs.filename == "<string>" and i < len(walked):
            py_frame, _lineno = walked[i]
            frame["locals"] = _safe_frame_locals(py_frame)
        out.append(frame)
    return out


def _safe_frame_locals(frame: FrameType) -> dict[str, Any]:
    try:
        raw = frame.f_locals
    except Exception:  # noqa: BLE001
        return {}
    if not isinstance(raw, dict):
        return {}
    items: dict[str, Any] = {}
    for key, value in raw.items():
        if len(items) >= _LOCALS_MAX_NAMES:
            break
        if not isinstance(key, str) or key.startswith("__"):
            continue
        try:
            if callable(value) or isinstance(value, types.ModuleType):
                continue
            type_name = type(value).__name__
            try:
                text = repr(value)
            except Exception:  # noqa: BLE001
                text = "<repr failed>"
            if len(text) > _LOCALS_REPR_MAX:
                text = text[: _LOCALS_REPR_MAX - 1] + "…"
            items[key] = {"type": type_name, "repr": text}
        except Exception:  # noqa: BLE001
            continue
    return items


def _syntax_block(exc: BaseException) -> dict[str, Any] | None:
    if not isinstance(exc, SyntaxError):
        return None
    return {
        "lineno": exc.lineno,
        "offset": exc.offset,
        "text": exc.text,
        "msg": exc.msg,
    }
