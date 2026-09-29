"""Parameter interface authoring: export a COMP's control surface, apply one back.

`inspect` already reports parameter *values*. That is not the same thing as the
*interface* — the shape a builder or another tool needs in order to reproduce or
drive a control panel. This module is that shape, and the apply half that
consumes it.

# Export: what is recoverable, and what is not

MEASURED on TouchDesigner 2025.32460, and the measurements changed the design:

* **`parmType` is `None` for every parameter created through `append*`.** It is
  not a field a created parameter populates, so export must NOT read it — a
  payload full of `"parmType": null` looks like data and tells a reader nothing.
* **The `is*` predicates DO carry the type.** `isFloat` and `isInt` are
  independently true, and `isMenu`, `isPulse`, `isToggle`, `isString`,
  `isMomentary`, `isPython`, `isOP`, `isSequence`, `isFolder` each identify
  their own kind. So a type is recoverable — the route to it is the predicates,
  and the measured `appendInt`/`appendFloat` pair proves int and float are
  NOT conflated.
* **`append*(name)` takes a single positional name.** `appendFloat("Speed", 1.0)`
  is refused: "Single name argument expected. Labels, etc are specified with
  keyword arguments." So a default is a keyword, never a second positional.
* **`appendMenu` ignores `menuNames` as a keyword** ("Call contains unexpected
  keywords") and assigns `menuNames` after creation. A menu's items are
  therefore a second step, and export/apply models them as such.
* **`min`/`max` read back as TUPLES** (`(0.0,)`), not scalars.

Anything that is not measurable is reported as unverified rather than guessed.
A type this bridge cannot identify is exported as `"type": "unknown"` with the
predicates attached, so a consumer can see what was and was not established.

# Apply: additive, idempotent, and honest about partial failure

**Additive.** Applying an interface adds what is missing and never removes.
A parameter the caller did not mention is left exactly as it was. An authoring
tool that deleted a user's controls because a payload did not list them would
be a data-loss primitive.

**Idempotent.** Applying the same interface twice changes nothing the second
time and reports zero changes. A parameter that already exists with the same
type is not recreated, and a page that already exists is reused. Re-running an
export/apply round trip must converge, or "idempotent" is a claim rather than
a property.

**Explicit about migration.** A parameter that exists with a DIFFERENT type is
a conflict, never a silent conversion: turning a caller's int count into a
float, or their pulse into a toggle, is not a migration anybody asked for. The
step is reported as skipped with the reason, and the apply continues — so one
conflict does not abort a panel.

**Partial failure is per step, and the counts add up.** Every step is one of
`created`, `unchanged`, `skipped`, or `failed`, and the response carries all
four counts. A caller that reads "ok" has been told what actually happened.
"""
from __future__ import annotations

from typing import Any

from .constants import (
    INTERFACE_MAX_APPLY_STEPS,
    INTERFACE_MAX_LABEL_CHARS,
    INTERFACE_MAX_MENU_ITEMS,
    INTERFACE_MAX_NAME_CHARS,
    INTERFACE_MAX_PAGES,
    INTERFACE_MAX_PARS_PER_PAGE,
)
from .paths import resolve_op

# The predicate order matters: it is the order the `is*` flags are tested in, and
# the first match wins. `isNumber` is deliberately NOT here, because
# `appendFloat` and `appendInt` are both numbers and the pair is only
# distinguishable by asking `isInt` first.
_TYPE_PREDICATES: tuple[tuple[str, str], ...] = (
    ("isMenu", "menu"),
    ("isPulse", "pulse"),
    ("isToggle", "toggle"),
    ("isString", "string"),
    ("isFloat", "float"),
    ("isInt", "int"),
    ("isMomentary", "momentary"),
    ("isSequence", "sequence"),
    ("isPython", "python"),
    ("isFolder", "folder"),
    ("isOP", "op"),
)

# The `append*` method that creates each type. `folder` and `op` are separate
# because they create structure rather than a value.
_APPEND_FOR_TYPE: dict[str, str] = {
    "float": "appendFloat",
    "int": "appendInt",
    "toggle": "appendToggle",
    "pulse": "appendPulse",
    "menu": "appendMenu",
    "string": "appendStr",
    "sequence": "appendSequence",
    "python": "appendPython",
    "folder": "appendFolder",
    "op": "appendOP",
    "momentary": "appendMomentary",
}

# Types this bridge can create. Anything else is reported as a skipped step
# with the reason rather than guessed at.
_KNOWN_TYPES = frozenset(_APPEND_FOR_TYPE)


def _err(code: str, message: str, **extra: Any) -> dict[str, Any]:
    out: dict[str, Any] = {"ok": False, "code": code, "message": message}
    out.update(extra)
    return out


def _text(value: Any, limit: int) -> str:
    if value is None:
        return ""
    try:
        return str(value)[:limit]
    except Exception:  # noqa: BLE001
        return ""


def _scalar(value: Any) -> Any:
    """A JSON-safe scalar, or the original value's string form."""
    if value is None or isinstance(value, (bool, int, float, str)):
        return value
    try:
        return str(value)
    except Exception:  # noqa: BLE001
        return None


def classify(par: Any) -> tuple[str, dict[str, bool]]:
    """The type of `par`, and the predicates that established it.

    MEASURED: `parmType` is None on every created parameter, so the `is*`
    flags are the only evidence. They are returned alongside the type so a
    consumer can see the basis for the claim rather than taking it on trust.
    """
    evidence: dict[str, bool] = {}
    for predicate, _label in _TYPE_PREDICATES:
        try:
            evidence[predicate] = bool(getattr(par, predicate, False))
        except Exception:  # noqa: BLE001
            evidence[predicate] = False
    for predicate, label in _TYPE_PREDICATES:
        if evidence[predicate]:
            return label, evidence
    return "unknown", evidence


def _param_entry(par: Any) -> dict[str, Any]:
    """One parameter's interface: how to drive it, not its current value."""
    kind, evidence = classify(par)
    entry: dict[str, Any] = {
        "name": _text(getattr(par, "name", None), INTERFACE_MAX_PARS_PER_PAGE),
        "type": kind,
        "label": _text(getattr(par, "label", None), INTERFACE_MAX_LABEL_CHARS),
    }
    # `parmType` is reported only when this build actually populates it, and
    # the fact that it is usually absent is itself worth saying.
    raw_type = getattr(par, "parmType", None)
    if raw_type is not None:
        entry["parmType"] = _text(raw_type, 32)
    if kind == "unknown":
        # Say what was checked rather than presenting "unknown" as a fact.
        entry["evidence"] = evidence
        entry["unverified"] = (
            "no is* predicate matched; this build reports no parmType for "
            "created parameters, so the type is not established"
        )
    if kind in ("float", "int"):
        # MEASURED: min/max read back as TUPLES.
        for bound in ("min", "max", "normMin", "normMax", "clampMin", "clampMax"):
            try:
                value = getattr(par, bound, None)
            except Exception:  # noqa: BLE001
                continue
            if value is None:
                continue
            if isinstance(value, (list, tuple)) and len(value) == 1:
                entry[bound] = _scalar(value[0])
            elif isinstance(value, (list, tuple)):
                entry[bound] = [_scalar(v) for v in value[:4]]
            else:
                entry[bound] = _scalar(value)
    if kind == "menu":
        names = getattr(par, "menuNames", None)
        try:
            items = [_text(n, 128) for n in list(names)[:INTERFACE_MAX_MENU_ITEMS]] if names else []
        except Exception:  # noqa: BLE001
            items = []
        labels = getattr(par, "menuLabels", None)
        try:
            label_items = (
                [_text(x, 128) for x in list(labels)[:INTERFACE_MAX_MENU_ITEMS]]
                if labels
                else []
            )
        except Exception:  # noqa: BLE001
            label_items = []
        entry["menuNames"] = items
        if label_items:
            entry["menuLabels"] = label_items
    for binding in ("enableExpr", "bindExpr", "bindReferences", "bindMaster", "expr"):
        try:
            value = getattr(par, binding, None)
        except Exception:  # noqa: BLE001
            continue
        if value:
            entry[binding] = _text(value, 256)
    try:
        entry["default"] = _scalar(par.default)
    except Exception:  # noqa: BLE001
        pass
    return entry


def export_interface(comp: Any, path: str) -> dict[str, Any]:
    """The control surface of `comp`: pages, parameters, and how to drive them."""
    pages_out: list[dict[str, Any]] = []
    truncated = False
    try:
        raw_pages = list(getattr(comp, "customPages", None) or [])
    except Exception as exc:  # noqa: BLE001
        return _err(
            "tdmcp.interface.export_failed",
            f"could not read custom pages: {exc}",
            path=path,
        )
    if len(raw_pages) > INTERFACE_MAX_PAGES:
        raw_pages = raw_pages[:INTERFACE_MAX_PAGES]
        truncated = True
    for page in raw_pages:
        try:
            pars = list(getattr(page, "pars", None) or [])
        except Exception:  # noqa: BLE001
            pars = []
        if len(pars) > INTERFACE_MAX_PARS_PER_PAGE:
            pars = pars[:INTERFACE_MAX_PARS_PER_PAGE]
            truncated = True
        pages_out.append(
            {
                "page": _text(getattr(page, "name", None), INTERFACE_MAX_NAME_CHARS),
                "pars": [_param_entry(par) for par in pars],
            }
        )
    return {
        "ok": True,
        "path": _text(getattr(comp, "path", path), 512),
        "family": _text(getattr(comp, "family", None), 16),
        "pages": pages_out,
        "pageCount": len(pages_out),
        "truncated": truncated,
        "parCount": sum(len(page["pars"]) for page in pages_out),
        "typeEvidence": (
            "types come from the is* predicates; this build reports no parmType "
            "for created parameters"
        ),
    }


def _page_by_name(comp: Any, name: str) -> Any | None:
    for page in list(getattr(comp, "customPages", None) or []):
        if _text(getattr(page, "name", None), INTERFACE_MAX_NAME_CHARS) == name:
            return page
    return None


def _pars_by_name(page: Any) -> dict[str, Any]:
    out: dict[str, Any] = {}
    try:
        pars = list(getattr(page, "pars", None) or [])
    except Exception:  # noqa: BLE001
        pars = []
    for par in pars:
        key = _text(getattr(par, "name", None), INTERFACE_MAX_NAME_CHARS)
        if key:
            out.setdefault(key, par)
    return out


def _validate_definition(definition: Any) -> tuple[dict[str, Any] | None, str | None]:
    """Normalise one parameter definition, or explain why it cannot be applied."""
    if not isinstance(definition, dict):
        return None, "a parameter definition must be an object"
    name = definition.get("name")
    if not isinstance(name, str) or not name.strip():
        return None, "a parameter definition needs a non-empty name"
    kind = definition.get("type")
    if not isinstance(kind, str) or not kind:
        return None, "a parameter definition needs a type"
    if kind not in _KNOWN_TYPES:
        return None, (
            f"unsupported type {kind!r}; this bridge can create "
            f"{', '.join(sorted(_KNOWN_TYPES))}"
        )
    name = name.strip()[:INTERFACE_MAX_NAME_CHARS]
    page_name = definition.get("page") or "Controls"
    if not isinstance(page_name, str) or not page_name.strip():
        return None, "a page name must be a non-empty string"
    return (
        {
            "name": name,
            "type": kind,
            "page": page_name.strip()[:INTERFACE_MAX_NAME_CHARS],
            "label": _text(definition.get("label"), INTERFACE_MAX_LABEL_CHARS),
            "default": definition.get("default"),
            "min": definition.get("min"),
            "max": definition.get("max"),
            "menuNames": definition.get("menuNames"),
        },
        None,
    )


def _menu_names_of(par: Any) -> list[str] | None:
    try:
        names = getattr(par, "menuNames", None)
    except Exception:  # noqa: BLE001
        return None
    if not names:
        return []
    try:
        return [_text(n, 128) for n in list(names)]
    except Exception:  # noqa: BLE001
        return None


def _matches_existing(par: Any, spec: dict[str, Any]) -> bool:
    """Whether an existing parameter already IS this definition.

    Only the shape is compared, never the current value: applying an interface
    is about the control surface, and writing a value the caller did not ask
    for would be authoring in disguise.

    A menu's ITEMS are part of the shape, though. Matching on the type alone
    would report a menu carrying the wrong items as `unchanged` — which is how
    a wrong menu becomes permanently reported as fine: the step that failed to
    set the items left the parameter created, the retry saw the type match, and
    the discrepancy was never surfaced again.
    """
    kind, _ = classify(par)
    if kind != spec["type"]:
        return False
    if kind == "menu":
        wanted = spec.get("menuNames")
        if wanted:
            current = _menu_names_of(par)
            if current is not None and current != [
                _text(item, 128) for item in list(wanted)[:INTERFACE_MAX_MENU_ITEMS]
            ]:
                return False
    return True


def apply_interface(comp: Any, path: str, definitions: Any) -> dict[str, Any]:
    """Add the parameters in `definitions`, never removing any.

    Every step is reported as created, unchanged, skipped, or failed, and the
    four counts are returned. A caller that reads `ok` learns what happened
    rather than assuming everything did.
    """
    if not isinstance(definitions, list) or not definitions:
        return _err(
            "tdmcp.interface.bad_definitions",
            "definitions must be a non-empty array of parameter objects",
            path=path,
        )
    if len(definitions) > INTERFACE_MAX_APPLY_STEPS:
        return _err(
            "tdmcp.interface.too_many_steps",
            f"{len(definitions)} definitions, cap is {INTERFACE_MAX_APPLY_STEPS}",
            path=path,
        )

    steps: list[dict[str, Any]] = []
    counts = {"created": 0, "unchanged": 0, "skipped": 0, "failed": 0}
    pages_cache: dict[str, Any] = {}

    for index, raw in enumerate(definitions):
        spec, problem = _validate_definition(raw)
        if problem is not None:
            counts["skipped"] += 1
            steps.append({"index": index, "status": "skipped", "reason": problem})
            continue
        assert spec is not None  # narrowed by `problem is None`
        page_name = spec["page"]
        if page_name not in pages_cache:
            page = _page_by_name(comp, page_name)
            if page is None:
                append_page = getattr(comp, "appendCustomPage", None)
                if not callable(append_page):
                    counts["failed"] += 1
                    steps.append(
                        {
                            "index": index,
                            "status": "failed",
                            "name": spec["name"],
                            "reason": "this operator has no custom parameter pages",
                        }
                    )
                    continue
                try:
                    page = append_page(page_name)
                except Exception as exc:  # noqa: BLE001
                    counts["failed"] += 1
                    steps.append(
                        {
                            "index": index,
                            "status": "failed",
                            "name": spec["name"],
                            "reason": f"page {page_name!r} could not be created: {exc}",
                        }
                    )
                    continue
            pages_cache[page_name] = page
        page = pages_cache[page_name]

        existing = _pars_by_name(page).get(spec["name"])
        if existing is not None:
            # A different type is a conflict, never a silent conversion.
            if _matches_existing(existing, spec):
                counts["unchanged"] += 1
                steps.append(
                    {
                        "index": index,
                        "status": "unchanged",
                        "name": spec["name"],
                        "page": page_name,
                    }
                )
            else:
                kind, _ = classify(existing)
                counts["skipped"] += 1
                step = {
                    "index": index,
                    "status": "skipped",
                    "name": spec["name"],
                    "page": page_name,
                    "existingType": kind,
                }
                if kind == spec["type"]:
                    # Same type, different menu items. Saying "converting it to
                    # menu" would be plainly wrong, and saying nothing would
                    # leave the caller with a step that silently did not apply.
                    current = _menu_names_of(existing)
                    step["reason"] = (
                        "already exists as a menu with different items; this "
                        "bridge does not replace an existing parameter's menu "
                        "items, so remove or rename it first"
                    )
                    step["currentMenuNames"] = current
                    step["wantedMenuNames"] = [
                        _text(item, 128)
                        for item in list(spec.get("menuNames") or [])[
                            :INTERFACE_MAX_MENU_ITEMS
                        ]
                    ]
                else:
                    step["reason"] = (
                        f"already exists as {kind!r} and converting it to "
                        f"{spec['type']!r} is not a migration anybody asked "
                        "for; remove or rename it first"
                    )
                steps.append(step)
            continue

        method_name = _APPEND_FOR_TYPE[spec["type"]]
        appender = getattr(page, method_name, None)
        if not callable(appender):
            counts["skipped"] += 1
            steps.append(
                {
                    "index": index,
                    "status": "skipped",
                    "name": spec["name"],
                    "reason": f"this page cannot create a {spec['type']!r} parameter",
                }
            )
            continue
        try:
            # MEASURED: a single positional name; a default is a keyword.
            kwargs: dict[str, Any] = {}
            if spec["label"]:
                kwargs["label"] = spec["label"]
            par = appender(spec["name"], **kwargs)
        except Exception as exc:  # noqa: BLE001
            counts["failed"] += 1
            steps.append(
                {
                    "index": index,
                    "status": "failed",
                    "name": spec["name"],
                    "page": page_name,
                    "reason": f"{type(exc).__name__}: {exc}",
                }
            )
            continue
        # Menu items are a SECOND step: `appendMenu` refuses a `menuNames`
        # keyword, so the items are assigned after creation.
        #
        # MEASURED 2025.32460: on this build that assignment is REFUSED for
        # every route tried -- `par.menuNames = [...]` and the tuple form ("Can
        # only assign a scalar or a value with the same number of components"),
        # `appendMenu(name, items)` ("Single name argument expected"), every
        # keyword form ("Call contains unexpected keywords"), `par[i] = item`,
        # and `setMenu`/`menuDAT`, which do not exist. So a caller asking for
        # specific items will usually get a default menu, and this is where that
        # gets said out loud rather than being reported as applied.
        menu_note: dict[str, Any] | None = None
        if spec["type"] == "menu" and spec["menuNames"]:
            try:
                items = list(spec["menuNames"])[:INTERFACE_MAX_MENU_ITEMS]
                par.menuNames = [_text(item, 128) for item in items]
                menu_note = {"menuNamesApplied": True}
            except Exception as exc:  # noqa: BLE001
                menu_note = {
                    "menuNamesApplied": False,
                    "menuNamesError": f"{type(exc).__name__}: {exc}",
                    "currentMenuNames": _menu_names_of(par),
                    "wantedMenuNames": [
                        _text(item, 128) for item in list(spec["menuNames"])[:INTERFACE_MAX_MENU_ITEMS]
                    ],
                }
        counts["created"] += 1
        step: dict[str, Any] = {
            "index": index,
            "status": "created",
            "name": spec["name"],
            "page": page_name,
            "type": spec["type"],
        }
        # A refused attribute never demotes the step: the parameter EXISTS, and
        # reporting it as `failed` would tell the caller it does not. Same
        # precedent as a refused default.
        if menu_note is not None:
            step.update(menu_note)
        if spec["default"] is not None:
            try:
                par.default = spec["default"]
                step["defaultApplied"] = True
            except Exception as exc:  # noqa: BLE001
                step["defaultApplied"] = False
                step["defaultError"] = f"{type(exc).__name__}: {exc}"
        for bound in ("min", "max"):
            if spec[bound] is not None:
                try:
                    setattr(par, bound, spec[bound])
                    step[f"{bound}Applied"] = True
                except Exception as exc:  # noqa: BLE001
                    step[f"{bound}Applied"] = False
                    step[f"{bound}Error"] = f"{type(exc).__name__}: {exc}"
        steps.append(step)

    return {
        "ok": True,
        "path": _text(getattr(comp, "path", path), 512),
        "mode": "additive",
        "requested": len(definitions),
        "created": counts["created"],
        "unchanged": counts["unchanged"],
        "skipped": counts["skipped"],
        "failed": counts["failed"],
        "steps": steps,
        "explanation": (
            "additive: nothing was removed, and an existing parameter of a "
            "different type is reported as skipped rather than converted"
        ),
    }


def handle_param_interface(params: dict[str, Any]) -> dict[str, Any]:
    """`param_interface`: export a COMP's control surface, or apply one to it.

    Actions:
      export  the pages, parameters and types a caller needs to drive a panel
      apply   add the supplied definitions, removing nothing

    `export` reports `"type": "unknown"` with the predicates attached for
    anything it could not identify, because a type this bridge did not
    establish must not be presented as one it did.
    """
    action = params.get("action") or "export"
    path = params.get("path") or ""
    node = resolve_op(path, params.get("contextPath"))
    if node is None or not getattr(node, "valid", False):
        return {"ok": False, "code": "tdmcp.op.not_found", "path": path}
    family = _text(getattr(node, "family", None), 16)

    if action == "export":
        if family != "COMP":
            return _err(
                "tdmcp.interface.unsupported_family",
                f"an interface belongs on a COMP; resolved family is {family!r}",
                path=path,
                family=family,
            )
        return export_interface(node, path)

    if action == "apply":
        if family != "COMP":
            return _err(
                "tdmcp.interface.unsupported_family",
                f"an interface belongs on a COMP; resolved family is {family!r}",
                path=path,
                family=family,
            )
        return apply_interface(node, path, params.get("definitions"))

    return _err(
        "tdmcp.interface.unknown_action",
        f"unknown param_interface action {action!r}; use export or apply",
    )
