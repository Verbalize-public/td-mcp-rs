"""Parameter interface authoring: export, apply, and the honesty between them.

Every fact pinned here was measured against TouchDesigner 2025.32460. The
measurements changed the design twice:

  * `parmType` is None for every parameter created through `append*`, so
    export reads the `is*` predicates instead — and `isInt`/`isFloat` are
    independently true, so a count is not silently conflated with a float.
  * `append*(name)` takes a single positional name, and `appendMenu` refuses a
    `menuNames` keyword, so a menu's items are a second step.

The fakes reproduce those observations, so a change that contradicts a measured
behaviour fails here rather than at a caller's parameter panel.
"""

from __future__ import annotations

import os
import sys
import unittest
from types import SimpleNamespace
from typing import Any
from unittest import mock

sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))

import tdmcp_bridge  # noqa: E402
from tdmcp_bridge import param_interface  # noqa: E402
from tdmcp_bridge.param_interface import classify, export_interface  # noqa: E402

NAN = float("nan")


class FakePar:
    """A `td.Par` stand-in.

    `parmType` is deliberately None, exactly as measured for a created
    parameter — the tests that depend on that would not be testing anything if
    the fake answered a field the real thing does not.
    """

    def __init__(
        self,
        name: str,
        *,
        kind: str = "float",
        menu_names: list[str] | None = None,
        minimum: Any = None,
        maximum: Any = None,
    ) -> None:
        self.name = name
        self.label = name
        self.parmType = None
        # A real parameter refuses a default of the wrong shape rather than
        # storing it, and `apply` reports that refusal per attribute. A plain
        # attribute here would accept anything and make that reporting untestable.
        self._default = 0
        self._min = (0.0,)
        self._max = (1.0,)
        self.enableExpr = ""
        self.expr = ""
        for predicate, label in param_interface._TYPE_PREDICATES:
            setattr(self, predicate, label == kind)
        self.isNumber = kind in ("float", "int")
        self.isCustom = True
        self.isFolder = kind == "folder"
        self.isDefault = True
        self.isSamePar = True
        self.menuNames = menu_names
        self.menuLabels = None
        # MEASURED: min/max read back as TUPLES.
        self._min = (minimum,) if minimum is not None else (0.0,)
        self._max = (maximum,) if maximum is not None else (1.0,)
        self.normMin = (0.0,)
        self.normMax = (1.0,)

    @property
    def min(self):  # type: ignore[override]
        return self._min

    @min.setter
    def min(self, value: Any) -> None:
        if not isinstance(value, (int, float)) or isinstance(value, bool):
            raise TypeError(f"min must be a number, got {type(value).__name__}")
        self._min = (float(value),)

    @property
    def max(self):  # type: ignore[override]
        return self._max

    @max.setter
    def max(self, value: Any) -> None:
        if not isinstance(value, (int, float)) or isinstance(value, bool):
            raise TypeError(f"max must be a number, got {type(value).__name__}")
        self._max = (float(value),)

    @property
    def default(self) -> Any:
        return self._default

    @default.setter
    def default(self, value: Any) -> None:
        if not isinstance(value, (int, float, str, bool)) and value is not None:
            raise TypeError(f"default must be a scalar, got {type(value).__name__}")
        # A string default is only valid where the parameter is textual.
        if isinstance(value, str) and self.isNumber:
            raise TypeError(f"a numeric parameter cannot default to {value!r}")
        self._default = value


class FakePage:
    def __init__(self, name: str, pars: list[FakePar]) -> None:
        self.name = name
        self.pars = pars


class FakeAppendPage:
    """A page that actually creates parameters, as TD's `appendCustomPage` does."""

    def __init__(self, name: str) -> None:
        self.name = name
        self.pars: list[FakePar] = []
        self._appenders: dict[str, Any] = {}

    def _record(self, kind: str) -> Any:
        def append(name: str, **_kwargs: Any) -> FakePar:
            par = FakePar(name, kind=kind)
            self.pars.append(par)
            return par

        return append

    def __getattr__(self, item: str) -> Any:
        if item.startswith("append"):
            kind = item[len("append") :].lower()
            if kind in ("float", "int", "toggle", "pulse", "menu", "str", "sequence",
                        "python", "folder", "op", "momentary"):
                return self._record(kind)
        raise AttributeError(item)


class FakeComp:
    def __init__(self, path: str = "/project1/comp", family: str = "COMP") -> None:
        self.path = path
        self.family = family
        self.valid = True
        self.customPages: list[Any] = []

    def appendCustomPage(self, name: str) -> FakeAppendPage:
        page = FakeAppendPage(name)
        self.customPages.append(page)
        return page


class TypeClassificationTests(unittest.TestCase):
    def test_each_kind_is_recovered_from_its_predicate(self) -> None:
        for kind in ("float", "int", "toggle", "pulse", "menu", "string",
                     "momentary", "sequence", "python", "folder", "op"):
            par = FakePar("P", kind=kind)
            got, evidence = classify(par)
            self.assertEqual(got, kind, kind)
            # The evidence travels with the claim rather than being dropped.
            self.assertIsInstance(evidence, dict)

    def test_an_int_is_not_reported_as_a_float(self) -> None:
        # The pair `appendInt`/`appendFloat` both look numeric. Measured: only
        # `isInt` tells them apart, which is why `isNumber` is deliberately not
        # in the classification order.
        self.assertEqual(classify(FakePar("I", kind="int"))[0], "int")
        self.assertEqual(classify(FakePar("F", kind="float"))[0], "float")

    def test_a_menu_wins_over_a_number(self) -> None:
        # A toggle parameter reports `menuNames` (`['off', 'on']`) while
        # `isMenu` is False; a real menu is `isMenu` True. Order matters.
        self.assertEqual(classify(FakePar("M", kind="menu"))[0], "menu")

    def test_an_unidentifiable_parameter_is_unknown_with_its_evidence(self) -> None:
        # A type this bridge did not establish must not be presented as one it
        # did, so `unknown` carries the predicates that were checked.
        par = FakePar("X", kind="float")
        for predicate, _ in param_interface._TYPE_PREDICATES:
            setattr(par, predicate, False)
        kind, evidence = classify(par)
        self.assertEqual(kind, "unknown")
        self.assertIn("isFloat", evidence)
        self.assertFalse(evidence["isFloat"])


class ExportTests(unittest.TestCase):
    def _comp(self) -> FakeComp:
        comp = FakeComp()
        page = comp.appendCustomPage("Controls")
        page.pars = [FakePar("Speed", kind="float"), FakePar("Count", kind="int")]
        second = comp.appendCustomPage("Advanced")
        second.pars = [FakePar("Mode", kind="menu", menu_names=["a", "b", "c"])]
        return comp

    def test_export_reports_pages_and_parameters(self) -> None:
        out = export_interface(self._comp(), "/project1/comp")
        self.assertTrue(out["ok"])
        self.assertEqual(out["pageCount"], 2)
        self.assertEqual(out["parCount"], 3)
        self.assertEqual([page["page"] for page in out["pages"]], ["Controls", "Advanced"])

    def test_export_names_the_type_it_recovered(self) -> None:
        out = export_interface(self._comp(), "/project1/comp")
        by_name = {p["name"]: p for page in out["pages"] for p in page["pars"]}
        self.assertEqual(by_name["Speed"]["type"], "float")
        self.assertEqual(by_name["Count"]["type"], "int")
        self.assertEqual(by_name["Mode"]["type"], "menu")
        self.assertEqual(by_name["Mode"]["menuNames"], ["a", "b", "c"])

    def test_export_states_that_parmtype_is_absent(self) -> None:
        # Measured: `parmType` is None on every created parameter. A payload
        # full of nulls would look like data and say nothing.
        out = export_interface(self._comp(), "/project1/comp")
        self.assertNotIn("parmType", out["pages"][0]["pars"][0])
        self.assertIn("is* predicates", out["typeEvidence"])

    def test_range_bounds_are_unwrapped_from_the_measured_tuple(self) -> None:
        par = FakePar("Speed", kind="float", minimum=0.25, maximum=4.0)
        entry = param_interface._param_entry(par)
        # MEASURED: min/max come back as one-element tuples.
        self.assertEqual(entry["min"], 0.25)
        self.assertEqual(entry["max"], 4.0)

    def test_an_unidentified_parameter_is_exported_as_unknown_with_evidence(self) -> None:
        comp = FakeComp()
        page = comp.appendCustomPage("Controls")
        par = FakePar("Mystery", kind="float")
        for predicate, _ in param_interface._TYPE_PREDICATES:
            setattr(par, predicate, False)
        page.pars = [par]
        entry = export_interface(comp, "/p")["pages"][0]["pars"][0]
        self.assertEqual(entry["type"], "unknown")
        self.assertIn("evidence", entry)
        self.assertIn("unverified", entry)

    def test_a_capped_page_list_says_it_was_truncated(self) -> None:
        from tdmcp_bridge import constants as C

        comp = FakeComp()
        for index in range(C.INTERFACE_MAX_PAGES + 3):
            comp.appendCustomPage(f"p{index}")
        out = export_interface(comp, "/p")
        self.assertTrue(out["truncated"])
        self.assertEqual(out["pageCount"], C.INTERFACE_MAX_PAGES)

    def test_a_comp_that_cannot_report_pages_is_an_error_not_an_empty_list(self) -> None:
        # A property here would clash with the base class's assignment, so the
        # failure is injected on read instead — which is the case that matters:
        # an unreadable panel must not look like an empty one.
        comp = FakeComp()
        comp.customPages = mock.PropertyMock(
            side_effect=RuntimeError("no pages here")
        )
        out = export_interface(comp, "/p")
        self.assertFalse(out["ok"])
        self.assertEqual(out["code"], "tdmcp.interface.export_failed")


class ApplyTests(unittest.TestCase):
    def test_a_definition_is_created_with_its_kind(self) -> None:
        comp = FakeComp()
        out = param_interface.apply_interface(
            comp,
            "/p",
            [
                {"name": "Speed", "type": "float"},
                {"name": "Count", "type": "int"},
                {"name": "Enabled", "type": "toggle"},
                {"name": "Reset", "type": "pulse"},
            ],
        )
        self.assertTrue(out["ok"])
        self.assertEqual(out["mode"], "additive")
        self.assertEqual(out["created"], 4)
        names = [p.name for p in comp.customPages[0].pars]
        self.assertEqual(names, ["Speed", "Count", "Enabled", "Reset"])
        self.assertEqual(classify(comp.customPages[0].pars[0])[0], "float")
        self.assertEqual(classify(comp.customPages[0].pars[1])[0], "int")

    def test_applying_the_same_interface_twice_changes_nothing(self) -> None:
        # Idempotence is a property, not a claim: the second apply must report
        # zero creations, and the panel must not grow a duplicate.
        comp = FakeComp()
        definitions = [{"name": "Speed", "type": "float"}, {"name": "Count", "type": "int"}]
        first = param_interface.apply_interface(comp, "/p", definitions)
        self.assertEqual(first["created"], 2)
        second = param_interface.apply_interface(comp, "/p", definitions)
        self.assertEqual(second["created"], 0)
        self.assertEqual(second["unchanged"], 2)
        self.assertEqual(len(comp.customPages[0].pars), 2)

    def test_a_menu_gets_its_items(self) -> None:
        # MEASURED: `appendMenu` refuses a `menuNames` keyword, so the items are
        # a second step after creation.
        comp = FakeComp()
        param_interface.apply_interface(
            comp, "/p", [{"name": "Mode", "type": "menu", "menuNames": ["a", "b"]}]
        )
        self.assertEqual(comp.customPages[0].pars[0].menuNames, ["a", "b"])

    def test_apply_removes_nothing(self) -> None:
        # An authoring tool that deleted a user's controls because a payload did
        # not list them would be a data-loss primitive.
        comp = FakeComp()
        page = comp.appendCustomPage("Controls")
        page.pars = [FakePar("Mine", kind="float"), FakePar("AlsoMine", kind="toggle")]
        param_interface.apply_interface(
            comp, "/p", [{"name": "Theirs", "type": "float"}]
        )
        names = [p.name for p in comp.customPages[0].pars]
        self.assertEqual(names, ["Mine", "AlsoMine", "Theirs"])

    def test_a_type_conflict_is_skipped_not_converted(self) -> None:
        # Turning a caller's int count into a float, or a pulse into a toggle,
        # is not a migration anybody asked for.
        comp = FakeComp()
        page = comp.appendCustomPage("Controls")
        page.pars = [FakePar("Count", kind="int")]
        out = param_interface.apply_interface(
            comp, "/p", [{"name": "Count", "type": "float"}]
        )
        self.assertTrue(out["ok"])
        self.assertEqual(out["skipped"], 1)
        self.assertEqual(out["created"], 0)
        step = out["steps"][0]
        self.assertEqual(step["status"], "skipped")
        self.assertEqual(step["existingType"], "int")
        self.assertIn("not a migration anybody asked for", step["reason"])
        # And the existing parameter is untouched.
        self.assertEqual(classify(comp.customPages[0].pars[0])[0], "int")

    def test_one_conflict_does_not_abort_the_rest_of_the_panel(self) -> None:
        comp = FakeComp()
        page = comp.appendCustomPage("Controls")
        page.pars = [FakePar("Count", kind="int")]
        out = param_interface.apply_interface(
            comp,
            "/p",
            [
                {"name": "Count", "type": "float"},
                {"name": "Speed", "type": "float"},
                {"name": "Mode", "type": "menu"},
            ],
        )
        self.assertEqual(out["skipped"], 1)
        self.assertEqual(out["created"], 2)

    def test_every_step_is_reported_and_the_counts_add_up(self) -> None:
        # "ok" must mean "here is what happened", not "everything happened".
        comp = FakeComp()
        page = comp.appendCustomPage("Controls")
        page.pars = [FakePar("Count", kind="int")]
        out = param_interface.apply_interface(
            comp,
            "/p",
            [
                {"name": "Speed", "type": "float"},
                {"name": "Count", "type": "float"},
                {"name": "Nope", "type": "quantum"},
                {"name": "Missing", "type": "float"},
            ],
        )
        total = out["created"] + out["unchanged"] + out["skipped"] + out["failed"]
        self.assertEqual(total, out["requested"])
        self.assertEqual(len(out["steps"]), out["requested"])

    def test_a_malformed_definition_is_skipped_with_a_reason(self) -> None:
        comp = FakeComp()
        out = param_interface.apply_interface(
            comp,
            "/p",
            [
                {"type": "float"},
                {"name": "X"},
                {"name": "Y", "type": "notAType"},
                "not an object",
                {"name": "Z", "type": "float"},
            ],
        )
        self.assertEqual(out["skipped"], 4)
        self.assertEqual(out["created"], 1)
        for step in out["steps"][:4]:
            self.assertIn("reason", step)

    def test_a_default_and_range_are_applied_and_reported_separately(self) -> None:
        comp = FakeComp()
        out = param_interface.apply_interface(
            comp,
            "/p",
            [{"name": "Speed", "type": "float", "default": 2.5, "min": 0.0, "max": 10.0}],
        )
        step = out["steps"][0]
        self.assertEqual(step["status"], "created")
        self.assertTrue(step["defaultApplied"])
        self.assertTrue(step["minApplied"])
        self.assertTrue(step["maxApplied"])

    def test_a_refused_default_does_not_hide_the_created_parameter(self) -> None:
        # The parameter exists; only the default was refused. Reporting the
        # step as failed would tell the caller it does not exist.
        comp = FakeComp()
        out = param_interface.apply_interface(
            comp, "/p", [{"name": "Speed", "type": "float", "default": "not a number"}]
        )
        step = out["steps"][0]
        self.assertEqual(step["status"], "created")
        self.assertFalse(step["defaultApplied"])
        self.assertIn("defaultError", step)
        self.assertEqual(len(comp.customPages[0].pars), 1)

    def test_a_page_is_reused_rather_than_duplicated(self) -> None:
        comp = FakeComp()
        param_interface.apply_interface(
            comp,
            "/p",
            [
                {"name": "A", "type": "float", "page": "Controls"},
                {"name": "B", "type": "float", "page": "Controls"},
                {"name": "C", "type": "float", "page": "Advanced"},
            ],
        )
        self.assertEqual([p.name for p in comp.customPages], ["Controls", "Advanced"])

    def test_an_empty_or_oversized_apply_is_refused(self) -> None:
        from tdmcp_bridge import constants as C

        comp = FakeComp()
        for bad in ([], "nope", None, [{"name": "X", "type": "float"}] * (C.INTERFACE_MAX_APPLY_STEPS + 1)):
            out = param_interface.apply_interface(comp, "/p", bad)
            self.assertFalse(out["ok"], bad)
            self.assertIn(out["code"], ("tdmcp.interface.bad_definitions",
                                       "tdmcp.interface.too_many_steps"), bad)

    def test_an_operator_with_no_pages_fails_the_step_rather_than_the_call(self) -> None:
        class NoPages(FakeComp):
            appendCustomPage = None  # type: ignore[assignment]

        out = param_interface.apply_interface(
            NoPages(), "/p", [{"name": "Speed", "type": "float"}]
        )
        self.assertTrue(out["ok"])
        self.assertEqual(out["failed"], 1)
        self.assertIn("no custom parameter pages", out["steps"][0]["reason"])


class HandlerTests(unittest.TestCase):
    def test_export_is_the_default_action(self) -> None:
        comp = FakeComp()
        comp.appendCustomPage("Controls").pars = [FakePar("Speed")]
        with mock.patch.object(param_interface, "resolve_op", return_value=comp):
            out = param_interface.handle_param_interface({"path": "/p"})
        self.assertTrue(out["ok"])
        self.assertEqual(out["parCount"], 1)

    def test_a_non_comp_is_refused_by_family(self) -> None:
        comp = FakeComp(family="TOP")
        with mock.patch.object(param_interface, "resolve_op", return_value=comp):
            out = param_interface.handle_param_interface({"path": "/p"})
        self.assertFalse(out["ok"])
        self.assertEqual(out["code"], "tdmcp.interface.unsupported_family")
        self.assertEqual(out["family"], "TOP")

    def test_an_unknown_action_is_refused_by_name(self) -> None:
        comp = FakeComp()
        with mock.patch.object(param_interface, "resolve_op", return_value=comp):
            out = param_interface.handle_param_interface({"path": "/p", "action": "rewrite"})
        self.assertFalse(out["ok"])
        self.assertEqual(out["code"], "tdmcp.interface.unknown_action")

    def test_an_unresolvable_path_is_not_found(self) -> None:
        with mock.patch.object(param_interface, "resolve_op", return_value=None):
            out = param_interface.handle_param_interface({"path": "/p"})
        self.assertFalse(out["ok"])
        self.assertEqual(out["code"], "tdmcp.op.not_found")

    def test_param_interface_is_a_registered_route(self) -> None:
        self.assertIn("param_interface", tdmcp_bridge.HANDLERS)
        self.assertIs(
            tdmcp_bridge.HANDLERS["param_interface"],
            param_interface.handle_param_interface,
        )

    def test_an_export_round_trips_through_apply(self) -> None:
        # The property the whole tool exists for: export a panel, apply it to a
        # fresh COMP, and export that. The second export must match the first.
        source = FakeComp()
        page = source.appendCustomPage("Controls")
        page.pars = [
            FakePar("Speed", kind="float", minimum=0.5, maximum=8.0),
            FakePar("Count", kind="int"),
            FakePar("Mode", kind="menu", menu_names=["a", "b"]),
            FakePar("Reset", kind="pulse"),
        ]
        first = export_interface(source, "/src")

        target = FakeComp()
        definitions = [
            {
                "name": par["name"],
                "type": par["type"],
                "page": page_name["page"],
                "menuNames": par.get("menuNames"),
                "min": par.get("min"),
                "max": par.get("max"),
            }
            for page_name in first["pages"]
            for par in page_name["pars"]
        ]
        applied = param_interface.apply_interface(target, "/dst", definitions)
        self.assertEqual(applied["failed"], 0, applied["steps"])
        self.assertEqual(applied["created"], len(definitions))

        second = export_interface(target, "/dst")
        self.assertEqual(
            [p["type"] for page in second["pages"] for p in page["pars"]],
            [p["type"] for page in first["pages"] for p in page["pars"]],
        )
        self.assertEqual(
            [p["name"] for page in second["pages"] for p in page["pars"]],
            [p["name"] for page in first["pages"] for p in page["pars"]],
        )
        # Menu items survive the trip, which is the part most easily lost.
        modes = [p for page in second["pages"] for p in page["pars"] if p["name"] == "Mode"]
        self.assertEqual(modes[0]["menuNames"], ["a", "b"])


if __name__ == "__main__":
    unittest.main()
