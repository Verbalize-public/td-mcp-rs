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

import sys
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
