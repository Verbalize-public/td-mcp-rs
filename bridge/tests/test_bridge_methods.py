"""Cross-language parity: Python HANDLERS keys == Rust BridgeMethod wire strings.

The fixture `bridge/fixtures/bridge_methods.json` is the shared source of truth
for the wire set. Rust unit test in tdmcp-core asserts the same list; this test
asserts HANDLERS and BRIDGE_METHODS match it.
"""

from __future__ import annotations

import json
import os
import sys
import unittest

sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))

import tdmcp_bridge  # noqa: E402

FIXTURE = os.path.join(
    os.path.dirname(__file__), "..", "fixtures", "bridge_methods.json"
)


class BridgeMethodParityTest(unittest.TestCase):
    def test_handlers_match_fixture(self) -> None:
        with open(FIXTURE, encoding="utf-8") as f:
            expected = set(json.load(f))
        self.assertEqual(set(tdmcp_bridge.HANDLERS.keys()), expected)
        self.assertEqual(set(tdmcp_bridge.BRIDGE_METHODS), expected)


if __name__ == "__main__":
    unittest.main()
