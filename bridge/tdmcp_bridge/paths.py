"""Path resolution helpers shared by inspect / mutate / capture."""

from __future__ import annotations

from typing import Any

from . import state as _state


def tdmcp_resolve(path: str, context_path: str | None = None):
    """Optional OpPath helper for execute_python scripts."""
    import td  # type: ignore

    if path.startswith("/"):
        return td.op(path)
    base = context_path or "/project1"
    return td.op(base).op(path) if td.op(base) is not None else td.op(path)


def resolve_op(path: str, context_path: str | None = None):
    """Resolve via package ``tdmcp_resolve`` so tests can monkeypatch it."""
    import sys

    pkg = sys.modules.get(__package__)
    if pkg is not None:
        fn = getattr(pkg, "tdmcp_resolve", None)
        if callable(fn):
            return fn(path, context_path)
    return tdmcp_resolve(path, context_path)


def set_bridge_host(comp) -> None:
    """Record the bootstrap COMP path so ``./debug`` can be resolved without op.Debug."""
    try:
        path = getattr(comp, "path", None)
        if isinstance(path, str) and path:
            _state.set_bridge_host_path(path)
    except Exception:  # noqa: BLE001
        pass


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
