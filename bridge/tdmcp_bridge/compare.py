"""Exact deltas between two FRESH observations of the same projection.

Why this is not ``inspect`` called twice and diffed by the caller: three of the
things that make a delta trustworthy cannot be established by a caller holding
only two JSON documents.

1. **Freshness has to be proven, not assumed.** A diff of two documents says
   nothing about *when* they were read. This module reads the projection on the
   caller's side of the bridge and stamps the observation it compared. A
   document that arrived from elsewhere is still compared as exactly that: a
   document.
2. **A missing baseline must not read as "nothing changed".** This is the
   failure mode that makes a comparison tool dangerous. If the baseline is gone
   and the tool returns an empty diff, every field is silently reported
   unchanged — and the caller writes on top of a scene it never actually
   looked at. A lost baseline produces ``baselineAvailable: false`` and a
   refusal to diff, never an empty and reassuring result.
3. **There is no cheap "did anything change" shortcut.** A daemon mutation
   counter increments on *dispatch*, not on *effect*, so it is not evidence the
   scene is unchanged. This module has no counter path at all; it re-reads every
   field it reports. A field that reads back identical is reported unchanged
   *because it was re-read*, and a field that cannot be read is reported
   unreadable rather than unchanged.

What a diff here does NOT claim:

* **Not atomic.** Two reads and a comparison are three moments. A user, a
  callback, or a timeline can change a field between the two reads. The result
  is a truthful report about those reads, not a snapshot anyone can write
  against without re-reading.
* **Not exhaustive.** Only the requested projection is compared. An unrequested
  field is not evidence of anything, so it is absent from the result rather
  than reported unchanged.
* **Value identity is JSON-exact, with presence tracked separately**, so a
  missing field, an explicit ``null``, ``false``, ``0`` and ``""`` are five
  different states and stay different.
"""
from __future__ import annotations

import json
import time
from typing import Any

from .constants import COMPARE_MAX_FIELDS, COMPARE_MAX_NODES
from .paths import resolve_op

_ADDED = "added"
_REMOVED = "removed"
_CHANGED = "changed"


class _Missing:
    """A field that is not present at all — distinct from an explicit null.

    ``None`` is a legitimate value in TouchDesigner, and collapsing the two
    would make "the parameter does not exist" and "the parameter is null" the
    same diff entry. Every read goes through :func:`_wrap` so the distinction
    survives into the result.
    """

    __slots__ = ()

    def __repr__(self) -> str:  # pragma: no cover - debugging aid
        return "<missing>"


MISSING = _Missing()


def _json_safe(value: Any) -> Any:
    """Coerce a TD value into something JSON can carry, without inventing meaning.

    A value that cannot be represented is NOT rounded or stringified into
    something that looks exact — it becomes an explicit ``unrepresentable``
    marker, because a delta computed against an approximation is not a delta.
    """
    if value is None or isinstance(value, (bool, int, str)):
        return value
    if isinstance(value, float):
        # NaN/Infinity are real TD values that JSON cannot carry. Kept as a
        # marker rather than clamped, so a NaN change stays visible.
        if value != value:
            return {"unrepresentable": "nan"}
        if value in (float("inf"), float("-inf")):
            return {"unrepresentable": "inf" if value > 0 else "-inf"}
        return value
    try:
        json.dumps(value)
        return value
    except (TypeError, ValueError):
        return {"unrepresentable": type(value).__name__}


def _wrap(value: Any) -> dict[str, Any]:
    """Tag a read value with its presence, keeping the five states distinct."""
    if isinstance(value, _Missing):
        return {"present": False}
    return {"present": True, "value": _json_safe(value)}


def _read_wire(node: Any, spec: str) -> Any:
    """Read a connection set as a sorted path list, e.g. ``wire.input.0``."""
    parts = spec.split(".")
    if len(parts) != 2:
        return MISSING
    direction, raw_index = parts
    try:
        index = int(raw_index)
    except ValueError:
        return MISSING
    if direction == "input":
        connectors = getattr(node, "inputConnectors", None) or []
    elif direction == "output":
        connectors = getattr(node, "outputConnectors", None) or []
    else:
        return MISSING
    if index < 0 or index >= len(connectors):
        return MISSING
    peers = []
    for conn in getattr(connectors[index], "connections", None) or []:
        owner = getattr(conn, "owner", None)
        peers.append(
            {
                "path": str(getattr(owner, "path", "")) if owner is not None else None,
                "index": getattr(conn, "index", None),
            }
        )
    peers.sort(key=lambda peer: (peer["path"] or "", peer["index"] or 0))
    return peers


def _read_field(node: Any, field: str) -> Any:
    """Read one projected field. Returns :data:`MISSING` when absent.

    Every read is defensive: TouchDesigner operators raise on some accessors
    and return ``None`` on others, and "we could not read it" must never be
    laundered into "it did not change".
    """
    if field == "comment":
        raw = getattr(node, "comment", MISSING)
        if raw is MISSING:
            return MISSING
        return raw if isinstance(raw, str) else str(raw)
    if field == "opType":
        raw = getattr(node, "opType", MISSING)
        return MISSING if raw is MISSING else str(raw)
    if field.startswith("par."):
        par = getattr(node, "par", None)
        if par is None:
            return MISSING
        parm = getattr(par, field[len("par.") :], MISSING)
        if parm is MISSING or parm is None:
            return MISSING
        return getattr(parm, "val", MISSING)
    if field.startswith("wire."):
        return _read_wire(node, field[len("wire.") :])
    return getattr(node, field, MISSING)


def read_field(node: Any, field: str) -> Any:
    """Public seam: read one projected field, returning :data:`MISSING` if absent.

    Exposed so the guarded-write path reads fields through exactly the same
    semantics the diff used. A guard that compared values differently from the
    comparison it was derived from would be checking something else.
    """
    return _read_field(node, field)


def json_safe(value: Any) -> Any:
    """Public seam for the JSON coercion, shared with the guarded-write path."""
    return _json_safe(value)


def project(node: Any, fields: list[str]) -> dict[str, dict[str, Any]]:
    """Read ``fields`` off one node, freshly, preserving presence."""
    return {field: _wrap(_read_field(node, field)) for field in fields}


def strict_eq(left: Any, right: Any) -> bool:
    """Exact equality that does NOT collapse the falsy states.

    Python says ``False == 0`` and ``0 == False``. That single fact makes a
    plain ``==`` wrong for this module twice over: a guard expecting ``False``
    would pass against a field holding ``0``, and a diff would call a change
    from ``0`` to ``False`` "no change". `false` and `zero` are two of the
    states the caller needs told apart, so they are compared by type first.

    Floats compare numerically, because 1.0 and 1 are the same value in
    TouchDesigner and inventing a difference there would be noise.
    """
    if left is None or right is None:
        return left is None and right is None
    # bool is a subclass of int, so it has to be settled before the int case.
    if isinstance(left, bool) or isinstance(right, bool):
        return isinstance(left, bool) and isinstance(right, bool) and left is right
    if isinstance(left, (int, float)) and isinstance(right, (int, float)):
        return left == right
    if isinstance(left, str) or isinstance(right, str):
        return isinstance(left, str) and isinstance(right, str) and left == right
    if isinstance(left, dict) or isinstance(right, dict):
        if not (isinstance(left, dict) and isinstance(right, dict)):
            return False
        if set(left) != set(right):
            return False
        return all(strict_eq(left[k], right[k]) for k in left)
    if isinstance(left, list) or isinstance(right, list):
        if not (isinstance(left, list) and isinstance(right, list)):
            return False
        return len(left) == len(right) and all(
            strict_eq(a, b) for a, b in zip(left, right)
        )
    if type(left) is not type(right):
        return False
    return bool(left == right)


def _same(before: Any, after: Any) -> bool:
    """Exact equality over the wrapped form, with presence part of the value."""
    if before.get("present") != after.get("present"):
        return False
    if not before.get("present"):
        return True
    # strict_eq, never `==`: see its docstring for why False must not match 0.
    return strict_eq(before.get("value"), after.get("value"))


def diff_field(field: str, before: Any, after: Any) -> dict[str, Any] | None:
    """One field's change, or ``None`` when it did not change."""
    if _same(before, after):
        return None
    if not before.get("present"):
        kind = _ADDED
    elif not after.get("present"):
        kind = _REMOVED
    else:
        kind = _CHANGED
    return {"field": field, "change": kind, "before": before, "after": after}


def diff_projection(
    before: dict[str, dict[str, Any]],
    after: dict[str, dict[str, Any]],
) -> list[dict[str, Any]]:
    """Exact per-field deltas, in the projection's own field order.

    Field order is the projection's rather than sorted: a caller who asked for
    ``[par.t, par.r, opType]`` reads the diff in the order they asked for.
    """
    changes: list[dict[str, Any]] = []
    for field, after_value in after.items():
        entry = diff_field(field, before.get(field, {"present": False}), after_value)
        if entry is not None:
            changes.append(entry)
    return changes


def _fail(code: str, message: str, **extra: Any) -> dict[str, Any]:
    out: dict[str, Any] = {"ok": False, "code": code, "message": message}
    out.update(extra)
    return out


def _read_paths(
    paths: list[str], fields: list[str], context_path: str | None
) -> tuple[list[dict[str, Any]], int]:
    """Freshly read projection. Unreadable nodes are marked, not dropped."""
    nodes: list[dict[str, Any]] = []
    unreadable = 0
    for path in paths:
        node = resolve_op(path, context_path)
        if node is None or not getattr(node, "valid", True):
            unreadable += 1
            nodes.append({"path": path, "readable": False, "fields": {}})
            continue
        nodes.append(
            {"path": path, "readable": True, "fields": project(node, fields)}
        )
    return nodes, unreadable


def _observation(nodes: list[dict[str, Any]], fields: list[str]) -> dict[str, Any]:
    """A baseline/observation envelope: the stamp is what makes it a read."""
    return {
        "observedAtMs": int(time.time() * 1000),
        "fields": list(fields),
        "nodes": nodes,
    }


def handle_compare(params: dict[str, Any]) -> dict[str, Any]:
    """Diff a retained baseline against a fresh read of the same projection.

    Two modes, and the difference is the honest part:

    * ``baseline`` supplied — compare. The baseline must be an observation
      envelope with an ``observedAtMs`` stamp, because a bare projection has no
      provenance and cannot be shown to be a read of anything.
    * ``capture: true`` — CAPTURE. There is nothing to compare yet, so this
      takes a fresh read and hands it back to be retained. It says so rather
      than returning an empty diff, because "I have no baseline" and "nothing
      changed" are the two answers a caller most needs to tell apart.
    * ``baseline`` absent and no capture requested — REFUSE. This is the safe
      default: a compare that quietly became a capture would let a caller who
      lost their baseline read `ok: true` as "compared, nothing changed".
    """
    paths = params.get("paths")
    fields = params.get("fields")
    if not isinstance(paths, list) or not paths:
        return _fail("tdmcp.compare.invalid", "paths must be a non-empty list")
    if not isinstance(fields, list) or not fields:
        return _fail("tdmcp.compare.invalid", "fields must be a non-empty list")
    paths = [str(p) for p in paths]
    fields = [str(f) for f in fields]
    if len(paths) > COMPARE_MAX_NODES:
        return _fail(
            "tdmcp.compare.too_large",
            f"compare is bounded to {COMPARE_MAX_NODES} paths; got {len(paths)}",
        )
    if len(fields) > COMPARE_MAX_FIELDS:
        return _fail(
            "tdmcp.compare.too_large",
            f"compare is bounded to {COMPARE_MAX_FIELDS} fields; got {len(fields)}",
        )
    duplicates = [p for p in dict.fromkeys(paths) if paths.count(p) > 1]
    if duplicates:
        return _fail(
            "tdmcp.compare.invalid",
            f"paths must be distinct; repeated: {sorted(set(duplicates))}",
        )

    context_path = params.get("contextPath")
    nodes, unreadable = _read_paths(paths, fields, context_path)
    current = _observation(nodes, fields)

    baseline = params.get("baseline")
    if baseline is None:
        if params.get("capture") is not True:
            # Refuse by default. A caller who asked to COMPARE and lost their
            # baseline gets the same answer as a caller who forgot to pass it,
            # and both get a refusal — never an empty and reassuring diff that
            # reads as "nothing changed". Capturing is opt-in, so a compare
            # can never quietly turn into something else.
            return {
                "ok": False,
                "code": "tdmcp.compare.baseline_unavailable",
                "message": (
                    "no baseline was supplied, so nothing was compared. An "
                    "absent baseline is not evidence that the scene is "
                    "unchanged. Take a fresh read with capture=true, retain it, "
                    "and pass it back as `baseline`."
                ),
                "baselineAvailable": False,
                "compared": False,
                "unreadableNodes": unreadable,
            }
        return {
            "ok": True,
            "mode": "capture",
            "baselineAvailable": False,
            "compared": False,
            "note": (
                "this is a fresh read, not a comparison — nothing has been "
                "compared yet, so nothing can be reported as changed or "
                "unchanged. Retain this observation and pass it back as "
                "`baseline` on the next compare."
            ),
            "atomic": False,
            "unreadableNodes": unreadable,
            "observation": current,
        }

    if not isinstance(baseline, dict) or "observedAtMs" not in baseline:
        return _fail(
            "tdmcp.compare.baseline_unavailable",
            "baseline is not an observation: it has no observedAtMs stamp, so it "
            "cannot be shown to be a read of this projection",
            baselineAvailable=False,
            compared=False,
        )
    before_nodes = baseline.get("nodes")
    if not isinstance(before_nodes, list) or not before_nodes:
        return _fail(
            "tdmcp.compare.baseline_unavailable",
            "baseline carries no node list, so there is nothing to compare against",
            baselineAvailable=False,
            compared=False,
        )
    before_fields = baseline.get("fields")
    if isinstance(before_fields, list) and before_fields and sorted(before_fields) != sorted(fields):
        return _fail(
            "tdmcp.compare.projection_mismatch",
            "the baseline was taken over a different field set, so a diff would "
            "silently omit fields that were never observed",
            baselineAvailable=True,
            compared=False,
            baselineFields=before_fields,
            requestedFields=fields,
        )

    # The whole node entry is kept, not just its `fields`: dropping `readable`
    # here would make the baseline-unreadable branch below unreachable, and a
    # node the baseline could not read would then be reported as unchanged.
    before_by_path = {
        node.get("path"): node
        for node in before_nodes
        if isinstance(node, dict)
    }
    missing = [p for p in paths if not isinstance(before_by_path.get(p), dict)]
    if missing:
        return _fail(
            "tdmcp.compare.baseline_unavailable",
            "the baseline does not cover every requested path, so a diff would "
            "silently omit them",
            baselineAvailable=True,
            compared=False,
            missingFromBaseline=missing,
        )

    compared: list[dict[str, Any]] = []
    total_changes = 0
    for node in current["nodes"]:
        path = node["path"]
        if not node["readable"]:
            compared.append({"path": path, "readable": False, "changes": []})
            continue
        before_node = before_by_path.get(path) or {}
        if not before_node.get("readable", True):  # noqa: SIM908 — kept explicit
            compared.append(
                {
                    "path": path,
                    "readable": True,
                    "baselineReadable": False,
                    "changes": [],
                    "note": "the baseline could not read this node, so no field here is reported unchanged",
                }
            )
            continue
        changes = diff_projection(before_node.get("fields") or {}, node["fields"])
        total_changes += len(changes)
        compared.append(
            {"path": path, "readable": True, "changes": changes}
        )

    baseline_at = int(baseline.get("observedAtMs") or 0)
    return {
        "ok": True,
        "mode": "compare",
        "compared": True,
        "baselineAvailable": True,
        "observedAtMs": current["observedAtMs"],
        "baselineObservedAtMs": baseline_at,
        "freshness": {
            "baselineMs": baseline_at,
            "currentMs": current["observedAtMs"],
            "ageMs": current["observedAtMs"] - baseline_at,
            "note": (
                "two separate reads; a field can change between them. This is a "
                "report about these reads, not a snapshot safe to write against "
                "without re-reading."
            ),
        },
        "atomic": False,
        "exactlyOnce": False,
        "unreadableNodes": unreadable,
        "changedFields": total_changes,
        "nodes": compared,
        "observation": current,
    }
