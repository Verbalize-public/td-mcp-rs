"""Unit tests for capture pop_data shaping (no live TD)."""

from __future__ import annotations

import os
import sys
import unittest
from types import SimpleNamespace
from typing import Any
from unittest import mock

sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))

import tdmcp_bridge  # noqa: E402


class FakeAttr:
    def __init__(
        self,
        name: str,
        vals: list[Any],
        *,
        size: int = 3,
        typ: type | None = float,
    ):
        self.name = name
        self.size = size
        self.type = typ
        self._vals = vals

    def vals(self) -> list[Any]:
        return list(self._vals)


class FakePointAttrs:
    def __init__(self, attrs: list[FakeAttr]):
        self._by_name = {a.name: a for a in attrs}
        self._list = attrs

    def __iter__(self):
        return iter(self._list)

    def __getitem__(self, key: str) -> FakeAttr:
        return self._by_name[key]


def _fake_pop(
    *,
    num_points: int,
    num_prims: int = 0,
    attrs: dict[str, list[Any]] | None = None,
    family: str = "POP",
    path: str = "/project1/zone/sphere1",
    components: int = 3,
) -> SimpleNamespace:
    if attrs is None:
        attrs = {
            "P": [[float(i), 0.0, 0.0] for i in range(num_points)],
        }
    fake_attrs = [
        FakeAttr(name, vals, size=components)
        for name, vals in attrs.items()
    ]
    return SimpleNamespace(
        family=family,
        path=path,
        name="sphere1",
        numPoints=num_points,
        numPrims=num_prims,
        pointAttributes=FakePointAttrs(fake_attrs),
        cook=lambda force=False: None,
        valid=True,
    )


class CapturePopDataTests(unittest.TestCase):
    def test_happy_path_default_p(self) -> None:
        node = _fake_pop(num_points=4, num_prims=2)
        out = tdmcp_bridge._capture_pop_data(node, node.path)
        self.assertTrue(out["ok"])
        self.assertEqual(out["mode"], "pop_data")
        self.assertEqual(out["family"], "POP")
        self.assertEqual(out["numPoints"], 4)
        self.assertEqual(out["numPrims"], 2)
        self.assertEqual(out["numPointsReturned"], 4)
        self.assertEqual(out["attributes"][0]["name"], "P")
        self.assertEqual(len(out["data"]["P"]), 4)
        self.assertEqual(out["data"]["P"][1], [1.0, 0.0, 0.0])
        self.assertNotIn("truncation", out)
        self.assertNotIn("imageBase64", out)

    def test_attrs_filter(self) -> None:
        node = _fake_pop(
            num_points=2,
            attrs={
                "P": [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0]],
                "N": [[0.0, 1.0, 0.0], [0.0, 1.0, 0.0]],
            },
        )
        out = tdmcp_bridge._capture_pop_data(node, node.path, attrs=["N"])
        self.assertTrue(out["ok"])
        self.assertEqual(list(out["data"].keys()), ["N"])
        self.assertEqual(out["attributes"][0]["name"], "N")

    def test_empty_pop(self) -> None:
        node = _fake_pop(num_points=0)
        out = tdmcp_bridge._capture_pop_data(node, node.path)
        self.assertFalse(out["ok"])
        self.assertEqual(out["code"], "tdmcp.perception.empty_pop")

    def test_wrong_family(self) -> None:
        node = _fake_pop(num_points=1, family="TOP")
        out = tdmcp_bridge._capture_pop_data(node, node.path)
        self.assertFalse(out["ok"])
        self.assertEqual(out["code"], "tdmcp.perception.wrong_family")

    def test_method_num_points(self) -> None:
        node = _fake_pop(num_points=3)
        node.numPoints = lambda: 3  # type: ignore[method-assign]
        out = tdmcp_bridge._capture_pop_data(node, node.path)
        self.assertTrue(out["ok"])
        self.assertEqual(out["numPoints"], 3)
        self.assertEqual(out["numPointsReturned"], 3)

    def test_truncate_points(self) -> None:
        n = tdmcp_bridge.POP_DATA_MAX_POINTS + 10
        node = _fake_pop(num_points=n)
        out = tdmcp_bridge._capture_pop_data(node, node.path)
        self.assertTrue(out["ok"])
        self.assertEqual(out["numPoints"], n)
        self.assertEqual(
            out["numPointsReturned"], tdmcp_bridge.POP_DATA_MAX_POINTS
        )
        self.assertEqual(out["truncation"]["field"], "points")
        self.assertEqual(
            out["truncation"]["code"], "tdmcp.perception.pop_truncated"
        )

    def test_truncate_attrs(self) -> None:
        attrs = {
            f"a{i}": [[0.0, 0.0, 0.0] for _ in range(2)]
            for i in range(tdmcp_bridge.POP_DATA_MAX_ATTRS + 3)
        }
        # Ensure default P is absent so selection follows requested order
        node = _fake_pop(num_points=2, attrs=attrs)
        names = list(attrs.keys())
        out = tdmcp_bridge._capture_pop_data(node, node.path, attrs=names)
        self.assertTrue(out["ok"])
        self.assertEqual(len(out["attributes"]), tdmcp_bridge.POP_DATA_MAX_ATTRS)
        self.assertEqual(out["truncation"]["field"], "attributes")
        self.assertEqual(
            out["truncation"]["code"], "tdmcp.perception.pop_truncated"
        )

    def test_handle_pop_data_mode(self) -> None:
        node = _fake_pop(num_points=2)
        with mock.patch.object(tdmcp_bridge, "tdmcp_resolve", return_value=node):
            out = tdmcp_bridge.handle_capture(
                {"path": node.path, "mode": "pop_data"}
            )
        self.assertTrue(out["ok"])
        self.assertEqual(out["mode"], "pop_data")


if __name__ == "__main__":
    unittest.main()
