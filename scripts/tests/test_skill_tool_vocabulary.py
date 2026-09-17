"""Catch tool enum drift that identical template/render bytes cannot detect."""
import json
from pathlib import Path
import re
import unittest

ROOT = Path(__file__).resolve().parents[2]


class SkillToolVocabularyTests(unittest.TestCase):
    def test_look_grade_capture_modes_match_derived_schema(self):
        schema = json.loads((ROOT / "crates/tdmcp-mcp/tests/fixtures/schemas/capture.json").read_text())
        allowed = {variant["const"] for variant in schema["$defs"]["CaptureMode"]["oneOf"]}
        for path in ("skills/templates/touchdesigner/reference/look-grade.jinja.md",
                     "claude-skills/touchdesigner/reference/look-grade.md"):
            with self.subTest(path=path):
                text = (ROOT / path).read_text()
                paragraph = re.search(r"Modes \(tool is self-describing\): (.*?)(?:\n\n|$)", text, re.S)
                self.assertIsNotNone(paragraph, "keep the mode list discoverable")
                actual = set(re.findall(r"`([a-z_]+)`", paragraph.group(1)))
                self.assertEqual(actual, allowed)
