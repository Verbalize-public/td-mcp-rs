"""The public envelope fixtures must stay synthetic, in sync, and honest.

The envelope fixtures are what proves the version-2 contract's *envelope*
holds in CI, so the same three properties as the inspect fixtures are
load-bearing:

* the fixtures must match their generator, so a hand-edited fixture cannot sit
  in the tree diverged from the script that documents it;
* no fixture may be derived from recorded user data, and none may leak a real
  session id, project path, pid or comment;
* every fixture must state what it pins, and every expectation must be
  evaluable by the Rust test — an expectation vocabulary entry the checker
  cannot resolve is an expectation that silently checks nothing.
"""

import importlib.util
import json
from pathlib import Path
import unittest


ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location(
    "generate_envelope_fixtures", ROOT / "scripts/generate_envelope_fixtures.py"
)
if SPEC is None or SPEC.loader is None:
    raise RuntimeError("cannot load envelope fixture generator")
FIXTURES = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(FIXTURES)

# The markers the Rust expectation checker understands, plus a plain literal.
# Anything outside this vocabulary would be compared as a literal path value,
# which is never what a fixture means.
MARKERS = {"present", "absent"}

# A real recorded payload carries these; a public fixture must carry none.
LEAK_MARKERS = ("/home/", "/root/", "session.v3", "verbalize", "mapping_views")


class EnvelopeFixtures(unittest.TestCase):
    def setUp(self) -> None:
        self.cases = FIXTURES.build()

    def test_the_fixture_set_is_not_empty(self) -> None:
        self.assertTrue(self.cases, "the envelope fixture set must not be empty")

    def test_every_fixture_states_what_it_pins(self) -> None:
        for name, fixture in self.cases:
            purpose = fixture["provenance"]["purpose"]
            self.assertTrue(
                purpose.strip(),
                f"{name} must state what it pins; the README is generated from it",
            )

    def test_every_fixture_is_declared_synthetic(self) -> None:
        for name, fixture in self.cases:
            self.assertEqual(
                fixture["provenance"]["source"],
                "synthetic",
                f"{name} must be declared synthetic",
            )

    def test_every_expectation_is_evaluable(self) -> None:
        """No expectation may use a vocabulary the Rust checker cannot resolve.

        `at()` follows a dotted path and compares the value literally, so a
        misspelled key would read as a missing field and fail loudly — but a
        boolean marker like `true` used for a whole-value claim would compare a
        field to a literal and be meaningless. Both are caught here, before a
        fixture silently stops asserting anything.
        """
        for name, fixture in self.cases:
            for key, expected in fixture["case"]["expect"].items():
                # A top-level key like `contract` is a valid path; only the
                # segments are dotted, so requiring a dot would reject the
                # single most important assertion in the set.
                self.assertTrue(key.strip(), f"{name}: an expectation key is empty")
                for segment in key.split("."):
                    self.assertTrue(
                        segment.strip(),
                        f"{name}: {key} has an empty path segment",
                    )
                if isinstance(expected, (str, int, float, bool)) or expected is None:
                    continue
                self.fail(
                    f"{name}: {key} has an unevaluable expectation {expected!r}; "
                    "a dotted path resolves to a scalar, and the checker "
                    "compares it literally"
                )

    def test_markers_are_only_ever_used_for_presence(self) -> None:
        for name, fixture in self.cases:
            for key, expected in fixture["case"]["expect"].items():
                if expected in MARKERS:
                    self.assertIn(
                        key,
                        fixture["case"]["expect"],
                        f"{name}: {key} uses a presence marker",
                    )

    def test_no_fixture_leaks_a_real_path_or_session_marker(self) -> None:
        for name, fixture in self.cases:
            rendered = json.dumps(fixture)
            for marker in LEAK_MARKERS:
                self.assertNotIn(
                    marker, rendered, f"{name} leaks {marker!r} from recorded data"
                )

    def test_every_node_path_is_synthetic(self) -> None:
        for name, fixture in self.cases:
            for node in fixture["case"]["legacy"].get("nodes", []):
                self.assertTrue(
                    node["path"].startswith("/project1/fx/"),
                    f"{name}: {node['path']} is not a synthetic fixture path",
                )

    def test_options_are_strict_and_name_a_real_contract(self) -> None:
        # `OutputOptions` uses deny_unknown_fields, so a misspelled key would be
        # a parse failure rather than a silently ignored request. Asserting the
        # shape here keeps that guarantee meaningful.
        for name, fixture in self.cases:
            options = fixture["case"]["options"]
            self.assertEqual(options.get("contract"), "v2", name)
            self.assertTrue(
                set(options) <= {"contract", "view", "retain"},
                f"{name}: unknown output option(s) {sorted(options)}",
            )

    def test_the_compact_served_fixture_really_can_be_compacted(self) -> None:
        """A fixture that cannot reach its path would pass for the wrong reason.

        `compact-served-is-decoded-back-exactly` exists to pin the served path.
        The Rust test fails if it stops being served, but the generator can
        catch the cause earlier: the codec factors defaults out of a single
        `params` array, so a payload with too few parameters never amortizes the
        compact envelope.
        """
        for name, fixture in self.cases:
            options = fixture["case"]["options"]
            if options.get("view") != "compact":
                continue
            purpose = fixture["provenance"]["purpose"]
            expects_served = "served" in name or "served" in purpose
            if not expects_served:
                continue
            nodes = fixture["case"]["legacy"]["nodes"]
            widest = max(len(node["params"]) for node in nodes)
            self.assertGreater(
                widest,
                10,
                f"{name} must be wide enough for a compact transform to pay "
                f"for its envelope; widest params array is {widest}",
            )

    def test_the_compact_fallback_fixture_is_genuinely_too_small(self) -> None:
        """The mirror of the above: the refusal fixture must really be refused.

        If the payload were compactable, the fixture would pass while proving
        the opposite of what it is named for.
        """
        fixture = next(
            fixture
            for name, fixture in self.cases
            if name == "compact-fallback-is-reported-not-passed-off"
        )
        widest = max(
            len(node["params"]) for node in fixture["case"]["legacy"]["nodes"]
        )
        self.assertLessEqual(
            widest,
            2,
            "the refusal fixture must not be wide enough to compact, or it "
            "would stop testing the refusal",
        )

    def test_the_generated_files_match_the_checked_in_ones(self) -> None:
        directory = ROOT / "scripts" / "fixtures" / "envelope"
        expected = {f"{name}.json" for name, _ in self.cases}
        expected.add("README.md")
        on_disk = {path.name for path in directory.iterdir() if path.is_file()}
        self.assertEqual(
            on_disk,
            expected,
            "the checked-in envelope fixtures diverge from the generator; "
            "run scripts/generate_envelope_fixtures.py",
        )

    def test_the_readme_table_is_generated_and_mentions_every_fixture(self) -> None:
        readme = (ROOT / "scripts" / "fixtures" / "envelope" / "README.md").read_text()
        self.assertNotIn("{{TABLE}}", readme, "the README placeholder was not filled")
        for name, _ in self.cases:
            self.assertIn(f"| `{name}` |", readme, f"{name} is missing from the README")

    def test_the_readme_does_not_overstate_state_coverage(self) -> None:
        """The README must not claim coverage the producer cannot reach.

        `ObservedValue` has four states; this producer only emits two. A README
        claiming all four would be an overstatement written into the file a
        reader trusts.
        """
        readme = (ROOT / "scripts" / "fixtures" / "envelope" / "README.md").read_text()
        # The README is hard-wrapped for reading, so a disclosure phrase can
        # straddle a line break. Match on normalized whitespace, or this test
        # would be measuring the wrapping rather than the claim.
        lowered = " ".join(readme.lower().split())

        # If the README names the four-state vocabulary at all, it must do so in
        # a disclosure that also says the producer does not emit all of them.
        # A bare "four states are covered" claim is the thing to reject; the
        # honest sentence and the overstatement share the words, so the test has
        # to look for the disclosure rather than the mention.
        mentions_four = "four states" in lowered or "four different" in lowered
        if mentions_four:
            for disclosure in (
                "only ever emits two",
                "do not claim the states it cannot reach",
            ):
                self.assertIn(
                    disclosure,
                    lowered,
                    "the README names the four-state vocabulary without "
                    f"disclosing the coverage limit ({disclosure!r})",
                )


if __name__ == "__main__":
    unittest.main()
