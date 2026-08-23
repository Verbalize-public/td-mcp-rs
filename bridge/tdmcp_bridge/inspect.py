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
from .mutate import _TdMutateContext
from .shader_lint import (
    _GLSL_OP_TYPES,
    _GLSL_STAGE_PARS,
    _eval_par,
    classify_compile_result,
    lint_dat_consumers,
)

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


# GLSL opTypes / stage-par maps / _eval_par are imported from shader_lint
# (single source of truth) and remain accessible as inspect-module names.


def _text_bytes(text: str) -> int:
    """UTF-8 byte length of a text body."""
    return len(text.encode("utf-8"))


def _dat_content(n: Any) -> dict[str, Any]:
    """Shape DAT body from ``.text`` / ``isText`` / ``isTable``."""
    raw = getattr(n, "text", None)
    text = "" if raw is None else str(raw)
    return {
        "kind": "dat",
        "isText": bool(getattr(n, "isText", False)),
        "isTable": bool(getattr(n, "isTable", False)),
        "bytes": _text_bytes(text),
        "text": text,
    }


def _shader_stage_from_ref(role: str, ref: Any) -> dict[str, Any] | None:
    """Follow one shader DAT ref into a stage object; None when unset."""
    if ref is None:
        return None
    # Bare path string (unusual but possible) — report without body.
    if isinstance(ref, str):
        path = ref.strip()
        if not path:
            return None
        return {
            "role": role,
            "path": path,
            "error": "shader DAT ref is a path string, not an OP",
        }
    path = getattr(ref, "path", None)
    if path is None:
        return None
    path_s = str(path)
    stage: dict[str, Any] = {
        "role": role,
        "path": path_s,
        "opType": getattr(ref, "opType", None),
    }
    if getattr(ref, "valid", True) is False:
        stage["error"] = "shader DAT ref is invalid"
        return stage
    try:
        raw = getattr(ref, "text", None)
        text = "" if raw is None else str(raw)
        stage["bytes"] = _text_bytes(text)
        stage["text"] = text
    except Exception as exc:  # noqa: BLE001 — surface follow error, keep node ok
        stage["error"] = str(exc) or type(exc).__name__
    return stage


def _shader_content(n: Any) -> dict[str, Any]:
    """Shape GLSL content: compileResult + followed DAT stages."""
    op_type = getattr(n, "opType", None) or ""
    compile_raw = getattr(n, "compileResult", None)
    compile_result = "" if compile_raw is None else str(compile_raw)
    stages: list[dict[str, Any]] = []
    for par_name, role in _GLSL_STAGE_PARS.get(op_type, ()):
        ref = _eval_par(n, par_name)
        stage = _shader_stage_from_ref(role, ref)
        if stage is not None:
            stages.append(stage)
    out: dict[str, Any] = {
        "kind": "shader",
        "compileResult": compile_result,
        "stages": stages,
    }
    # Same classifier as mutate lint; omitted for unsupported surfaces (glslPOP).
    verdict = classify_compile_result(op_type, compile_raw)
    if verdict["code"] != "tdmcp.shader.unsupported_consumer":
        out["compileState"] = "error" if verdict["severity"] == "error" else "compiled"
    return out


def _attach_dat_consumers(
    content: dict[str, Any], n: Any, lint_ctx: Any, scope_root: str | None
) -> None:
    """Add consumers[] (+ truncation keys) to a DAT content object. Never raises."""
    try:
        result = lint_dat_consumers(lint_ctx, str(getattr(n, "path", "")), scope_root)
    except Exception:  # noqa: BLE001 — enrichment must never fail inspect
        return
    if not result:
        return
    content["consumers"] = result.get("consumers") or []
    for key in ("consumersTruncated", "truncation"):
        if key in result:
            content[key] = result[key]


def _attach_content(
    n: Any,
    out: dict[str, Any],
    *,
    lint_ctx: Any = None,
    scope_root: str | None = None,
) -> None:
    """Attach ``content`` when node is DAT or known GLSL op; omit otherwise.

    When ``lint_ctx`` is supplied, DAT content additionally carries the shared
    shader-consumer diagnostics (docs/SHADER_LINT.md §3/§5).
    """
    try:
        family = getattr(n, "family", None)
        is_dat = family == "DAT" or bool(getattr(n, "isDAT", False))
        if is_dat:
            content = _dat_content(n)
            if lint_ctx is not None:
                _attach_dat_consumers(content, n, lint_ctx, scope_root)
            out["content"] = content
            return
        op_type = getattr(n, "opType", None)
        if op_type in _GLSL_OP_TYPES:
            out["content"] = _shader_content(n)
    except Exception:  # noqa: BLE001 — content must never fail inspect
        return


def build_inspect_node(
    n: Any,
    *,
    detail_level: str = "summary",
    want_nodes: bool = True,
    want_params: bool = False,
    want_errors: bool = False,
    want_warnings: bool = False,
    want_content: bool = False,
    lint_ctx: Any = None,
    scope_root: str | None = None,
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
    if want_content:
        _attach_content(n, out, lint_ctx=lint_ctx, scope_root=scope_root)
    return out


def handle_inspect(params: dict[str, Any]) -> dict[str, Any]:
    """Structural read for an explicit list of paths (no auto-recursion).

    Requires live TD. Each path is shaped independently; a bad path does not
    fail the whole batch (partial success). Soft-caps at ``INSPECT_PATHS_LIMIT``
    with ``tdmcp.op.paths_truncated``. Cooking is left to TD / the caller.

    With ``content``: DAT nodes carry ``consumers[]`` shader diagnostics and
    GLSL nodes a classified ``compileState``. Reading ``compileResult`` forces
    a synchronous recompile of that consumer (docs/SHADER_LINT.md §3).
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
        want_content = False
    else:
        want_nodes = "nodes" in include
        want_params = "params" in include
        want_errors = "errors" in include
        want_warnings = "warnings" in include
        want_content = "content" in include

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
                want_content=want_content,
                lint_ctx=(_TdMutateContext(context_path) if want_content else None),
                scope_root=context_path or "/project1",
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

