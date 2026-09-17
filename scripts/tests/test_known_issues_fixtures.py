"""Offline shape/syntax checks complement, not replace, the recorded TD runs."""
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[2] / "scripts/fixtures/known_issues"


class KnownIssuesFixtureTests(unittest.TestCase):
    def test_td_scripts_compile_without_importing_td(self):
        scripts = sorted(ROOT.glob("*.py"))
        self.assertTrue(scripts)
        for path in scripts:
            with self.subTest(path=path.name):
                compile(path.read_text(encoding="utf-8"), str(path), "exec")
