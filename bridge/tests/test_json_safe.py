"""Unit tests for JSON-safe result coercion (no live TD)."""

from __future__ import annotations

import json
import os
import sys
import unittest

sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))

from tdmcp_bridge.json_safe import json_safe, json_utf8_size  # noqa: E402
import tdmcp_bridge  # noqa: E402


class _FakeOp:
    path = "/project1/foo"
    opType = "nullTOP"
    family = "TOP"


class JsonSafeTest(unittest.TestCase):
    def test_primitives_pass_through(self) -> None:
        self.assertEqual(json_safe(None), None)
        self.assertEqual(json_safe(True), True)
        self.assertEqual(json_safe(3), 3)
        self.assertEqual(json_safe(1.5), 1.5)
        self.assertEqual(json_safe("hi"), "hi")

    def test_callable_wrapped_not_invoked(self) -> None:
        called = {"n": 0}

        def boom() -> None:
            called["n"] += 1
            raise RuntimeError("should not run")

        out = json_safe({"f": boom})
        self.assertEqual(out["f"]["__td"], "callable")
        self.assertEqual(out["f"]["name"], "boom")
        self.assertEqual(called["n"], 0)
        json.dumps(out)  # must not raise

    def test_builtin_abs_wrapped(self) -> None:
        out = json_safe({"f": abs})
        self.assertEqual(out["f"]["__td"], "callable")
        self.assertEqual(out["f"]["name"], "abs")
        json.dumps(out)

    def test_op_like_card(self) -> None:
        out = json_safe(_FakeOp())
        self.assertEqual(out["__td"], "op")
        self.assertEqual(out["path"], "/project1/foo")
        self.assertEqual(out["opType"], "nullTOP")
        json.dumps(out)

    def test_nested_list_dict(self) -> None:
        out = json_safe({"a": [1, {"b": abs}]})
        self.assertEqual(out["a"][1]["b"]["__td"], "callable")
        self.assertEqual(json_utf8_size(out), len(json.dumps(out, separators=(",", ":")).encode()))

    def test_execute_python_wraps_abs_in_result(self) -> None:
        out = tdmcp_bridge.handle_execute_python(
            {"script": "result = {'f': abs}", "includeLogs": False}
        )
        self.assertTrue(out["ok"])
        self.assertEqual(out["result"]["f"]["__td"], "callable")
        self.assertEqual(out["result"]["f"]["name"], "abs")


if __name__ == "__main__":
    unittest.main()
