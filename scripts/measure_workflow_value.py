#!/usr/bin/env python3
"""Score the NEW TOOLS on their own, apart from encoding gains.

`measure_tokens.py` measures what the compact view saves. `measure_decision_fidelity.py`
asks whether it still answers the same questions. Neither says anything about
`sample_data`, `script_source`, `param_interface` or `project_snapshot`. Those
tools ship (`ToolName::SampleData` in crates/tdmcp-mcp/src/tools.rs, handlers in
bridge/tdmcp_bridge/), so an agent COULD have used them while the encoding work
was measured -- which is exactly why reporting their cost next to an encoding
saving would attribute one project's gain to another. The conflation the release
plan's gate 4 exists to prevent is not hypothetical.

The four workflows are scored SEPARATELY, and each scenario declares which one
it exercises. Two facts make the separation checkable rather than a claim:

* The tool set a scenario allows and the actions it requires are both
  declared, so a scenario that reaches for a numeric sample through
  `execute_python` instead of `sample_data` is a MISS, not a tie. The harness
  has a deterministic response for `sample_data`; using another route is not
  the same answer by another name.
* The answer to "is this the right tool" is ground truth, not a judgement. It
  comes from the scenario's own `workflow` field and from the recorded
  outcome, so there is nothing to score against a model.

`measured` is the one field that must not be taken on trust, so it is not read
off the fixture. `run_oracle` actually plays each workflow's scenario through
`guidance_eval` and asks the two questions the gate cares about, in two
separate sessions so neither contaminates the other: is the task ACCEPTED using
the new tool, and is the route the tool replaces actually REFUSED. A workflow
whose oracle does not play is reported as unmeasured.

The cost side is where this earns its keep. A new tool is not free: its schema
is in every session that allows it, whether or not the session uses it. The
harness already records `toolSchemaReads` / `toolSchemaChars` / `toolSchemaBytes`
per session, so adding one tool to a scenario's allowed set has a measurable
price, and that price is reported per workflow rather than in a single total
that would hide which tool is expensive.

WHAT THIS IS NOT. This is a fixture harness. It measures what a DETERMINISTIC
backend answers, and it counts the schema bytes a session exposes. It does not
run a model, does not connect to TouchDesigner, and cannot show that any agent
would prefer `sample_data` over `execute_python` in the wild. What it can show,
and what the release gate needs, is that each new workflow has a scenario whose
oracle is executable and accepted, that the route it replaces is refused rather
than merely discouraged, and that the schema cost of carrying the tool is stated
rather than assumed.
"""
from __future__ import annotations

import argparse
import copy
import importlib.util
import json
import sys
import tempfile
from pathlib import Path
from typing import Any


ROOT = Path(__file__).resolve().parents[1]


def _load_guidance_eval() -> Any:
    """Import `guidance_eval.py` by path.

    It is a sibling script rather than a package module, and importing it
    normally would execute its CLI wiring. Loading it by path keeps this
    script honest about the fact that it is a caller, not a second harness.
    """
    path = ROOT / "scripts/guidance_eval.py"
    spec = importlib.util.spec_from_file_location("guidance_eval", path)
    if spec is None or spec.loader is None:
        raise WorkflowValueError("cannot load scripts/guidance_eval.py")
    module = importlib.util.module_from_spec(spec)
    # A module must be registered before it executes: anything that resolves
    # annotations through `sys.modules[cls.__module__]` (a dataclass, a
    # forward reference) fails at class-definition time otherwise, which reads
    # as a dataclass bug rather than a loader one.
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


GUIDANCE = _load_guidance_eval()

# The four tools gate 4 names, and the workflow each one exists to serve.
# A tool with no workflow of its own would be measured as overhead with no
# benefit to offset it, which is the shape of a tool that should not ship.
NEW_TOOLS: dict[str, str] = {
    "sample_data": "numeric",
    "script_source": "source",
    "param_interface": "interface",
    "project_snapshot": "checkpoint",
}

WORKFLOWS = tuple(sorted(set(NEW_TOOLS.values())))


class WorkflowValueError(ValueError):
    """A safe, user-facing refusal."""


def load_scenarios(path: Path | None = None) -> list[dict[str, Any]]:
    fixtures = GUIDANCE.load_fixtures(path or GUIDANCE.DEFAULT_FIXTURES)
    scenarios = []
    for raw in fixtures["scenarios"]:
        scenario, _identity, _variant = GUIDANCE.resolve_scenario(
            fixtures, raw["id"], 918_273
        )
        scenarios.append(scenario)
    return scenarios


def workflow_of(scenario: dict[str, Any]) -> str | None:
    declared = scenario.get("workflow")
    if declared is None:
        return None
    if declared not in WORKFLOWS:
        raise WorkflowValueError(
            f"{scenario['id']}: workflow {declared!r} is not one of {list(WORKFLOWS)}"
        )
    return declared


def required_tools(scenario: dict[str, Any]) -> list[str]:
    return [
        row["match"]["tool"]
        for row in scenario["expected_actions"]
        if row.get("required", True)
    ]


def reached_route(scenario: dict[str, Any]) -> str | None:
    """The new tool the scenario's required actions actually use, if any."""
    used = [tool for tool in required_tools(scenario) if tool in NEW_TOOLS]
    if not used:
        return None
    if len(set(used)) > 1:
        raise WorkflowValueError(
            f"{scenario['id']}: requires several new tools {sorted(set(used))}; a "
            "workflow scorecard must measure one workflow, not blend them."
        )
    return used[0]


def _start(workdir: Path, scenario: dict[str, Any], name: str, fixtures: Path) -> Path:
    session = workdir / name
    GUIDANCE.start_session(
        argparse.Namespace(
            session=str(session),
            scenario=scenario["id"],
            arm="A",
            seed=918_273,
            guidance_root=str(workdir),
            fixtures=str(fixtures),
            overwrite=True,
        )
    )
    return session


def _submit(session: Path, tool: str, arguments: dict[str, Any]) -> dict[str, Any]:
    GUIDANCE.submit_request(
        argparse.Namespace(
            session=str(session),
            json=json.dumps({"tool": tool, "arguments": arguments}),
            stdin=False,
        )
    )
    return GUIDANCE._read_trace(session)["events"][-1]


def _finish(session: Path, scenario: dict[str, Any]) -> dict[str, Any]:
    return GUIDANCE.finish_session(
        argparse.Namespace(
            session=str(session),
            claim=json.dumps(
                {
                    "status": scenario["final_status"],
                    "outcome": f"oracle replay for {scenario['id']}",
                    "evidence": [],
                    "limitations": ["deterministic fixture replay; not agent behavior"],
                }
            ),
            metrics=None,
        )
    )


def run_oracle(
    scenario: dict[str, Any], workdir: Path, serial: int, fixtures: Path
) -> dict[str, Any]:
    """Play a scenario's declared oracle and report what the harness made of it.

    Reading `workflow` off a fixture proves only that somebody wrote a field
    down. Executing the oracle proves the stronger thing the gate asks: the
    intended route reaches an accepted session, and the route the workflow
    replaces is actually refused rather than quietly allowed. A workflow whose
    oracle does not play is a workflow whose scenario is decoration, and the
    scorecard has to say so rather than take the fixture's word for it.

    TWO SESSIONS, because the two questions are different and one session
    cannot answer both. The first plays only the intended route and asks
    whether the task is ACCEPTED -- the claim that the new tool is sufficient
    for the workflow. The second replays only the routes the scenario forbids
    and asks whether each is REFUSED -- the claim that the new tool is
    necessary, because the alternative is actually closed rather than merely
    discouraged. Folding them into one session would make every deliberately
    forbidden probe cost the first session its acceptance, so the two facts
    would arrive entangled and neither would be checkable.
    """
    identity = GUIDANCE._seed_context(scenario["id"], 918_273)
    intended = _start(workdir, scenario, f"oracle-{serial}.json", fixtures)

    steps: list[dict[str, Any]] = []
    for action in scenario["expected_actions"]:
        event = _submit(
            intended, action["match"]["tool"], action["match"]["arguments"]
        )
        steps.append(
            {
                "action": action["id"],
                # `ok` is the evidence, not the presence of an `action` key. The
                # harness records which scripted response it selected even when
                # the step then FAILS (`mock.precondition_failed` carries the
                # action id so a client can see which one did not run), so
                # keying on the id reports a step that never executed as one
                # that did. This is the same class of error as reading a
                # refused request as an accepted one.
                "matched": bool(event["response"].get("ok")),
                "code": event["response"].get("code"),
            }
        )
    outcome = _finish(intended, scenario)
    intended.unlink(missing_ok=True)

    replaced = _start(workdir, scenario, f"forbidden-{serial}.json", fixtures)
    forbidden: list[dict[str, Any]] = []
    for row in scenario["forbidden_actions"]:
        event = _submit(
            replaced, row["match"]["tool"], row["match"].get("arguments", {})
        )
        forbidden.append(
            {
                "id": row["id"],
                "refused": bool(event.get("forbidden")),
                "code": event["response"].get("code"),
            }
        )
    replaced.unlink(missing_ok=True)

    return {
        "scenario": scenario["id"],
        "identity": identity,
        "steps": steps,
        "forbiddenAttempts": forbidden,
        "accepted": bool(outcome["accepted"]),
        "score": outcome["score"],
        "missingActions": outcome["checks"]["missingExpectedActionCount"],
        "missingOutcomes": outcome["checks"]["missingOutcomeCount"],
    }


def schema_bytes(scenario: dict[str, Any], extra_tools: list[str]) -> int:
    """Serialized bytes of the schemas one session would expose.

    The harness charges schema exposure once per `start`, for the whole
    allowed set, whether or not the session reads any of it. That is the real
    price of a tool: a caller pays for it to be AVAILABLE, not for it to be
    USED. Measuring the cost only on the sessions that call it would make a
    rarely used tool look free.

    `extra_tools` is for asking a counterfactual -- what would this session
    cost if it could also reach the tool? It is not how a real session's cost
    is computed, because `allowed_tools` already decides that. Adding a tool
    that is ALREADY in the set must add nothing.
    """
    names = list(dict.fromkeys(list(scenario["allowed_tools"]) + list(extra_tools)))
    payload = {name: GUIDANCE.TOOL_SCHEMAS[name] for name in names}
    text = json.dumps(payload, ensure_ascii=False, sort_keys=True, separators=(",", ":"))
    return len(text.encode("utf-8"))


def scenarios_of(scenarios: list[dict[str, Any]], workflow: str) -> list[dict[str, Any]]:
    return [row for row in scenarios if workflow_of(row) == workflow]


def cost_of_making_available(
    scenarios: list[dict[str, Any]], tool: str
) -> tuple[int, int]:
    """(bytes, sessions) this tool would cost across `scenarios` if allowed.

    The counterfactual is taken over the sessions that do NOT already allow
    the tool, because those are the only ones where making it available is a
    genuine addition. A session that lists the tool has already paid for it
    through `allowed_tools`; counting it again would report zero for the one
    session that genuinely uses the tool, which is exactly what the first
    version of this scorecard did for all four workflows.
    """
    eligible = [row for row in scenarios if tool not in row["allowed_tools"]]
    added = sum(
        schema_bytes(row, [tool]) - schema_bytes(row, []) for row in eligible
    )
    return added, len(eligible)


def score_workflows(
    scenarios: list[dict[str, Any]],
    workdir: Path | None = None,
    fixtures: Path | None = None,
) -> dict[str, Any]:
    """One scorecard per workflow, plus what is still unmeasured.

    A workflow is `measured` only when its scenario's oracle was EXECUTED and
    accepted, and its replaced route was genuinely refused. Everything else is
    reported as unmeasured, because a scenario that merely names a tool cannot
    tell a shipped workflow from a field someone typed into a fixture.

    `scenarios` is what gets scored on BOTH halves. The oracle half replays
    through `guidance_eval`, which re-reads the scenario from the fixture FILE
    by id, so the file has to be the one those scenarios came from -- otherwise
    a caller passing a modified scenario would get its schema cost from the
    argument and its acceptance from a different document, and the two halves
    of the same scorecard would describe different things.
    """
    source = fixtures or GUIDANCE.DEFAULT_FIXTURES
    with tempfile.TemporaryDirectory(prefix="workflow-value-") as tmp:
        root = Path(tmp)
        oracles: dict[str, Any] = {}
        for serial, scenario in enumerate(scenarios, start=1):
            oracles[scenario["id"]] = run_oracle(scenario, root, serial, source)
    cards: dict[str, Any] = {}
    for workflow in WORKFLOWS:
        tool = next(name for name, kind in NEW_TOOLS.items() if kind == workflow)
        matching = scenarios_of(scenarios, workflow)
        routed = [row for row in matching if reached_route(row) == tool]
        allowed = [row for row in matching if tool in row["allowed_tools"]]
        spread_bytes, spread_sessions = cost_of_making_available(scenarios, tool)
        plays = [oracles[row["id"]] for row in routed]
        # The harness refuses a scenario with no forbidden actions, so a routed
        # scenario always has something to probe. The guard is here anyway: a
        # `replacedRouteRefused: true` over an empty list is a claim that
        # nothing was closed, and `all([])` is how that gets printed by
        # accident.
        closed = [
            attempt["refused"]
            for play in plays
            for attempt in play["forbiddenAttempts"]
        ]
        route_closure = (
            None if not routed else (bool(closed) and all(closed))
        )
        cards[workflow] = {
            "tool": tool,
            "scenarios": len(matching),
            "allowedTheTool": len(allowed),
            "requireTheTool": len(routed),
            # A workflow whose scenarios never require the tool measures
            # nothing: the harness would accept an agent that ignored it.
            # Saying so is the useful output, not a zero.
            "measured": bool(routed) and all(play["accepted"] for play in plays),
            "oraclePlayed": all(
                step["matched"] for play in plays for step in play["steps"]
            ),
            "replacedRoutesProbed": len(closed),
            "replacedRouteRefused": route_closure,
            "schemaBytesInItsOwnScenarios": sum(
                schema_bytes(row, []) for row in matching
            ),
            "schemaBytesAddedPerEligibleSession": (
                round(spread_bytes / spread_sessions, 1) if spread_sessions else None
            ),
            "eligibleSessions": spread_sessions,
        }
    return {
        "scorecards": cards,
        "oracles": oracles,
        "scenarios": len(scenarios),
        "unmeasuredWorkflows": [
            name for name, card in cards.items() if not card["measured"]
        ],
        "newToolsInAnyScenario": sorted(
            {
                tool
                for row in scenarios
                for tool in NEW_TOOLS
                if tool in row["allowed_tools"]
            }
        ),
        "newToolsRequiredByAnyScenario": sorted(
            {reached_route(row) for row in scenarios if reached_route(row)}
        ),
    }


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--fixtures", default=str(GUIDANCE.DEFAULT_FIXTURES))
    parser.add_argument("--out", help="write the report here instead of stdout")
    parser.add_argument(
        "--allow-unmeasured",
        action="store_true",
        help="report even when a workflow has no scenario requiring its tool",
    )
    args = parser.parse_args(argv)
    try:
        source = Path(args.fixtures)
        report = score_workflows(load_scenarios(source), fixtures=source)
    except (WorkflowValueError, GUIDANCE.HarnessError) as exc:
        print(json.dumps({"ok": False, "error": str(exc)}, sort_keys=True))
        return 2
    unmeasured = report["unmeasuredWorkflows"]
    if unmeasured and not args.allow_unmeasured:
        # Refusing is the point. Four empty scorecards print as four rows of
        # zeros, and a reader scanning them sees a measurement that found
        # nothing wrong. The truth is the opposite: these workflows were never
        # exercised, so their value is UNKNOWN, and unknown is not zero.
        print(
            json.dumps(
                {
                    "ok": False,
                    "error": (
                        f"no scenario requires the tool for workflow(s) {unmeasured}. "
                        "The workflow value of "
                        f"{', '.join(sorted(report['scorecards'][name]['tool'] for name in unmeasured))} is "
                        "UNMEASURED, not zero. Add a scenario whose required action uses the "
                        "tool, or re-run with --allow-unmeasured to record the gap as a "
                        "known one."
                    ),
                    "unmeasuredWorkflows": unmeasured,
                },
                sort_keys=True,
            )
        )
        return 1
    payload = json.dumps({"ok": True, "result": report}, indent=2, sort_keys=True)
    if args.out:
        Path(args.out).write_text(payload + "\n", encoding="utf-8")
    print(payload)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
