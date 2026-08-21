"""Structural inspect + child roster."""
from __future__ import annotations

import json
import traceback
from typing import Any

from .constants import (
    CHILDREN_ROSTER_LIMIT,
    ENABLE_EXPR_EVAL_LIMIT,
    ENABLE_PARM_WARN_MARKERS,
    INSPECT_PATHS_LIMIT,
    _ENABLE_EXPR_FAILED_CODE,
    _ENABLE_EXPR_MITIGATION,
)
from .paths import resolve_op

def _child_name(child: Any) -> str:
    """Best-effort operator name; fall back to last path segment."""
    name = getattr(child, "name", None)
    if name:
        return str(name)
    path = getattr(child, "path", "") or ""
    return str(path).rsplit("/", 1)[-1]


def _wire_peer(op: Any) -> dict[str, Any] | None:
    """Shape one wired peer as {path, name, opType}, or None for an empty slot."""
    if op is None:
        return None
    path = getattr(op, "path", None)
    if path is None:
        return None
    return {
        "path": str(path),
        "name": _child_name(op),
        "opType": getattr(op, "opType", None),
    }


def _wire_peers(seq: Any) -> list[dict[str, Any] | None]:
    """Positional wire list from TD OP.inputs / OP.outputs; best-effort → []."""
    try:
        items = list(seq) if seq is not None else []
    except Exception:  # noqa: BLE001 — wire enrichment must never fail inspect
        return []
    return [_wire_peer(item) for item in items]


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


def _is_enable_parm_warning(msg: str) -> bool:
    """True when a TD warning string mentions enable-parm expression failures."""
    lower = msg.lower()
    return any(marker in lower for marker in ENABLE_PARM_WARN_MARKERS)


def _collect_enable_expr_issues(n: Any) -> list[dict[str, Any]]:
    """Eval unique custom enableExprs; return structured failures (capped)."""
    issues: list[dict[str, Any]] = []
    seen: set[str] = set()
    groups = list(getattr(n, "customParGroups", None) or [])
    targets = groups or list(getattr(n, "customPars", None) or [])
    eval_fn = getattr(n, "evalExpression", None)
    for item in targets:
        if len(seen) >= ENABLE_EXPR_EVAL_LIMIT:
            break
        expr = (getattr(item, "enableExpr", None) or "").strip()
        if not expr or expr in seen:
            continue
        seen.add(expr)
        if not callable(eval_fn):
            continue
        try:
            eval_fn(expr)
        except Exception as e:  # noqa: BLE001 — surface type/message only
            issues.append({
                "kind": "enableExpr",
                "par": getattr(item, "name", None),
                "label": getattr(item, "label", None),
                "expr": expr,
                "errorType": type(e).__name__,
                "message": str(e),
            })
    return issues


def _enable_expr_diagnostics(issues: list[dict[str, Any]]) -> list[dict[str, Any]]:
    """Catalog-shaped soft diagnostics parallel to parmExprIssues."""
    out: list[dict[str, Any]] = []
    for issue in issues:
        par = issue.get("par")
        out.append({
            "code": _ENABLE_EXPR_FAILED_CODE,
            "severity": "warning",
            "message": f"Enable expression failed on {par}",
            "mitigation": list(_ENABLE_EXPR_MITIGATION),
            "context": {"par": par, "expr": issue.get("expr")},
        })
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
        try:
            out["inputs"] = _wire_peers(getattr(n, "inputs", []))
        except Exception:  # noqa: BLE001
            out["inputs"] = []
        try:
            out["outputs"] = _wire_peers(getattr(n, "outputs", []))
        except Exception:  # noqa: BLE001
            out["outputs"] = []
    if want_params:
        out["params"] = [_inspect_param_entry(p) for p in n.pars()]
    if want_errors:
        out["errors"] = _op_messages(getattr(n, "errors", lambda: ""))
    if want_warnings:
        warnings = _op_messages(getattr(n, "warnings", lambda: ""))
        out["warnings"] = warnings
        if any(_is_enable_parm_warning(w) for w in warnings):
            try:
                issues = _collect_enable_expr_issues(n)
            except Exception:  # noqa: BLE001 — enrichment must never fail inspect
                issues = []
            if issues:
                out["parmExprIssues"] = issues
                out["diagnostics"] = _enable_expr_diagnostics(issues)
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
        node = resolve_op(path, context_path)
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

