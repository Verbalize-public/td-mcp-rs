"""Unit tests for execute_python structured exception reports."""

from __future__ import annotations

import unittest

import tdmcp_bridge as bridge


class ExceptionReportTests(unittest.TestCase):
    def test_nested_raise_frames_and_type(self) -> None:
        script = (
            "def a():\n"
            "    return b()\n"
            "def b():\n"
            "    return c()\n"
            "def c():\n"
            "    raise RuntimeError('deep boom')\n"
            "result = a()\n"
        )
        out = bridge.handle_execute_python(
            {"script": script, "includeLogs": False, "formatMode": "normal"}
        )
        self.assertFalse(out["ok"])
        self.assertEqual(out["error"], "deep boom")
        self.assertIn("RuntimeError", out.get("traceback") or "")
        exc = out["exception"]
        self.assertEqual(exc["type"], "RuntimeError")
        self.assertEqual(exc["message"], "deep boom")
        self.assertTrue(exc["raw"])
        self.assertIsNone(exc["syntax"])
        self.assertTrue(all(f["filename"] == "<string>" for f in exc["frames"]))
        names = [f["name"] for f in exc["frames"]]
        self.assertEqual(names, ["<module>", "a", "b", "c"])
        self.assertTrue(all("locals" not in f for f in exc["frames"]))
        self.assertNotIn("execute.py", exc["raw"])
        self.assertNotIn("handle_execute_python", exc["raw"])
        self.assertNotIn("execute.py", out.get("traceback") or "")

    def test_syntax_error_block(self) -> None:
        out = bridge.handle_execute_python(
            {"script": "def broken(\n  pass", "includeLogs": False}
        )
        self.assertFalse(out["ok"])
        exc = out["exception"]
        self.assertEqual(exc["type"], "SyntaxError")
        self.assertIsNotNone(exc["syntax"])
        self.assertEqual(exc["syntax"]["lineno"], 1)
        self.assertTrue(exc["syntax"]["msg"])

    def test_debug_locals_only_on_string_frames(self) -> None:
        script = "x = 42\nraise ValueError('with locals')\n"
        normal = bridge.handle_execute_python(
            {"script": script, "includeLogs": False, "formatMode": "normal"}
        )
        debug = bridge.handle_execute_python(
            {"script": script, "includeLogs": False, "formatMode": "debug"}
        )
        self.assertFalse(normal["ok"])
        self.assertFalse(debug["ok"])
        self.assertTrue(
            all("locals" not in f for f in normal["exception"]["frames"])
        )
        string_frames = [
            f
            for f in debug["exception"]["frames"]
            if f["filename"] == "<string>"
        ]
        self.assertTrue(string_frames)
        self.assertIn("locals", string_frames[-1])
        locs = string_frames[-1]["locals"]
        self.assertIn("x", locs)
        self.assertEqual(locs["x"]["type"], "int")
        self.assertIn("42", locs["x"]["repr"])

    def test_string_frame_line_filled_from_script(self) -> None:
        script = "a = 1\nraise RuntimeError('line check')\n"
        out = bridge.handle_execute_python(
            {"script": script, "includeLogs": False}
        )
        string_frames = [
            f
            for f in out["exception"]["frames"]
            if f["filename"] == "<string>"
        ]
        self.assertTrue(string_frames)
        last = string_frames[-1]
        self.assertEqual(last["lineno"], 2)
        self.assertIsNotNone(last["line"])
        self.assertIn("RuntimeError", last["line"])

    def test_wrapper_frames_stripped_from_report(self) -> None:
        out = bridge.handle_execute_python(
            {"script": "raise ValueError('trim me')", "includeLogs": False}
        )
        self.assertFalse(out["ok"])
        exc = out["exception"]
        self.assertTrue(all(f["filename"] == "<string>" for f in exc["frames"]))
        for blob in (exc["raw"], out.get("traceback") or ""):
            self.assertNotIn("execute.py", blob)
            self.assertNotIn("handle_execute_python", blob)
            self.assertNotIn("tdmcp_bridge", blob)


if __name__ == "__main__":
    unittest.main()
