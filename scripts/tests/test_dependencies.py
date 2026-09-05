"""The weekly check must cover each shipped graph and propagate failures."""

import importlib.util
import re
from pathlib import Path
import subprocess
import unittest


ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location("check_dependencies", ROOT / "scripts/check_dependencies.py")
CHECKS = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(CHECKS)


class DependencyCheckTest(unittest.TestCase):
    def test_each_release_target_is_checked_even_after_a_failure(self):
        calls = []

        def run(command, **kwargs):
            calls.append(command)
            self.assertEqual(kwargs["cwd"], ROOT)
            self.assertIn("--locked", command)
            return subprocess.CompletedProcess(command, 1 if len(calls) == 1 else 0, "", "")

        self.assertEqual(CHECKS.check(run), 1)
        self.assertEqual([c[c.index("--target") + 1] for c in calls], list(CHECKS.TARGETS))
        workflow = (ROOT / ".github/workflows/release.yml").read_text()
        self.assertEqual(set(CHECKS.TARGETS), set(re.findall(r"- target: ([\w-]+)", workflow)))

    def test_success_requires_every_target_to_pass(self):
        self.assertEqual(CHECKS.check(lambda command, **_: subprocess.CompletedProcess(command, 0, "", "")), 0)


if __name__ == "__main__":
    unittest.main()
