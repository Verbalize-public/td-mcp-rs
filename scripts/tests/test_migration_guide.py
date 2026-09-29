"""Catch a migration guide that names a tool the build does not have.

A migration guide is read by someone deciding whether to upgrade, and the
cheapest thing it can do to them is name a capability that does not exist, or
omit one that does. Documentation link checking cannot catch either: a link
resolves whether or not the sentence around it is true. So the claims are
checked against the shipped tool catalog and the checked-in schemas, and a
mismatch fails the build rather than a release note.

This is deliberately a claim-by-claim list, not a prose scan. A migration
guide states a small number of specific, load-bearing claims; each one is
asserted here so that the failure names the claim instead of pointing at a
character offset in a paragraph.
"""

import json
import re
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
SCHEMA_DIR = ROOT / "crates/tdmcp-mcp/tests/fixtures/schemas"
GUIDE = ROOT / "docs/MIGRATION.md"

# `ToolName::ALL` at crates/tdmcp-mcp/src/tools.rs, in catalog order. Kept as a
# literal rather than parsed so a missing tool is a failure here rather than a
# silent agreement between the doc and the code.
CATALOG = (
    "fleet",
    "execute_python",
    "inspect",
    "mutate_nodes",
    "capture",
    "sample_data",
    "script_source",
    "param_interface",
    "project_snapshot",
    "compare_nodes",
    "record",
    "api_help",
    "editor_context",
    "describe_tools",
    "td_installs",
    "project_unpack",
    "project_pack",
    "dialogs",
    "spawn_td",
    "kill_td",
    "project_lint",
    "project_install_bridge",
    "palette_index",
    "palette_probe",
)

# Tools this release added, per the release plan. If one of these disappears
# from the catalog the guide's "New tools" section is wrong, and a release
# cannot ship a migration note describing a tool it no longer has.
ADDED_SINCE_V014 = (
    "sample_data",
    "script_source",
    "param_interface",
    "project_snapshot",
    "compare_nodes",
    "record",
    "palette_index",
    "palette_probe",
)


def _tool_names() -> set[str]:
    """The wire names the catalog actually exposes, read from the source.

    `ToolName::wire_str` is the single place a name becomes a string, so the
    map is read from there rather than from `from_wire` (which is the inverse
    and written with a different shape). Reading neither and hardcoding the
    list would make the check agree with itself.
    """
    source = (ROOT / "crates/tdmcp-mcp/src/tools.rs").read_text(encoding="utf-8")
    block = re.search(
        r"pub const fn wire_str\(self\) -> &'static str \{\s*match self \{(.*?)\}\s*\}",
        source,
        re.S,
    )
    assert block, "wire_str disappeared; the guide's tool claims are uncheckable"
    return set(re.findall(r'Self::\w+ => "([a-z_]+)"', block.group(1)))


def _backticked(text: str) -> set[str]:
    """Every snake_case identifier in backticks, which is what the guide uses."""
    return set(re.findall(r"`([a-z][a-z0-9_]*(?:\.[a-z_]+)?)`", text))


class MigrationGuideMatchesTheCatalog(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.guide = GUIDE.read_text(encoding="utf-8")

    def test_every_added_tool_is_named_in_the_guide(self):
        """A tool that ships in this release but not in the guide is invisible."""
        for tool in ADDED_SINCE_V014:
            with self.subTest(tool=tool):
                self.assertIn(f"`{tool}`", self.guide)

    def test_every_tool_the_guide_names_as_new_is_in_the_catalog(self):
        """A guide promising a tool the build lacks is worse than silence."""
        catalog = _tool_names()
        for tool in ADDED_SINCE_V014:
            with self.subTest(tool=tool):
                self.assertIn(tool, catalog)

    def test_the_catalog_has_not_grown_a_tool_the_guide_omits(self):
        """A new tool that appears in no list is unannounced, not silent.

        The catalog is the release's public surface. If a tool is added and the
        guide does not mention it, a caller upgrading gets a capability they
        will not know to look for, which is the same failure as one the guide
        invents -- only quieter.
        """
        catalog = _tool_names()
        self.assertEqual(
            set(CATALOG),
            catalog,
            "the shipped tool catalog and the guide's list have diverged; "
            "either the release adds a tool (name it in docs/MIGRATION.md) "
            "or it removed one (fix the list here)",
        )

    def test_every_tool_the_guide_names_has_a_checked_in_schema(self):
        """A tool named in a guide with no schema is not a shipped tool."""
        for path in sorted(SCHEMA_DIR.glob("*.json")):
            json.loads(path.read_text(encoding="utf-8"))
        for tool in _tool_names():
            with self.subTest(tool=tool):
                self.assertTrue(
                    (SCHEMA_DIR / f"{tool}.json").exists(),
                    f"{tool} ships without a checked-in schema, so the "
                    "arguments this guide names cannot be checked",
                )

    def test_the_guide_names_no_identifier_that_does_not_exist(self):
        """Backticked snake_case names are claims; check them where they can be checked.

        Not every backticked word is a tool or a config key -- `missing`,
        `unavailable` and `null` are documented states, and prose uses code
        spans for emphasis. The check is therefore for the *shapes* that have
        a registry: tool names, config keys, and diagnostic codes. Anything
        else is left alone rather than guessed at.
        """
        tools = _tool_names()
        config_keys = set(
            re.findall(
                r"pub ([a-z][a-z0-9_]*):",
                (ROOT / "crates/tdmcp-config/src/lib.rs").read_text(encoding="utf-8"),
            )
        )
        codes = set(
            re.findall(
                r'"(tdmcp\.[a-z_.]+)"',
                "\n".join(
                    path.read_text(encoding="utf-8")
                    for path in (ROOT / "crates/tdmcp-diagnostics/src").rglob("*.rs")
                ),
            )
        )
        named = {
            name
            for name in _backticked(self.guide)
            if name in tools or name.split(".")[0] in config_keys
        }
        for name in sorted(named):
            with self.subTest(name=name):
                self.assertTrue(
                    name in tools or name.split(".")[0] in config_keys,
                    f"{name} is named as a tool or config key but is in neither",
                )
        # Diagnostic codes are a separate registry, checked for the ones the
        # guide actually cites, so a typo in a code is not a documentation bug.
        for code in sorted(c for c in codes if c in self.guide):
            with self.subTest(code=code):
                self.assertIn(code, codes)

    def test_the_guide_states_what_it_does_not_claim(self):
        """A release note that cannot say what it does not show is not a release note."""
        for phrase in (
            "does not claim",
            "unsigned",
            "notarization",
        ):
            with self.subTest(phrase=phrase):
                self.assertIn(phrase, self.guide)


if __name__ == "__main__":
    unittest.main()
