"""Shared shader-compile lint: consumer discovery + compileResult classifier.

Best-effort enrichment only — every entry point degrades silently and never
raises. Implements live-verified TD patterns:
``OP.errors()`` is silent for shader failures, so status comes exclusively
from ``OP.compileResult`` (present on glslTOP/glslmultiTOP/glslMAT only).
"""
from __future__ import annotations

from typing import Any

from .constants import SHADER_CONSUMER_LIMIT, SHADER_SCAN_LIMIT

# GLSL opTypes that expose shader DAT refs (live TD class names); also the
# consumer-scan universe. glslPOP is scanned too so a bound DAT still reports
# it as ``tdmcp.shader.unsupported_consumer`` instead of silently ignoring it.
_GLSL_OP_TYPES = frozenset({"glslTOP", "glslmultiTOP", "glslMAT", "glslPOP"})

# Par name → stage role per GLSL family (live TD par names).
_GLSL_STAGE_PARS: dict[str, tuple[tuple[str, str], ...]] = {
    "glslTOP": (
        ("pixeldat", "pixel"),
        ("vertexdat", "vertex"),
        ("computedat", "compute"),
        ("predat", "pre"),
    ),
    "glslmultiTOP": (
        ("pixeldat", "pixel"),
        ("vertexdat", "vertex"),
        ("computedat", "compute"),
        ("predat", "pre"),
    ),
    "glslMAT": (
        ("pdat", "pixel"),
        ("vdat", "vertex"),
        ("gdat", "geometry"),
        ("predat", "pre"),
    ),
    "glslPOP": (("computedat", "compute"),),
}

# opTypes with a verified OP.compileResult surface (V2). Everything else in
# _GLSL_STAGE_PARS classifies as unsupported_consumer.
_COMPILE_RESULT_OP_TYPES = frozenset({"glslTOP", "glslmultiTOP", "glslMAT"})


def stage_pars_for(op_type: str) -> tuple[tuple[str, str], ...]:
    """Stage (par, role) pairs for one GLSL opType; empty when unknown."""
    return _GLSL_STAGE_PARS.get(op_type, ())


def _eval_par(n: Any, par_name: str) -> Any:
    """Best-effort ``n.par.<name>.eval()``; None when missing/fails."""
    par_owner = getattr(n, "par", None)
    if par_owner is None:
        return None
    par = getattr(par_owner, par_name, None)
    if par is None:
        return None
    try:
        return par.eval()
    except Exception:  # noqa: BLE001
        return None


def _read_compile_result(n: Any) -> Any:
    """Raw ``n.compileResult`` read; None when missing/raising."""
    try:
        return getattr(n, "compileResult", None)
    except Exception:  # noqa: BLE001
        return None


def classify_compile_result(op_type: str, compile_result: Any) -> dict[str, Any]:
    """Classify one ``compileResult`` read.

    Returns ``{severity, code, message[, lines]}``; callers add
    consumer/consumerOpType/role. ``compile_result=None`` means the attribute
    was missing or unreadable.
    """
    if op_type not in _COMPILE_RESULT_OP_TYPES or compile_result is None:
        reason = (
            f"{op_type} exposes no compileResult surface"
            if op_type not in _COMPILE_RESULT_OP_TYPES
            else f"{op_type} returned no compileResult"
        )
        return {
            "severity": "note",
            "code": "tdmcp.shader.unsupported_consumer",
            "message": f"{reason}; compile state not checked",
        }
    text = str(compile_result)
    error_lines = [ln for ln in text.splitlines() if ln.startswith("ERROR:")]
    if error_lines:
        return {
            "severity": "error",
            "code": "tdmcp.shader.compile_failed",
            "message": f"shader compile failed ({len(error_lines)} error line(s))",
            "lines": error_lines,
        }
    parts = []
    if "Compiled Successfully" in text:
        parts.append("Compiled Successfully")
    if "Linked Successfully" in text:
        parts.append("Linked Successfully")
    return {
        "severity": "note",
        "code": "tdmcp.shader.compiled",
        "message": ", ".join(parts) if parts else "compiled",
    }


def discover_consumers(
    ctx: Any,
    dat_path: str,
    *,
    scope_root: str = "/project1",
    scan_limit: int = SHADER_SCAN_LIMIT,
    consumer_limit: int = SHADER_CONSUMER_LIMIT,
) -> dict[str, Any]:
    """Find GLSL ops whose stage pars evaluate to ``dat_path``.

    ``ctx`` needs ``resolve(path)`` and ``find_children(root, type_name)``
    (see ``MutateContext``). Returns ``{"consumers": [items]}`` plus
    ``consumersTruncated`` + standard ``truncation`` when caps bite. Raises on
    ctx misbehavior — wrap with :func:`lint_dat_consumers` at call sites.
    """
    root = ctx.resolve(scope_root)
    if root is None:
        return {}
    target = str(dat_path)
    consumers: list[dict[str, Any]] = []
    scanned = 0
    overflow = 0
    scan_truncated = False
    for type_name in sorted(_GLSL_STAGE_PARS):
        if scan_truncated:
            break
        try:
            children = ctx.find_children(root, type_name)
        except Exception:  # noqa: BLE001 — one family failing must not stop others
            continue
        for child in children:
            scanned += 1
            if scanned > scan_limit:
                scan_truncated = True
                break
            path = getattr(child, "path", None)
            if not isinstance(path, str) or not path:
                continue
            child_type = getattr(child, "opType", None) or type_name
            roles: list[str] = []
            for par_name, role in stage_pars_for(type_name):
                ref = _eval_par(child, par_name)
                if ref is None:
                    continue
                ref_path = ref if isinstance(ref, str) else getattr(ref, "path", None)
                if ref_path is None:
                    continue
                if str(ref_path) == target and role not in roles:
                    roles.append(role)
            if not roles:
                continue
            classified = classify_compile_result(child_type, _read_compile_result(child))
            for role in roles:
                if len(consumers) >= consumer_limit:
                    overflow += 1
                    continue
                item = dict(classified)
                item["consumer"] = path
                item["consumerOpType"] = child_type
                item["role"] = role
                consumers.append(item)
    out: dict[str, Any] = {"consumers": consumers}
    if scan_truncated or overflow:
        out["consumersTruncated"] = True
        detail = (
            f"scan capped at {scan_limit} ops"
            if scan_truncated
            else f"{overflow} more consumer(s) beyond cap"
        )
        out["truncation"] = {
            "field": "consumers",
            "limit": scan_limit if scan_truncated else consumer_limit,
            "code": "tdmcp.shader.consumers_truncated",
            "message": f"Shader consumer diagnostics truncated: {detail}",
            "mitigation": [
                "Narrow contextPath to the relevant subtree",
                "Inspect specific GLSL nodes directly for full detail",
            ],
        }
    return out


def lint_dat_consumers(ctx: Any, dat_path: str, scope_root: str | None = None) -> dict[str, Any]:
    """Never-raises :func:`discover_consumers` — {} on any failure."""
    try:
        return discover_consumers(ctx, dat_path, scope_root=scope_root or "/project1")
    except Exception:  # noqa: BLE001 — lint must never fail the parent call
        return {}
