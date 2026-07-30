"""Unit tests for handshake identity helpers (no live TD required)."""

from __future__ import annotations

import os
import sys
import unittest

sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))

import tdmcp_bridge  # noqa: E402


class ComposeToePathTest(unittest.TestCase):
    def test_joins_when_both_present(self) -> None:
        got = tdmcp_bridge.compose_toe_path("C:\\proj", "demo.toe")
        self.assertEqual(got, os.path.join("C:\\proj", "demo.toe"))

    def test_none_when_folder_missing(self) -> None:
        self.assertIsNone(tdmcp_bridge.compose_toe_path("", "demo.toe"))
        self.assertIsNone(tdmcp_bridge.compose_toe_path(None, "demo.toe"))

    def test_none_when_name_missing(self) -> None:
        self.assertIsNone(tdmcp_bridge.compose_toe_path("C:\\proj", ""))
        self.assertIsNone(tdmcp_bridge.compose_toe_path("C:\\proj", None))

    def test_strips_whitespace(self) -> None:
        got = tdmcp_bridge.compose_toe_path("  /tmp/p  ", "  a.toe  ")
        self.assertEqual(got, os.path.join("/tmp/p", "a.toe"))


class IdentityFromProjectTest(unittest.TestCase):
    def test_title_and_toe(self) -> None:
        title, toe = tdmcp_bridge.identity_from_project("foo.toe", "/data")
        self.assertEqual(title, "foo.toe")
        self.assertEqual(toe, os.path.join("/data", "foo.toe"))

    def test_null_outside_project(self) -> None:
        title, toe = tdmcp_bridge.identity_from_project("", "")
        self.assertIsNone(title)
        self.assertIsNone(toe)


class IdentitySnapshotTest(unittest.TestCase):
    def test_snapshot_without_td_has_image_fallback(self) -> None:
        snap = tdmcp_bridge._identity_snapshot()
        self.assertIsNone(snap["title"])
        self.assertIsNone(snap["toe_path"])
        self.assertIsInstance(snap["image"], str)
        self.assertTrue(snap["image"])
        # start_time is best-effort; may be str or None depending on OS.
        self.assertTrue(snap["start_time"] is None or isinstance(snap["start_time"], str))


if __name__ == "__main__":
    unittest.main()
