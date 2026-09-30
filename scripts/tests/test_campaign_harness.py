#!/usr/bin/env python3
"""Tests for the campaign runner itself.

A campaign harness that cannot fail is worse than no harness: it reports
coverage nobody earned. These tests are about the runner, not the product --
each one feeds a payload the runner should reject and asserts the rejection
happens for the RIGHT reason, which is the same discipline the product's own
suite follows.

Run: .venv/bin/python -m pytest scripts/tests/test_campaign_harness.py -q
"""
from __future__ import annotations

import json
import sys
from unittest import mock
from pathlib import Path

import pytest

REPO = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(REPO / "scripts" / "campaign"))
sys.path.insert(0, str(REPO / "scripts"))

import scenario  # noqa: E402
from scenario import (  # noqa: E402
    Scenario,
    Step,
    _dig,
    envelope_ok,
    is_missing,
    run_scenario,
)


def _scenario_dict(sc: "scenario.Scenario") -> dict:
    """Serialise a Scenario the way its own JSON file does.

    Written out here rather than borrowed from the loader, because the loader
    only goes one way. `Step` keeps the assertions under `assert_` because
    `assert` is a statement, so the JSON key has to be restored by hand.
    """
    return {
        "id": sc.id,
        "title": sc.title,
        "tools": sc.tools,
        "description": sc.description,
        "needs_td": sc.needs_td,
        "steps": [
            {
                "name": s.name,
                "tool": s.tool,
                "args": s.args,
                "expect_refusal": s.expect_refusal,
                "needs_pid": s.needs_pid,
                "timeout_s": s.timeout_s,
                "capture": s.capture,
                "assert": s.assert_,
            }
            for s in sc.steps
        ],
    }


ENVELOPED = {"items": [{"ok": True, "result": {"value": 1}}], "ok": True}
BARE = {"ok": True, "value": 1}


class _Recorder:
    """A fake transport that returns prepared payloads and counts calls."""

    def __init__(self, payloads: dict):
        self.payloads = payloads
        self.calls: list[tuple[str, dict]] = []

    def __call__(self, tool: str, args: dict, timeout_s: int = 120) -> dict:
        self.calls.append((tool, args))
        if tool not in self.payloads:
            raise AssertionError(f"unexpected tool call: {tool}")
        payload = self.payloads[tool]
        if callable(payload):
            return payload(tool, args)
        return payload


# --- path resolution --------------------------------------------------------


def test_a_dotted_path_walks_nested_objects():
    assert _dig({"a": {"b": {"c": 7}}}, "a.b.c") == 7


def test_a_dotted_path_indexes_lists_numerically():
    assert _dig({"items": [{"ok": True}]}, "items.0.ok") is True


def test_an_absent_path_is_distinguishable_from_a_null_one():
    """The runner must be able to TELL these apart, or it cannot test the claim.

    A harness that flattened `missing` into `None` would pass every scenario
    that asserts a null and fail every scenario that asserts an absence --
    which is exactly backwards for the product's central claim.
    """
    assert is_missing(_dig({"a": 1}, "nope"))
    assert not is_missing(_dig({"a": None}, "a"))
    assert _dig({"a": None}, "a") is None


def test_a_path_through_a_scalar_is_absent_not_an_error():
    assert is_missing(_dig({"a": 5}, "a.b.c"))


# --- envelope handling ------------------------------------------------------


def test_envelope_ok_reads_the_wrapped_shape():
    assert envelope_ok(ENVELOPED) is True


def test_envelope_ok_reads_the_bare_shape_compare_nodes_uses():
    assert envelope_ok(BARE) is True


def test_envelope_ok_reports_a_refusal_in_either_shape():
    assert envelope_ok({"ok": False, "summary": "no"}) is False
    assert envelope_ok({"items": [{"ok": False}], "ok": False}) is False


def test_envelope_ok_is_absent_rather_than_false_when_there_is_no_ok():
    """A reply with no `ok` at all is NOT `ok:false`.

    The product keeps those states distinct, so the runner refuses to invent
    one: a missing verdict reports as absent and lets the caller decide.
    """
    assert is_missing(envelope_ok({"value": 1}))


# --- predicates -------------------------------------------------------------


def test_a_correct_payload_passes_ok():
    passed, observed = scenario.PREDICATES["ok"](ENVELOPED, {"path": "ok"})
    assert passed
    assert "ok=true" in observed


def test_a_false_payload_fails_ok():
    passed, observed = scenario.PREDICATES["ok"]({"ok": False}, {"path": "ok"})
    assert not passed
    assert "false" in observed


def test_path_equals_fails_on_absent_and_says_so():
    passed, observed = scenario.PREDICATES["path_equals"]({"a": 1}, {"path": "b", "equals": 1})
    assert not passed
    assert "<absent>" in observed


def test_path_absent_passes_only_when_really_absent():
    assert scenario.PREDICATES["path_absent"]({"a": None}, {"path": "b"})[0]
    assert not scenario.PREDICATES["path_absent"]({"a": None}, {"path": "a"})[0]


def test_distinct_catches_a_collapsed_state():
    """A product that returned `null` for both states fails here and nowhere else."""
    collapsed = {"missing": None, "value": None}
    assert not scenario.PREDICATES["distinct"](collapsed, {"left": "missing", "right": "value"})[0]
    honest = {"missing": None, "value": 0}
    assert scenario.PREDICATES["distinct"](honest, {"left": "missing", "right": "value"})[0]


def test_distinct_accepts_both_absent():
    assert scenario.PREDICATES["distinct"]({"a": 1}, {"left": "x", "right": "y"})[0]


def test_step_refused_reads_the_per_step_code_not_the_wrapper():
    """The item code is the generic wrapper; only steps[i].code names the cause."""
    payload = {
        "items": [{"ok": False, "code": "tdmcp.mutate.step_failed"}],
        "ok": False,
        "steps": [{"ok": False, "code": "tdmcp.mutate.precondition_failed"}],
    }
    passed, observed = scenario.PREDICATES["step_refused"](payload, {"contains": "precondition"})
    assert passed
    assert "precondition" in observed
    # And the wrong read would have produced the wrong answer:
    assert not scenario.PREDICATES["step_refused"](
        {"items": [{"ok": False, "code": "tdmcp.mutate.step_failed"}], "ok": False},
        {"contains": "precondition"},
    )[0]


def test_regex_on_an_absent_path_fails_rather_than_matching_empty():
    passed, observed = scenario.PREDICATES["regex"]({"a": 1}, {"path": "b", "pattern": "x"})
    assert not passed
    assert "<absent>" in observed


# --- step construction ------------------------------------------------------


def test_an_unknown_predicate_is_rejected_at_load_time():
    """A typo in a scenario must not become a silently-skipped assertion."""
    with pytest.raises(ValueError, match="unknown predicate"):
        Step.from_dict({"name": "x", "tool": "fleet", "assert": [{"predicate": "no_such_thing"}]})


# --- runner behaviour -------------------------------------------------------


def test_a_scenario_reports_the_observed_value_when_it_fails():
    rec = _Recorder({"fleet": {"ok": True, "processes": []}})
    scen = Scenario(
        id="t",
        title="t",
        tools=["fleet"],
        needs_td=False,
        steps=[Step(name="needs-a-row", tool="fleet", assert_=[
            {"predicate": "path_contains", "path": "processes", "value": "9999"}])],
    )
    result = run_scenario(scen, rec, ctx={"pid": None})
    assert not result.passed
    assert any("9999" in f for f in result.findings)


def test_a_step_that_needs_a_pid_without_one_is_inconclusive_not_a_defect():
    """A missing prerequisite is an unrun test, and calling it a defect is a lie."""
    rec = _Recorder({})
    scen = Scenario(id="t", title="t", tools=["inspect"], needs_td=False,
                    steps=[Step(name="needs-pid", tool="inspect", needs_pid=True)])
    result = run_scenario(scen, rec, ctx={"pid": None})
    assert not result.passed
    assert result.verdict == "inconclusive"
    assert rec.calls == [], "the tool must not have been called at all"


def test_a_transport_error_is_inconclusive_not_a_defect():
    def boom(tool, args):
        raise TimeoutError("bridge heartbeat pong timeout")

    rec = _Recorder({"fleet": boom})
    scen = Scenario(id="t", title="t", tools=["fleet"], needs_td=False,
                    steps=[Step(name="read", tool="fleet", assert_=[{"predicate": "ok"}])])
    result = run_scenario(scen, rec, ctx={"pid": None})
    assert not result.passed
    assert result.verdict == "inconclusive"


def test_an_expected_refusal_that_does_not_happen_is_a_failure():
    """A guard that let the write through must be reported, not passed.

    This is the assertion that would catch the most serious class of defect
    the campaign exists to look for, so its own failure mode is tested: an
    `expect_refusal` step that receives `ok:true` has to be a FAIL.
    """
    rec = _Recorder({"mutate_nodes": {"ok": True, "summary": "wrote 99"}})
    scen = Scenario(id="t", title="t", tools=["mutate_nodes"], needs_td=False,
                    steps=[Step(name="guard", tool="mutate_nodes", expect_refusal=True,
                                assert_=[{"predicate": "not_ok", "path": "ok"}])])
    result = run_scenario(scen, rec, ctx={"pid": None})
    assert not result.passed, "an unguarded write reported as a refused guard"
    assert result.verdict == "defect"


def test_an_expected_refusal_that_happens_passes():
    rec = _Recorder({"mutate_nodes": {"ok": False, "summary": "precondition failed"}})
    scen = Scenario(id="t", title="t", tools=["mutate_nodes"], needs_td=False,
                    steps=[Step(name="guard", tool="mutate_nodes", expect_refusal=True,
                                assert_=[{"predicate": "not_ok", "path": "ok"}])])
    result = run_scenario(scen, rec, ctx={"pid": None})
    assert result.passed


def test_a_passing_scenario_keeps_every_observed_value_for_the_record():
    rec = _Recorder({"fleet": {"ok": True, "processes": [{"pid": 1}]}})
    scen = Scenario(id="t", title="t", tools=["fleet"], needs_td=False,
                    steps=[Step(name="read", tool="fleet", assert_=[{"predicate": "ok"}])])
    result = run_scenario(scen, rec, ctx={"pid": None})
    assert result.passed
    assert result.results[0].observed.startswith("ok=true")


def test_one_failing_step_does_not_hide_the_steps_after_it():
    """A scenario that stopped at the first failure would hide the rest of the run."""
    rec = _Recorder({"fleet": {"ok": True, "processes": []}})
    scen = Scenario(
        id="t", title="t", tools=["fleet"], needs_td=False,
        steps=[
            Step(name="first-fails", tool="fleet", assert_=[{"predicate": "not_ok", "path": "ok"}]),
            Step(name="second-runs", tool="fleet", assert_=[{"predicate": "ok", "path": "ok"}]),
        ],
    )
    result = run_scenario(scen, rec, ctx={"pid": None})
    assert len(result.results) == 2
    assert not result.results[0].passed and result.results[1].passed


def test_placeholders_resolve_to_values_and_keep_their_type():
    rec = _Recorder({"inspect": {"ok": True}})
    scen = Scenario(id="t", title="t", tools=["inspect"], needs_td=False,
                    steps=[Step(name="read", tool="inspect",
                                args={"pid": "{{pid}}", "paths": ["/p", "{{pid}}"]})])
    run_scenario(scen, rec, ctx={"pid": 4321})
    tool, args = rec.calls[0]
    assert args["pid"] == 4321 and isinstance(args["pid"], int)
    assert args["paths"] == ["/p", 4321]


def test_an_unbound_placeholder_is_left_visible_rather_than_blanked():
    """`{{typo}}` must not silently become an empty string the tool accepts."""
    rec = _Recorder({"inspect": {"ok": True}})
    scen = Scenario(id="t", title="t", tools=["inspect"], needs_td=False,
                    steps=[Step(name="read", tool="inspect", args={"pid": "{{nope}}"})])
    run_scenario(scen, rec, ctx={"pid": 1})
    assert rec.calls[0][1]["pid"] == "{{nope}}"


# --- coverage map -----------------------------------------------------------


def test_coverage_refuses_to_call_an_unmapped_tool_complete():
    import coverage as cov

    report = cov.build(["alpha", "beta"], [Scenario(id="s", title="s", tools=["alpha"], steps=[], needs_td=False)])
    assert not report["complete"]
    assert report["unmapped"] == ["beta"]


def test_coverage_reports_an_unknown_roster_rather_than_zero_unmapped():
    import coverage as cov

    assert cov.live_tool_names(lambda t, a: (_ for _ in ()).throw(OSError("down"))) is None


def test_coverage_flags_a_scenario_naming_a_tool_the_daemon_does_not_serve():
    import coverage as cov

    report = cov.build(["alpha"], [Scenario(id="s", title="s", tools=["ghost"], steps=[], needs_td=False)])
    assert report["unknown_tools"] == ["ghost"]


# ---------------------------------------------------------------------------
# Teardown: a campaign that leaks cannot be read as clean
# ---------------------------------------------------------------------------


def test_a_leaked_touch_designer_is_named_rather_than_reported_as_clean():
    """`stop_td` must keep a pid it could not kill, so the leftover check sees it.

    The failure this pins is silent: a session that forgets its pid on the
    first attempt reports 'clean' while a TouchDesigner it started keeps
    running, and the next round then measures the previous round's debris.
    """
    import harness

    session = harness.Session(data_dir=Path("/nonexistent"), run_dir=Path("/nonexistent"))
    session.td_pid = 4242
    session._extra_td_pids = [4243]
    # A pid that is not a live process: _terminate_td returns without doing
    # anything, and /proc says it is gone, so both are forgotten.
    session.stop_td()
    assert session.td_pid is None
    assert session._extra_td_pids == []


def test_a_touch_designer_that_survives_stays_owned_so_the_report_names_it():
    """A pid that is still there after termination keeps its place in the session.

    Driven through the observable the leftover report uses -- the session's own
    pid lists -- rather than by mocking the kill away. `stop_td` must forget a
    pid only once /proc says it is gone, and must NOT forget one still alive, or
    the leftover report cannot name what was left running.
    """
    import harness

    session = harness.Session(data_dir=Path("/nonexistent"), run_dir=Path("/nonexistent"))
    live = __import__("os").getpid()
    session.td_pid = live
    real = harness._terminate_td
    killed: list[int] = []

    def _kill(pid: int) -> None:
        # A TouchDesigner that refuses to die: the termination path is
        # exercised, this process is NOT signalled (it is not TouchDesigner),
        # and /proc still reports it present afterwards.
        killed.append(pid)

    harness._terminate_td = _kill
    try:
        session.stop_td()
    finally:
        harness._terminate_td = real
    assert killed == [live]
    assert session.td_pid == live, "a process still in /proc must stay owned"
    assert session._extra_td_pids == []


def test_the_runner_downgrades_a_verified_verdict_when_it_leaked_a_session():
    """A run that verified every scenario but leaked a TD is not `verified`.

    The exit code and the printed verdict are the campaign's only output, so
    a leak that leaves them saying 'verified' teaches the next round that the
    host was clean. It must be a distinct, visible state.
    """
    src = (REPO / "scripts" / "campaign" / "run.py").read_text(encoding="utf-8")
    assert "leaked_td" in src
    assert "the run cannot be read as clean" in src
    # And the leak must reach the exit code, not just the message.
    assert "or leaked_td" in src


def test_the_leftover_report_names_a_survivor_that_is_not_the_session_pid():
    """A second adopted TouchDesigner that survives must still be reported.

    The report read only `td_pid`, so a run that adopted two instances and
    failed to kill the second one reported 'clean' with a live process behind
    it -- the silent-false-clean class this harness exists to avoid.
    """
    import harness

    session = harness.Session(data_dir=Path("/nonexistent"), run_dir=Path("/nonexistent"))
    live = __import__("os").getpid()
    session.td_pid = None
    session._extra_td_pids = [live]
    report = session.teardown()
    assert f"td:{live}" in report["leftovers"]


def test_every_usage_example_in_the_runner_docstring_parses():
    """A usage example that does not parse is a false promise.

    The runner's own docstring is how a person drives the campaign, so an
    example there that no longer matches the parser is a documented way to
    fail. Checked against the REAL parser, extracted as `build_parser`, and
    a wrong script name fails this too -- that was the first state of this
    test's own predecessors.
    """
    import contextlib
    import re

    import run as runner

    parser = runner.build_parser()
    examples = re.findall(
        r"\.venv/bin/python scripts/campaign/run\.py([^\n`]*)", runner.__doc__ or ""
    )
    assert examples, "the runner docstring should show how to drive it"
    for argv_text in examples:
        argv = argv_text.split()
        with contextlib.suppress(SystemExit):  # not reached; parse must succeed
            pass
        # A failing parse raises SystemExit, so let it propagate to the assert.
        ns = parser.parse_args(argv)
        assert ns is not None


def test_the_runner_refuses_to_start_on_a_malformed_scenario():
    """The gate in the RUNNER, not just the validator, is what keeps a bad run honest.

    `validate_scenarios` has its own tests, and those all passed while the
    runner's call to it was still dead code -- a mutant that emptied the
    runner's result went 43/43 green. The defect this gate prevents costs a
    whole live run: a malformed step fails every assertion after it, and the
    record then reads as many product defects with one cause.

    Driven through `main()` rather than by re-stating the rule, so the test
    cannot be satisfied by a local copy of the check it is meant to pin.
    """
    import copy
    import tempfile

    import run as runner
    import scenario as sc

    scenarios = sc.load_scenarios(REPO / "scripts" / "campaign" / "scenarios")
    broken = copy.deepcopy(scenarios)
    mutated = False
    for sc_obj in broken:
        for step in sc_obj.steps:
            if step.tool != "mutate_nodes":
                continue
            for sub in step.args.get("steps", []):
                if sub.get("op") == "create":
                    sub["no_such_field"] = "x"
                    mutated = True
                    break
            if mutated:
                break
        if mutated:
            break
    assert mutated, "no create step to mutate -- the check would be vacuous"

    with tempfile.TemporaryDirectory() as tmp:
        record_root = Path(tmp) / "records"
        scenario_dir = Path(tmp) / "scenarios"
        scenario_dir.mkdir(parents=True)
        for sc_obj in broken:
            (scenario_dir / f"{sc_obj.id}.json").write_text(
                json.dumps(_scenario_dict(sc_obj)), encoding="utf-8"
            )

        original_scenario_dir = runner.SCENARIO_DIR
        original_record_root = runner.RECORD_ROOT

        class _Exploding:
            """Fails loudly if the runner gets as far as starting a daemon.

            The gate must answer BEFORE anything live happens, so reaching this
            is a failure of the gate, not a setup problem in the test. Without
            it, a mutant that drops the gate costs a real daemon inside the
            unit suite -- 25 seconds of the campaign host to prove a boolean.
            """

            def __getattr__(self, name):  # noqa: ANN001
                raise AssertionError(
                    f"the runner reached Session.{name} -- the scenario gate let a "
                    "malformed scenario through into a live run"
                )

        try:
            runner.SCENARIO_DIR = scenario_dir
            runner.RECORD_ROOT = record_root
            # A preflight refusal would mask the gate, so this asks for the
            # one thing the gate must answer before anything live happens.
            with mock.patch.object(runner, "Session", _Exploding):
                code = runner.main(
                    [
                        "--scenario",
                        broken[0].id,
                        "--no-preflight",
                        "--run-id",
                        "gate-check",
                    ],
                )
        finally:
            runner.SCENARIO_DIR = original_scenario_dir
            runner.RECORD_ROOT = original_record_root

    # 2 is INCONCLUSIVE, not the 0 of a clean run and not the 1 of a defect:
    # the runner declined to produce a verdict because its own data was wrong.
    assert code == 2, f"a malformed scenario should stop the runner, got exit {code}"


def test_the_shipped_scenario_files_all_load_and_declare_their_tools():
    """A scenario that cannot be loaded is invisible coverage, not a finding.

    Scenarios are data, and data drifts: a renamed id, a predicate that no
    longer exists, a tool the daemon does not serve. The coverage map reports
    unmapped tools, but a scenario that FAILS TO LOAD is not even a map
    entry, so it is checked here where it cannot be missed.
    """
    import coverage as cov
    from scenario import load_scenarios

    scenario_dir = REPO / "scripts" / "campaign" / "scenarios"
    scenarios = load_scenarios(scenario_dir)
    assert scenarios, f"no scenarios loaded from {scenario_dir}"
    for sc in scenarios:
        assert sc.id and sc.title, f"a scenario in {scenario_dir} is missing id/title"
        assert sc.tools, f"{sc.id} names no tools, so it would map to nothing"
        assert sc.steps, f"{sc.id} has no steps, so it asserts nothing"
    # And the roster they claim must at least be self-consistent.
    tools = sorted({t for sc in scenarios for t in sc.tools})
    report = cov.build(tools, scenarios)
    assert not report["unknown_tools"], f"scenarios name tools outside the roster: {report['unknown_tools']}"


def test_a_bare_spawn_is_reported_as_no_bridge_not_as_a_failed_connection():
    import harness

    """A blank project has no bridge, so waiting for a handshake cannot help.

    This is the harness bug the campaign hit: `spawn_td {}` opens a project
    with no bridge DATs, and the ledger records at round 30 that such a spawn
    "correctly never handshakes". A harness that waits the full budget for a
    handshake that is structurally impossible learns nothing and reports a
    lifecycle failure that never happened.
    """
    calls = []

    class FakeSession:
        def call(self, tool, args, timeout_s=120):
            calls.append((tool, args, timeout_s))
            if tool == "spawn_td":
                return {"ok": True, "pid": 4321, "handshake": {"title": "blank.toe"}}
            return {"processes": [{"pid": 4321, "bridge": "starting"}]}

    session = FakeSession()
    pid = harness.Session.spawn_td(session, timeout_s=180.0)
    assert pid == 4321
    assert calls == [("spawn_td", {}, 180.0)], (
        "a bare spawn must be a single call: waiting for a handshake that a "
        f"blank project cannot produce is the bug, not the fix. calls={calls}"
    )
    assert session.td_pid == 4321


def test_a_spawn_with_a_project_waits_for_the_bridge_and_reports_its_row():
    import harness

    """With a project that carries the bridge, the wait is the whole point."""
    states = iter(["starting", "starting", "connected", "connected"])
    calls = []

    class FakeSession:
        def call(self, tool, args, timeout_s=120):
            calls.append((tool, args))
            if tool == "spawn_td":
                return {"ok": True, "pid": 99, "handshake": {"title": "probe.toe"}}
            return {"processes": [{"pid": 99, "bridge": next(states)}]}

    session = FakeSession()
    pid = harness.Session.spawn_td(session, timeout_s=30.0, project_path="/tmp/probe.toe")
    assert pid == 99
    assert calls[0] == ("spawn_td", {"projectPath": "/tmp/probe.toe"}), (
        f"the project must be handed to spawn_td, not guessed: {calls[0]}"
    )
    fleet_calls = [c for c in calls if c[0] == "fleet"]
    assert len(fleet_calls) >= 3, (
        f"the wait must poll until the bridge reports connected, not stop early: {calls}"
    )


def test_a_fault_run_that_never_ran_a_scenario_did_not_catch_the_fault():
    """A setup failure is INCONCLUSIVE, never a detection.

    `not all_verified` is the tempting verdict for a fault-injection run and it
    is vacuous: a scenario whose setup failed never passed, so the shortcut
    reports "the injected fault was caught" for a run in which the fault was
    never on screen. The pilot measured exactly that. The decision is exercised
    through the runner's own `fault_verdict`, not re-stated here -- asserting
    the properties of a local copy is how the previous version of this test
    survived the mutant it was written for.
    """
    import run as runner

    def res(verdict, *, passed, results=("step",), sid="s"):
        return runner.ScenarioResult(
            id=sid, title="t", passed=passed, verdict=verdict, results=list(results)
        )

    detected = runner.fault_verdict([res("defect", passed=False, results=("a", "b"))])
    assert detected[0].startswith("verified"), detected

    # The exact pilot shape: setup failed, no step ever ran.
    setup_failed = [res("inconclusive", passed=False, results=(), sid="guarded")]
    assert runner.fault_verdict(setup_failed)[0] == "HARNESS FAILS TO DETECT"

    # ...and the reason must say the fault was never exercised, because a
    # reader deciding what to do next needs to know which of the two happened.
    assert "never exercised" in runner.fault_verdict(setup_failed)[1]

    # A scenario that ran and was inconclusive is still not a detection.
    ran_but_inconclusive = [res("inconclusive", passed=False, results=("a",))]
    verdict, line = runner.fault_verdict(ran_but_inconclusive)
    assert verdict == "HARNESS FAILS TO DETECT"
    assert "no scenario reported a defect verdict" in line

    # A clean run is not a detection either: the injector failed to inject.
    assert runner.fault_verdict([res("verified", passed=True)])[0] == "HARNESS FAILS TO DETECT"

    # No scenarios at all is the worst case, and must not read as caught.
    assert runner.fault_verdict([])[0] == "HARNESS FAILS TO DETECT"


def test_every_scenario_step_is_valid_against_the_shipped_tool_schemas():
    """A malformed step fails every assertion after it, which reads as nine
    product defects instead of one scenario-data defect.

    The pilot put a `comment` field on a `mutate_nodes` DELETE step, which the
    daemon rejects as an unknown field. The create that step guards never
    happened, so every later assertion reported "operator not found" for a node
    that had never been created. Checking the arguments here costs a unit test
    instead of a campaign run.
    """
    import scenario as sc

    scenarios = sc.load_scenarios(REPO / "scripts" / "campaign" / "scenarios")
    problems = sc.validate_scenarios(scenarios)
    assert problems == {}, f"scenario arguments do not match the shipped schemas: {problems}"


def test_the_schema_validator_can_actually_fail():
    """A validator that accepts everything is an assertion of nothing."""
    import copy

    import scenario as sc

    scenarios = sc.load_scenarios(REPO / "scripts" / "campaign" / "scenarios")
    assert sc.validate_scenarios(scenarios) == {}

    # The exact defect the pilot hit: a field that exists on `create` but not
    # on `delete`.
    broken = copy.deepcopy(scenarios)
    mutated = False
    for sc_obj in broken:
        for step in sc_obj.steps:
            if step.tool != "mutate_nodes":
                continue
            for sub in step.args.get("steps", []):
                if sub.get("op") == "delete":
                    sub["comment"] = "not a field on delete"
                    mutated = True
                    break
            if mutated:
                break
        if mutated:
            break
    assert mutated, "no delete step to mutate -- the check would be vacuous"
    problems = sc.validate_scenarios(broken)
    assert problems, "the schema validator accepted a field the tool rejects"


def test_an_unvalidatable_step_is_reported_rather_than_silently_skipped():
    """A tool with no checked-in schema is a fact the reader needs.

    Silence here would be a step that looks validated and was not, which is the
    same shape of lie as a scenario that fails to load being invisible.
    """
    import scenario as sc

    assert sc.tool_schema("no_such_tool") is None
    step = sc.Step(name="s", tool="no_such_tool", args={}, assert_=[])
    assert sc.validate_step_arguments(step) == [], "no schema means no claim, not a pass"

    # A tool that DOES have a schema, given a field it does not have.
    real = sc.Step(
        name="s",
        tool="inspect",
        args={"paths": ["/project1/x"], "not_a_field": 1},
        assert_=[],
    )
    problems = sc.validate_step_arguments(real)
    assert problems, "a real schema must reject an unknown field"


def test_no_predicate_crashes_when_the_value_it_reads_is_absent():
    """Absence is a value the product reports, so every predicate must render it.

    The pilot aborted with `TypeError: Object of type _Missing is not JSON
    serializable`: a predicate formatted an absent field with `json.dumps`, and
    the sentinel that represents "this path is not in the payload" has no JSON
    form. The crash surfaced as "scenario setup failed", which reads as a
    product failure when it is a harness one -- and it hides every later
    assertion, because the scenario stops there.

    Each predicate is exercised with ITS OWN well-formed spec, because a spec
    missing the key a predicate reads is a different failure (a malformed
    scenario) and conflating the two would test the wrong thing.
    """
    import scenario as sc

    payload = {"ok": True, "items": [{"ok": True, "result": {"a": 1}}], "summary": "s", "steps": []}
    absent = {
        "ok": {"path": "nope.deep"},
        "not_ok": {"path": "nope.deep"},
        "envelope_ok": {},
        "step_refused": {"index": 0},
        "path_equals": {"path": "nope.deep", "equals": 1},
        "path_contains": {"path": "nope.deep", "value": "x"},
        "path_present": {"path": "nope.deep"},
        "path_absent": {"path": "nope.deep"},
        "all_true": {"paths": ["nope.deep"]},
        "distinct": {"left": "nope.deep", "right": "also.nope"},
        "regex": {"path": "nope.deep", "pattern": "x"},
    }
    assert set(absent) == set(sc.PREDICATES), (
        f"the roster changed: predicates={sorted(sc.PREDICATES)} covered={sorted(absent)}"
    )
    for name, predicate in sc.PREDICATES.items():
        passed, text = predicate(payload, absent[name])
        assert isinstance(passed, bool), f"{name} returned {passed!r}"
        assert isinstance(text, str), f"{name} returned non-text: {text!r}"
    assert sc._show(sc._MISSING) == "<absent>"


def test_a_crash_inside_a_predicate_is_reported_not_silently_passed():
    """A predicate that raises must not be read as a passing assertion."""
    import scenario as sc

    def exploding(payload, spec):
        raise RuntimeError("predicate bug")

    original = dict(sc.PREDICATES)
    try:
        sc.PREDICATES["ok"] = exploding
        sc.PREDICATES["not_ok"] = exploding
        sc.PREDICATES["envelope_ok"] = exploding
        sc.PREDICATES["path_equals"] = exploding
        sc.PREDICATES["path_contains"] = exploding
        sc.PREDICATES["path_present"] = exploding
        sc.PREDICATES["path_absent"] = exploding
        sc.PREDICATES["all_true"] = exploding
        sc.PREDICATES["distinct"] = exploding
        sc.PREDICATES["regex"] = exploding
        sc.PREDICATES["step_refused"] = exploding

        sc_obj = sc.Scenario(
            id="s",
            title="t",
            tools=["inspect"],
            steps=[sc.Step(name="s1", tool="inspect", args={}, assert_=[{"predicate": "ok"}])],
        )
        result = sc.run_scenario(sc_obj, lambda tool, args, timeout_s=1: {"ok": True})
        assert result.passed is False, "a crashing predicate read as a pass"
        assert any("predicate bug" in f or "predicate" in f.lower() for f in result.findings), (
            f"the crash must be named in the findings, not swallowed: {result.findings}"
        )
    finally:
        sc.PREDICATES.clear()
        sc.PREDICATES.update(original)
