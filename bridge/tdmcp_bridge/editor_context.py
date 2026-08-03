"""Live editor pane / selection snapshot."""
from __future__ import annotations

import traceback
from typing import Any

from .constants import EDITOR_PANES_LIMIT, EDITOR_SELECTION_LIMIT


def _pane_type_name(pane: Any) -> str | None:
    """Stable PaneType name string (NETWORKEDITOR / PANEL / …)."""
    pane_type = getattr(pane, "type", None)
    if pane_type is None:
        return None
    name = getattr(pane_type, "name", None)
    if isinstance(name, str) and name:
        return name
    s = str(pane_type)
    if "." in s:
        return s.rsplit(".", 1)[-1]
    return s or None


def _op_path(op: Any) -> str | None:
    path = getattr(op, "path", None)
    return path if isinstance(path, str) else None


def _build_selection(owner: Any) -> tuple[list[dict[str, Any]], int]:
    """Return (capped selection entries, total count before cap).

    Empty when owner is missing / non-COMP / has no selectedChildren.
    """
    if owner is None or not bool(getattr(owner, "isCOMP", False)):
        return [], 0
    try:
        raw = list(getattr(owner, "selectedChildren", None) or [])
    except Exception:  # noqa: BLE001 — selection read must not fail the pane
        return [], 0

    current = getattr(owner, "currentChild", None)
    current_path = _op_path(current)
    total = len(raw)
    out: list[dict[str, Any]] = []
    for child in raw[:EDITOR_SELECTION_LIMIT]:
        path = _op_path(child)
        if not path:
            continue
        entry: dict[str, Any] = {"path": path, "current": False}
        if current_path is not None and path == current_path:
            entry["current"] = True
        out.append(entry)
    return out, total


def _shape_pane(pane: Any, focused_id: int | None) -> dict[str, Any]:
    """Shape one pane entry (raises on hard failures — caller wraps)."""
    pane_id = getattr(pane, "id", None)
    focused = False
    if focused_id is not None and pane_id is not None:
        try:
            focused = int(pane_id) == int(focused_id)
        except (TypeError, ValueError):
            focused = False

    owner = getattr(pane, "owner", None)
    owner_path = _op_path(owner)
    selection, selection_total = _build_selection(owner)

    out: dict[str, Any] = {
        "ok": True,
        "id": pane_id,
        "name": getattr(pane, "name", None),
        "type": _pane_type_name(pane),
        "focused": focused,
        "ownerPath": owner_path,
    }
    if selection:
        out["selection"] = selection
        if len(selection) < selection_total:
            out["selectionTruncated"] = True
            out["truncation"] = {
                "field": "selection",
                "limit": EDITOR_SELECTION_LIMIT,
                "code": "tdmcp.editor.selection_truncated",
                "message": (
                    f"Selection capped at {EDITOR_SELECTION_LIMIT} of {selection_total}"
                ),
                "mitigation": [
                    "Narrow selection in TD",
                    "Use inspect on the owner COMP for full roster",
                ],
            }
    return out


def handle_editor_context(_params: dict[str, Any]) -> dict[str, Any]:
    """Live multi-pane editor snapshot (owner + selection).

    Requires live TD. Iterates ``td.ui.panes``; a bad pane does not fail the
    whole batch (partial success). Soft-caps panes and per-pane selection.
    """
    try:
        import td  # type: ignore  # noqa: F401 — ensure TD runtime is importable

        ui = getattr(td, "ui", None)
        if ui is None:
            return {"ok": True, "panes": []}

        panes_obj = getattr(ui, "panes", None)
        if panes_obj is None:
            return {"ok": True, "panes": []}

        focused_id: int | None = None
        try:
            current = getattr(panes_obj, "current", None)
            if current is not None:
                cid = getattr(current, "id", None)
                if cid is not None:
                    focused_id = int(cid)
        except Exception:  # noqa: BLE001 — no focused pane is fine
            focused_id = None

        try:
            raw_panes = list(panes_obj)
        except Exception:  # noqa: BLE001
            raw_panes = []

        pane_count = len(raw_panes)
        panes_out: list[dict[str, Any]] = []
        for pane in raw_panes[:EDITOR_PANES_LIMIT]:
            try:
                panes_out.append(_shape_pane(pane, focused_id))
            except Exception as exc:  # noqa: BLE001 — partial success
                panes_out.append({
                    "ok": False,
                    "id": getattr(pane, "id", None),
                    "name": getattr(pane, "name", None),
                    "code": "tdmcp.editor.pane_failed",
                    "message": str(exc),
                })

        out: dict[str, Any] = {"ok": True, "panes": panes_out}
        if len(panes_out) < pane_count:
            out["panesTruncated"] = True
            out["truncation"] = {
                "field": "panes",
                "limit": EDITOR_PANES_LIMIT,
                "code": "tdmcp.editor.panes_truncated",
                "message": (
                    f"Panes capped at {EDITOR_PANES_LIMIT} of {pane_count}"
                ),
                "mitigation": [
                    "Close unused panes in TD",
                    "Soft cap is 32 panes per call",
                ],
            }
        return out
    except Exception as exc:  # noqa: BLE001 — coded top-level failure
        return {
            "ok": False,
            "code": "tdmcp.editor.context_failed",
            "message": str(exc),
            "error": str(exc),
            "traceback": traceback.format_exc(),
        }
