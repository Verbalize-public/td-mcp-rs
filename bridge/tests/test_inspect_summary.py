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


class FakeEnableParGroup:
    """Minimal custom ParGroup with enableExpr for inspect enrichment."""

    def __init__(
        self,
        name: str,
        *,
        label: str | None = None,
        enable_expr: str = "",
    ) -> None:
        self.name = name
        self.label = label if label is not None else name.lower()
        self.enableExpr = enable_expr


def _fake_node(
    children: list[SimpleNamespace],
    *,
    path: str = "/project1",
    family: str = "COMP",
    op_type: str = "baseCOMP",
    errors: object = "",
    warnings: object = "",
    cook: object | None = None,
    pars: list[Any] | None = None,
    custom_par_groups: list[Any] | None = None,
    custom_pars: list[Any] | None = None,
    eval_expression: Any | None = None,
    eval_expression_raises: Exception | None = None,
    inputs: list[Any] | None = None,
    outputs: list[Any] | None = None,
    text: str | None = None,
    is_text: bool = False,
    is_table: bool = False,
    compile_result: str | None = None,
    par: Any | None = None,
) -> SimpleNamespace:
    par_list = list(pars) if pars is not None else []

    def _eval_expression(expr: str) -> Any:
        if eval_expression_raises is not None:
            raise eval_expression_raises
        if callable(eval_expression):
            return eval_expression(expr)
        return None

    ns = SimpleNamespace(
        path=path,
        family=family,
        opType=op_type,
        children=children,
        pars=lambda: par_list,
        errors=lambda: errors,
        warnings=lambda: warnings,
        valid=True,
        cook=cook if cook is not None else MagicMock(),
        evalExpression=_eval_expression,
        inputs=[] if inputs is None else inputs,
        outputs=[] if outputs is None else outputs,
    )
    if custom_par_groups is not None:
        ns.customParGroups = custom_par_groups
    if custom_pars is not None:
        ns.customPars = custom_pars
    if text is not None:
        ns.text = text
        ns.isText = is_text
        ns.isTable = is_table
    if compile_result is not None:
        ns.compileResult = compile_result
    if par is not None:
        ns.par = par
    return ns


class _FakeShaderPar:
    """Minimal Par that ``.eval()`` returns a shader DAT ref (or raises)."""

    def __init__(self, val: Any = None, *, eval_raises: Exception | None = None) -> None:
        self._val = val
        self._eval_raises = eval_raises

    def eval(self) -> Any:
        if self._eval_raises is not None:
            raise self._eval_raises
        return self._val


def _fake_dat(
    *,
    path: str = "/project1/code",
    op_type: str = "textDAT",
    text: str = "",
    is_text: bool = True,
    is_table: bool = False,
    valid: bool = True,
) -> SimpleNamespace:
    return SimpleNamespace(
        path=path,
        family="DAT",
        opType=op_type,
        text=text,
        isText=is_text,
        isTable=is_table,
        valid=valid,
        children=[],
        pars=lambda: [],
        errors=lambda: "",
        warnings=lambda: "",
        inputs=[],
        outputs=[],
    )


class _BrokenInputsNode:
    """Fake node whose ``inputs`` property raises (wire enrichment fault path)."""

    path = "/project1/broken"
    family = "TOP"
    opType = "nullTOP"
    children: list[Any] = []
    outputs: list[Any] = []

    @property
    def inputs(self) -> list[Any]:
        raise RuntimeError("inputs boom")

    def pars(self) -> list[Any]:
        return []

    def errors(self) -> str:
        return ""

    def warnings(self) -> str:
        return ""


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
        cap = tdmcp_bridge.CHILDREN_ROSTER_LIMIT
        kids = [_fake_child(f"op{i}") for i in range(cap + 1)]
        out = tdmcp_bridge.build_inspect_node(
            _fake_node(kids), detail_level="summary"
        )
        self.assertEqual(out["childCount"], cap + 1)
        self.assertEqual(out["childrenReturned"], cap)
        self.assertTrue(out["childrenTruncated"])
        trunc = out["truncation"]
        self.assertEqual(trunc["field"], "children")
        self.assertEqual(trunc["limit"], cap)
        self.assertEqual(trunc["code"], "tdmcp.op.children_truncated")
        self.assertIn(f"{cap} of {cap + 1}", trunc["message"])
        self.assertIn("detailLevel does not raise this cap", trunc["mitigation"])
        self.assertEqual(len(out["children"]), cap)
        self.assertEqual(out["children"][0]["name"], "op0")
        self.assertEqual(out["children"][-1]["name"], f"op{cap - 1}")

    def test_detailed_does_not_raise_cap(self) -> None:
        cap = tdmcp_bridge.CHILDREN_ROSTER_LIMIT
        kids = [_fake_child(f"op{i}") for i in range(cap + 6)]
        out = tdmcp_bridge.build_inspect_node(
            _fake_node(kids), detail_level="detailed"
        )
        self.assertEqual(out["childCount"], cap + 6)
        self.assertEqual(out["childrenReturned"], cap)
        self.assertTrue(out["childrenTruncated"])
        self.assertEqual(out["truncation"]["limit"], cap)
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

    def test_handle_inspect_does_not_force_cook(self) -> None:
        cook = MagicMock()
        node = _fake_node([], errors="err1", cook=cook)
        with patch.dict(sys.modules, {"td": SimpleNamespace()}):
            with patch.object(tdmcp_bridge, "tdmcp_resolve", return_value=node):
                result = tdmcp_bridge.handle_inspect({
                    "paths": ["/project1"],
                    "include": [],
                })
        self.assertTrue(result["ok"])
        cook.assert_not_called()
        self.assertEqual(result["nodes"][0]["errors"], ["err1"])

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
        mitigation = " ".join(result["truncation"].get("mitigation") or [])
        self.assertNotIn("force-cook", mitigation.lower())
        self.assertNotIn("force cook", mitigation.lower())

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


ENABLE_PARM_WARN = (
    "Warning: Error(s) in enable parm expressions. (/project1/container1)"
)


class InspectEnableExprEnrichmentTest(unittest.TestCase):
    def test_match_and_eval_raise(self) -> None:
        def _eval(expr: str) -> None:
            raise SyntaxError("'(' was never closed (, line 1)")

        node = _fake_node(
            [],
            warnings=ENABLE_PARM_WARN,
            custom_par_groups=[
                FakeEnableParGroup("Sad", label="sad", enable_expr="app(1"),
            ],
            eval_expression=_eval,
        )
        out = tdmcp_bridge.build_inspect_node(
            node, want_nodes=False, want_warnings=True
        )
        self.assertEqual(out["warnings"], [ENABLE_PARM_WARN])
        self.assertEqual(
            out["parmExprIssues"],
            [{
                "kind": "enableExpr",
                "par": "Sad",
                "label": "sad",
                "expr": "app(1",
                "errorType": "SyntaxError",
                "message": "'(' was never closed (, line 1)",
            }],
        )
        self.assertEqual(out["diagnostics"][0]["code"], "tdmcp.par.enable_expr_failed")
        self.assertEqual(out["diagnostics"][0]["severity"], "warning")
        self.assertEqual(out["diagnostics"][0]["context"]["par"], "Sad")
        self.assertEqual(out["diagnostics"][0]["context"]["expr"], "app(1")

    def test_no_match_omits_enrichment(self) -> None:
        def _eval(expr: str) -> None:
            raise SyntaxError("bad")

        node = _fake_node(
            [],
            warnings="missing input",
            custom_par_groups=[
                FakeEnableParGroup("Sad", enable_expr="app(1"),
            ],
            eval_expression=_eval,
        )
        out = tdmcp_bridge.build_inspect_node(
            node, want_nodes=False, want_warnings=True
        )
        self.assertEqual(out["warnings"], ["missing input"])
        self.assertNotIn("parmExprIssues", out)
        self.assertNotIn("diagnostics", out)

    def test_clean_enable_expr_omits_keys(self) -> None:
        node = _fake_node(
            [],
            warnings=ENABLE_PARM_WARN,
            custom_par_groups=[
                FakeEnableParGroup("Sad", enable_expr="1"),
            ],
            eval_expression=lambda expr: True,
        )
        out = tdmcp_bridge.build_inspect_node(
            node, want_nodes=False, want_warnings=True
        )
        self.assertEqual(out["warnings"], [ENABLE_PARM_WARN])
        self.assertNotIn("parmExprIssues", out)
        self.assertNotIn("diagnostics", out)

    def test_dedupe_same_expr(self) -> None:
        def _eval(expr: str) -> None:
            raise NameError("app")

        node = _fake_node(
            [],
            warnings=ENABLE_PARM_WARN,
            custom_par_groups=[
                FakeEnableParGroup("Sad", enable_expr="app(1"),
                FakeEnableParGroup("Glad", enable_expr="app(1"),
            ],
            eval_expression=_eval,
        )
        out = tdmcp_bridge.build_inspect_node(
            node, want_nodes=False, want_warnings=True
        )
        self.assertEqual(len(out["parmExprIssues"]), 1)
        self.assertEqual(out["parmExprIssues"][0]["par"], "Sad")

    def test_eval_cap(self) -> None:
        calls: list[str] = []

        def _eval(expr: str) -> None:
            calls.append(expr)
            raise SyntaxError(expr)

        groups = [
            FakeEnableParGroup(f"P{i}", enable_expr=f"bad{i}(")
            for i in range(tdmcp_bridge.ENABLE_EXPR_EVAL_LIMIT + 5)
        ]
        node = _fake_node(
            [],
            warnings=ENABLE_PARM_WARN,
            custom_par_groups=groups,
            eval_expression=_eval,
        )
        out = tdmcp_bridge.build_inspect_node(
            node, want_nodes=False, want_warnings=True
        )
        self.assertEqual(len(calls), tdmcp_bridge.ENABLE_EXPR_EVAL_LIMIT)
        self.assertEqual(len(out["parmExprIssues"]), tdmcp_bridge.ENABLE_EXPR_EVAL_LIMIT)

    def test_collect_throws_still_shapes(self) -> None:
        node = _fake_node([], warnings=ENABLE_PARM_WARN)
        # Force gather path to raise outside per-expr try (bad iterable).
        node.customParGroups = object()  # type: ignore[assignment]
        out = tdmcp_bridge.build_inspect_node(
            node, want_nodes=False, want_warnings=True
        )
        self.assertEqual(out["warnings"], [ENABLE_PARM_WARN])
        self.assertNotIn("parmExprIssues", out)
        self.assertNotIn("diagnostics", out)

    def test_fallback_custom_pars(self) -> None:
        def _eval(expr: str) -> None:
            raise SyntaxError("bad")

        node = _fake_node(
            [],
            warnings="Enable expression error",
            custom_pars=[FakeEnableParGroup("Sad", enable_expr="app(1")],
            eval_expression=_eval,
        )
        out = tdmcp_bridge.build_inspect_node(
            node, want_nodes=False, want_warnings=True
        )
        self.assertEqual(out["parmExprIssues"][0]["par"], "Sad")

    def test_handle_inspect_ok_with_enrichment(self) -> None:
        def _eval(expr: str) -> None:
            raise SyntaxError("bad")

        node = _fake_node(
            [],
            warnings=ENABLE_PARM_WARN,
            custom_par_groups=[
                FakeEnableParGroup("Sad", enable_expr="app(1"),
            ],
            eval_expression=_eval,
        )
        with patch.dict(sys.modules, {"td": SimpleNamespace()}):
            with patch.object(tdmcp_bridge, "tdmcp_resolve", return_value=node):
                result = tdmcp_bridge.handle_inspect({
                    "paths": ["/project1"],
                    "include": [],
                })
        self.assertTrue(result["ok"])
        self.assertTrue(result["nodes"][0]["ok"])
        self.assertIn("parmExprIssues", result["nodes"][0])
        self.assertEqual(
            result["nodes"][0]["diagnostics"][0]["code"],
            "tdmcp.par.enable_expr_failed",
        )


class InspectWiresTest(unittest.TestCase):
    def test_unwired_empty_arrays(self) -> None:
        out = tdmcp_bridge.build_inspect_node(_fake_node([]))
        self.assertEqual(out["inputs"], [])
        self.assertEqual(out["outputs"], [])

    def test_wired_with_gap(self) -> None:
        a = _fake_child("in_mask", op_type="inTOP")
        c = _fake_child("in_color", op_type="inTOP")
        out_peer = _fake_child("null_beauty", op_type="nullTOP")
        node = _fake_node(
            [],
            inputs=[a, None, c],
            outputs=[out_peer],
        )
        out = tdmcp_bridge.build_inspect_node(node)
        self.assertEqual(
            out["inputs"],
            [
                {
                    "path": "/project1/in_mask",
                    "name": "in_mask",
                    "opType": "inTOP",
                },
                None,
                {
                    "path": "/project1/in_color",
                    "name": "in_color",
                    "opType": "inTOP",
                },
            ],
        )
        self.assertEqual(
            out["outputs"],
            [{
                "path": "/project1/null_beauty",
                "name": "null_beauty",
                "opType": "nullTOP",
            }],
        )

    def test_want_nodes_false_omits_wires(self) -> None:
        a = _fake_child("in_mask", op_type="inTOP")
        out = tdmcp_bridge.build_inspect_node(
            _fake_node([], inputs=[a]),
            want_nodes=False,
            want_params=True,
        )
        self.assertNotIn("inputs", out)
        self.assertNotIn("outputs", out)
        self.assertNotIn("children", out)

    def test_broken_inputs_accessor_yields_empty(self) -> None:
        out = tdmcp_bridge.build_inspect_node(_BrokenInputsNode())
        self.assertEqual(out["inputs"], [])
        self.assertEqual(out["outputs"], [])
        self.assertEqual(out["path"], "/project1/broken")


class InspectContentTest(unittest.TestCase):
    def test_want_content_false_omits_key_on_dat(self) -> None:
        node = _fake_dat(text="hello")
        out = tdmcp_bridge.build_inspect_node(node, want_content=False)
        self.assertNotIn("content", out)

    def test_dat_text_content(self) -> None:
        body = "void main() {}\n"
        node = _fake_dat(path="/project1/shader", text=body)
        out = tdmcp_bridge.build_inspect_node(node, want_nodes=False, want_content=True)
        self.assertEqual(
            out["content"],
            {
                "kind": "dat",
                "isText": True,
                "isTable": False,
                "bytes": len(body.encode("utf-8")),
                "text": body,
            },
        )

    def test_dat_table_content(self) -> None:
        body = "name\tval\na\t1\n"
        node = _fake_dat(
            path="/project1/tbl",
            op_type="tableDAT",
            text=body,
            is_text=False,
            is_table=True,
        )
        out = tdmcp_bridge.build_inspect_node(node, want_nodes=False, want_content=True)
        content = out["content"]
        self.assertEqual(content["kind"], "dat")
        self.assertFalse(content["isText"])
        self.assertTrue(content["isTable"])
        self.assertEqual(content["text"], body)

    def test_non_eligible_omits_content(self) -> None:
        node = _fake_node([], family="TOP", op_type="noiseTOP")
        out = tdmcp_bridge.build_inspect_node(node, want_nodes=False, want_content=True)
        self.assertNotIn("content", out)

    def test_handle_inspect_default_include_omits_content(self) -> None:
        dat = _fake_dat(path="/project1/code", text="x = 1\n")
        with patch.dict(sys.modules, {"td": SimpleNamespace()}):
            with patch.object(tdmcp_bridge, "tdmcp_resolve", return_value=dat):
                out = tdmcp_bridge.handle_inspect({"paths": ["/project1/code"]})
        self.assertTrue(out["ok"])
        self.assertNotIn("content", out["nodes"][0])

    def test_handle_inspect_allowlist_without_content_omits(self) -> None:
        dat = _fake_dat(path="/project1/code", text="x = 1\n")
        with patch.dict(sys.modules, {"td": SimpleNamespace()}):
            with patch.object(tdmcp_bridge, "tdmcp_resolve", return_value=dat):
                out = tdmcp_bridge.handle_inspect({
                    "paths": ["/project1/code"],
                    "include": ["nodes", "errors"],
                })
        self.assertNotIn("content", out["nodes"][0])

    def test_handle_inspect_include_content_pulls_dat(self) -> None:
        dat = _fake_dat(path="/project1/code", text="print(1)\n")
        with patch.dict(sys.modules, {"td": SimpleNamespace()}):
            with patch.object(tdmcp_bridge, "tdmcp_resolve", return_value=dat):
                out = tdmcp_bridge.handle_inspect({
                    "paths": ["/project1/code"],
                    "include": ["nodes", "content"],
                })
        self.assertEqual(out["nodes"][0]["content"]["kind"], "dat")
        self.assertEqual(out["nodes"][0]["content"]["text"], "print(1)\n")

    def test_glsl_follow_pixeldat_and_compile_result(self) -> None:
        pixel = _fake_dat(
            path="/project1/rd_sim_glsl",
            text="// pixel\n",
        )
        node = _fake_node(
            [],
            path="/project1/rd_sim",
            family="TOP",
            op_type="glslTOP",
            compile_result="Compiled Successfully\n",
            par=SimpleNamespace(pixeldat=_FakeShaderPar(pixel)),
        )
        out = tdmcp_bridge.build_inspect_node(node, want_nodes=False, want_content=True)
        content = out["content"]
        self.assertEqual(content["kind"], "shader")
        self.assertEqual(content["compileResult"], "Compiled Successfully\n")
        self.assertEqual(len(content["stages"]), 1)
        stage = content["stages"][0]
        self.assertEqual(stage["role"], "pixel")
        self.assertEqual(stage["path"], "/project1/rd_sim_glsl")
        self.assertEqual(stage["opType"], "textDAT")
        self.assertEqual(stage["text"], "// pixel\n")
        self.assertNotIn("error", stage)

    def test_glsl_null_par_omits_stage(self) -> None:
        node = _fake_node(
            [],
            path="/project1/g",
            family="TOP",
            op_type="glslTOP",
            compile_result="",
            par=SimpleNamespace(
                pixeldat=_FakeShaderPar(None),
                vertexdat=None,
            ),
        )
        out = tdmcp_bridge.build_inspect_node(node, want_nodes=False, want_content=True)
        self.assertEqual(out["content"]["stages"], [])

    def test_glsl_broken_follow_stage_error(self) -> None:
        bad = SimpleNamespace(
            path="/project1/missing_glsl",
            opType="textDAT",
            valid=False,
        )
        node = _fake_node(
            [],
            path="/project1/g",
            family="TOP",
            op_type="glslTOP",
            compile_result="err",
            par=SimpleNamespace(pixeldat=_FakeShaderPar(bad)),
        )
        out = tdmcp_bridge.build_inspect_node(node, want_nodes=False, want_content=True)
        stage = out["content"]["stages"][0]
        self.assertEqual(stage["path"], "/project1/missing_glsl")
        self.assertIn("error", stage)
        self.assertNotIn("text", stage)

    def test_glsl_mat_stage_par_names(self) -> None:
        pixel = _fake_dat(path="/project1/mat_pixel", text="void main(){}")
        node = _fake_node(
            [],
            path="/project1/mat",
            family="MAT",
            op_type="glslMAT",
            compile_result="ok",
            par=SimpleNamespace(pdat=_FakeShaderPar(pixel)),
        )
        out = tdmcp_bridge.build_inspect_node(node, want_nodes=False, want_content=True)
        self.assertEqual(out["content"]["stages"][0]["role"], "pixel")
        self.assertEqual(out["content"]["stages"][0]["path"], "/project1/mat_pixel")


# --- shader lint in content (docs/SHADER_LINT.md §5) -------------------------

_FAILURE_RESULT = (
    "Pixel Shader Compile Results:\n\n"
    "ERROR: /project1/code:5: '' : syntax error, unexpected RIGHT_BRACE\n"
    "ERROR: 1 compilation errors.  No code generated.\n"
)


class _LintTdStub:
    """Fake TD world for lint discovery: resolve + find_children registry."""

    def __init__(self) -> None:
        self.root = SimpleNamespace(path="/project1")
        self.by_type: dict[str, list[Any]] = {}
        self.resolve_raises: Exception | None = None

    def register(self, type_name: str, *ops: Any) -> None:
        self.by_type.setdefault(type_name, []).extend(ops)

    def resolve(self, path: str) -> Any | None:
        if self.resolve_raises is not None:
            raise self.resolve_raises
        return self.root if path == "/project1" else None

    def find_children(self, root: Any, type_name: str) -> list[Any]:
        return list(self.by_type.get(type_name, []))


def _lint_glsl(
    path: str,
    dat: Any,
    compile_result: str = "",
    *,
    op_type: str = "glslTOP",
    stage_par: str = "pixeldat",
) -> SimpleNamespace:
    return SimpleNamespace(
        path=path,
        opType=op_type,
        compileResult=compile_result,
        par=SimpleNamespace(**{stage_par: _FakeShaderPar(dat)}),
    )


class InspectShaderLintContentTest(unittest.TestCase):
    def test_dat_content_gains_consumers(self) -> None:
        dat = _fake_dat(path="/project1/code", text="body")
        td_stub = _LintTdStub()
        td_stub.register(
            "glslTOP", _lint_glsl("/project1/g", dat, _FAILURE_RESULT)
        )
        out = tdmcp_bridge.build_inspect_node(
            dat, want_nodes=False, want_content=True, lint_ctx=td_stub,
            scope_root="/project1",
        )
        consumers = out["content"]["consumers"]
        self.assertEqual(len(consumers), 1)
        self.assertEqual(consumers[0]["consumer"], "/project1/g")
        self.assertEqual(consumers[0]["severity"], "error")
        self.assertEqual(consumers[0]["role"], "pixel")

    def test_dat_content_truncation_passes_through(self) -> None:
        dat = _fake_dat(path="/project1/code")
        td_stub = _LintTdStub()
        limit = tdmcp_bridge.SHADER_CONSUMER_LIMIT
        td_stub.register(
            "glslTOP",
            *[_lint_glsl(f"/project1/g{i}", dat) for i in range(limit + 4)],
        )
        out = tdmcp_bridge.build_inspect_node(
            dat, want_nodes=False, want_content=True, lint_ctx=td_stub,
            scope_root="/project1",
        )
        content = out["content"]
        self.assertEqual(len(content["consumers"]), limit)
        self.assertTrue(content["consumersTruncated"])
        self.assertEqual(
            content["truncation"]["code"], "tdmcp.shader.consumers_truncated"
        )

    def test_dat_content_lint_failure_omits_consumers(self) -> None:
        dat = _fake_dat(path="/project1/code")
        td_stub = _LintTdStub()
        td_stub.resolve_raises = RuntimeError("scan boom")
        out = tdmcp_bridge.build_inspect_node(
            dat, want_nodes=False, want_content=True, lint_ctx=td_stub,
            scope_root="/project1",
        )
        self.assertNotIn("consumers", out["content"])

    def test_no_lint_ctx_keeps_legacy_shape(self) -> None:
        dat = _fake_dat(path="/project1/code", text="x")
        out = tdmcp_bridge.build_inspect_node(dat, want_nodes=False, want_content=True)
        self.assertNotIn("consumers", out["content"])

    def test_glsl_compile_state_compiled(self) -> None:
        node = _fake_node([], path="/project1/g", family="TOP", op_type="glslTOP",
                          compile_result="Compiled Successfully\n")
        out = tdmcp_bridge.build_inspect_node(node, want_nodes=False, want_content=True)
        self.assertEqual(out["content"]["compileState"], "compiled")

    def test_glsl_compile_state_error(self) -> None:
        node = _fake_node([], path="/project1/g", family="TOP", op_type="glslTOP",
                          compile_result=_FAILURE_RESULT)
        out = tdmcp_bridge.build_inspect_node(node, want_nodes=False, want_content=True)
        self.assertEqual(out["content"]["compileState"], "error")

    def test_glsl_pop_omits_compile_state(self) -> None:
        node = _fake_node([], path="/project1/p", family="POP", op_type="glslPOP",
                          compile_result="")
        out = tdmcp_bridge.build_inspect_node(node, want_nodes=False, want_content=True)
        self.assertNotIn("compileState", out["content"])


if __name__ == "__main__":
    unittest.main()
