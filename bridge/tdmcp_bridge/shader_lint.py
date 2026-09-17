"""Shared shader-compile lint: consumer discovery + compileResult classifier.

Best-effort enrichment only; unavailable compile evidence is never success.
The verified compileResult consumers are glslTOP/glslmultiTOP/glslMAT.
GLSL POP can use an existing bound non-passive general Info DAT. Other
consumers or missing surfaces remain unsupported; no nodes are created by reads.
"""
from __future__ import annotations

from itertools import islice
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


COMPILE_LOG_MAX_BYTES = 4096


def _observe_pop_info(n: Any) -> dict[str, Any] | None:
    """Read an existing, bound non-passive general Info DAT; never create one.

    Verified against fresh success/failure logs on TD 2025.32460. Docked nodes
    are preferred, then siblings; each candidate list is capped at 64.
    """
    candidates = []
    try:
        candidates.extend(islice(iter(n.docked), 64))
    except Exception:  # noqa: BLE001
        pass
    try:
        candidates.extend(islice(iter(n.parent().children), 64))
    except Exception:  # noqa: BLE001
        pass
    seen = set()
    for info in candidates:
        try:
            path = info.path
            if path in seen or info.opType != "infoDAT" or not info.valid:
                continue
            seen.add(path)
            target = _eval_par(info, "op")
            if getattr(target, "path", None) != n.path:
                continue
            if _eval_par(info, "infotype") != "general" or _eval_par(info, "passive") is not False:
                continue
        except Exception:  # noqa: BLE001 — no verified binding
            continue
        try:
            verdict = _classify_compile_text(info.text)
        except Exception as exc:  # noqa: BLE001 — unavailable is not compiled
            verdict = _classify_compile_text(None)
            verdict["readError"] = {"type": type(exc).__name__, "message": str(exc)[:256]}
        verdict.update(source="infoDAT", infoDatPath=path)
        return verdict
    return None


def observe_compile_result(n: Any, op_type: str) -> dict[str, Any]:
    """Read one verified surface; shared by inspect and mutation lint."""
    if op_type == "glslPOP":
        observed = _observe_pop_info(n)
        if observed is not None:
            return observed
    try:
        raw = getattr(n, "compileResult", None)
    except Exception as exc:  # noqa: BLE001 — unavailable is not compiled
        item = classify_compile_result(op_type, None)
        item["readError"] = {"type": type(exc).__name__, "message": str(exc)[:256]}
        return item
    item = classify_compile_result(op_type, raw)
    if op_type not in _COMPILE_RESULT_OP_TYPES:
        # Preserve TD errors without inventing a compiler API or verdict.
        try:
            error_text = n.errors()
            encoded = str(error_text or "").encode("utf-8")
            item["operatorErrors"] = encoded[:COMPILE_LOG_MAX_BYTES].decode("utf-8", errors="ignore")
            item["operatorErrorsAvailable"] = True
            item["operatorErrorsTruncated"] = len(encoded) > COMPILE_LOG_MAX_BYTES
        except Exception as exc:  # noqa: BLE001
            item["operatorErrorsAvailable"] = False
            item["operatorErrorsReadError"] = {"type": type(exc).__name__, "message": str(exc)[:256]}
    return item


def classify_compile_result(op_type: str, compile_result: Any) -> dict[str, Any]:
    """Require affirmative, recognized evidence; never infer success from silence.

    Only verified consumer types are classified. Logs are bounded before parsing;
    truncation may prove an observed error but can never prove overall success.
    """
    if op_type not in _COMPILE_RESULT_OP_TYPES:
        return {
            "severity": "note",
            "code": "tdmcp.shader.unsupported_consumer",
            "message": f"{op_type} has no usable compile-status surface in this observation; compile state not checked",
        }
    return _classify_compile_text(compile_result)


def _classify_compile_text(compile_result: Any) -> dict[str, Any]:
    """Classify bounded text obtained from a verified compile-log surface."""
    text = compile_result if isinstance(compile_result, str) else ""
    encoded = text.encode("utf-8")
    truncated = len(encoded) > COMPILE_LOG_MAX_BYTES
    log = encoded[:COMPILE_LOG_MAX_BYTES].decode("utf-8", errors="ignore")
    evidence = {"log": log, "logTruncated": truncated}
    lines = log.splitlines()
    error_lines = [line for line in lines if (
        line.lstrip().lower().startswith("error:")
        or line.strip().lower() in ("compile failed", "link failed", "compilation failed")
    )]
    if error_lines:
        return {
            "severity": "error",
            "code": "tdmcp.shader.compile_failed",
            "message": f"shader compile failed ({len(error_lines)} observed error line(s))",
            "lines": error_lines,
            **evidence,
        }
    # Known TD section headers must each be followed by positive evidence.
    parts: list[str] = []
    unknown = False
    pending_section = False
    for line in lines:
        token = line.strip()
        if not token or set(token) == {"="}:
            continue
        if token in ("Compiled Successfully", "Linked Successfully"):
            pending_section = False
            if token not in parts:
                parts.append(token)
        elif token.endswith("Shader Compile Results:") or token == "Program Link Results:":
            unknown = unknown or pending_section
            pending_section = True
        elif token.startswith("WARNING:"):
            continue
        else:
            unknown = True
    if parts and not (unknown or pending_section or truncated):
        return {
            "severity": "note",
            "code": "tdmcp.shader.compiled",
            "message": ", ".join(parts),
            **evidence,
        }
    return {
        "severity": "note",
        "code": "tdmcp.shader.state_unknown",
        "message": "Compile evidence is missing, empty, unrecognized, incomplete or truncated; success not established",
        **evidence,
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
            classified = observe_compile_result(child, child_type)
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
