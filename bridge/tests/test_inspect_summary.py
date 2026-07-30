"""Unit tests for inspect child roster shaping (no live TD required)."""

from __future__ import annotations

import os
import sys
import unittest
from types import SimpleNamespace

sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))

import tdmcp_bridge  # noqa: E402


def _fake_child(name: str, *, op_type: str = "nullTOP", family: str = "TOP") -> SimpleNamespace:
    return SimpleNamespace(
        name=name,
        path=f"/project1/{name}",
        family=family,
        opType=op_type,
    )


def _fake_node(children: list[SimpleNamespace]) -> SimpleNamespace:
    return SimpleNamespace(
        path="/project1",
        family="COMP",
        opType="baseCOMP",
        children=children,
        pars=lambda: [],
        errors=lambda: [],
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


if __name__ == "__main__":
    unittest.main()
