"""Ordered mutate steps (create / set / delete / connect / disconnect)."""
from __future__ import annotations

from typing import Any

from .constants import _FLAG_NAMES
from .paths import _absolutize_path, _get_par, _parent_and_name, resolve_op
from .suggest import _is_op_type_name, _suggest_names, _suggest_op_types

def _par_names(node: Any) -> list[str]:
    """Best-effort list of .par names on ``node`` (via ``pars()``)."""
    try:
        pars_fn = getattr(node, "pars", None)
        if not callable(pars_fn):
            return []
        names: list[str] = []
        for p in pars_fn() or []:
            n = getattr(p, "name", None)
            if isinstance(n, str) and n:
                names.append(n)
        return names
    except Exception:  # noqa: BLE001
        return []


def _with_similar_param_hint(err: dict[str, Any], node: Any) -> dict[str, Any]:
    """Best-effort near-miss .par name lint. Never raises; never changes code/ok."""
    try:
        name = err.get("field")
        if not isinstance(name, str) or not name:
            return err
        suggestions = _suggest_names(name, _par_names(node))
        if not suggestions:
            return err
        top = suggestions[0]
        out = dict(err)
        out["message"] = f"unknown parameter: {name} (did you mean: {top}?)"
        out["lints"] = [
            {
                "severity": "lint",
                "code": "tdmcp.par.similar_name",
                "message": f"similar parameter '{top}' found on node",
                "confidence": "medium",
                "suggestion": {"replace": top},
            }
        ]
        return out
    except Exception:  # noqa: BLE001
        return err


def _with_similar_type_hint(err: dict[str, Any], ctx: "MutateContext") -> dict[str, Any]:
    """Best-effort near-miss opType lint. Never raises; never changes code/ok."""
    try:
        name = err.get("field")
        if not isinstance(name, str) or not name:
            return err
        list_fn = getattr(ctx, "list_op_type_names", None)
        candidates = list_fn() if callable(list_fn) else []
        if not isinstance(candidates, list):
            return err
        suggestions = _suggest_op_types(
            name, [c for c in candidates if isinstance(c, str)]
        )
        if not suggestions:
            return err
        top = suggestions[0]
        out = dict(err)
        out["message"] = f"unknown opType: {name} (did you mean: {top}?)"
        out["lints"] = [
            {
                "severity": "lint",
                "code": "tdmcp.op.similar_type",
                "message": f"similar opType '{top}' found",
                "confidence": "medium",
                "suggestion": {"replace": top},
            }
        ]
        return out
    except Exception:  # noqa: BLE001
        return err


def _with_collection_hint(
    err: dict[str, Any], node: Any, *, as_param: bool
) -> dict[str, Any]:
    """Best-effort cross-collection lint. Never raises; never changes code/ok.

    When a name is missing from the requested bag but exists in the other
    collection, attach a nested lint and a short message suffix. Any enrich
    failure returns ``err`` unchanged (simplified base diagnostic only).
    Builds on a shallow copy so a mid-enrich failure cannot leave a partial
    message/lint on the caller's base dict.
    """
    try:
        name = err.get("field")
        if not isinstance(name, str) or not name:
            return err
        if as_param and name in _FLAG_NAMES:
            out = dict(err)
            out["message"] = f"unknown parameter: {name} (exists as flag — use flags)"
            out["lints"] = [
                {
                    "severity": "lint",
                    "code": "tdmcp.par.wrong_collection",
                    "message": f"'{name}' is an OP flag; use flags, not values",
                    "confidence": "high",
                    "suggestion": {"replace": f"flags.{name}"},
                }
            ]
            return out
        if not as_param and _get_par(node, name) is not None:
            out = dict(err)
            out["message"] = f"unknown flag: {name} (exists as parameter — use values)"
            out["lints"] = [
                {
                    "severity": "lint",
                    "code": "tdmcp.flag.wrong_collection",
                    "message": f"'{name}' is a .par parameter; use values, not flags",
                    "confidence": "high",
                    "suggestion": {"replace": f"values.{name}"},
                }
            ]
            return out
        if as_param:
            return _with_similar_param_hint(err, node)
        return err
    except Exception:  # noqa: BLE001
        return err


def _echo_op_type(err: dict[str, Any], node: Any) -> dict[str, Any]:
    """Best-effort echo of node.opType for api_help diagnostic refs. Never raises."""
    try:
        ot = getattr(node, "opType", None)
        if isinstance(ot, str) and ot:
            out = dict(err)
            out["opType"] = ot
            return out
    except Exception:  # noqa: BLE001
        pass
    return err


def _apply_values(node: Any, values: dict[str, Any]) -> dict[str, Any] | None:
    """Assign plain parameter values. Returns an error step dict, or None on ok."""
    for name, val in values.items():
        par = _get_par(node, name)
        if par is None:
            return _echo_op_type(
                _with_collection_hint(
                    {
                        "ok": False,
                        "code": "tdmcp.par.unknown",
                        "path": getattr(node, "path", None),
                        "message": f"unknown parameter: {name}",
                        "field": name,
                    },
                    node,
                    as_param=True,
                ),
                node,
            )
        try:
            if hasattr(par, "val"):
                par.val = val
            else:
                setattr(node.par, name, val)
        except Exception as exc:  # noqa: BLE001
            return _echo_op_type(
                {
                    "ok": False,
                    "code": "tdmcp.mutate.step_failed",
                    "path": getattr(node, "path", None),
                    "message": str(exc),
                    "field": name,
                },
                node,
            )
    return None


def _apply_flags(node: Any, flags: dict[str, Any]) -> dict[str, Any] | None:
    """Assign direct OP attributes (flags). Returns an error step dict, or None on ok."""
    for name, val in flags.items():
        if name not in _FLAG_NAMES:
            return _with_collection_hint(
                {
                    "ok": False,
                    "code": "tdmcp.flag.unknown",
                    "path": getattr(node, "path", None),
                    "message": f"unknown flag: {name}",
                    "field": name,
                },
                node,
                as_param=False,
            )
        try:
            setattr(node, name, val)
        except Exception as exc:  # noqa: BLE001
            return {
                "ok": False,
                "code": "tdmcp.mutate.step_failed",
                "path": getattr(node, "path", None),
                "message": str(exc),
                "field": name,
            }
    return None


def _apply_expressions(
    node: Any, expressions: dict[str, Any], expression_mode: Any
) -> dict[str, Any] | None:
    """Set expression mode explicitly, then assign .expr."""
    for name, expr in expressions.items():
        par = _get_par(node, name)
        if par is None:
            return _echo_op_type(
                _with_collection_hint(
                    {
                        "ok": False,
                        "code": "tdmcp.par.unknown",
                        "path": getattr(node, "path", None),
                        "message": f"unknown parameter: {name}",
                        "field": name,
                    },
                    node,
                    as_param=True,
                ),
                node,
            )
        try:
            par.mode = expression_mode
            par.expr = expr
        except Exception as exc:  # noqa: BLE001
            return _echo_op_type(
                {
                    "ok": False,
                    "code": "tdmcp.mutate.step_failed",
                    "path": getattr(node, "path", None),
                    "message": str(exc),
                    "field": name,
                },
                node,
            )
    return None


def _apply_pulse(node: Any, pulse: list[str]) -> dict[str, Any] | None:
    """Pulse named parameters."""
    for name in pulse:
        par = _get_par(node, name)
        if par is None:
            return _echo_op_type(
                _with_collection_hint(
                    {
                        "ok": False,
                        "code": "tdmcp.par.unknown",
                        "path": getattr(node, "path", None),
                        "message": f"unknown parameter: {name}",
                        "field": name,
                    },
                    node,
                    as_param=True,
                ),
                node,
            )
        try:
            pulse_fn = getattr(par, "pulse", None)
            if not callable(pulse_fn):
                return _echo_op_type(
                    {
                        "ok": False,
                        "code": "tdmcp.mutate.step_failed",
                        "path": getattr(node, "path", None),
                        "message": f"parameter {name} has no pulse()",
                        "field": name,
                    },
                    node,
                )
            pulse_fn()
        except Exception as exc:  # noqa: BLE001
            return _echo_op_type(
                {
                    "ok": False,
                    "code": "tdmcp.mutate.step_failed",
                    "path": getattr(node, "path", None),
                    "message": str(exc),
                    "field": name,
                },
                node,
            )
    return None


class MutateContext:
    """Resolution hooks for apply_step — no ``td`` import in the pure seam.

    Live TD supplies :class:`_TdMutateContext`; unit tests supply fakes.
    """

    def resolve(self, path: str) -> Any | None:  # noqa: D401
        raise NotImplementedError

    def get_op_type(self, op_type: str) -> Any | None:  # noqa: D401
        raise NotImplementedError

    def list_op_type_names(self) -> list[str]:
        """Candidate opType names for near-miss suggestions (default: none)."""
        return []

    def expression_mode(self) -> Any:
        """Value assigned to ``par.mode`` before setting ``.expr``."""
        return "EXPRESSION"


class _TdMutateContext(MutateContext):
    """Live TD resolution via ``tdmcp_resolve`` / ``getattr(td, …)``."""

    def __init__(self, context_path: str | None) -> None:
        self._context_path = context_path

    def resolve(self, path: str) -> Any | None:
        node = resolve_op(path, self._context_path)
        if node is None or not getattr(node, "valid", True):
            return None
        return node

    def get_op_type(self, op_type: str) -> Any | None:
        import td  # type: ignore

        return getattr(td, op_type, None)

    def list_op_type_names(self) -> list[str]:
        import td  # type: ignore

        try:
            return [n for n in dir(td) if isinstance(n, str) and _is_op_type_name(n)]
        except Exception:  # noqa: BLE001
            return []

    def expression_mode(self) -> Any:
        import td  # type: ignore

        mode_enum = getattr(td, "ParMode", None)
        if mode_enum is not None:
            expr = getattr(mode_enum, "EXPRESSION", None)
            if expr is not None:
                return expr
        return "EXPRESSION"


def apply_step(
    ctx: MutateContext,
    step: dict[str, Any],
    *,
    context_path: str | None = None,
    detail_level: str = "summary",
) -> dict[str, Any]:
    """Apply one mutate step. Pure seam — no ``td`` import.

    Returns a per-step result dict: ``{ok, path?, code?, message?, …}``.
    """
    op = step.get("op") or ""
    try:
        if op == "create":
            return _step_create(ctx, step, context_path, detail_level)
        if op == "set":
            return _step_set(ctx, step, context_path, detail_level)
        if op == "delete":
            return _step_delete(ctx, step, context_path)
        if op == "connect":
            return _step_connect(ctx, step, context_path, detail_level)
        if op == "disconnect":
            return _step_disconnect(ctx, step, context_path, detail_level)
        return {
            "ok": False,
            "code": "tdmcp.mutate.step_failed",
            "message": f"unknown mutate op: {op}",
            "path": step.get("path") or step.get("dst"),
        }
    except Exception as exc:  # noqa: BLE001 — never propagate raw
        return {
            "ok": False,
            "code": "tdmcp.mutate.step_failed",
            "message": str(exc),
            "path": step.get("path"),
        }


def _rollback_create(created: Any) -> None:
    """Best-effort destroy of a just-created node. Never raises; never masks the step error."""
    try:
        destroy = getattr(created, "destroy", None)
        if callable(destroy):
            destroy()
    except Exception:  # noqa: BLE001
        pass


def _step_create(
    ctx: MutateContext,
    step: dict[str, Any],
    context_path: str | None,
    detail_level: str,
) -> dict[str, Any]:
    path = step.get("path") or ""
    op_type = step.get("opType") or ""
    full = _absolutize_path(path, context_path)
    parent_path, name = _parent_and_name(full)
    if not name:
        return {
            "ok": False,
            "code": "tdmcp.mutate.step_failed",
            "message": "create path has no leaf name",
            "path": full,
        }
    parent = ctx.resolve(parent_path)
    if parent is None:
        return {
            "ok": False,
            "code": "tdmcp.op.not_found",
            "message": f"parent not found: {parent_path}",
            "path": full,
        }
    op_cls = ctx.get_op_type(op_type)
    if op_cls is None:
        return _with_similar_type_hint(
            {
                "ok": False,
                "code": "tdmcp.op.unknown_type",
                "message": f"unknown opType: {op_type}",
                "path": full,
                "field": op_type,
                "opType": op_type,
            },
            ctx,
        )
    created = parent.create(op_cls, name)
    raw_created = getattr(created, "path", None)
    if isinstance(raw_created, str) and raw_created.strip():
        created_path = _absolutize_path(raw_created, None)
    else:
        # Missing path → assume requested; never emit a false rename lint.
        created_path = full
    values = step.get("values")
    if values:
        err = _apply_values(created, values)
        if err is not None:
            err["path"] = created_path
            _rollback_create(created)
            return err
    flags = step.get("flags")
    if flags:
        err = _apply_flags(created, flags)
        if err is not None:
            err["path"] = created_path
            _rollback_create(created)
            return err
    out: dict[str, Any] = {"ok": True, "path": created_path}
    if detail_level == "detailed":
        if values:
            out["values"] = values
        if flags:
            out["flags"] = flags
    if created_path != full:
        try:
            out["lints"] = [
                {
                    "severity": "lint",
                    "code": "tdmcp.op.renamed",
                    "message": (
                        f"requested '{full}', created as '{created_path}'"
                    ),
                    "confidence": "high",
                    "suggestion": {
                        "opPath": created_path,
                        "replace": created_path,
                    },
                }
            ]
        except Exception:  # noqa: BLE001 — never change ok after materialization
            pass
    return out


def _step_set(
    ctx: MutateContext,
    step: dict[str, Any],
    context_path: str | None,
    detail_level: str,
) -> dict[str, Any]:
    path = step.get("path") or ""
    full = _absolutize_path(path, context_path)
    node = ctx.resolve(full)
    if node is None:
        return {
            "ok": False,
            "code": "tdmcp.op.not_found",
            "message": f"node not found: {full}",
            "path": full,
        }
    node_path = getattr(node, "path", None) or full
    values = step.get("values")
    expressions = step.get("expressions")
    pulse = step.get("pulse")
    flags = step.get("flags")
    if values:
        err = _apply_values(node, values)
        if err is not None:
            return err
    if expressions:
        err = _apply_expressions(node, expressions, ctx.expression_mode())
        if err is not None:
            return err
    if pulse:
        err = _apply_pulse(node, list(pulse))
        if err is not None:
            return err
    if flags:
        err = _apply_flags(node, flags)
        if err is not None:
            return err
    out: dict[str, Any] = {"ok": True, "path": node_path}
    if detail_level == "detailed":
        if values:
            out["values"] = values
        if expressions:
            out["expressions"] = expressions
        if pulse:
            out["pulse"] = list(pulse)
        if flags:
            out["flags"] = flags
    return out


def _step_delete(
    ctx: MutateContext,
    step: dict[str, Any],
    context_path: str | None,
) -> dict[str, Any]:
    path = step.get("path") or ""
    full = _absolutize_path(path, context_path)
    node = ctx.resolve(full)
    if node is None:
        return {
            "ok": False,
            "code": "tdmcp.op.not_found",
            "message": f"node not found: {full}",
            "path": full,
        }
    node_path = getattr(node, "path", None) or full
    try:
        node.destroy()
    except Exception as exc:  # noqa: BLE001
        return {
            "ok": False,
            "code": "tdmcp.mutate.step_failed",
            "message": str(exc),
            "path": node_path,
        }
    return {"ok": True, "path": node_path}


def _connector_index(step: dict[str, Any], key: str, default: int = 0) -> int:
    """Coerce a connector index from a step field; default when missing/null."""
    raw = step.get(key, default)
    if raw is None:
        return default
    try:
        return int(raw)
    except (TypeError, ValueError):
        return default


def _step_connect(
    ctx: MutateContext,
    step: dict[str, Any],
    context_path: str | None,
    detail_level: str,
) -> dict[str, Any]:
    """Wire ``src.outputConnectors[i]`` to ``dst.inputConnectors[j]``."""
    src_path = _absolutize_path(step.get("src") or "", context_path)
    dst_path = _absolutize_path(step.get("dst") or "", context_path)
    src_output = _connector_index(step, "srcOutput", 0)
    dst_input = _connector_index(step, "dstInput", 0)

    src = ctx.resolve(src_path)
    if src is None:
        return {
            "ok": False,
            "code": "tdmcp.op.not_found",
            "message": f"node not found: {src_path}",
            "path": src_path,
        }
    dst = ctx.resolve(dst_path)
    if dst is None:
        return {
            "ok": False,
            "code": "tdmcp.op.not_found",
            "message": f"node not found: {dst_path}",
            "path": dst_path,
        }

    dst_canon = getattr(dst, "path", None) or dst_path
    src_canon = getattr(src, "path", None) or src_path
    try:
        out_cons = getattr(src, "outputConnectors", None)
        in_cons = getattr(dst, "inputConnectors", None)
        if out_cons is None or in_cons is None:
            return {
                "ok": False,
                "code": "tdmcp.wire.connect_failed",
                "message": "node missing inputConnectors/outputConnectors",
                "path": dst_canon,
            }
        try:
            out_c = out_cons[src_output]
            in_c = in_cons[dst_input]
        except (IndexError, KeyError, TypeError) as exc:
            return {
                "ok": False,
                "code": "tdmcp.wire.bad_index",
                "message": (
                    f"bad connector index srcOutput={src_output} "
                    f"dstInput={dst_input}: {exc}"
                ),
                "path": dst_canon,
            }
        out_c.connect(in_c)
    except IndexError as exc:
        return {
            "ok": False,
            "code": "tdmcp.wire.bad_index",
            "message": str(exc),
            "path": dst_canon,
        }
    except Exception as exc:  # noqa: BLE001
        return {
            "ok": False,
            "code": "tdmcp.wire.connect_failed",
            "message": str(exc),
            "path": dst_canon,
        }

    out: dict[str, Any] = {"ok": True, "path": dst_canon}
    if detail_level == "detailed":
        out["src"] = src_canon
        out["srcOutput"] = src_output
        out["dstInput"] = dst_input
    return out


def _step_disconnect(
    ctx: MutateContext,
    step: dict[str, Any],
    context_path: str | None,
    detail_level: str,
) -> dict[str, Any]:
    """Clear ``path.inputConnectors[input]``."""
    full = _absolutize_path(step.get("path") or "", context_path)
    input_idx = _connector_index(step, "input", 0)
    node = ctx.resolve(full)
    if node is None:
        return {
            "ok": False,
            "code": "tdmcp.op.not_found",
            "message": f"node not found: {full}",
            "path": full,
        }
    node_path = getattr(node, "path", None) or full
    try:
        in_cons = getattr(node, "inputConnectors", None)
        if in_cons is None:
            return {
                "ok": False,
                "code": "tdmcp.wire.connect_failed",
                "message": "node missing inputConnectors",
                "path": node_path,
            }
        try:
            in_c = in_cons[input_idx]
        except (IndexError, KeyError, TypeError) as exc:
            return {
                "ok": False,
                "code": "tdmcp.wire.bad_index",
                "message": f"bad input connector index {input_idx}: {exc}",
                "path": node_path,
            }
        in_c.disconnect()
    except IndexError as exc:
        return {
            "ok": False,
            "code": "tdmcp.wire.bad_index",
            "message": str(exc),
            "path": node_path,
        }
    except Exception as exc:  # noqa: BLE001
        return {
            "ok": False,
            "code": "tdmcp.wire.connect_failed",
            "message": str(exc),
            "path": node_path,
        }

    out: dict[str, Any] = {"ok": True, "path": node_path}
    if detail_level == "detailed":
        out["input"] = input_idx
    return out


def _rewrite_step_aliases(
    step: dict[str, Any],
    aliases: dict[str, str],
    context_path: str | None,
) -> dict[str, Any]:
    """Shallow-copy ``step`` and remap path/src/dst via create aliases.

    Best-effort: never raises; returns ``step`` unchanged on any failure.
    """
    if not aliases:
        return step
    try:
        out = dict(step)
        for key in ("path", "src", "dst"):
            raw = out.get(key)
            if not isinstance(raw, str) or not raw:
                continue
            abs_path = _absolutize_path(raw, context_path)
            mapped = aliases.get(abs_path)
            if mapped:
                out[key] = mapped
        return out
    except Exception:  # noqa: BLE001
        return step


def _alias_lookup(
    path: str, aliases: dict[str, str], context_path: str | None
) -> str:
    """Absolutize then apply alias; best-effort, never raises."""
    try:
        if not path:
            return path
        abs_path = _absolutize_path(path, context_path)
        return aliases.get(abs_path, abs_path)
    except Exception:  # noqa: BLE001
        return path


def run_mutate_steps(
    ctx: MutateContext,
    steps: list[dict[str, Any]],
    *,
    context_path: str | None = None,
    detail_level: str = "summary",
) -> dict[str, Any]:
    """Sequential apply; stop on first hard error; mark rest skipped.

    When a create step is auto-renamed by TD, later steps in this batch that
    still reference the requested path are remapped to the actual created path
    (create-intent wins inside one ``mutate_nodes`` call).
    """
    results: list[dict[str, Any]] = []
    applied = 0
    failed_at: int | None = None
    aliases: dict[str, str] = {}
    for i, step in enumerate(steps):
        if failed_at is not None:
            raw_path = step.get("path") or step.get("dst") or ""
            results.append(
                {
                    "ok": False,
                    "skipped": True,
                    "code": "tdmcp.batch.skipped_dependent",
                    "path": _alias_lookup(raw_path, aliases, context_path)
                    if raw_path
                    else raw_path,
                }
            )
            continue
        rewritten = _rewrite_step_aliases(step, aliases, context_path)
        result = apply_step(
            ctx, rewritten, context_path=context_path, detail_level=detail_level
        )
        results.append(result)
        if result.get("ok"):
            applied += 1
            if (step.get("op") or "") == "create":
                try:
                    requested = _absolutize_path(
                        step.get("path") or "", context_path
                    )
                    actual = result.get("path")
                    if (
                        isinstance(actual, str)
                        and actual
                        and actual != requested
                    ):
                        aliases[requested] = actual
                except Exception:  # noqa: BLE001 — never fail the batch
                    pass
        else:
            failed_at = i
    return {
        "ok": failed_at is None,
        "applied": applied,
        "failedAt": failed_at,
        "steps": results,
    }


def handle_mutate(params: dict[str, Any]) -> dict[str, Any]:
    """Live TD wrapper for mutate_nodes — resolves via ``td.op()``."""
    steps = params.get("steps") or []
    if not isinstance(steps, list):
        return {
            "ok": False,
            "applied": 0,
            "failedAt": 0,
            "steps": [
                {
                    "ok": False,
                    "code": "tdmcp.mutate.step_failed",
                    "message": "steps must be a list",
                }
            ],
        }
    context_path = params.get("contextPath")
    detail_level = params.get("detailLevel") or "summary"
    ctx = _TdMutateContext(context_path)
    return run_mutate_steps(
        ctx, steps, context_path=context_path, detail_level=detail_level
    )

