"""JSON-true coercion for execute_python results (and wire defense).

Never invokes callables. TD OP-like objects become path cards. Used so
``json.dumps`` without ``default=`` cannot tear down ``serve_queued``.
"""

from __future__ import annotations

import json
from typing import Any

_MAX_DEPTH = 32
_REPR_LIMIT = 120


def _is_op_like(value: Any) -> bool:
    path = getattr(value, "path", None)
    if not isinstance(path, str) or not path:
        return False
    # Prefer TD OP markers; avoid plain objects that happen to have .path
    if getattr(value, "OPType", None) is not None or getattr(value, "opType", None) is not None:
        return True
    if getattr(value, "family", None) is not None:
        return True
    return type(value).__name__.endswith(("OP", "COMP", "TOP", "CHOP", "SOP", "POP", "DAT", "MAT"))


def _callable_name(value: Any) -> str:
    name = getattr(value, "__name__", None)
    if isinstance(name, str) and name:
        return name
    return type(value).__name__


def json_safe(value: Any, *, depth: int = 0) -> Any:
    """Return a value that ``json.dumps`` can encode without ``default=``.

    Raises ``TypeError`` only if recursion/coercion itself fails unexpectedly;
    callers may catch and emit ``tdmcp.script.result_not_serializable``.
    """
    if depth > _MAX_DEPTH:
        return {"__td": "repr", "type": "max_depth", "repr": "…"}

    if value is None or isinstance(value, (bool, int, float, str)):
        return value

    if isinstance(value, (bytes, bytearray, memoryview)):
        raw = bytes(value)
        return {
            "__td": "bytes",
            "encoding": "base64",
            "nbytes": len(raw),
            "repr": repr(raw)[:_REPR_LIMIT],
        }

    if isinstance(value, dict):
        out: dict[str, Any] = {}
        for key, item in value.items():
            k = key if isinstance(key, str) else str(key)
            out[k] = json_safe(item, depth=depth + 1)
        return out

    if isinstance(value, (list, tuple)):
        return [json_safe(item, depth=depth + 1) for item in value]

    if _is_op_like(value):
        card: dict[str, Any] = {
            "__td": "op",
            "path": getattr(value, "path", None),
        }
        op_type = getattr(value, "opType", None) or getattr(value, "OPType", None)
        if op_type is not None:
            card["opType"] = str(op_type)
        family = getattr(value, "family", None)
        if family is not None:
            card["family"] = str(family)
        return card

    # Methods / functions / other callables — never invoke.
    if callable(value) and not isinstance(value, type):
        return {
            "__td": "callable",
            "name": _callable_name(value),
            "type": type(value).__name__,
            "hint": "call with ()",
        }

    try:
        json.dumps(value)
        return value
    except (TypeError, ValueError, OverflowError):
        try:
            text = repr(value)
        except Exception:  # noqa: BLE001
            text = f"<unreprable {type(value).__name__}>"
        return {
            "__td": "repr",
            "type": type(value).__name__,
            "repr": text[:_REPR_LIMIT],
        }


def json_utf8_size(value: Any) -> int:
    """UTF-8 byte length of a JSON-true value (no ``default=``)."""
    return len(json.dumps(value, separators=(",", ":")).encode("utf-8"))
