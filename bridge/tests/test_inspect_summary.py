"""Unit tests for inspect child roster shaping (no live TD required)."""

from __future__ import annotations

import os
import sys
import unittest
from types import SimpleNamespace
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


def _fake_node(
    children: list[SimpleNamespace],
    *,
    errors: object = "",
    warnings: object = "",
    cook: object | None = None,
) -> SimpleNamespace:
    return SimpleNamespace(
        path="/project1",
        family="COMP",
        opType="baseCOMP",
        children=children,
        pars=lambda: [],
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

    def test_handle_inspect_empty_include_defaults(self) -> None:
        node = _fake_node([], errors="err1", warnings="warn1")
        with patch.dict(sys.modules, {"td": SimpleNamespace()}):
            with patch.object(tdmcp_bridge, "tdmcp_resolve", return_value=node):
                result = tdmcp_bridge.handle_inspect({
                    "path": "/project1",
                    "include": [],
                })
        self.assertTrue(result["ok"])
        self.assertEqual(result["node"]["errors"], ["err1"])
        self.assertEqual(result["node"]["warnings"], ["warn1"])
        self.assertIn("children", result["node"])
        self.assertNotIn("params", result["node"])

    def test_handle_inspect_allowlist_nodes_only(self) -> None:
        node = _fake_node([], errors="err1", warnings="warn1")
        with patch.dict(sys.modules, {"td": SimpleNamespace()}):
            with patch.object(tdmcp_bridge, "tdmcp_resolve", return_value=node):
                result = tdmcp_bridge.handle_inspect({
                    "path": "/project1",
                    "include": ["nodes"],
                })
        self.assertTrue(result["ok"])
        self.assertNotIn("errors", result["node"])
        self.assertNotIn("warnings", result["node"])
        self.assertIn("children", result["node"])

    def test_handle_inspect_allowlist_warnings_only(self) -> None:
        node = _fake_node([], errors="err1", warnings="warn1")
        with patch.dict(sys.modules, {"td": SimpleNamespace()}):
            with patch.object(tdmcp_bridge, "tdmcp_resolve", return_value=node):
                result = tdmcp_bridge.handle_inspect({
                    "path": "/project1",
                    "include": ["warnings"],
                })
        self.assertTrue(result["ok"])
        self.assertEqual(result["node"]["warnings"], ["warn1"])
        self.assertNotIn("errors", result["node"])
        self.assertEqual(result["node"]["children"], [])
        self.assertEqual(result["node"]["childCount"], 0)

    def test_handle_inspect_force_cooks_target(self) -> None:
        cook = MagicMock()
        node = _fake_node([], errors="err1", cook=cook)
        with patch.dict(sys.modules, {"td": SimpleNamespace()}):
            with patch.object(tdmcp_bridge, "tdmcp_resolve", return_value=node):
                result = tdmcp_bridge.handle_inspect({
                    "path": "/project1",
                    "include": [],
                })
        self.assertTrue(result["ok"])
        cook.assert_called_once_with(force=True)
        self.assertEqual(result["node"]["errors"], ["err1"])

    def test_handle_inspect_cook_raise_still_ok(self) -> None:
        cook = MagicMock(side_effect=RuntimeError("cook boom"))
        node = _fake_node([], errors="err1", warnings="warn1", cook=cook)
        with patch.dict(sys.modules, {"td": SimpleNamespace()}):
            with patch.object(tdmcp_bridge, "tdmcp_resolve", return_value=node):
                result = tdmcp_bridge.handle_inspect({
                    "path": "/project1",
                    "include": [],
                })
        self.assertTrue(result["ok"])
        cook.assert_called_once_with(force=True)
        self.assertEqual(result["node"]["errors"], ["err1"])
        self.assertEqual(result["node"]["warnings"], ["warn1"])

    def test_force_cook_positional_fallback(self) -> None:
        calls: list[tuple] = []

        def cook(*args: object, **kwargs: object) -> None:
            if kwargs.get("force") is True:
                raise TypeError("no force kw")
            calls.append((args, kwargs))

        node = SimpleNamespace(cook=cook)
        tdmcp_bridge._force_cook(node)
        self.assertEqual(calls, [((True,), {})])

    def test_force_cook_missing_is_noop(self) -> None:
        tdmcp_bridge._force_cook(SimpleNamespace())  # no cook attr


if __name__ == "__main__":
    unittest.main()
