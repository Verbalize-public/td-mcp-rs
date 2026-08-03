"""Fuzzy name / op-type suggestions for mutate diagnostics."""
from __future__ import annotations

import difflib
from typing import Any

def _suggest_names(name: str, candidates: list[str], *, n: int = 3) -> list[str]:
    """Case-insensitive near-miss suggestions via difflib. Never raises."""
    try:
        if not isinstance(name, str) or not name or not candidates:
            return []
        lower_map: dict[str, str] = {}
        for c in candidates:
            if isinstance(c, str) and c and not c.startswith("_"):
                lower_map.setdefault(c.lower(), c)
        if not lower_map:
            return []
        key = name.lower()
        out: list[str] = []
        if key in lower_map:
            out.append(lower_map[key])
        for m in difflib.get_close_matches(key, list(lower_map.keys()), n=n, cutoff=0.5):
            cand = lower_map[m]
            if cand not in out:
                out.append(cand)
            if len(out) >= n:
                break
        return out[:n]
    except Exception:  # noqa: BLE001
        return []


# Longest family first so COMP is not mis-split as … + OP-less suffix.
_OP_TYPE_FAMILIES: tuple[str, ...] = (
    "COMP",
    "CHOP",
    "TOP",
    "SOP",
    "DAT",
    "MAT",
    "POP",
)


def _split_op_type_family(name: str) -> tuple[str, str | None]:
    """Split an opType into ``(stem, family)`` using known suffixes. Never raises."""
    try:
        if not isinstance(name, str) or not name:
            return ("", None)
        key = name.casefold()
        for fam in _OP_TYPE_FAMILIES:
            suffix = fam.casefold()
            if key.endswith(suffix) and len(key) > len(suffix):
                return (key[: -len(suffix)], fam)
        return (key, None)
    except Exception:  # noqa: BLE001
        return ("", None)


def _is_op_type_name(name: str) -> bool:
    """True when ``name`` looks like a TD op class (family suffix, not private)."""
    try:
        if not isinstance(name, str) or not name or name.startswith("_"):
            return False
        _stem, family = _split_op_type_family(name)
        return family is not None and bool(_stem)
    except Exception:  # noqa: BLE001
        return False


def _suggest_op_types(name: str, candidates: list[str], *, n: int = 3) -> list[str]:
    """Family-aware opType near-miss suggestions. Prefer silence over wrong hints.

    Exact casefold hits always win. Same-family queries score stems (cutoff 0.6).
    Bare names score against stems only (cutoff 0.8). Never raises.
    """
    try:
        if not isinstance(name, str) or not name or not candidates:
            return []
        lower_map: dict[str, str] = {}
        stem_to_full: dict[str, list[str]] = {}
        for c in candidates:
            if not isinstance(c, str) or not _is_op_type_name(c):
                continue
            lower_map.setdefault(c.casefold(), c)
            stem, fam = _split_op_type_family(c)
            if not stem or fam is None:
                continue
            stem_to_full.setdefault(stem, [])
            if c not in stem_to_full[stem]:
                stem_to_full[stem].append(c)
        if not lower_map:
            return []

        key = name.casefold()
        out: list[str] = []
        if key in lower_map:
            out.append(lower_map[key])

        q_stem, q_family = _split_op_type_family(name)
        if q_family is not None:
            if not q_stem:
                return out[:n]
            stem_map: dict[str, str] = {}
            for full in lower_map.values():
                stem, fam = _split_op_type_family(full)
                if fam == q_family and stem:
                    stem_map.setdefault(stem, full)
            for m in difflib.get_close_matches(
                q_stem, list(stem_map.keys()), n=n, cutoff=0.6
            ):
                cand = stem_map[m]
                if cand not in out:
                    out.append(cand)
                if len(out) >= n:
                    break
            return out[:n]

        # Bare name: high stem cutoff so short prefixes like "geo" stay silent.
        for m in difflib.get_close_matches(
            key, list(stem_to_full.keys()), n=n, cutoff=0.8
        ):
            for cand in stem_to_full[m]:
                if cand not in out:
                    out.append(cand)
                if len(out) >= n:
                    return out[:n]
        return out[:n]
    except Exception:  # noqa: BLE001
        return []

