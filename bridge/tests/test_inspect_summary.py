"""Unit tests for inspect child roster shaping (no live TD required)."""

from __future__ import annotations

import os
import sys
import unittest
from types import SimpleNamespace
from typing import Any
from unittest.mock import MagicMock, patch

sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))

import tdmcp_bridge  # noqa: E402


def _fake_child(name: str, *, op_type: str = "nullTOP", family: str = "TOP") -> SimpleNamespace:
    return SimpleNamespace(
        name=name,
        path=f"/project1/{name}",
        family=family,
        opType=op_type,
    )


class FakeInspectPar:
    """Minimal Par for inspect params shaping (mode / expr / eval)."""

    def __init__(
        self,
        name: str,
        *,
        val: Any = None,
        mode: Any = "CONSTANT",
        expr: str | None = None,
        eval_raises: Exception | None = None,
    ) -> None:
        self.name = name
        self.mode = mode
        self.expr = expr
        self._val = val
        self._eval_raises = eval_raises

    def eval(self) -> Any:
        if self._eval_raises is not None:
            raise self._eval_raises
        return self._val


def _fake_node(
    children: list[SimpleNamespace],
    *,
    errors: object = "",
    warnings: object = "",
    cook: object | None = None,
    pars: list[Any] | None = None,
) -> SimpleNamespace:
    par_list = list(pars) if pars is not None else []
    return SimpleNamespace(
        path="/project1",
        family="COMP",
        opType="baseCOMP",
        children=children,
        pars=lambda: par_list,
        errors=lambda: errors,
        warnings=lambda: warnings,
        valid=True,
        cook=cook if cook is not None else MagicMock(),
    )


class InspectSummaryRosterTest(unittest.TestCase):
    def test_summary_small_roster(self) -> None:
        node = _fake_node([
            _fake_child("noise1", op_type="noiseTOP"),
            _fake_child("out1", op_type="outTOP"),
            _fake_child("geo1", op_type="geometryCOMP", family="COMP"),
        ])
        out = tdmcp_bridge.build_inspect_node(node, detail_level="summary")
        self.assertEqual(out["childCount"], 3)
        self.assertEqual(out["childrenReturned"], 3)
        self.assertNotIn("childrenTruncated", out)
        self.assertNotIn("truncation", out)
        self.assertNotIn("errors", out)
        self.assertNotIn("warnings", out)
        self.assertEqual(
            out["children"],
            [
                {"name": "noise1", "opType": "noiseTOP"},
                {"name": "out1", "opType": "outTOP"},
                {"name": "geo1", "opType": "geometryCOMP"},
            ],
        )

    def test_detailed_small_roster(self) -> None:
        node = _fake_node([_fake_child("noise1", op_type="noiseTOP")])
        out = tdmcp_bridge.build_inspect_node(node, detail_level="detailed")
        self.assertEqual(out["childCount"], 1)
        self.assertEqual(out["childrenReturned"], 1)
        self.assertNotIn("truncation", out)
        self.assertEqual(
            out["children"],
            [{
                "path": "/project1/noise1",
                "family": "TOP",
                "opType": "noiseTOP",
            }],
        )

    def test_truncation_at_cap_summary(self) -> None:
        kids = [_fake_child(f"op{i}") for i in range(65)]
        out = tdmcp_bridge.build_inspect_node(
            _fake_node(kids), detail_level="summary"
        )
        self.assertEqual(out["childCount"], 65)
        self.assertEqual(out["childrenReturned"], 64)
        self.assertTrue(out["childrenTruncated"])
        trunc = out["truncation"]
        self.assertEqual(trunc["field"], "children")
        self.assertEqual(trunc["limit"], tdmcp_bridge.CHILDREN_ROSTER_LIMIT)
        self.assertEqual(trunc["code"], "tdmcp.op.children_truncated")
        self.assertIn("64 of 65", trunc["message"])
        self.assertIn("detailLevel does not raise this cap", trunc["mitigation"])
        self.assertEqual(len(out["children"]), 64)
        self.assertEqual(out["children"][0]["name"], "op0")
        self.assertEqual(out["children"][-1]["name"], "op63")

    def test_detailed_does_not_raise_cap(self) -> None:
        kids = [_fake_child(f"op{i}") for i in range(70)]
        out = tdmcp_bridge.build_inspect_node(
            _fake_node(kids), detail_level="detailed"
        )
        self.assertEqual(out["childCount"], 70)
        self.assertEqual(out["childrenReturned"], 64)
        self.assertTrue(out["childrenTruncated"])
        self.assertEqual(out["truncation"]["limit"], 64)
        self.assertIn("path", out["children"][0])

    def test_name_fallback_from_path(self) -> None:
        child = SimpleNamespace(
            name=None,
            path="/project1/fallback1",
            family="TOP",
            opType="nullTOP",
        )
        out = tdmcp_bridge.build_inspect_node(
            _fake_node([child]), detail_level="summary"
        )
        self.assertEqual(out["children"][0]["name"], "fallback1")


class InspectParamsTest(unittest.TestCase):
    def test_constant_par_no_expr_key(self) -> None:
        node = _fake_node([], pars=[FakeInspectPar("resolutionw", val=128)])
        out = tdmcp_bridge.build_inspect_node(node, want_nodes=False, want_params=True)
        self.assertEqual(
            out["params"],
            [{"name": "resolutionw", "mode": "CONSTANT", "val": 128}],
        )
        self.assertNotIn("expr", out["params"][0])

    def test_expression_par_includes_expr(self) -> None:
        node = _fake_node(
            [],
            pars=[
                FakeInspectPar(
                    "resolutionw",
                    val=3554,
                    mode="EXPRESSION",
                    expr="absTime.seconds*4",
                )
            ],
        )
        out = tdmcp_bridge.build_inspect_node(node, want_nodes=False, want_params=True)
        self.assertEqual(
            out["params"],
            [{
                "name": "resolutionw",
                "mode": "EXPRESSION",
                "val": 3554,
                "expr": "absTime.seconds*4",
            }],
        )

    def test_enum_like_mode_name(self) -> None:
        mode = SimpleNamespace(name="EXPRESSION")
        node = _fake_node(
            [],
            pars=[
                FakeInspectPar(
                    "resolutionw",
                    val=1,
                    mode=mode,
                    expr="me.time.seconds",
                )
            ],
        )
        out = tdmcp_bridge.build_inspect_node(node, want_nodes=False, want_params=True)
        self.assertEqual(out["params"][0]["mode"], "EXPRESSION")
        self.assertEqual(out["params"][0]["expr"], "me.time.seconds")

    def test_op_val_coerced_to_path(self) -> None:
        op_val = SimpleNamespace(path="/project1/out1")
        node = _fake_node(
            [],
            pars=[FakeInspectPar("opviewer", val=op_val)],
        )
        out = tdmcp_bridge.build_inspect_node(node, want_nodes=False, want_params=True)
        self.assertEqual(out["params"][0]["val"], "/project1/out1")
        self.assertEqual(out["params"][0]["mode"], "CONSTANT")

    def test_eval_raises_keeps_mode_and_expr(self) -> None:
        node = _fake_node(
            [],
            pars=[
                FakeInspectPar(
                    "resolutionw",
                    mode="EXPRESSION",
                    expr="1/0",
                    eval_raises=RuntimeError("bad expr"),
                )
            ],
        )
        out = tdmcp_bridge.build_inspect_node(node, want_nodes=False, want_params=True)
        self.assertEqual(
            out["params"],
            [{
                "name": "resolutionw",
                "mode": "EXPRESSION",
                "val": None,
                "expr": "1/0",
            }],
        )


class InspectMessagesTest(unittest.TestCase):
    def test_op_messages_string_and_empty(self) -> None:
        self.assertEqual(tdmcp_bridge._op_messages(lambda: "one warn"), ["one warn"])
        self.assertEqual(tdmcp_bridge._op_messages(lambda: ""), [])
        self.assertEqual(tdmcp_bridge._op_messages(lambda: "a\nb\n"), ["a", "b"])
        self.assertEqual(tdmcp_bridge._op_messages(lambda: ["x", "  ", "y"]), ["x", "y"])

        def _boom() -> str:
            raise RuntimeError("x")

        self.assertEqual(tdmcp_bridge._op_messages(_boom), [])

    def test_build_inspect_errors_warnings_strings(self) -> None:
        node = _fake_node([], errors="cook failed", warnings="missing input")
        out = tdmcp_bridge.build_inspect_node(
            node, want_errors=True, want_warnings=True
        )
        self.assertEqual(out["errors"], ["cook failed"])
        self.assertEqual(out["warnings"], ["missing input"])

    def test_build_inspect_want_nodes_false_omits_roster(self) -> None:
        child = SimpleNamespace(name="n1", opType="nullTOP", path="/project1/n1")
        node = _fake_node([child], errors="err1", warnings="warn1")
        out = tdmcp_bridge.build_inspect_node(
            node,
            want_nodes=False,
            want_errors=True,
            want_warnings=True,
        )
        self.assertEqual(out["path"], "/project1")
        self.assertEqual(out["opType"], "baseCOMP")
        self.assertEqual(out["errors"], ["err1"])
        self.assertEqual(out["warnings"], ["warn1"])
        self.assertNotIn("children", out)
        self.assertNotIn("childCount", out)
        self.assertNotIn("childrenReturned", out)
        self.assertNotIn("childrenTruncated", out)
        self.assertNotIn("truncation", out)

    def test_handle_inspect_empty_include_defaults(self) -> None:
        node = _fake_node([], errors="err1", warnings="warn1")
        with patch.dict(sys.modules, {"td": SimpleNamespace()}):
            with patch.object(tdmcp_bridge, "tdmcp_resolve", return_value=node):
                result = tdmcp_bridge.handle_inspect({
                    "paths": ["/project1"],
                    "include": [],
                })
        self.assertTrue(result["ok"])
        self.assertEqual(len(result["nodes"]), 1)
        self.assertTrue(result["nodes"][0]["ok"])
        self.assertEqual(result["nodes"][0]["errors"], ["err1"])
        self.assertEqual(result["nodes"][0]["warnings"], ["warn1"])
        self.assertIn("children", result["nodes"][0])
        self.assertNotIn("params", result["nodes"][0])

    def test_handle_inspect_allowlist_nodes_only(self) -> None:
        node = _fake_node([], errors="err1", warnings="warn1")
        with patch.dict(sys.modules, {"td": SimpleNamespace()}):
            with patch.object(tdmcp_bridge, "tdmcp_resolve", return_value=node):
                result = tdmcp_bridge.handle_inspect({
                    "paths": ["/project1"],
                    "include": ["nodes"],
                })
        self.assertTrue(result["ok"])
        self.assertNotIn("errors", result["nodes"][0])
        self.assertNotIn("warnings", result["nodes"][0])
        self.assertIn("children", result["nodes"][0])

    def test_handle_inspect_allowlist_warnings_only(self) -> None:
        child = SimpleNamespace(name="n1", opType="nullTOP", path="/project1/n1")
        node = _fake_node([child], errors="err1", warnings="warn1")
        with patch.dict(sys.modules, {"td": SimpleNamespace()}):
            with patch.object(tdmcp_bridge, "tdmcp_resolve", return_value=node):
                result = tdmcp_bridge.handle_inspect({
                    "paths": ["/project1"],
                    "include": ["warnings"],
                })
        self.assertTrue(result["ok"])
        self.assertEqual(result["nodes"][0]["warnings"], ["warn1"])
        self.assertNotIn("errors", result["nodes"][0])
        self.assertNotIn("children", result["nodes"][0])
        self.assertNotIn("childCount", result["nodes"][0])
        self.assertNotIn("childrenReturned", result["nodes"][0])

    def test_handle_inspect_allowlist_errors_only(self) -> None:
        child = SimpleNamespace(name="n1", opType="nullTOP", path="/project1/n1")
        node = _fake_node([child], errors="err1", warnings="warn1")
        with patch.dict(sys.modules, {"td": SimpleNamespace()}):
            with patch.object(tdmcp_bridge, "tdmcp_resolve", return_value=node):
                result = tdmcp_bridge.handle_inspect({
                    "paths": ["/project1"],
                    "include": ["errors"],
                })
        self.assertTrue(result["ok"])
        self.assertEqual(result["nodes"][0]["errors"], ["err1"])
        self.assertEqual(result["nodes"][0]["path"], "/project1")
        self.assertEqual(result["nodes"][0]["opType"], "baseCOMP")
        self.assertNotIn("warnings", result["nodes"][0])
        self.assertNotIn("children", result["nodes"][0])
        self.assertNotIn("childCount", result["nodes"][0])
        self.assertNotIn("childrenReturned", result["nodes"][0])

    def test_handle_inspect_force_cooks_target(self) -> None:
        cook = MagicMock()
        node = _fake_node([], errors="err1", cook=cook)
        with patch.dict(sys.modules, {"td": SimpleNamespace()}):
            with patch.object(tdmcp_bridge, "tdmcp_resolve", return_value=node):
                result = tdmcp_bridge.handle_inspect({
                    "paths": ["/project1"],
                    "include": [],
                })
        self.assertTrue(result["ok"])
        cook.assert_called_once_with(force=True)
        self.assertEqual(result["nodes"][0]["errors"], ["err1"])

    def test_handle_inspect_cook_raise_still_ok(self) -> None:
        cook = MagicMock(side_effect=RuntimeError("cook boom"))
        node = _fake_node([], errors="err1", warnings="warn1", cook=cook)
        with patch.dict(sys.modules, {"td": SimpleNamespace()}):
            with patch.object(tdmcp_bridge, "tdmcp_resolve", return_value=node):
                result = tdmcp_bridge.handle_inspect({
                    "paths": ["/project1"],
                    "include": [],
                })
        self.assertTrue(result["ok"])
        cook.assert_called_once_with(force=True)
        self.assertEqual(result["nodes"][0]["errors"], ["err1"])
        self.assertEqual(result["nodes"][0]["warnings"], ["warn1"])

    def test_handle_inspect_paths_required(self) -> None:
        with patch.dict(sys.modules, {"td": SimpleNamespace()}):
            result = tdmcp_bridge.handle_inspect({"include": []})
        self.assertFalse(result["ok"])
        self.assertEqual(result["code"], "tdmcp.op.paths_required")

    def test_handle_inspect_partial_success(self) -> None:
        good = _fake_node([], errors="", warnings="")
        with patch.dict(sys.modules, {"td": SimpleNamespace()}):
            with patch.object(
                tdmcp_bridge,
                "tdmcp_resolve",
                side_effect=[good, None],
            ):
                result = tdmcp_bridge.handle_inspect({
                    "paths": ["/project1", "/missing"],
                    "include": ["nodes"],
                })
        self.assertTrue(result["ok"])
        self.assertEqual(len(result["nodes"]), 2)
        self.assertTrue(result["nodes"][0]["ok"])
        self.assertFalse(result["nodes"][1]["ok"])
        self.assertEqual(result["nodes"][1]["code"], "tdmcp.op.not_found")

    def test_handle_inspect_paths_truncated(self) -> None:
        node = _fake_node([])
        paths = [f"/project1/op{i}" for i in range(tdmcp_bridge.INSPECT_PATHS_LIMIT + 3)]
        with patch.dict(sys.modules, {"td": SimpleNamespace()}):
            with patch.object(tdmcp_bridge, "tdmcp_resolve", return_value=node):
                result = tdmcp_bridge.handle_inspect({
                    "paths": paths,
                    "include": ["nodes"],
                })
        self.assertTrue(result["ok"])
        self.assertTrue(result["pathsTruncated"])
        self.assertEqual(result["truncation"]["code"], "tdmcp.op.paths_truncated")
        self.assertEqual(len(result["nodes"]), tdmcp_bridge.INSPECT_PATHS_LIMIT)

    def test_handle_inspect_legacy_path_compat(self) -> None:
        node = _fake_node([])
        with patch.dict(sys.modules, {"td": SimpleNamespace()}):
            with patch.object(tdmcp_bridge, "tdmcp_resolve", return_value=node):
                result = tdmcp_bridge.handle_inspect({
                    "path": "/project1",
                    "include": ["nodes"],
                })
        self.assertTrue(result["ok"])
        self.assertEqual(len(result["nodes"]), 1)

    def test_force_cook_positional_fallback(self) -> None:
        calls: list[tuple] = []

        def cook(*args: object, **kwargs: object) -> None:
            if kwargs.get("force") is True:
                raise TypeError("no force kw")
            calls.append((args, kwargs))

        node = SimpleNamespace(cook=cook)
        self.assertTrue(tdmcp_bridge._force_cook(node))
        self.assertEqual(calls, [((True,), {})])

    def test_force_cook_missing_is_noop(self) -> None:
        self.assertFalse(tdmcp_bridge._force_cook(SimpleNamespace()))  # no cook attr

    def test_force_cook_none_returns_false(self) -> None:
        self.assertFalse(tdmcp_bridge._force_cook(None))

    def test_force_cook_runtime_error_swallowed(self) -> None:
        def cook(*, force: bool = False) -> None:  # noqa: ARG001
            raise RuntimeError("cook boom")

        self.assertFalse(tdmcp_bridge._force_cook(SimpleNamespace(cook=cook)))

    def test_force_cook_bare_call_fallback(self) -> None:
        calls: list[str] = []

        def cook(*args: object, **kwargs: object) -> None:
            if kwargs or args:
                raise TypeError("signature mismatch")
            calls.append("bare")

        self.assertTrue(tdmcp_bridge._force_cook(SimpleNamespace(cook=cook)))
        self.assertEqual(calls, ["bare"])

    def test_force_cook_kw_success(self) -> None:
        seen: list[bool] = []

        def cook(*, force: bool = False) -> None:
            seen.append(force)

        self.assertTrue(tdmcp_bridge._force_cook(SimpleNamespace(cook=cook)))
        self.assertEqual(seen, [True])


if __name__ == "__main__":
    unittest.main()
