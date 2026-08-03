"""Live TD Python API cards — class / classes index / thin module."""
from __future__ import annotations

from typing import Any

from .constants import (
    API_HELP_CLASSES_LIMIT,
    API_HELP_MEMBERS_DETAILED,
    API_HELP_MEMBERS_SUMMARY,
    API_HELP_MODULE_SAMPLE,
    API_HELP_QUERIES_LIMIT,
)
from .suggest import _is_op_type_name, _split_op_type_family

_FAMILIES = frozenset({"COMP", "TOP", "CHOP", "SOP", "POP", "MAT", "DAT"})
_NOT_FOUND = "tdmcp.api_help.not_found"
_QUERIES_TRUNCATED = "tdmcp.api_help.queries_truncated"
_CLASSES_TRUNCATED = "tdmcp.api_help.classes_truncated"


def _wiki_url(name: str) -> str:
    """Best-effort Derivative class wiki URL (no HTTP check)."""
    if not isinstance(name, str) or not name:
        return ""
    page = name[0].upper() + name[1:] + "_Class"
    return f"https://docs.derivative.ca/{page}"


def _short_doc(obj: Any) -> str | None:
    doc = getattr(obj, "__doc__", None)
    if not isinstance(doc, str):
        return None
    text = doc.strip()
    return text or None


def _public_members(obj: Any) -> list[str]:
    try:
        names = dir(obj)
    except Exception:  # noqa: BLE001
        return []
    out: list[str] = []
    for n in names:
        if isinstance(n, str) and n and not n.startswith("_"):
            out.append(n)
    out.sort()
    return out


def _mro_names(cls: Any) -> list[str]:
    try:
        mro = getattr(cls, "__mro__", None) or ()
        return [getattr(c, "__name__", str(c)) for c in mro]
    except Exception:  # noqa: BLE001
        return []


def _query_class(name: str, detail_level: str) -> dict[str, Any]:
    import td  # type: ignore

    if not isinstance(name, str) or not name:
        return {
            "ok": False,
            "kind": "class",
            "name": name,
            "code": _NOT_FOUND,
            "message": "class query requires a non-empty name",
        }
    try:
        obj = getattr(td, name)
    except AttributeError:
        return {
            "ok": False,
            "kind": "class",
            "name": name,
            "code": _NOT_FOUND,
            "message": f"name not found on td: {name}",
        }
    except Exception as exc:  # noqa: BLE001
        return {
            "ok": False,
            "kind": "class",
            "name": name,
            "code": _NOT_FOUND,
            "message": str(exc),
        }

    members = _public_members(obj)
    cap = (
        API_HELP_MEMBERS_SUMMARY
        if detail_level != "detailed"
        else API_HELP_MEMBERS_DETAILED
    )
    card: dict[str, Any] = {
        "ok": True,
        "kind": "class",
        "name": name,
        "doc": _short_doc(obj),
        "members": members[:cap],
        "memberCount": len(members),
    }
    op_type = getattr(obj, "opType", None)
    if isinstance(op_type, str) and op_type:
        card["opType"] = op_type
    family = getattr(obj, "family", None)
    if isinstance(family, str) and family:
        card["family"] = family
    elif _is_op_type_name(name):
        _stem, fam = _split_op_type_family(name)
        if fam:
            card["family"] = fam

    if detail_level == "detailed":
        card["mro"] = _mro_names(obj)
        wiki = _wiki_url(name)
        if wiki:
            card["wikiUrl"] = wiki
    else:
        # Summary: short MRO (self + parents up to 4) without wikiUrl.
        mro = _mro_names(obj)
        if mro:
            card["mro"] = mro[:4]
    return card


def _query_classes(
    family: str | None, prefix: str | None, detail_level: str
) -> dict[str, Any]:
    import td  # type: ignore

    fam = family.upper() if isinstance(family, str) and family else None
    if fam is not None and fam not in _FAMILIES:
        return {
            "ok": False,
            "kind": "classes",
            "code": _NOT_FOUND,
            "message": f"unknown family filter: {family}",
            "family": family,
            "prefix": prefix,
        }
    pref = prefix.casefold() if isinstance(prefix, str) and prefix else None

    names: list[str] = []
    try:
        for n in dir(td):
            if not isinstance(n, str) or not _is_op_type_name(n):
                continue
            if fam is not None:
                _stem, nfam = _split_op_type_family(n)
                if nfam != fam:
                    continue
            if pref is not None and not n.casefold().startswith(pref):
                continue
            names.append(n)
    except Exception as exc:  # noqa: BLE001
        return {
            "ok": False,
            "kind": "classes",
            "code": _NOT_FOUND,
            "message": str(exc),
            "family": fam,
            "prefix": prefix,
        }

    names.sort()
    truncated = False
    if len(names) > API_HELP_CLASSES_LIMIT:
        names = names[:API_HELP_CLASSES_LIMIT]
        truncated = True

    out: dict[str, Any] = {
        "ok": True,
        "kind": "classes",
        "names": names,
        "count": len(names),
    }
    if fam is not None:
        out["family"] = fam
    if isinstance(prefix, str) and prefix:
        out["prefix"] = prefix
    if truncated:
        out["truncation"] = {
            "field": "names",
            "limit": API_HELP_CLASSES_LIMIT,
            "code": _CLASSES_TRUNCATED,
            "message": f"classes index capped at {API_HELP_CLASSES_LIMIT}",
        }
    # detail_level unused for index shape today (kept for wire parity).
    _ = detail_level
    return out


def _query_module(name: str, detail_level: str) -> dict[str, Any]:
    if not isinstance(name, str) or name != "td":
        return {
            "ok": False,
            "kind": "module",
            "name": name,
            "code": _NOT_FOUND,
            "message": "module query supports only name 'td' in v1",
        }
    import td  # type: ignore

    try:
        public = [
            n
            for n in dir(td)
            if isinstance(n, str) and n and not n.startswith("_")
        ]
    except Exception as exc:  # noqa: BLE001
        return {
            "ok": False,
            "kind": "module",
            "name": name,
            "code": _NOT_FOUND,
            "message": str(exc),
        }
    public.sort()
    type_count = 0
    for n in public:
        try:
            if isinstance(getattr(td, n, None), type):
                type_count += 1
        except Exception:  # noqa: BLE001
            continue
    sample_n = (
        API_HELP_MODULE_SAMPLE
        if detail_level != "detailed"
        else min(len(public), API_HELP_MODULE_SAMPLE * 2)
    )
    return {
        "ok": True,
        "kind": "module",
        "name": "td",
        "doc": _short_doc(td),
        "publicCount": len(public),
        "typeCount": type_count,
        "sample": public[:sample_n],
    }


def _run_query(q: Any, detail_level: str) -> dict[str, Any]:
    if not isinstance(q, dict):
        return {
            "ok": False,
            "kind": "class",
            "code": _NOT_FOUND,
            "message": "query entry must be an object",
        }
    kind = q.get("kind") or "class"
    if kind == "class":
        return _query_class(q.get("name"), detail_level)
    if kind == "classes":
        return _query_classes(q.get("family"), q.get("prefix"), detail_level)
    if kind == "module":
        return _query_module(q.get("name"), detail_level)
    return {
        "ok": False,
        "kind": str(kind),
        "code": _NOT_FOUND,
        "message": f"unknown query kind: {kind}",
    }


def handle_api_help(params: dict[str, Any]) -> dict[str, Any]:
    """Batch live TD API cards. Read-only — never help() or create/destroy."""
    import td  # type: ignore  # noqa: F401 — ensure TD runtime is importable

    raw = params.get("queries")
    if not isinstance(raw, list) or len(raw) == 0:
        return {
            "ok": False,
            "code": "tdmcp.api_help.queries_required",
            "message": "api_help requires a non-empty queries array",
        }

    detail_level = params.get("detailLevel") or "summary"
    queries = list(raw)
    truncated = False
    if len(queries) > API_HELP_QUERIES_LIMIT:
        queries = queries[:API_HELP_QUERIES_LIMIT]
        truncated = True

    results = [_run_query(q, detail_level) for q in queries]
    out: dict[str, Any] = {"ok": True, "results": results}
    if truncated:
        out["queriesTruncated"] = True
        out["truncation"] = {
            "field": "queries",
            "limit": API_HELP_QUERIES_LIMIT,
            "code": _QUERIES_TRUNCATED,
            "message": (
                f"api_help queries batch capped at {API_HELP_QUERIES_LIMIT} "
                f"of {len(raw)}"
            ),
            "mitigation": [
                "Split into multiple api_help calls",
                "Prefer classes with family/prefix filters",
            ],
        }
    return out
