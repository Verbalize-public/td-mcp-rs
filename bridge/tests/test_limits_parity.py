"""Cross-language parity: Python bridge limits == bridge/fixtures/limits.json."""

from __future__ import annotations

import json
import os
import sys
import unittest

sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))

import tdmcp_bridge  # noqa: E402

FIXTURE = os.path.join(os.path.dirname(__file__), "..", "fixtures", "limits.json")
MANIFEST = os.path.join(os.path.dirname(__file__), "..", "manifest.json")


class LimitsParityTest(unittest.TestCase):
    def test_python_constants_match_fixture(self) -> None:
        with open(FIXTURE, encoding="utf-8") as f:
            expected = json.load(f)
        self.assertEqual(tdmcp_bridge.INSPECT_PATHS_LIMIT, expected["inspect_paths_limit"])
        self.assertEqual(tdmcp_bridge.CHILDREN_ROSTER_LIMIT, expected["children_roster_limit"])
        self.assertEqual(
            tdmcp_bridge.EDITOR_SELECTION_LIMIT, expected["editor_selection_limit"]
        )
        self.assertEqual(tdmcp_bridge.EDITOR_PANES_LIMIT, expected["editor_panes_limit"])
        self.assertEqual(tdmcp_bridge.DEFAULT_MAX_CALL_WAIT_S, float(expected["bridge_timeout_secs"]))
        self.assertEqual(tdmcp_bridge.HEARTBEAT_INTERVAL_S, float(expected["heartbeat_interval_secs"]))
        self.assertEqual(tdmcp_bridge.PONG_TIMEOUT_S, float(expected["pong_timeout_secs"]))
        self.assertEqual(tdmcp_bridge.IDLE_DEAD_S, float(expected["idle_dead_secs"]))

    def test_manifest_matches_package_version_constants(self) -> None:
        with open(MANIFEST, encoding="utf-8") as f:
            manifest = json.load(f)
        self.assertEqual(manifest["protocolVersion"], tdmcp_bridge.__protocol_version__)
        self.assertEqual(manifest["minDaemon"], tdmcp_bridge.__min_daemon__)


if __name__ == "__main__":
    unittest.main()
