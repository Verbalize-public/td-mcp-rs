"""Deterministic authoring checks, never execution of collected examples."""
import copy
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location("check_guidance", ROOT / "scripts/check_guidance.py")
assert SPEC is not None and SPEC.loader is not None
checker = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(checker)


class GuidanceExampleTests(unittest.TestCase):
    def test_all_examples_classified_and_requests_validate(self):
        result = checker.validate()
        self.assertGreaterEqual(result["requests"], 14)
        self.assertEqual(result["negativeRequests"], 2)

    def test_nested_markdown_fence_is_one_explanatory_fragment(self):
        text = '````markdown\nexample\n```python\nnot executable\n```\n````\n'
        self.assertEqual(list(checker.blocks(text)), [
            (0, "markdown", "example\n```python\nnot executable\n```")
        ])
        with self.assertRaisesRegex(ValueError, "unclosed"):
            list(checker.blocks('```json\n{}'))

    def test_external_schema_references_never_fetch(self):
        checker.require_local_refs({"$ref": "#/$defs/Local"})
        for ref in ("https://example.invalid/schema", "file:///private/schema", 12):
            with self.subTest(ref=ref), self.assertRaisesRegex(ValueError, "local references"):
                checker.require_local_refs({"allOf": [{"$ref": ref}]})

    def test_unreviewed_example_change_fails(self):
        actual = checker.inventory()
        key = next(iter(actual))
        altered = dict(actual)
        language, body = altered[key]
        altered[key] = (language, body + " changed")
        with patch.object(checker, "inventory", return_value=altered):
            with self.assertRaisesRegex(ValueError, "without classification review"):
                checker.validate()

    def test_duplicate_registry_entry_fails(self):
        data = json.loads(checker.REGISTRY.read_text())
        data["blocks"].append(copy.deepcopy(data["blocks"][0]))
        with tempfile.TemporaryDirectory() as temp:
            path = Path(temp) / "EXAMPLES.json"
            path.write_text(json.dumps(data))
            with patch.object(checker, "REGISTRY", path):
                with self.assertRaisesRegex(ValueError, "inventory drift"):
                    checker.validate()

    def test_decoder_fixture_must_equal_authored_request(self):
        data = json.loads(checker.REGISTRY.read_text())
        request = next(row for row in data["blocks"] if row["kind"] == "request")
        request["arguments"]["unexpected"] = True
        with tempfile.TemporaryDirectory() as temp:
            path = Path(temp) / "EXAMPLES.json"
            path.write_text(json.dumps(data))
            with patch.object(checker, "REGISTRY", path):
                with self.assertRaisesRegex(ValueError, "decoder fixture drift"):
                    checker.validate()
