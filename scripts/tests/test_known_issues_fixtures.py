"""Offline shape/syntax checks complement, not replace, the recorded TD runs.

These scripts are the scratch probes recorded from real TouchDesigner runs (see
the README beside them). They are written to be run *inside* TD, top to bottom,
which is why they resolve `tdmcp_resolve(".")` at module scope. They therefore
cannot be executed offline, and nothing here claims they reproduce their
recorded results — that evidence lives in the README and in
docs/KNOWN_ISSUES_LIVE_VALIDATION.md.

Two properties CAN be checked offline, and both are checked here:

1. every script parses, so a recorded probe is not left in a state that cannot
   even be read; and
2. no script `import`s `td`. The probes are executed inside TD, where the
   Execute DAT injects the `td` module and the `tdmcp_resolve`/`result`/`op`
   helpers into the namespace (bridge/tdmcp_bridge/execute.py:189-216). An
   `import td` would fail in that environment, so the absence of one is what
   makes these scripts runnable in TD at all.

The second check is a real assertion over the parsed tree rather than a
property of `compile()`, which never executes a line and so cannot observe any
import behaviour.
"""
from __future__ import annotations

import ast
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[2] / "scripts/fixtures/known_issues"

# TouchDesigner exists only inside TD, as an injected module rather than an
# importable one. A probe that tried to import it would never have run.
UNDECLARABLE = frozenset({"td"})


class KnownIssuesFixtureTests(unittest.TestCase):
    def _trees(self) -> list[tuple[Path, ast.Module]]:
        scripts = sorted(ROOT.glob("*.py"))
        self.assertTrue(scripts, "the recorded probes are the evidence; there must be some")
        trees = []
        for path in scripts:
            with self.subTest(path=path.name):
                trees.append((path, ast.parse(path.read_text(encoding="utf-8"), filename=str(path))))
        return trees

    def test_td_scripts_compile(self) -> None:
        self._trees()

    def test_td_scripts_never_import_td(self) -> None:
        """`td` is injected by the Execute DAT; an import of it cannot work in TD."""
        for path, tree in self._trees():
            imported: list[str] = []
            for node in ast.walk(tree):
                if isinstance(node, ast.Import):
                    imported += [a.name for a in node.names]
                elif isinstance(node, ast.ImportFrom) and node.level == 0 and node.module:
                    imported.append(node.module)
            offenders = sorted(
                {name for name in imported if name.split(".")[0] in UNDECLARABLE}
            )
            with self.subTest(path=path.name):
                self.assertEqual(
                    offenders,
                    [],
                    f"{path.name} imports {offenders}; the Execute DAT injects the module, "
                    "so an import of it would fail inside TouchDesigner",
                )
