"""Unit tests for api_help cards / index (no live TD required)."""

from __future__ import annotations

import os
import sys
import unittest
from types import SimpleNamespace
from typing import Any
from unittest.mock import patch

sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))

import tdmcp_bridge  # noqa: E402


def _fake_op_class(
    name: str,
    *,
    family: str = "TOP",
    doc: str = "A fake OP class.",
    members: list[str] | None = None,
    mro_names: list[str] | None = None,
) -> type:
    member_names = members or ["cook", "par", "pars", "path", "opType", "family"]
    bases = (object,)
    ns: dict[str, Any] = {
        "__doc__": doc,
        "opType": name,
        "family": family,
    }
    for m in member_names:
        if m not in ns:
            ns[m] = None
    cls = type(name, bases, ns)
    if mro_names is not None:
        # dir/mro still come from real type; card uses __mro__ names.
        pass
    return cls


class FakeTd:
    """Minimal td module stand-in for api_help."""

    def __init__(self) -> None:
        self.noiseTOP = _fake_op_class("noiseTOP", family="TOP", doc="Noise TOP class.")
        self.hsvadjustTOP = _fake_op_class("hsvadjustTOP", family="TOP")
        self.nullCHOP = _fake_op_class("nullCHOP", family="CHOP", doc="Null CHOP.")
        self.math = lambda: None  # non-type, non-op
        self._private = object()
        self.__doc__ = "Fake td module."

    def __dir__(self) -> list[str]:
        return [
            "noiseTOP",
            "hsvadjustTOP",
            "nullCHOP",
            "math",
            "_private",
            "TOP",  # bare family — not op-like
        ]


class TestApiHelp(unittest.TestCase):
    def test_class_card_summary(self) -> None:
        fake = FakeTd()
        with patch.dict(sys.modules, {"td": fake}):
            out = tdmcp_bridge.handle_api_help(
                {"queries": [{"kind": "class", "name": "noiseTOP"}], "detailLevel": "summary"}
            )
        self.assertTrue(out["ok"])
        card = out["results"][0]
        self.assertTrue(card["ok"])
        self.assertEqual(card["kind"], "class")
        self.assertEqual(card["name"], "noiseTOP")
        self.assertEqual(card["opType"], "noiseTOP")
        self.assertEqual(card["family"], "TOP")
        self.assertIn("Noise TOP", card["doc"])
        self.assertLessEqual(len(card["members"]), tdmcp_bridge.API_HELP_MEMBERS_SUMMARY)
        self.assertGreaterEqual(card["memberCount"], len(card["members"]))
        self.assertNotIn("wikiUrl", card)

    def test_class_card_detailed_has_wiki(self) -> None:
        fake = FakeTd()
        with patch.dict(sys.modules, {"td": fake}):
            out = tdmcp_bridge.handle_api_help(
                {
                    "queries": [{"kind": "class", "name": "noiseTOP"}],
                    "detailLevel": "detailed",
                }
            )
        card = out["results"][0]
        self.assertTrue(card["ok"])
        self.assertEqual(card["wikiUrl"], "https://docs.derivative.ca/NoiseTOP_Class")
        self.assertIn("mro", card)

    def test_class_not_found(self) -> None:
        fake = FakeTd()
        with patch.dict(sys.modules, {"td": fake}):
            out = tdmcp_bridge.handle_api_help(
                {"queries": [{"kind": "class", "name": "hsvAdjustTOP"}]}
            )
        card = out["results"][0]
        self.assertFalse(card["ok"])
        self.assertEqual(card["code"], "tdmcp.api_help.not_found")

    def test_classes_family_prefix(self) -> None:
        fake = FakeTd()
        with patch.dict(sys.modules, {"td": fake}):
            out = tdmcp_bridge.handle_api_help(
                {
                    "queries": [
                        {"kind": "classes", "family": "TOP", "prefix": "noise"}
                    ]
                }
            )
        idx = out["results"][0]
        self.assertTrue(idx["ok"])
        self.assertEqual(idx["names"], ["noiseTOP"])
        self.assertEqual(idx["count"], 1)
        self.assertEqual(idx["family"], "TOP")
        self.assertEqual(idx["prefix"], "noise")

    def test_classes_excludes_non_op(self) -> None:
        fake = FakeTd()
        with patch.dict(sys.modules, {"td": fake}):
            out = tdmcp_bridge.handle_api_help({"queries": [{"kind": "classes"}]})
        names = out["results"][0]["names"]
        self.assertIn("noiseTOP", names)
        self.assertIn("nullCHOP", names)
        self.assertNotIn("math", names)
        self.assertNotIn("TOP", names)

    def test_module_td(self) -> None:
        fake = FakeTd()
        with patch.dict(sys.modules, {"td": fake}):
            out = tdmcp_bridge.handle_api_help(
                {"queries": [{"kind": "module", "name": "td"}]}
            )
        mod = out["results"][0]
        self.assertTrue(mod["ok"])
        self.assertEqual(mod["name"], "td")
        self.assertGreater(mod["publicCount"], 0)
        self.assertIn("sample", mod)

    def test_module_other_not_found(self) -> None:
        fake = FakeTd()
        with patch.dict(sys.modules, {"td": fake}):
            out = tdmcp_bridge.handle_api_help(
                {"queries": [{"kind": "module", "name": "numpy"}]}
            )
        self.assertFalse(out["results"][0]["ok"])

    def test_queries_required(self) -> None:
        fake = FakeTd()
        with patch.dict(sys.modules, {"td": fake}):
            out = tdmcp_bridge.handle_api_help({"queries": []})
        self.assertFalse(out["ok"])
        self.assertEqual(out["code"], "tdmcp.api_help.queries_required")

    def test_queries_truncation(self) -> None:
        fake = FakeTd()
        limit = tdmcp_bridge.API_HELP_QUERIES_LIMIT
        qs = [{"kind": "class", "name": "noiseTOP"} for _ in range(limit + 3)]
        with patch.dict(sys.modules, {"td": fake}):
            out = tdmcp_bridge.handle_api_help({"queries": qs})
        self.assertTrue(out["ok"])
        self.assertTrue(out["queriesTruncated"])
        self.assertEqual(len(out["results"]), limit)

    def test_partial_success_batch(self) -> None:
        fake = FakeTd()
        with patch.dict(sys.modules, {"td": fake}):
            out = tdmcp_bridge.handle_api_help(
                {
                    "queries": [
                        {"kind": "class", "name": "noiseTOP"},
                        {"kind": "class", "name": "missingTOP"},
                    ]
                }
            )
        self.assertTrue(out["ok"])
        self.assertTrue(out["results"][0]["ok"])
        self.assertFalse(out["results"][1]["ok"])

    def test_no_create_and_no_help(self) -> None:
        """Handler must not call help() or create on td types."""
        fake = FakeTd()
        calls: list[str] = []

        def boom_help(*_a: Any, **_k: Any) -> None:
            calls.append("help")
            raise AssertionError("help() must not be called")

        class TrackingTd(FakeTd):
            def create(self, *_a: Any, **_k: Any) -> None:  # noqa: ANN401
                calls.append("create")
                raise AssertionError("create must not be called")

        tracking = TrackingTd()
        with patch.dict(sys.modules, {"td": tracking}):
            with patch("builtins.help", side_effect=boom_help):
                out = tdmcp_bridge.handle_api_help(
                    {
                        "queries": [
                            {"kind": "class", "name": "noiseTOP"},
                            {"kind": "classes", "family": "TOP"},
                            {"kind": "module", "name": "td"},
                        ],
                        "detailLevel": "detailed",
                    }
                )
        self.assertTrue(out["ok"])
        self.assertEqual(calls, [])


if __name__ == "__main__":
    unittest.main()
