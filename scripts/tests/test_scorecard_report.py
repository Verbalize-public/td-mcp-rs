"""Tests for the four-scorecard report.

The interesting tests here are the refusals, not the happy path. A report that
prints four rows of numbers is easy; the failure this work exists to prevent is
four rows of numbers where one of them was never measured, so every test below
asks what the report does when an input is missing, stale or lying.
"""

from __future__ import annotations

import importlib.util
import json
import sys
import unittest
from pathlib import Path
from unittest import mock

SPEC = importlib.util.spec_from_file_location(
    "tdmcp_scorecard_report",
    Path(__file__).resolve().parent.parent / "scorecard_report.py",
)
SCORECARD = importlib.util.module_from_spec(SPEC)
# A dataclass resolves its annotations through sys.modules[cls.__module__], so
# the module has to be registered before it is executed or the import raises
# at class-definition time.
sys.modules[SPEC.name] = SCORECARD
SPEC.loader.exec_module(SCORECARD)
REPO = SCORECARD.REPO


class TheFourScorecardsAreTheShape(unittest.TestCase):
    """The format exists to keep four claims from covering for one another."""

    def test_the_report_carries_exactly_the_four_named_cards(self):
        self.assertEqual(
            ("encoding", "projections", "workflows", "transport"),
            SCORECARD.SCORECARDS,
            "a scorecard set that can grow a fifth member can blend the four",
        )

    def test_a_report_missing_a_card_is_refused_not_reported(self):
        report = {
            "scorecards": {
                "encoding": {"measured": True},
                "workflows": {"measured": True},
                "transport": {"measured": True},
            }
        }
        with self.assertRaises(SCORECARD.ScorecardError) as caught:
            SCORECARD.check_report(report)
        self.assertIn("projections", str(caught.exception))

    def test_cards_in_the_wrong_order_are_refused(self):
        cards = {
            name: {"measured": True}
            for name in ("projections", "encoding", "transport", "workflows")
        }
        with self.assertRaises(SCORECARD.ScorecardError) as caught:
            SCORECARD.check_report({"scorecards": cards})
        self.assertIn("in order", str(caught.exception))

    def test_an_unmeasured_card_is_named_rather_than_hidden(self):
        cards = {
            name: {"measured": name != "projections"}
            for name in SCORECARD.SCORECARDS
        }
        self.assertEqual(["projections"], SCORECARD.check_report({"scorecards": cards}))

    def test_a_three_of_four_report_does_not_pass_because_three_passed(self):
        """The whole point: three measured cards must not cover a fourth."""
        cards = {name: {"measured": name != "transport"} for name in SCORECARD.SCORECARDS}
        with self.assertRaises(SCORECARD.ScorecardError) as caught:
            SCORECARD.require_measured(cards)
        message = str(caught.exception)
        self.assertIn("transport", message)
        self.assertIn("UNMEASURED, not zero", message)

    def test_the_relief_flag_still_names_what_is_unmeasured(self):
        """`--allow-unmeasured` may proceed, but it may not pass silently."""
        cards = {name: {"measured": name != "transport"} for name in SCORECARD.SCORECARDS}
        self.assertEqual(["transport"], SCORECARD.require_measured(cards, allow=True))
        with self.assertRaises(SCORECARD.ScorecardError):
            SCORECARD.require_measured(cards, allow=False)


class AMissingNumberIsNotAZero(unittest.TestCase):
    """`copy_number` is the guard that keeps zeros out of a scorecard."""

    def test_a_key_that_was_never_measured_is_refused(self):
        with self.assertRaises(SCORECARD.ScorecardError) as caught:
            SCORECARD.copy_number({"measured": True}, "tokens_saved", "encoding")
        message = str(caught.exception)
        self.assertIn("tokens_saved", message)
        self.assertIn("not zero", message)

    def test_a_measured_zero_is_still_copied(self):
        """Zero is a real measurement. The full arm saves nothing, by design."""
        self.assertEqual(0, SCORECARD.copy_number({"percent_saved": 0.0}, "percent_saved", "x"))
        self.assertIsNone(SCORECARD.copy_number({"p": None}, "p", "x"))


class TheEncodingCardMeasuresTheDocumentsItWasGiven(unittest.TestCase):
    def test_no_dump_is_an_error_not_an_empty_card(self):
        with self.assertRaises(SCORECARD.ScorecardError) as caught:
            SCORECARD.read_dump(Path("/tmp/does-not-exist-abc.json"))
        self.assertIn("no token dump", str(caught.exception))
        self.assertIn("a scorecard of zeros", str(caught.exception))

    def test_an_empty_dump_is_refused(self):
        """`[]` is a dump that happened, of nothing.

        Reading it succeeds; producing a scorecard from it must not. The
        failure this prevents is a 0-of-0 percentage, which is not a
        measurement of a 0% saving.
        """
        import tempfile

        handle = tempfile.NamedTemporaryFile("w", suffix=".json", delete=False)
        handle.write("[]")
        handle.close()
        with self.assertRaises(SCORECARD.ScorecardError) as caught:
            SCORECARD.read_dump(Path(handle.name))
        self.assertIn("no documents", str(caught.exception))

    def test_a_dump_whose_declined_rows_differ_is_refused_by_the_measure(self):
        """The two halves must be one set of answers, not two in one file.

        `measure_tokens` already refuses this, and the card inherits the
        refusal rather than re-deriving a percentage from documents that
        describe different behaviour.
        """
        rows = [
            {"label": "declined", "plan": "full", "full": "{}", "adaptive": '{"a":1}'},
            {"label": "compact", "plan": "compact", "full": "{}", "adaptive": "{}"},
        ]
        encodings = SCORECARD.TOKENS.load_encodings(("cl100k_base",))
        with self.assertRaises(SCORECARD.TOKENS.MeasurementError) as caught:
            SCORECARD.TOKENS.measure(rows, encodings)
        self.assertIn("does not match the behaviour it is measuring", str(caught.exception))

    def test_the_card_reports_the_declined_arm_separately(self):
        """Ten calls that returned the full value must not dilute two that did not."""
        card = SCORECARD.encoding_scorecard(SCORECARD.DEFAULT_DUMP)
        for name, row in card["tokensByEncoding"].items():
            self.assertIn("compact", row["per_plan"], f"{name} lost its compact arm")
            self.assertIn("full", row["per_plan"], f"{name} lost its declined arm")
            self.assertEqual(0.0, row["per_plan"]["full"]["percent_saved"])


class TheProjectionCardCountsTheCostOfReassembly(unittest.TestCase):
    """A projection that has to be reassembled is not free."""

    def test_a_missing_study_is_unmeasured_not_zero(self):
        with mock.patch.object(SCORECARD, "PROJECTION_STUDY", REPO / "no-such-study.json"):
            with mock.patch.object(SCORECARD, "DELTA_STUDY", REPO / "no-such-delta.json"):
                card = SCORECARD.projections_scorecard()
        self.assertFalse(card["measured"])
        self.assertIn("UNMEASURED", card["reason"])
        self.assertNotIn("percent_saved", card)

    def test_a_study_with_no_denominator_is_refused(self):
        with mock.patch.object(SCORECARD, "PROJECTION_STUDY", Path(self._write({"totals": {}}))):
            with mock.patch.object(SCORECARD, "DELTA_STUDY", Path(self._write({"pairs": 0}))):
                with self.assertRaises(SCORECARD.ScorecardError) as caught:
                    SCORECARD.projections_scorecard()
        self.assertIn("denominator", str(caught.exception))

    def test_the_read_all_cost_is_reported_beside_the_saving(self):
        card = SCORECARD.projections_scorecard()
        if not card["measured"]:
            self.skipTest("the private study is not in this checkout")
        self.assertIsNotNone(card["simulatedReadAllChars"])
        self.assertIn("only cheaper if", card["readAllNote"])
        self.assertGreater(card["simulatedReadAllChars"], card["referenceFrontChars"])

    def _write(self, body: dict) -> str:
        import tempfile

        handle = tempfile.NamedTemporaryFile("w", suffix=".json", delete=False)
        json.dump(body, handle)
        handle.close()
        return handle.name


class TheWorkflowCardKeepsCostAndValueApart(unittest.TestCase):
    """A token saving must not be allowed to pay for a schema cost."""

    def test_the_card_does_not_net_schema_against_saving(self):
        card = SCORECARD.workflows_scorecard(SCORECARD.WORKFLOW.GUIDANCE.DEFAULT_FIXTURES)
        self.assertTrue(card["measured"], card["unmeasuredWorkflows"])
        self.assertIn("NOT netted", card["caveat"])
        for name, row in card["cards"].items():
            self.assertIsInstance(row["schemaBytesPerEligibleSession"], float)
            self.assertGreater(row["schemaBytesPerEligibleSession"], 0, name)

    def test_an_unmeasured_workflow_makes_the_card_unmeasured(self):
        """A card over three measured workflows is a card over three."""
        import tempfile

        handle = tempfile.NamedTemporaryFile("w", suffix=".json", delete=False)
        json.dump({"version": 1, "scenarios": []}, handle)
        handle.close()
        with mock.patch.object(
            SCORECARD.WORKFLOW,
            "score_workflows",
            return_value={
                "scorecards": {
                    name: {
                        "tool": name,
                        "measured": name != "numeric",
                        "schemaBytesAddedPerEligibleSession": 1.0,
                        "eligibleSessions": 1,
                        "replacedRoutesProbed": 0,
                        "replacedRouteRefused": None,
                    }
                    for name in SCORECARD.WORKFLOW.WORKFLOWS
                },
                "oracles": {},
                "scenarios": 0,
                "unmeasuredWorkflows": ["numeric"],
                "newToolsInAnyScenario": [],
                "newToolsRequiredByAnyScenario": [],
            },
        ):
            card = SCORECARD.workflows_scorecard(Path(handle.name))
        self.assertFalse(card["measured"])
        self.assertEqual(["numeric"], card["unmeasuredWorkflows"])

    def test_the_cost_of_a_tool_is_charged_to_the_sessions_that_need_it(self):
        card = SCORECARD.workflows_scorecard(SCORECARD.WORKFLOW.GUIDANCE.DEFAULT_FIXTURES)
        for name, row in card["cards"].items():
            self.assertGreater(row["eligibleSessions"], 0, f"{name} was charged to nobody")


class TheTransportCardNamesItsEvidence(unittest.TestCase):
    """A parity claim with no test behind it is a number in a table."""

    def test_a_missing_suite_is_unmeasured(self):
        with mock.patch.object(SCORECARD, "REPO", Path("/tmp/no-such-repo")):
            card = SCORECARD.transport_scorecard()
        self.assertFalse(card["measured"])
        self.assertIn("no evidence", card["reason"])

    def test_a_deleted_test_is_not_a_passing_test(self):
        """The card names the tests it depends on, and checks they are there.

        A test that was renamed or deleted leaves the claim exactly as true
        and the evidence exactly as gone, so a report built from a grep of the
        source is only worth anything if the grep can fail.
        """
        with mock.patch.object(
            SCORECARD,
            "TRANSPORT_TESTS",
            (SCORECARD.TRANSPORT_TESTS[0],) + SCORECARD.TRANSPORT_TESTS[1:] + ("a_test_that_never_existed",),
        ):
            card = SCORECARD.transport_scorecard()
        self.assertFalse(card["measured"])
        self.assertIn("a_test_that_never_existed", card["reason"])
        self.assertIn("not a passing one", card["reason"])

    def test_a_shrunken_suite_is_visible(self):
        """The card must not silently shrink with the evidence."""
        card = SCORECARD.transport_scorecard()
        if not card["measured"]:
            self.skipTest("no parity suite in this checkout")
        self.assertEqual(len(SCORECARD.TRANSPORT_TESTS) - 1, card["parityTestsFound"])


class TheAdmissionFreezeIsAFloorNotACeiling(unittest.TestCase):
    def test_it_reports_the_largest_capture_and_its_share_of_the_bound(self):
        freeze = SCORECARD.admission_freeze()
        self.assertTrue(freeze["measured"])
        self.assertEqual(9, freeze["observations"])
        self.assertEqual(440234, freeze["largestCaptureBytes"])
        self.assertLess(freeze["largestAsPercentOfBound"], 100.0)
        self.assertIn("not a frozen distribution", freeze["caveat"])

    def test_a_sample_with_no_observations_is_refused(self):
        import tempfile

        handle = tempfile.NamedTemporaryFile("w", suffix=".json", delete=False)
        json.dump({"note": "empty", "observations": []}, handle)
        handle.close()
        with mock.patch.object(SCORECARD, "ADMISSION_SAMPLE", Path(handle.name)):
            with self.assertRaises(SCORECARD.ScorecardError) as caught:
                SCORECARD.admission_freeze()
        self.assertIn("no observations", str(caught.exception))


class TheReportRefusesToLookComplete(unittest.TestCase):
    def test_the_shipped_set_measures_all_four(self):
        report = SCORECARD.build_report(
            SCORECARD.DEFAULT_DUMP, SCORECARD.WORKFLOW.GUIDANCE.DEFAULT_FIXTURES
        )
        self.assertTrue(report["ok"], report["unmeasuredScorecards"])
        self.assertEqual([], report["unmeasuredScorecards"])
        self.assertEqual(4, len(report["scorecards"]))

    def test_it_states_what_it_is_not_claiming(self):
        report = SCORECARD.build_report(
            SCORECARD.DEFAULT_DUMP, SCORECARD.WORKFLOW.GUIDANCE.DEFAULT_FIXTURES
        )
        for claim in ("model token A/B significance", "agent task-acceptance rate"):
            self.assertIn(claim, report["notClaimed"])


if __name__ == "__main__":
    unittest.main()
