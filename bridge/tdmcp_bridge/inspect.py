"""Structural inspect + child roster."""
from __future__ import annotations

import json
import traceback
from itertools import islice
from typing import Any

from .constants import (
    CHILDREN_ROSTER_LIMIT,
    COMMENT_MAX_CHARS,
    COMMENT_ROSTER_MAX_CHARS,
    ENABLE_EXPR_EVAL_LIMIT,
    ENABLE_PARM_WARN_MARKERS,
    INSPECT_PATHS_LIMIT,
    INSPECT_TEXT_MAX_BYTES,
    _COMMENT_TRUNC_MARK,
    _ENABLE_EXPR_FAILED_CODE,
    _ENABLE_EXPR_MITIGATION,
)
from .paths import resolve_op
from .mutate import _TdMutateContext
from .shader_lint import (
    _GLSL_OP_TYPES,
    _GLSL_STAGE_PARS,
    _eval_par,
    observe_compile_result,
    lint_dat_consumers,
)

def _op_comment(op: Any, limit: int) -> tuple[str, bool] | None:
    """Operator comment capped at ``limit``; ``None`` when empty/unreadable.

    Returns ``(text, truncated)``. ``OP.comment`` is an unbounded str, so long
    bodies are cut and marked rather than shipped whole into a 256-entry
    roster. Never raises — comment is orientation metadata, not a claim.
    """
    try:
        raw = getattr(op, "comment", None)
    except Exception:  # noqa: BLE001 — comment must never fail inspect
        return None
    if raw is None:
        return None
    text = str(raw)
    if not text.strip():
        return None
    if len(text) > limit:
        return text[:limit] + _COMMENT_TRUNC_MARK, True
    return text, False


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


def _wire_peers(seq: Any, *, strict: bool = False) -> list[dict[str, Any] | None]:
    """Positional wire list from TD OP.inputs / OP.outputs; best-effort → []."""
    try:
        items = list(seq) if seq is not None else []
    except Exception:  # noqa: BLE001 — public reads retain availability separately
        if strict:
            raise
        return []
    return [_wire_peer(item) for item in items]


def _op_messages(fn: Any, *, strict: bool = False) -> list[str]:
    """Normalize TD OP.errors()/warnings() (str) or list-like fakes to string[]."""
    try:
        raw = fn()
    except Exception:  # noqa: BLE001
        if strict:
            raise
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


def _unavailable(exc: Exception) -> dict[str, Any]:
    return {
        "available": False,
        "code": "tdmcp.op.observation_unavailable",
        "errorType": type(exc).__name__,
        "message": str(exc)[:256],
    }


def _observe(out: dict[str, Any], field: str, read: Any, fallback: Any) -> Any:
    """Keep compatibility values while distinguishing a failed read from empty."""
    observations = out.setdefault("observations", {})
    try:
        value = read()
    except Exception as exc:  # noqa: BLE001 — section failure, not batch failure
        observations[field] = _unavailable(exc)
        return fallback
    observations[field] = {"available": True}
    return value


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


def _inspect_param_entry(p: Any, *, detailed: bool = False) -> dict[str, Any]:
    """One params[] entry: name + mode + JSON-safe val; expr when EXPRESSION."""
    mode = _par_mode_name(p)
    entry: dict[str, Any] = {"name": getattr(p, "name", None), "mode": mode}
    try:
        entry["val"] = _json_safe_par_val(p.eval())
        entry["evaluation"] = {"available": True}
    except Exception as exc:  # noqa: BLE001
        entry["val"] = None
        entry["evaluation"] = _unavailable(exc)
    if mode == "EXPRESSION":
        expr = getattr(p, "expr", None)
        entry["expr"] = "" if expr is None else str(expr)
    if detailed:
        entry["storedValue"] = _observe(entry, "storedValue", lambda: _json_safe_par_val(p.val), None)
        try:
            if p.isMenu:
                names = list(islice(iter(p.menuNames), 33))
                labels = list(islice(iter(p.menuLabels), 33))
                entry["menu"] = {
                    "available": True,
                    "names": [str(name)[:128] for name in names[:32]],
                    "labels": [str(label)[:128] for label in labels[:32]],
                    "truncated": len(names) > 32 or len(labels) > 32
                    or any(len(str(value)) > 128 for value in names + labels),
                }
        except Exception as exc:  # noqa: BLE001 — metadata does not prove a closed menu
            entry["menu"] = _unavailable(exc)
    return entry


# GLSL opTypes / stage-par maps / _eval_par are imported from shader_lint
# (single source of truth) and remain accessible as inspect-module names.


def _text_bytes(text: str) -> int:
    """UTF-8 byte length of a text body."""
    return len(text.encode("utf-8"))


def _text_preview(raw: Any) -> dict[str, Any]:
    """Bound DAT/shader source text without splitting a UTF-8 character."""
    text = "" if raw is None else str(raw)
    encoded = text.encode("utf-8")
    total = len(encoded)
    if total <= INSPECT_TEXT_MAX_BYTES:
        return {"bytes": total, "text": text}
    preview = encoded[:INSPECT_TEXT_MAX_BYTES].decode("utf-8", errors="ignore")
    return {
        "bytes": _text_bytes(preview),
        "text": preview,
        "totalBytes": total,
        "textTruncation": {
            "field": "text",
            "limit": INSPECT_TEXT_MAX_BYTES,
            "code": "tdmcp.op.content_truncated",
            "message": "Text preview truncated; the DAT itself is unchanged",
            "mitigation": ["Read further slices with execute_python: result = op(path).text[start:end]"],
        },
    }


def _dat_content(n: Any) -> dict[str, Any]:
    """Shape DAT body from ``.text`` / ``isText`` / ``isTable``."""
    raw = getattr(n, "text", None)
    return {
        "kind": "dat",
        "isText": bool(getattr(n, "isText", False)),
        "isTable": bool(getattr(n, "isTable", False)),
        **_text_preview(raw),
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
        stage.update(_text_preview(raw))
    except Exception as exc:  # noqa: BLE001 — surface follow error, keep node ok
        stage["error"] = str(exc) or type(exc).__name__
    return stage


def _shader_content(n: Any) -> dict[str, Any]:
    """Read compile evidence once; preserve failed stage-reference evaluations."""
    op_type = getattr(n, "opType", None) or ""
    verdict = observe_compile_result(n, op_type)
    stages: list[dict[str, Any]] = []
    for par_name, role in _GLSL_STAGE_PARS.get(op_type, ()):
        try:
            par = getattr(getattr(n, "par", None), par_name, None)
            if par is None:
                continue
            ref = par.eval()
            stage = _shader_stage_from_ref(role, ref)
        except Exception as exc:  # noqa: BLE001 — keep the other shader stages
            stage = {"role": role, "parameter": par_name, "evaluation": _unavailable(exc)}
        if stage is not None:
            stages.append(stage)
    return {
        "kind": "shader",
        "compileResult": verdict.get("log", ""),
        "compileDiagnostic": verdict,
        "compileState": {
            "tdmcp.shader.compiled": "compiled",
            "tdmcp.shader.compile_failed": "error",
            "tdmcp.shader.unsupported_consumer": "unsupported",
        }.get(verdict["code"], "unknown"),
        "stages": stages,
    }

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
    shader-consumer diagnostics.
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


def _selected_params(n: Any, param_names: list[str] | None, params_mode: str,
                     detailed: bool) -> tuple[list[dict[str, Any]], list[str]]:
    if param_names == []:
        return [], []
    if param_names is None and params_mode == "values":
        return [_inspect_param_entry(p, detailed=detailed) for p in n.pars()], []
    requested = None if param_names is None else set(param_names)
    selected = [(p.name, p) for p in n.pars()
                if requested is None or p.name in requested]
    found = {name for name, _ in selected}
    missing = list(dict.fromkeys(name for name in (param_names or []) if name not in found))
    entries = [{"name": name} if params_mode == "names"
               else _inspect_param_entry(p, detailed=detailed) for name, p in selected]
    return entries, missing


def build_inspect_node(
    n: Any,
    *,
    detail_level: str = "summary",
    want_nodes: bool = True,
    want_params: bool = False,
    want_errors: bool = False,
    want_warnings: bool = False,
    want_content: bool = False,
    param_names: list[str] | None = None,
    params_mode: str = "values",
    child_offset: int | None = None,
    child_limit: int | None = None,
    lint_ctx: Any = None,
    scope_root: str | None = None,
) -> dict[str, Any]:
    """Shape one inspect node payload (pure enough for unit tests without TD)."""
    children: list[dict[str, Any]] = []
    child_count = 0
    paging = child_offset is not None or child_limit is not None
    offset = 0 if child_offset is None else child_offset
    limit = CHILDREN_ROSTER_LIMIT if child_limit is None else child_limit
    if want_nodes:
        raw_children = list(n.children)  # TD OP.children is a list property
        child_count = len(raw_children)
        detailed = detail_level == "detailed"
        for child in raw_children[offset:offset + limit]:
            if detailed:
                entry: dict[str, Any] = {
                    "path": getattr(child, "path", None),
                    "family": getattr(child, "family", None),
                    "opType": getattr(child, "opType", None),
                }
            else:
                entry = {
                    "name": _child_name(child),
                    "opType": getattr(child, "opType", None),
                }
            # Roster comments are the cheap "what is this subtree for" surface.
            child_comment = _op_comment(child, COMMENT_ROSTER_MAX_CHARS)
            if child_comment is not None:
                entry["comment"] = child_comment[0]
            children.append(entry)

    out: dict[str, Any] = {
        "path": getattr(n, "path", None),
        "family": getattr(n, "family", None),
        "opType": getattr(n, "opType", None),
    }
    from .timing import snapshot
    out['timing'] = snapshot(n)
    # Identity metadata, not a section: emitted whenever non-empty, regardless
    # of `include` — the operator's own account of what it is for.
    comment = _op_comment(n, COMMENT_MAX_CHARS)
    if comment is not None:
        out["comment"] = comment[0]
        if comment[1]:
            out["commentTruncated"] = True
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
        if paging:
            next_offset = offset + len(children)
            out["childrenPage"] = {
                "offset": offset,
                "limit": limit,
                "total": child_count,
                "nextOffset": next_offset if next_offset < child_count else None,
                "complete": offset == 0 and len(children) == child_count,
            }
            if len(children) < child_count:
                out["truncation"].update({
                    "limit": limit,
                    "message": f"Direct-child page returned {len(children)} of {child_count} at offset {offset}",
                    "mitigation": [
                        "Continue with childrenPage.nextOffset when non-null",
                        "Offsets are unstable if the child roster changes between calls",
                    ],
                })
        for field in ("inputs", "outputs"):
            out[field] = _observe(
                out, field, lambda: _wire_peers(getattr(n, field), strict=True), []
            )
    if want_params:
        entries, missing = _observe(
            out, "params", lambda: _selected_params(
                n, param_names, params_mode, detail_level == "detailed"), ([], None)
        )
        out["params"] = entries
        if param_names is not None:
            out["paramsSelection"] = {
                "requested": param_names,
                "missing": missing,
                "complete": missing == [],
            }
    # Content can synchronously compile shaders; observe messages afterward so
    # one reply does not combine a fresh compile verdict with stale cook errors.
    if want_content:
        _attach_content(n, out, lint_ctx=lint_ctx, scope_root=scope_root)
    if want_errors:
        out["errors"] = _observe(out, "errors", lambda: _op_messages(n.errors, strict=True), [])
    if want_warnings:
        warnings = _observe(out, "warnings", lambda: _op_messages(n.warnings, strict=True), [])
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

    With ``content``: DAT nodes carry ``consumers[]`` shader diagnostics and
    GLSL nodes a classified ``compileState``. Reading ``compileResult`` forces
    a synchronous recompile of that consumer.
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

    param_names = params.get("paramNames")
    params_mode = params.get("paramsMode", "values")
    child_offset = params.get("childOffset")
    child_limit = params.get("childLimit")
    invalid = None
    if param_names is not None and (
        not isinstance(param_names, list) or any(not isinstance(name, str) for name in param_names)
    ):
        invalid = "paramNames must be an array of exact names or null"
    elif params_mode not in ("values", "names"):
        invalid = "paramsMode must be values or names"
    elif child_offset is not None and (
        type(child_offset) is not int or not 0 <= child_offset <= 0xFFFFFFFF
    ):
        invalid = "childOffset must be an unsigned 32-bit integer"
    elif child_limit is not None and (
        type(child_limit) is not int or not 1 <= child_limit <= CHILDREN_ROSTER_LIMIT
    ):
        invalid = "childLimit must be in 1..256"
    if invalid is not None:
        return {"ok": False, "code": "tdmcp.args.wrong_type", "message": invalid}

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
                param_names=param_names,
                params_mode=params_mode,
                child_offset=child_offset,
                child_limit=child_limit,
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

    from .timing import snapshot
    out: dict[str, Any] = {"ok": True, "nodes": nodes_out, "timing": snapshot()}
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
