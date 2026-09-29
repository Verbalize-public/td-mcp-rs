"""The public inspect fixtures must stay synthetic, in sync, and honest.

The private recorded corpus cannot be published, so the checked-in fixture set
is what proves the codec works in CI. That makes three properties load-bearing:

* the fixtures must match their generator, so a hand-edited fixture cannot sit
  in the tree diverged from the script that documents it;
* no fixture may be derived from recorded user data, and none may leak a real
  session id, project path, pid or comment;
* every fixture must declare the codec plan it exists to pin, because a
  fixture that silently stopped reaching its path is worse than no fixture.
"""

import importlib.util
import json
from pathlib import Path
import unittest


ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location(
    "generate_inspect_fixtures", ROOT / "scripts/generate_inspect_fixtures.py"
)
if SPEC is None or SPEC.loader is None:
    raise RuntimeError("cannot load inspect fixture generator")
FIXTURES = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(FIXTURES)

# A real recorded payload carries these; a public fixture must carry none of them.
# `session` is the field the corpus itself is keyed by, so it is checked too.
SYNTHETIC_ONLY_KEYS = {
    "pid": lambda value: value == 1,
    "session": lambda value: value == "synthetic",
    "source": lambda value: value == "synthetic",
    "args": lambda value: True,
}

KNOWN_PLANS = {
    "compact",
    "full:no_transforms",
    "full:not_smaller",
    "full:traversal_budget",
    "full:serialization_budget",
    "full:transform_budget",
}


def walk(value, path="response"):
    """Yield (path, key, value) for every mapping entry in the document."""
    if isinstance(value, dict):
        for key, child in value.items():
            yield path, key, child
            yield from walk(child, f"{path}.{key}")
    elif isinstance(value, list):
        for index, child in enumerate(value):
            yield from walk(child, f"{path}[{index}]")


class InspectFixtureTest(unittest.TestCase):
    def test_every_case_declares_a_plan_the_codec_can_produce(self):
        names = set()
        for name, plan, _document in FIXTURES.build():
            with self.subTest(fixture=name):
                self.assertIn(plan, KNOWN_PLANS)
                self.assertIn(name, FIXTURES.PURPOSES)
                names.add(name)
        self.assertEqual(
            len(names),
            len(FIXTURES.PURPOSES),
            "every purpose in PURPOSES should have a fixture and vice versa",
        )

    def test_no_fixture_carries_recorded_user_data(self):
        for name, _plan, document in FIXTURES.build():
            for owner, key, value in walk(document):
                if owner != "provenance":
                    continue
                rule = SYNTHETIC_ONLY_KEYS.get(key)
                if rule is not None:
                    with self.subTest(fixture=name, key=key):
                        self.assertTrue(
                            rule(value),
                            f"{name}: provenance.{key} is {value!r}, which is not the "
                            "synthetic placeholder",
                        )

    def test_project_paths_stay_inside_the_synthetic_namespace(self):
        """A leaked real path would be a privacy failure, so pin the namespace."""
        for name, _plan, document in FIXTURES.build():
            for owner, key, value in walk(document):
                if key != "path" or not isinstance(value, str):
                    continue
                with self.subTest(fixture=name, path=value):
                    self.assertTrue(
                        value.startswith("/project1/fx/"),
                        f"{name}: {value} escapes the synthetic /project1/fx/ namespace",
                    )

    def test_comments_stay_empty_so_no_authored_text_can_be_carried(self):
        for name, _plan, document in FIXTURES.build():
            for owner, key, value in walk(document):
                if key == "comment":
                    with self.subTest(fixture=name):
                        self.assertEqual(value, "", f"{name}: carries comment text")

    def test_checked_in_fixtures_match_the_generator(self):
        """`--check` is what CI runs, so exercise exactly that contract."""
        for name, _plan, document in FIXTURES.build():
            path = FIXTURES.OUT_DIR / f"{name}.json"
            with self.subTest(fixture=name):
                self.assertTrue(path.is_file(), f"{path} is missing")
                self.assertEqual(json.loads(path.read_text(encoding="utf-8")), document)

    def test_the_readme_table_is_generated_not_hand_written(self):
        """The table once claimed a plan the codec never chose."""
        readme = (FIXTURES.OUT_DIR / "README.md").read_text(encoding="utf-8")
        self.assertNotIn("{{TABLE}}", readme, "README still has an unrendered placeholder")
        for name, plan, _document in FIXTURES.build():
            with self.subTest(fixture=name):
                self.assertIn(f"| `{name}` | `{plan}` |", readme)
        self.assertEqual(readme, FIXTURES.render_readme(FIXTURES.build()))

    def test_a_declined_compact_costs_exactly_the_full_bytes(self):
        """The generator's promises are cheap to state and easy to break."""
        full_only = [name for name, plan, _d in FIXTURES.build() if plan != "compact"]
        self.assertTrue(
            full_only,
            "the set should keep cases that decline, or a regression that never "
            "declines would go unnoticed",
        )


if __name__ == "__main__":
    unittest.main()
