"""The new-tool scorecard must be able to find the gap it exists to report.

`scripts/measure_workflow_value.py` answers one question: is each new workflow
`sample_data` / `script_source` / `param_interface` / `project_snapshot`
exercised by a scenario that could only be completed with it? The release plan's
gate 4 separates that from encoding gains, because a tool that did not exist
cannot have contributed to a token saving.

The tests are written around the failure this is most likely to have. The
harness is deterministic and the fixtures are checked in, so a scorecard can
look perfect while measuring nothing — a scenario whose required action uses a
DIFFERENT tool, an oracle that any answer satisfies, a schema cost counted
only for the sessions that already used the tool. Each test below names the
claim it protects and asserts something that would fail if the claim stopped
holding.

They do not need a model, a tokenizer, or TouchDesigner.
"""

import importlib.util
import json
import sys
import tempfile
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location(
    "measure_workflow_value", ROOT / "scripts/measure_workflow_value.py"
)
if SPEC is None or SPEC.loader is None:
    raise RuntimeError("cannot load the workflow-value script")
WORKFLOW = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = WORKFLOW
SPEC.loader.exec_module(WORKFLOW)


#: The minimum arguments `sample_data` accepts. Every oracle step routes
#: through real schema validation, so a match missing a required field is an
#: INVALID REQUEST rather than a step, and the scenario would fail to play for
#: a reason that has nothing to do with the claim under test.
MINIMAL_SAMPLE = {"pid": "1", "path": "/project/n0"}


def _scenario(**overrides) -> dict:
    """A scenario shaped like the real fixtures, minimally.

    The fields here are the ones `guidance_eval.validate_fixtures` demands, so
    a scenario built by this helper is one the harness will accept: a well
    formed `S\\d{2,}` id, a nonempty forbidden list (the harness refuses a
    scenario that bounds nothing), and tool arguments that actually satisfy
    the tool's schema. A synthetic scenario that the product would reject
    tests the rejection, not the scorecard.
    """
    scenario = {
        "id": "S01",
        "title": "A scenario under test",
        "task": "Read the value of a parameter.",
        "stimulus": {},
        "allowed_tools": ["sample_data", "capture"],
        "initial_state": {"known": False},
        "expected_actions": [
            {
                "id": "read_it",
                "match": {
                    "tool": "sample_data",
                    "arguments": MINIMAL_SAMPLE,
                },
                "response": {"ok": True},
                "effects": [{"op": "set", "path": "known", "value": True}],
            }
        ],
        "forbidden_actions": [
            {
                "id": "an_image_is_not_a_number",
                "match": {"tool": "capture"},
                "reason": "a PNG shows a colour, not the exact channel value",
            }
        ],
        "expected_outcomes": [
            {
                "id": "value_known",
                "requires": ["read_it"],
                "state": {"known": True},
            }
        ],
        "final_status": "complete",
        "workflow": "numeric",
    }
    scenario.update(overrides)
    return scenario


class _ScoredAgainstItsOwnFile(unittest.TestCase):
    """Base for tests that need `score_workflows` to actually execute oracles.

    The oracle half of `score_workflows` re-reads each scenario from the
    fixture FILE by id, because that is the only way the harness can play it.
    So a synthetic scenario has to be written to a file and passed back: give
    the function a list of dicts from nowhere and it will score the cost half
    from the argument and the acceptance half from a different document, and
    one scorecard will describe two different scenarios. That defect was real
    and shipped into a first draft of these tests.
    """

    def setUp(self):
        self.tmpdir = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmpdir.cleanup)
        self.tmp = Path(self.tmpdir.name)
        self.fixtures = self.tmp / "fixtures.json"

    def _scored(self, *scenarios):
        self.fixtures.write_text(
            json.dumps({"version": 1, "scenarios": list(scenarios)}), encoding="utf-8"
        )
        return WORKFLOW.score_workflows(
            list(scenarios), workdir=self.tmp, fixtures=self.fixtures
        )


class TheToolMapIsGroundTruth(unittest.TestCase):
    def test_each_new_tool_maps_to_exactly_one_workflow(self):
        """Gate 4 names four workflows; a fifth would be a different scorecard.

        Two tools sharing a workflow would let one tool's cost hide another's,
        which is the conflation this script exists to prevent.
        """
        self.assertEqual(4, len(WORKFLOW.NEW_TOOLS))
        self.assertEqual(len(WORKFLOW.NEW_TOOLS), len(set(WORKFLOW.NEW_TOOLS.values())))
        self.assertEqual(
            {"sample_data", "script_source", "param_interface", "project_snapshot"},
            set(WORKFLOW.NEW_TOOLS),
        )

    def test_every_workflow_has_a_tool_and_a_scorecard_slot(self):
        self.assertEqual(sorted(WORKFLOW.NEW_TOOLS.values()), sorted(WORKFLOW.WORKFLOWS))
        cards = WORKFLOW.score_workflows([])
        self.assertEqual(sorted(WORKFLOW.WORKFLOWS), sorted(cards["scorecards"]))

    def test_an_undeclared_workflow_is_refused_not_ignored(self):
        """A typo would otherwise create a scenario that measures nothing."""
        with self.assertRaises(WORKFLOW.WorkflowValueError) as caught:
            WORKFLOW.workflow_of(_scenario(workflow="numerics"))
        self.assertIn("numerics", str(caught.exception))


class AWorkflowIsMeasuredOnlyByItsOwnTool(_ScoredAgainstItsOwnFile):
    """The route is the ground truth, so reaching the answer another way is a miss."""

    def test_a_scenario_requiring_the_tool_is_measured(self):
        report = self._scored(_scenario())
        card = report["scorecards"]["numeric"]
        self.assertTrue(card["measured"])
        self.assertEqual(1, card["requireTheTool"])
        # Only the workflow this scenario exercises becomes measurable. The
        # other three are still unexamined, and a report that said otherwise
        # would be claiming coverage it does not have.
        self.assertEqual(
            ["checkpoint", "interface", "source"], report["unmeasuredWorkflows"]
        )

    def test_reaching_the_same_answer_through_another_tool_is_not_a_pass(self):
        """`execute_python` can read a pixel. That is not the same measurement.

        The harness has a deterministic response for `sample_data`, so a
        scenario answered by any other route never exercised the tool whose
        value is on trial. Counting it would let a future removal of
        `sample_data` go unmeasured, which is the exact regression gate 4 exists
        to catch.
        """
        report = self._scored(
            _scenario(
                expected_actions=[
                    {
                        "id": "read_it",
                        "match": {"tool": "execute_python", "arguments": {"script": "1"}},
                        "response": {"ok": True},
                    }
                ]
            )
        )
        self.assertFalse(report["scorecards"]["numeric"]["measured"])
        self.assertIn("numeric", report["unmeasuredWorkflows"])

    def test_a_workflow_with_no_scenarios_is_unmeasured_not_zero(self):
        report = self._scored(_scenario(workflow="numeric"))
        for name in ("source", "interface", "checkpoint"):
            card = report["scorecards"][name]
            self.assertFalse(card["measured"], f"{name} must not claim a measurement")
            self.assertEqual(0, card["scenarios"])
            self.assertEqual(0, card["schemaBytesInItsOwnScenarios"])
        self.assertEqual(
            ["checkpoint", "interface", "source"], sorted(report["unmeasuredWorkflows"])
        )

    def test_a_scenario_that_never_allows_the_tool_cannot_require_it(self):
        """Allowed-but-unused and required are different facts, reported apart."""
        report = self._scored(_scenario(allowed_tools=["inspect", "sample_data"]))
        card = report["scorecards"]["numeric"]
        self.assertEqual(1, card["scenarios"])
        self.assertEqual(1, card["allowedTheTool"])
        self.assertEqual(1, card["requireTheTool"])
        self.assertTrue(card["measured"])

    def test_an_optional_action_does_not_make_a_workflow_measured(self):
        """An action marked `required: false` is optional work, not the task."""
        report = self._scored(
            _scenario(
                expected_actions=[
                    {
                        "id": "read_it",
                        "match": {"tool": "sample_data", "arguments": {"path": "/p/n0"}},
                        "response": {"ok": True},
                        "required": False,
                    }
                ]
            )
        )
        self.assertFalse(report["scorecards"]["numeric"]["measured"])

    def test_two_new_tools_in_one_scenario_is_refused(self):
        """Blending two workflows would make the scorecard mean nothing."""
        with self.assertRaises(WORKFLOW.WorkflowValueError) as caught:
            WORKFLOW.reached_route(
                _scenario(
                    expected_actions=[
                        {"id": "a", "match": {"tool": "sample_data"}, "response": {}},
                        {"id": "b", "match": {"tool": "script_source"}, "response": {}},
                    ]
                )
            )
        self.assertIn("sample_data", str(caught.exception))


class SchemaCostIsChargedToAvailability(_ScoredAgainstItsOwnFile):
    """A tool costs what it costs to be AVAILABLE, not only to be used."""

    def test_a_tool_the_scenario_never_calls_still_costs_schema_bytes(self):
        """Charging only on use would make a rarely used tool look free.

        The harness records schema exposure at `start` for the whole allowed
        set. A caller that is offered `sample_data` pays for it whether or not
        the task needs it, and that is the number a decision to ship the tool
        has to weigh.
        """
        not_offered = _scenario(allowed_tools=["inspect"])
        added, sessions = WORKFLOW.cost_of_making_available([not_offered], "sample_data")
        self.assertEqual(1, sessions)
        self.assertGreater(added, 0, "an offered-but-uncalled tool is not free")

    def test_the_price_is_the_tools_own_schema_and_nothing_else(self):
        """Otherwise the 'added' cost is the whole allowed set, not the addition."""
        alone, _ = WORKFLOW.cost_of_making_available(
            [_scenario(allowed_tools=[])], "sample_data"
        )
        with_inspect, _ = WORKFLOW.cost_of_making_available(
            [_scenario(allowed_tools=["inspect"])], "sample_data"
        )
        # The JSON key and its separator are real wire cost, so the bound is
        # the schema plus a small constant rather than exact equality: an exact
        # figure would fail on a formatting change while saying nothing about
        # the property under test.
        self.assertLessEqual(
            abs(alone - with_inspect), 2, "the delta must not include the rest of the set"
        )

    def test_a_scenario_that_already_allows_the_tool_is_not_charged_twice(self):
        """Otherwise every tool would look free in its own scenario.

        A session that lists `sample_data` has already paid for it through
        `allowed_tools`. Counting the tool again against that same session
        reports zero for the only session that genuinely uses it -- and that
        zero is what the first version of this scorecard printed for all four
        workflows.
        """
        only_user = [_scenario(allowed_tools=["sample_data"])]
        added, sessions = WORKFLOW.cost_of_making_available(only_user, "sample_data")
        self.assertEqual(0, added, "a tool the set already allows is free to add")
        self.assertEqual(0, sessions, "no session lacks it, so none is eligible")

        mixed = only_user + [_scenario(id="S99", allowed_tools=["inspect"])]
        added, sessions = WORKFLOW.cost_of_making_available(mixed, "sample_data")
        self.assertEqual(1, sessions, "only the session that lacks it is eligible")
        self.assertGreater(added, 0)

    def test_the_spread_cost_covers_every_session_that_lacks_the_tool(self):
        """The price is the cost of MAKING it available, session by session.

        A tool's own scenario already declares it, so the sessions priced are
        the ones that could reach the tool and do not -- which is the case a
        decision to ship it has to weigh.
        """
        own = _scenario(allowed_tools=["sample_data"])
        others = [
            _scenario(id="S99", workflow="source", allowed_tools=["inspect"]),
            _scenario(id="S98", workflow="checkpoint", allowed_tools=["inspect"]),
        ]
        added, sessions = WORKFLOW.cost_of_making_available([own, *others], "sample_data")
        self.assertEqual(2, sessions, "both sessions lacking it are eligible")
        self.assertGreater(added, 0)
        single, _ = WORKFLOW.cost_of_making_available([others[0]], "sample_data")
        self.assertEqual(
            single * 2, added, "the price must be per-session, not discounted by a bulk case"
        )

    def test_the_price_is_reported_per_session_not_only_in_total(self):
        """A total lets one cheap session hide an expensive one."""
        report = self._scored(
            _scenario(allowed_tools=["inspect"]),
            _scenario(id="S02", allowed_tools=["inspect", "execute_python"]),
        )
        card = report["scorecards"]["numeric"]
        self.assertEqual(2, card["eligibleSessions"])
        self.assertGreater(card["schemaBytesAddedPerEligibleSession"], 0)

    def test_every_schema_is_one_the_harness_actually_validates_against(self):
        """A cost computed from a schema the product would reject is fiction."""
        for name in WORKFLOW.NEW_TOOLS:
            self.assertIn(name, WORKFLOW.GUIDANCE.TOOL_SCHEMAS)


class TheReportRefusesToLookComplete(_ScoredAgainstItsOwnFile):
    """A vacuous scorecard is the failure this script is built to prevent."""

    def _run(self, *extra):
        return WORKFLOW.main(["--fixtures", str(self.fixtures), *extra])

    def _write(self, scenarios):
        self.fixtures.write_text(
            json.dumps({"version": 1, "scenarios": scenarios}), encoding="utf-8"
        )

    def test_the_shipped_fixture_set_measures_every_workflow(self):
        """Gate 4 is measured by the CHECKED-IN set, and that must stay true.

        This test was the reverse once: it pinned the fact that no scenario
        exercised a new tool, and failed when the four were added. A gap that
        gets closed should be pinned in the closed state instead -- a test
        that remembers a solved problem is still useful, because it is the one
        place that notices if the closure is undone.
        """
        report = WORKFLOW.score_workflows(WORKFLOW.load_scenarios())
        self.assertEqual(sorted(WORKFLOW.NEW_TOOLS), report["newToolsInAnyScenario"])
        self.assertEqual(
            sorted(WORKFLOW.NEW_TOOLS), report["newToolsRequiredByAnyScenario"]
        )
        self.assertEqual([], report["unmeasuredWorkflows"])

    def test_the_shipped_fixture_set_exits_zero(self):
        """The real fixtures must not need the override."""
        self._write(WORKFLOW.load_scenarios())
        self.assertEqual(0, self._run())

    def test_the_shipped_cost_figures_are_real_numbers(self):
        """A price of zero or null for a shipped tool is a broken measurement."""
        report = WORKFLOW.score_workflows(WORKFLOW.load_scenarios())
        for workflow, card in report["scorecards"].items():
            with self.subTest(workflow=workflow):
                self.assertGreater(card["schemaBytesAddedPerEligibleSession"], 0)
                self.assertGreater(card["eligibleSessions"], 0)
                self.assertGreater(card["schemaBytesInItsOwnScenarios"], 0)
                self.assertEqual(1, card["requireTheTool"])

    #: The minimum arguments each new tool's schema accepts. The oracle drives
    #: real schema validation, so a match that omits a required field is an
    #: invalid request rather than a step, and the scenario would fail to play
    #: for a reason that has nothing to do with the claim under test.
    MINIMAL_ARGS = {
        "sample_data": MINIMAL_SAMPLE,
        "script_source": {"pid": "1"},
        "param_interface": {"pid": "1", "path": "/project/n0"},
        "project_snapshot": {"pid": "1"},
    }

    def _measured_scenarios(self, count: int) -> list:
        """`count` scenarios, one per workflow, each requiring its own tool.

        The action keeps the id `read_it` because `expected_outcomes` names it
        in `requires`; an outcome that requires an id no action declares is
        never earned, and the scenario then fails to play for a reason that has
        nothing to do with the workflow it is standing in for.
        """
        pairs = list(WORKFLOW.NEW_TOOLS.items())[:count]
        return [
            _scenario(
                id=f"S{index + 1:02d}",
                workflow=workflow,
                allowed_tools=[tool, "capture"],
                expected_actions=[
                    {
                        "id": "read_it",
                        "match": {
                            "tool": tool,
                            "arguments": self.MINIMAL_ARGS[tool],
                        },
                        "response": {"ok": True},
                        "effects": [{"op": "set", "path": "known", "value": True}],
                    }
                ],
            )
            for index, (tool, workflow) in enumerate(pairs)
        ]

    def test_a_fully_measured_set_does_not_trigger_the_refusal(self):
        """The override must not become the normal path by being the only one tested."""
        self._write(self._measured_scenarios(len(WORKFLOW.NEW_TOOLS)))
        self.assertEqual(0, self._run())

    def test_a_partially_measured_set_still_refuses(self):
        """Three of four is not a measurement of the fourth."""
        self._write(self._measured_scenarios(3))
        self.assertEqual(1, self._run())


class TheOracleIsPlayedNotTrusted(_ScoredAgainstItsOwnFile):
    """`measured` must be a demonstrated fact, not a field a fixture asserts.

    A scorecard that reads `workflow` off the fixture and calls the result
    "measured" is reporting the author's opinion. These tests break the
    scenario in a way that leaves the declaration intact -- the oracle no
    longer plays, or the route it replaces is no longer closed -- and require
    the scorecard to withdraw the claim.
    """

    def _card(self, **overrides):
        report = self._scored(_scenario(**overrides))
        return report["scorecards"]["numeric"], report["oracles"]["S01"]

    def test_a_scenario_that_does_not_play_is_not_measured(self):
        """A workflow whose oracle does not execute is decoration, not value.

        The break is an outcome whose `state` the scripted response never
        produces, so the session is refused on the outcome check while the
        declaration `workflow: numeric` survives untouched. Reading the fixture
        would still call this measured.

        An earlier attempt broke this with `final_status: "blocked"`, which
        did NOT work: `run_oracle` claims whatever the scenario declares, so a
        scenario that lies about its own final status still plays cleanly. The
        oracle is self-consistent by construction, which is a property worth
        knowing and is why the break has to come from the OUTCOME side.
        """
        card, oracle = self._card(
            expected_outcomes=[
                {
                    "id": "exact_value_reported",
                    "requires": ["read_it"],
                    # The action only ever sets `known`; a fixture demanding
                    # `cooked` can never be earned by any route.
                    "state": {"known": True, "cooked": True},
                }
            ]
        )
        self.assertTrue(
            all(step["matched"] for step in oracle["steps"]), "the step still played"
        )
        self.assertEqual(0, oracle["missingActions"])
        self.assertEqual(1, oracle["missingOutcomes"], "the state was never reached")
        self.assertFalse(oracle["accepted"], "an unearned outcome must not be accepted")
        self.assertFalse(card["measured"], "an oracle that does not play is not measured")
        self.assertEqual(["numeric"], ["numeric"])

    def test_a_request_the_harness_cannot_answer_is_not_a_match(self):
        """A step that matched nothing must not be reported as matched.

        The action's `match` names arguments the schema accepts, so the oracle
        drives the real validation path. An `after` that no preceding action
        can satisfy is a precondition the harness refuses with
        `mock.precondition_failed`, which is the failure mode a scorecard that
        trusted its own fixtures would report as a played step.
        """
        report = self._scored(
            _scenario(
                expected_actions=[
                    {
                        "id": "read_it",
                        # Nothing is ever observed under this id, so the
                        # harness selects the scripted response and then
                        # refuses to run it.
                        "after": ["never_declared"],
                        "match": {
                            "tool": "sample_data",
                            "arguments": MINIMAL_SAMPLE,
                        },
                        "response": {"ok": True},
                        "effects": [{"op": "set", "path": "known", "value": True}],
                    }
                ]
            )
        )
        oracle = report["oracles"]["S01"]
        self.assertEqual("mock.precondition_failed", oracle["steps"][0]["code"])
        self.assertFalse(oracle["steps"][0]["matched"], "an unmet precondition is not a step")
        self.assertFalse(
            report["scorecards"]["numeric"]["oraclePlayed"],
            "a step that was selected but refused did not play",
        )
        self.assertFalse(report["scorecards"]["numeric"]["measured"])

    def test_a_refused_step_is_still_counted_as_missing_by_the_harness(self):
        """The session's own verdict is the cross-check on the scorecard.

        `submit_request` records the action it selected even when the step
        then fails, so a scorecard keying on the action id would report a
        refused step as played. The harness's own arithmetic still refuses the
        session -- an unobserved required action is a missing one -- and that
        independence is what makes the check below meaningful rather than a
        restatement of the same bug.
        """
        report = self._scored(
            _scenario(
                expected_actions=[
                    {
                        "id": "read_it",
                        "after": ["never_declared"],
                        "match": {"tool": "sample_data", "arguments": MINIMAL_SAMPLE},
                        "response": {"ok": True},
                        "effects": [{"op": "set", "path": "known", "value": True}],
                    }
                ]
            )
        )
        oracle = report["oracles"]["S01"]
        self.assertEqual(1, oracle["missingActions"], "the harness also says it is missing")
        self.assertFalse(oracle["accepted"])
        self.assertFalse(report["scorecards"]["numeric"]["measured"])

    def test_a_tool_the_scenario_forbids_cannot_also_be_required(self):
        """The intended route and the closed route must be different tools.

        A scenario that both required and forbade the same tool would prove
        nothing: the oracle's step would be refused by policy and the scorecard
        would have to choose which half to report. The product refuses this at
        fixture-validation time, and the scorecard inherits that refusal
        rather than producing a verdict from a contradictory scenario.
        """
        with self.assertRaises(WORKFLOW.GUIDANCE.HarnessError) as caught:
            self._scored(
                _scenario(
                    allowed_tools=["sample_data"],
                    forbidden_actions=[
                        {
                            "id": "same_tool",
                            "match": {"tool": "sample_data"},
                            "reason": "the very tool the scenario requires",
                        }
                    ],
                )
            )
        self.assertIn("S01", str(caught.exception))

    def test_a_scenario_that_forbids_nothing_is_refused_before_it_can_be_scored(self):
        """The harness will not accept a scenario with no bounded alternative.

        `validate_fixtures` requires nonempty `expected_actions`,
        `forbidden_actions` and `expected_outcomes`, because a scenario that
        bounds nothing measures nothing. That refusal is what makes the
        scorecard's `replacedRoutesProbed` count meaningful downstream: every
        routed scenario has at least one alternative to close.
        """
        self.fixtures.write_text(
            json.dumps({"version": 1, "scenarios": [_scenario(forbidden_actions=[])]}),
            encoding="utf-8",
        )
        with self.assertRaises(WORKFLOW.GUIDANCE.HarnessError) as caught:
            WORKFLOW.load_scenarios(self.fixtures)
        message = str(caught.exception)
        self.assertIn("S01", message)
        self.assertIn("nonempty", message)

    def test_an_alternative_that_is_merely_discouraged_is_not_a_refused_one(self):
        """Necessity is a separate claim, and the harness checks it separately.

        A scenario whose forbidden row names a request the harness does not
        actually close has demonstrated nothing about necessity, so
        `replacedRouteRefused` must not be reported as the strong "tested and
        closed" a reader scanning a column of trues would take it for. This is
        the realistic way necessity fails: the tool is not needed because the
        old way still works.

        The break is a forbidden matcher carrying a `$python` classifier whose
        expectation the submitted source does not meet. The matcher therefore
        selects no request, `submit_request` serves the alternative normally,
        and the probe reports `refused: false` on a scenario that LOOKS like it
        closed its alternative. A collision is not available for this test:
        `validate_fixtures` now refuses a forbidden matcher that overlaps a
        required one.
        """
        card, oracle = self._card(
            forbidden_actions=[
                {
                    "id": "an_image_is_not_a_number",
                    "match": {
                        "tool": "capture",
                        # Requires the argument to be a Python string
                        # classifying as benign; `capture` takes no `script`
                        # argument, so this matcher can never select anything.
                        "arguments": {
                            "$absent": ["arguments"],
                            "script": {"$python": {"imports": []}},
                        },
                    },
                    "reason": "a PNG shows a colour, not the exact channel value",
                }
            ]
        )
        self.assertEqual(1, card["replacedRoutesProbed"], "the probe did happen")
        self.assertEqual(
            [False], [row["refused"] for row in oracle["forbiddenAttempts"]]
        )
        self.assertFalse(
            card["replacedRouteRefused"],
            "a route that was never closed is not a closed one",
        )
        self.assertTrue(card["oraclePlayed"], "sufficiency is unaffected")
        self.assertTrue(card["measured"], "the workflow itself still plays")

    def test_a_closed_alternative_is_reported_as_probe_not_guarantee(self):
        """The count is the evidence; the boolean is a summary of it.

        A scorecard column of trues is unreadable as evidence. A reader who
        wants to know WHY a route was called closed needs the number of
        attempts behind the claim, so a workflow that closed three routes and
        one that closed one are not reported identically.
        """
        card, oracle = self._card()
        self.assertTrue(card["replacedRouteRefused"])
        self.assertEqual(
            card["replacedRoutesProbed"], len(oracle["forbiddenAttempts"])
        )
        for attempt in oracle["forbiddenAttempts"]:
            self.assertEqual("mock.policy.forbidden", attempt["code"])
            self.assertTrue(attempt["refused"], "a policy refusal is the closure")

    def test_the_two_claims_are_reported_separately(self):
        """Sufficiency and necessity are not one boolean.

        Each workflow scenario has to establish both, and a reader must be able
        to see which one failed rather than reading a single merged verdict.
        """
        report = self._scored(_scenario())
        card = report["scorecards"]["numeric"]
        for field in ("oraclePlayed", "replacedRouteRefused", "measured"):
            self.assertIn(field, card)
        self.assertTrue(card["oraclePlayed"])
        self.assertTrue(card["replacedRouteRefused"])
        self.assertTrue(card["measured"])

    def test_the_two_claims_use_separate_sessions(self):
        """One session cannot answer both questions at once.

        The harness marks a session unacceptable if it saw a forbidden event, so
        replaying the forbidden routes inside the same session would make every
        deliberately-refused probe cost the intended route its acceptance. The
        two facts would then arrive entangled: a failing alternative would look
        exactly like a failing new tool.
        """
        report = self._scored(_scenario())
        oracle = report["oracles"]["S01"]
        self.assertTrue(oracle["accepted"], "the intended route alone must accept")
        self.assertTrue(
            all(attempt["refused"] for attempt in oracle["forbiddenAttempts"]),
            "the replaced route is refused in its own session",
        )
        self.assertEqual(
            1, len(oracle["steps"]), "the intended session carries no forbidden probe"
        )

    def test_an_accepted_oracle_reports_its_score(self):
        """A score of 100 is only meaningful next to the fields that produce it."""
        report = self._scored(_scenario())
        oracle = report["oracles"]["S01"]
        self.assertEqual(100.0, oracle["score"])
        self.assertEqual(0, oracle["missingActions"])
        self.assertEqual(0, oracle["missingOutcomes"])

    def test_playing_the_oracle_leaves_no_trace_behind(self):
        """A measurement that litters the working tree is not a measurement.

        `run_oracle` writes real session traces through `guidance_eval`, so the
        temp directory has to come back empty. A leftover trace would make the
        next run read a stale one.
        """
        self._scored(_scenario())
        traces = [
            path.name
            for path in self.tmp.iterdir()
            if path.suffix == ".json" and path.name != "fixtures.json"
        ]
        self.assertEqual([], traces, "an oracle replay must leave no session trace")


class TheRealScenariosLoadCleanly(unittest.TestCase):
    def test_every_shipped_scenario_has_an_oracle(self):
        """Scoring is only possible because the fixtures declare ground truth."""
        for scenario in WORKFLOW.load_scenarios():
            with self.subTest(scenario=scenario["id"]):
                self.assertTrue(scenario["expected_actions"])
                self.assertTrue(scenario["expected_outcomes"])

    def test_only_the_new_workflow_scenarios_carry_a_workflow(self):
        """A `workflow` on an old scenario would claim a scorecard it lacks.

        The 33 pre-existing scenarios measure guidance behaviour, not a new
        tool. Tagging one `numeric` would put its schema cost and its oracle
        into the numeric scorecard, and the harness would then credit
        `sample_data` for a task it never appears in.
        """
        for scenario in WORKFLOW.load_scenarios():
            with self.subTest(scenario=scenario["id"]):
                declared = scenario.get("workflow")
                if declared is None:
                    continue
                self.assertIn(
                    WORKFLOW.reached_route(scenario),
                    WORKFLOW.NEW_TOOLS,
                    f"{scenario['id']} declares {declared} but requires no new tool",
                )

    def test_every_workflow_scenario_forbids_the_workflow_it_replaces(self):
        """A new tool with no stated alternative is not a preference.

        Each scenario declares a forbidden action naming the route a caller
        would otherwise reach for. Without it, the scenario cannot tell
        `sample_data` apart from a slower path to the same answer, which is
        the whole question the scorecard exists to ask.
        """
        replacements = {
            "sample_data": "capture",
            "script_source": "execute_python",
            "param_interface": "mutate_nodes",
            "project_snapshot": "fleet",
        }
        for scenario in WORKFLOW.load_scenarios():
            tool = WORKFLOW.reached_route(scenario)
            if tool is None:
                continue
            with self.subTest(scenario=scenario["id"]):
                forbidden = {
                    row["match"]["tool"] for row in scenario["forbidden_actions"]
                }
                self.assertIn(replacements[tool], forbidden)
                self.assertIn(tool, scenario["allowed_tools"])
                reasons = [row.get("reason", "") for row in scenario["forbidden_actions"]]
                self.assertTrue(
                    any(len(reason) > 40 for reason in reasons),
                    "a forbidden action must say WHY the route is wrong",
                )


if __name__ == "__main__":
    unittest.main()
