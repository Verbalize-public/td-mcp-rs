"""Unit tests for compare_nodes and guarded writes (no live TD).

The properties pinned here are the ones that make a delta safe to act on:

* An absent baseline is NOT an empty diff. This is the one that matters most:
  an empty diff would tell a caller every field is unchanged when nothing was
  actually compared, and the caller would then write on top of a scene it had
  never looked at.
* There is no counter shortcut. A daemon counter advances on dispatch, not on
  effect, so "nothing changed" is only ever said after a fresh re-read.
* `null`, `false`, `0`, `""` and an absent field are five different states.
* Freshness is reported, not implied, and the result says outright that it is
  neither atomic nor exactly-once.
"""

from __future__ import annotations

import math
import os
import sys
import unittest
from types import SimpleNamespace as NS
from typing import Any
from unittest import mock

sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))

from tdmcp_bridge import compare  # noqa: E402
from tdmcp_bridge.compare import handle_compare  # noqa: E402

sys.path.insert(0, os.path.join(os.path.dirname(__file__), "."))
from test_mutate import FakeCtx, FakeNode, FakePar, FakeParGroup  # noqa: E402


def node(**pars: Any) -> Any:
    """A fake operator whose `par` group carries exactly these parameters."""
    return NS(
        path="/project1/a",
        opType="constantTOP",
        comment="hello",
        valid=True,
        par=FakeParGroup({name: FakePar(value) for name, value in pars.items()}),
        inputConnectors=[],
        outputConnectors=[],
    )


def capture(target: Any, paths: list[str], fields: list[str], **extra: Any) -> dict[str, Any]:
    """Take a baseline through the real handler with resolution mocked."""
    with mock.patch.object(compare, "resolve_op", return_value=target):
        out = handle_compare({"paths": paths, "fields": fields, **extra})
    return out


class CompareCaptureTests(unittest.TestCase):
    def test_capture_mode_says_nothing_was_compared(self):
        out = capture(node(t=1.0), ["/project1/a"], ["par.t"], capture=True)
        self.assertTrue(out["ok"])
        self.assertEqual(out["mode"], "capture")
        # The critical bit: an empty change set here would be a lie.
        self.assertFalse(out["compared"])
        self.assertFalse(out["baselineAvailable"])
        self.assertIn("not a comparison", out["note"])
        self.assertIn("observedAtMs", out["observation"])

    def test_capture_reads_values_presence_aware(self):
        out = capture(node(t=1.0), ["/project1/a"], ["par.t", "par.missing"], capture=True)
        fields = out["observation"]["nodes"][0]["fields"]
        self.assertEqual(fields["par.t"], {"present": True, "value": 1.0})
        # A parameter that does not exist is ABSENT, not null.
        self.assertEqual(fields["par.missing"], {"present": False})

    def test_duplicate_paths_are_refused(self):
        out = capture(node(), ["/project1/a", "/project1/a"], ["par.t"])
        self.assertFalse(out["ok"])
        self.assertEqual(out["code"], "tdmcp.compare.invalid")

    def test_caps_are_enforced(self):
        many = [f"/project1/n{i}" for i in range(compare.COMPARE_MAX_NODES + 1)]
        self.assertFalse(capture(node(), many, ["par.t"])["ok"])
        fields = [f"par.p{i}" for i in range(compare.COMPARE_MAX_FIELDS + 1)]
        self.assertFalse(capture(node(), ["/project1/a"], fields)["ok"])


class CompareDeltaTests(unittest.TestCase):
    def baseline(self, target: Any, fields: list[str]) -> dict[str, Any]:
        out = capture(target, ["/project1/a"], fields, capture=True)
        self.assertTrue(out["ok"], out)
        return out["observation"]

    def test_no_change_is_reported_as_compared_not_as_baseline_missing(self):
        fields = ["par.t"]
        before = self.baseline(node(t=1.0), fields)
        out = capture(node(t=1.0), ["/project1/a"], fields, baseline=before)
        self.assertTrue(out["ok"])
        self.assertTrue(out["compared"])
        self.assertEqual(out["changedFields"], 0)
        self.assertEqual(out["nodes"][0]["changes"], [])

    def test_a_changed_value_is_an_exact_before_and_after(self):
        fields = ["par.t"]
        before = self.baseline(node(t=1.0), fields)
        out = capture(node(t=2.0), ["/project1/a"], fields, baseline=before)
        change = out["nodes"][0]["changes"][0]
        self.assertEqual(change["field"], "par.t")
        self.assertEqual(change["change"], "changed")
        self.assertEqual(change["before"]["value"], 1.0)
        self.assertEqual(change["after"]["value"], 2.0)

    def test_a_removed_field_is_not_reported_as_unchanged(self):
        fields = ["par.t"]
        before = self.baseline(node(t=1.0), fields)
        # Same path, but the parameter no longer exists on the operator.
        out = capture(node(), ["/project1/a"], fields, baseline=before)
        change = out["nodes"][0]["changes"][0]
        self.assertEqual(change["change"], "removed")
        self.assertTrue(change["before"]["present"])
        self.assertFalse(change["after"]["present"])

    def test_five_states_stay_five_states(self):
        """null, false, 0, "" and absent must never collapse into each other."""
        fields = ["par.n", "par.f", "par.z", "par.s", "par.absent"]
        before = self.baseline(
            NS(
                path="/project1/a",
                valid=True,
                par=FakeParGroup(
                    {"n": FakePar(None), "f": FakePar(False), "z": FakePar(0), "s": FakePar("")}
                ),
            ),
            fields,
        )
        # Every one of the four present values flips to another state.
        after_target = NS(
            path="/project1/a",
            valid=True,
            par=FakeParGroup(
                {"n": FakePar(1), "f": FakePar(True), "z": FakePar(1), "s": FakePar("x")}
            ),
        )
        out = capture(after_target, ["/project1/a"], fields, baseline=before)
        changes = {c["field"]: c for c in out["nodes"][0]["changes"]}
        # False -> True and 0 -> 1 are CHANGED, not "falsy, therefore same".
        self.assertEqual(changes["par.f"]["change"], "changed")
        self.assertIs(changes["par.f"]["after"]["value"], True)
        self.assertEqual(changes["par.z"]["change"], "changed")
        # None -> 1 is a change; None was never treated as absence.
        self.assertEqual(changes["par.n"]["change"], "changed")
        # The absent parameter was absent in both reads: correctly no entry.
        self.assertNotIn("par.absent", changes)

    def test_nan_and_infinity_are_marked_not_rounded(self):
        fields = ["par.t"]
        before = self.baseline(node(t=1.0), fields)
        out = capture(node(t=math.nan), ["/project1/a"], fields, baseline=before)
        change = out["nodes"][0]["changes"][0]
        self.assertEqual(change["after"]["value"], {"unrepresentable": "nan"})
        inf = capture(node(t=math.inf), ["/project1/a"], fields, baseline=before)
        self.assertEqual(
            inf["nodes"][0]["changes"][0]["after"]["value"], {"unrepresentable": "inf"}
        )

    def test_zero_to_false_is_a_change_in_both_directions(self):
        """Python says ``False == 0``; this module must not.

        A plain ``==`` here would report a change from ``0`` to ``False`` as
        "no change", which is precisely the falsy collapse the contract
        forbids. Pinned in both directions so neither can regress.
        """
        fields = ["par.t"]
        zero = self.baseline(node(t=0), fields)
        as_false = capture(node(t=False), ["/project1/a"], fields, baseline=zero)
        change = as_false["nodes"][0]["changes"][0]
        self.assertEqual(change["change"], "changed", "0 -> False must be a change")
        self.assertIs(change["after"]["value"], False)

        back = capture(node(t=0), ["/project1/a"], fields, baseline=as_false["observation"])
        self.assertEqual(
            back["nodes"][0]["changes"][0]["change"], "changed", "False -> 0 must be a change"
        )

    def test_strict_eq_keeps_the_falsy_states_apart(self):
        from tdmcp_bridge.compare import strict_eq

        self.assertFalse(strict_eq(0, False))
        self.assertFalse(strict_eq(False, 0))
        self.assertFalse(strict_eq(0, "0"))
        self.assertFalse(strict_eq("", None))
        self.assertFalse(strict_eq(None, False))
        # Genuinely equal pairs still match, including the numeric one.
        self.assertTrue(strict_eq(1.0, 1))
        self.assertTrue(strict_eq(False, False))
        self.assertTrue(strict_eq(None, None))
        self.assertTrue(strict_eq({"a": [1, 2]}, {"a": [1, 2]}))
        self.assertFalse(strict_eq({"a": [1, 2]}, {"a": [1, 3]}))


    def test_freshness_is_reported_and_atomicity_is_denied(self):
        fields = ["par.t"]
        before = self.baseline(node(t=1.0), fields)
        out = capture(node(t=1.0), ["/project1/a"], fields, baseline=before)
        self.assertIn("baselineMs", out["freshness"])
        self.assertIn("currentMs", out["freshness"])
        self.assertGreaterEqual(out["freshness"]["ageMs"], 0)
        self.assertIn("not a snapshot", out["freshness"]["note"])
        # Stated outright rather than left for the caller to assume.
        self.assertFalse(out["atomic"])
        self.assertFalse(out["exactlyOnce"])


class CompareBaselineFailureTests(unittest.TestCase):
    """Every way a baseline can be unusable must REFUSE, never diff nothing."""

    def test_absent_baseline_refuses_rather_than_reporting_no_change(self):
        out = capture(node(t=1.0), ["/project1/a"], ["par.t"], baseline=None)
        self.assertFalse(out["ok"])
        self.assertEqual(out["code"], "tdmcp.compare.baseline_unavailable")
        self.assertFalse(out["baselineAvailable"])
        self.assertFalse(out["compared"])
        self.assertIn("not evidence that the scene is unchanged", out["message"])
        self.assertNotIn("changes", out)

    def test_capture_must_be_asked_for_so_a_compare_cannot_become_one(self):
        """The refusal is the default, not a special case of a lost baseline."""
        refused = capture(node(t=1.0), ["/project1/a"], ["par.t"])
        self.assertFalse(refused["ok"])
        self.assertFalse(refused["compared"])
        captured = capture(node(t=1.0), ["/project1/a"], ["par.t"], capture=True)
        self.assertTrue(captured["ok"])
        self.assertEqual(captured["mode"], "capture")
        self.assertFalse(captured["compared"])

    def test_a_baseline_without_a_stamp_is_not_an_observation(self):
        out = capture(
            node(t=1.0),
            ["/project1/a"],
            ["par.t"],
            baseline={"nodes": [{"path": "/project1/a", "fields": {}}]},
        )
        self.assertFalse(out["ok"])
        self.assertEqual(out["code"], "tdmcp.compare.baseline_unavailable")
        self.assertIn("observedAtMs", out["message"])

    def test_a_baseline_over_a_different_projection_is_refused(self):
        before = capture(node(t=1.0), ["/project1/a"], ["par.t"], capture=True)["observation"]
        out = capture(node(t=2.0), ["/project1/a"], ["par.t", "par.r"], baseline=before)
        self.assertFalse(out["ok"])
        self.assertEqual(out["code"], "tdmcp.compare.projection_mismatch")
        self.assertEqual(out["requestedFields"], ["par.t", "par.r"])

    def test_a_baseline_missing_a_path_names_it(self):
        before = capture(node(t=1.0), ["/project1/a"], ["par.t"], capture=True)["observation"]
        out = capture(node(t=1.0), ["/project1/a", "/project1/b"], ["par.t"], baseline=before)
        self.assertFalse(out["ok"])
        self.assertEqual(out["code"], "tdmcp.compare.baseline_unavailable")
        self.assertEqual(out["missingFromBaseline"], ["/project1/b"])

    def test_an_unreadable_node_is_not_silently_unchanged(self):
        before = capture(node(t=1.0), ["/project1/a"], ["par.t"], capture=True)["observation"]
        with mock.patch.object(compare, "resolve_op", return_value=None):
            out = handle_compare(
                {"paths": ["/project1/a"], "fields": ["par.t"], "baseline": before}
            )
        self.assertTrue(out["ok"])
        self.assertEqual(out["unreadableNodes"], 1)
        self.assertFalse(out["nodes"][0]["readable"])

    def test_a_baseline_that_could_not_read_a_node_says_so(self):
        before = capture(node(t=1.0), ["/project1/a"], ["par.t"], capture=True)["observation"]
        before["nodes"][0]["readable"] = False
        out = capture(node(t=1.0), ["/project1/a"], ["par.t"], baseline=before)
        entry = out["nodes"][0]
        self.assertFalse(entry["baselineReadable"])
        self.assertEqual(entry["changes"], [])
        self.assertIn("no field here is reported unchanged", entry["note"])


class WireDeltaTests(unittest.TestCase):
    def test_a_rewire_is_detected_in_the_projection(self):
        src = NS(path="/project1/src")
        before_target = NS(
            path="/project1/a",
            valid=True,
            inputConnectors=[NS(connections=[])],
            outputConnectors=[],
        )
        before = capture(
            before_target, ["/project1/a"], ["wire.input.0"], capture=True
        )["observation"]
        after_target = NS(
            path="/project1/a",
            valid=True,
            inputConnectors=[NS(connections=[NS(owner=src, index=0)])],
            outputConnectors=[],
        )
        out = capture(after_target, ["/project1/a"], ["wire.input.0"], baseline=before)
        change = out["nodes"][0]["changes"][0]
        self.assertEqual(change["change"], "changed")
        self.assertEqual(change["after"]["value"], [{"path": src.path, "index": 0}])


if __name__ == "__main__":
    unittest.main()
