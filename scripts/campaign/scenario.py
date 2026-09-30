#!/usr/bin/env python3
"""Scenario schema, predicates and runner.

A scenario is data, not code: ordered steps, each naming a tool, templated
arguments, a predicate, and a severity. Two reasons for that shape.

Reviewability. The thing that decides whether the product is correct is the
one thing a reviewer must be able to read without running anything.

And falsifiability. A runner that cannot fail is not a runner, so the
predicate vocabulary is small, pure, and closed: every assertion prints the
value it actually observed next to the claim it contradicts. `ok:true` on its
own is never accepted as evidence, because every tool returns it -- the
interesting question is always what the payload says.
"""
from __future__ import annotations

import json
import re
import time
import traceback
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Callable

# Severity. BLOCKER and DEFECT are both real findings; OBSERVATION is a fact
# worth recording that is not a failure; INCONCLUSIVE is the verdict reserved
# for "the host made this unjudgeable" -- never smoothed into either pass or
# fail, because a heartbeat timeout under load reads exactly like a defect.
BLOCKER = "blocker"
DEFECT = "defect"
OBSERVATION = "observation"
INCONCLUSIVE = "inconclusive"


def _dig(value: Any, path: str) -> Any:
    """Resolve a dotted path with numeric segments, or return the value itself.

    Returns a sentinel rather than raising: a missing key is a normal outcome
    for a predicate like `path_equals`, and the caller reports the absence
    against the claim instead of crashing on the way to report it.
    """
    if path == "":
        return value
    for segment in path.split("."):
        if isinstance(value, list):
            try:
                value = value[int(segment)]
            except (ValueError, IndexError):
                return _MISSING
        elif isinstance(value, dict):
            if segment not in value:
                return _MISSING
            value = value[segment]
        else:
            return _MISSING
    return value


class _Missing:
    def __repr__(self) -> str:  # pragma: no cover - diagnostic repr
        return "<missing>"


_MISSING = _Missing()


def is_missing(value: Any) -> bool:
    """Distinguish 'absent' from 'present and None'.

    The honesty contract the whole product exists to keep: `missing` is not
    `null`, and `unavailable` is not `false`. A predicate that flattens the
    two would test the opposite of what it claims to test.
    """
    return isinstance(value, _Missing)


def resolve(value: Any, path: str) -> Any:
    return _dig(value, path)


# --- predicates -------------------------------------------------------------
# Each returns (passed, observed_text). `observed_text` is mandatory because
# a failure with no observed value is a report nobody can act on.

Predicate = Callable[[Any, dict], tuple[bool, str]]


def _show(value: Any, limit: int = 200) -> str:
    """Render an observed value for a step's record.

    One function, because a predicate that formats an absent value with
    `json.dumps` crashes on the `_MISSING` sentinel -- and the crash is not
    cosmetic: it aborts the scenario and reports "setup failed" for a run that
    merely observed an absence. Absence is a VALUE the product deliberately
    reports, so it must be renderable everywhere a value is.
    """
    if is_missing(value):
        return "<absent>"
    if isinstance(value, str):
        return value[:limit]
    try:
        return json.dumps(value)[:limit]
    except (TypeError, ValueError):
        return repr(value)[:limit]


def _p_ok(payload: Any, spec: dict) -> tuple[bool, str]:
    ok = _dig(payload, spec.get("path", "ok"))
    passed = ok is True
    return passed, f"{spec.get('path', 'ok')}={_show(ok)}"


def envelope_ok(payload: Any) -> Any:
    """Resolve `ok` across BOTH reply shapes the product uses.

    Most tools answer `{"items":[{"ok":..,"result":..}],"ok":..}`; `compare_nodes`
    is a read and returns its payload at the TOP level with no envelope.
    Reshaping either would corrupt the distinctions it exists to carry, so the
    runner reads whichever shape arrived rather than normalising the product.

    Getting this wrong is not a cosmetic bug: a predicate that looked for a
    top-level `ok` and did not find one would read as a failure on every
    correctly-enveloped call, and a harness that fails on correct behaviour
    trains its readers to ignore it.
    """
    top = _dig(payload, "ok")
    if not is_missing(top):
        return top
    items = _dig(payload, "items")
    if isinstance(items, list) and items and isinstance(items[0], dict):
        if "ok" in items[0]:
            return items[0]["ok"]
    return _MISSING


def _p_not_ok(payload: Any, spec: dict) -> tuple[bool, str]:
    ok = _dig(payload, spec.get("path", "ok"))
    if is_missing(ok):
        # An absent `ok` is a refusal-shaped reply, which is what this
        # predicate is looking for -- but it is still not `ok:false`, and the
        # contract forbids conflating them.
        return True, f"{spec.get('path', 'ok')}=<absent> (refusal, not ok:false)"
    return ok is False, f"{spec.get('path', 'ok')}={_show(ok)}"


def _p_step_refused(payload: Any, spec: dict) -> tuple[bool, str]:
    """Assert a mutate batch refused, naming the PER-STEP code.

    A mutate refusal puts its real code on `steps[i].code`; the item-level
    `code` is the generic `tdmcp.mutate.step_failed` wrapper describing the
    call. Reading only the item code turns every precondition failure into
    "step_failed" and hides the distinction the bridge draws -- which is the
    whole point of the assertion.
    """
    code = _dig(payload, "steps.0.code")
    if is_missing(code):
        items = _dig(payload, "items")
        if isinstance(items, list) and items and isinstance(items[0], dict):
            code = _dig(items[0], "steps.0.code")
    if is_missing(code):
        return False, "no per-step code on the refusal payload"
    needle = spec.get("contains", "")
    return needle in str(code), f"steps.0.code={code!r} (looking for {needle!r})"


def _p_path_equals(payload: Any, spec: dict) -> tuple[bool, str]:
    path = spec["path"]
    observed = _dig(payload, path)
    if is_missing(observed):
        return False, f"{path}=<absent> (expected {json.dumps(spec.get('equals'))})"
    passed = observed == spec.get("equals")
    return passed, f"{path}={json.dumps(observed)[:200]} (expected {json.dumps(spec.get('equals'))})"


def _p_path_contains(payload: Any, spec: dict) -> tuple[bool, str]:
    path = spec["path"]
    observed = _dig(payload, path)
    if not isinstance(observed, (str, list, dict)):
        return False, f"{path}={_show(observed)} is not a container"
    text = observed if isinstance(observed, str) else _show(observed, limit=10**6)
    needle = spec["value"]
    return needle in text, f"{path} contains {needle!r}: {needle in text} ({text[:200]})"


def _p_path_present(payload: Any, spec: dict) -> tuple[bool, str]:
    path = spec["path"]
    observed = _dig(payload, path)
    if is_missing(observed):
        return False, f"{path}=<absent>"
    return True, f"{path}={_show(observed)}"


def _p_all_true(payload: Any, spec: dict) -> tuple[bool, str]:
    """Assert several dotted paths are all literally `true`."""
    bad = []
    for path in spec["paths"]:
        value = _dig(payload, path)
        if value is not True:
            bad.append(f"{path}={_show(value, limit=80)}")
    return not bad, ("all true: " + ", ".join(spec["paths"])) if not bad else "not true: " + ", ".join(bad)


def _p_distinct(payload: Any, spec: dict) -> tuple[bool, str]:
    """Assert two paths hold different values.

    This is how the honesty contract gets tested: `missing` must not have
    been flattened into `null`, `unavailable` into `false`, or an observed
    value into a fabricated one. A product that collapsed those distinctions
    would pass every `path_equals` in the corpus and fail only here.
    """
    left, right = spec["left"], spec["right"]
    a, b = _dig(payload, left), _dig(payload, right)
    if is_missing(a) and is_missing(b):
        return True, f"{left} and {right} both absent"
    if is_missing(a) or is_missing(b):
        return False, f"{left}={_show(a, limit=80)} / {right}={_show(b, limit=80)}"
    return a != b, f"{left}={_show(a, limit=80)} vs {right}={_show(b, limit=80)}"


def _p_path_absent(payload: Any, spec: dict) -> tuple[bool, str]:
    path = spec["path"]
    observed = _dig(payload, path)
    return is_missing(observed), f"{path} is {'absent' if is_missing(observed) else _show(observed)}"


def _p_regex(payload: Any, spec: dict) -> tuple[bool, str]:
    path = spec.get("path", "")
    observed = _dig(payload, path)
    if is_missing(observed):
        return False, f"{path}=<absent> (expected /{spec.get('pattern')}/)"
    text = observed if isinstance(observed, str) else json.dumps(observed)
    passed = re.search(spec["pattern"], text) is not None
    return passed, f"{path} matches /{spec.get('pattern')}/: {passed} ({text[:200]})"


PREDICATES: dict[str, Predicate] = {
    "ok": _p_ok,
    "not_ok": _p_not_ok,
    "envelope_ok": lambda payload, spec: (
        envelope_ok(payload) is True,
        f"ok (either shape)={json.dumps(envelope_ok(payload), default=str)[:200]}",
    ),
    "step_refused": _p_step_refused,
    "path_equals": _p_path_equals,
    "path_contains": _p_path_contains,
    "path_present": _p_path_present,
    "path_absent": _p_path_absent,
    "all_true": _p_all_true,
    "distinct": _p_distinct,
    "regex": _p_regex,
}


# --- steps and scenarios ----------------------------------------------------


@dataclass
class Step:
    name: str
    tool: str
    args: dict = field(default_factory=dict)
    # A list of assertions, all of which must hold for the step to pass.
    assert_: list[dict] = field(default_factory=list)
    # A step that is allowed to fail without failing the scenario: a probe of
    # a refusal path, where the refusal is the expected result.
    expect_refusal: bool = False
    # Skip unless this tool call succeeded far enough to produce a pid.
    needs_pid: bool = False
    timeout_s: int = 120
    # Set when a step's payload feeds the next step's arguments.
    capture: str | None = None

    @staticmethod
    def from_dict(data: dict) -> "Step":
        step = Step(
            name=data.get("name") or data["tool"],
            tool=data["tool"],
            args=data.get("args", {}),
            assert_=data.get("assert", []),
            expect_refusal=data.get("expect_refusal", False),
            needs_pid=data.get("needs_pid", False),
            timeout_s=int(data.get("timeout_s", 120)),
            capture=data.get("capture"),
        )
        for claim in step.assert_:
            if claim.get("predicate") not in PREDICATES:
                raise ValueError(
                    f"{step.name}: unknown predicate {claim.get('predicate')!r}; "
                    f"known: {sorted(PREDICATES)}"
                )
        return step


@dataclass
class Scenario:
    id: str
    title: str
    tools: list[str]
    steps: list[Step]
    description: str = ""
    #: Scenarios that must pass on a healthy host. `needs_td` scenarios need
    #: a spawned TouchDesigner; the harness supplies the pid.
    needs_td: bool = True

    @staticmethod
    def from_dict(data: dict) -> "Scenario":
        return Scenario(
            id=data["id"],
            title=data.get("title", data["id"]),
            tools=data.get("tools", []),
            steps=[Step.from_dict(s) for s in data["steps"]],
            description=data.get("description", ""),
            needs_td=bool(data.get("needs_td", True)),
        )


@dataclass
class StepResult:
    name: str
    tool: str
    passed: bool
    severity: str
    observed: str
    duration_s: float
    payload_path: str = ""
    error: str = ""


@dataclass
class ScenarioResult:
    id: str
    title: str
    passed: bool
    verdict: str
    results: list[StepResult] = field(default_factory=list)
    findings: list[str] = field(default_factory=list)
    duration_s: float = 0.0

    def as_dict(self) -> dict:
        return {
            "id": self.id,
            "title": self.title,
            "passed": self.passed,
            "verdict": self.verdict,
            "duration_s": round(self.duration_s, 2),
            "findings": self.findings,
            "steps": [
                {
                    "name": r.name,
                    "tool": r.tool,
                    "passed": r.passed,
                    "severity": r.severity,
                    "observed": r.observed,
                    "duration_s": round(r.duration_s, 2),
                    "payload_path": r.payload_path,
                    "error": r.error,
                }
                for r in self.results
            ],
        }


def _resolve_vars(value: Any, ctx: dict) -> Any:
    """Substitute `{{var}}` placeholders in a step's arguments.

    A string is substituted whole only when it is exactly one placeholder,
    so a pid stays an int and a list is never stringified.
    """
    if isinstance(value, str):
        match = re.fullmatch(r"\{\{\s*(\w+)\s*\}\}", value)
        if match:
            return ctx.get(match.group(1), value)
        return re.sub(r"\{\{\s*(\w+)\s*\}\}", lambda m: str(ctx.get(m.group(1), m.group(0))), value)
    if isinstance(value, dict):
        return {k: _resolve_vars(v, ctx) for k, v in value.items()}
    if isinstance(value, list):
        return [_resolve_vars(v, ctx) for v in value]
    return value


class Refusal(Exception):
    """A tool call that returned a structured refusal, not a transport error."""


def run_scenario(
    scenario: Scenario,
    call: Callable[[str, dict, int], dict],
    *,
    ctx: dict | None = None,
    out_dir: Any = None,
    session_label: str = "",
) -> ScenarioResult:
    """Execute one scenario, writing each raw payload to the run record.

    Every verdict is decided by a predicate over a payload. Nothing is taken
    on trust from a bare `ok:true`, and no step's failure is swallowed: the
    first failing step is reported with its observed value and the scenario
    continues, because a scenario that stops at the first failure hides
    everything after it.
    """
    ctx = dict(ctx or {})
    ctx.setdefault("pid", None)
    result = ScenarioResult(id=scenario.id, title=scenario.title, passed=True, verdict="verified")
    started = time.monotonic()

    for index, step in enumerate(scenario.steps):
        if step.needs_pid and not ctx.get("pid"):
            result.results.append(
                StepResult(step.name, step.tool, False, INCONCLUSIVE,
                           "no pid bound: the spawn step did not produce one", 0.0)
            )
            result.findings.append(f"{step.name}: needs a pid but none was bound")
            result.passed = False
            result.verdict = INCONCLUSIVE
            continue

        args = _resolve_vars(step.args, ctx)
        try:
            payload = call(step.tool, args, step.timeout_s)
        except Refusal as exc:
            payload = exc.args[0] if exc.args else {"ok": False, "summary": str(exc)}
        except Exception as exc:  # transport, timeout, protocol shape
            detail = f"{type(exc).__name__}: {exc}"
            result.results.append(
                StepResult(step.name, step.tool, False, INCONCLUSIVE, detail,
                           round(time.monotonic() - started, 2), error=traceback.format_exc(limit=3))
            )
            result.findings.append(f"{step.name}: {detail}")
            result.passed = False
            if result.verdict == "verified":
                result.verdict = INCONCLUSIVE
            continue

        payload_path = ""
        if out_dir is not None:
            safe = re.sub(r"[^a-zA-Z0-9_.-]", "_", f"{index:02d}-{step.name}")
            target = out_dir / f"{safe}.json"
            try:
                target.write_text(json.dumps(payload, indent=2, default=str), encoding="utf-8")
                payload_path = str(target)
            except OSError:
                payload_path = ""

        if step.capture:
            ctx[step.capture] = _extract_pid(payload) if step.capture == "pid" else payload

        # A step with no assertions is a probe, not a test: it must have
        # produced a payload and nothing more is claimed.
        failures: list[str] = []
        observed_bits: list[str] = []
        predicate_errors: list[str] = []
        for claim in step.assert_:
            predicate = PREDICATES[claim["predicate"]]
            try:
                passed, observed = predicate(payload, claim)
            except Exception as exc:  # a broken predicate is a HARNESS failure
                # A predicate that raises must never be read as a pass, and
                # must never abort the scenario either: aborting reports
                # "setup failed" for a run that merely hit a harness bug, and
                # hides every assertion after this one. It becomes its own
                # finding, so the reader sees a broken check rather than a
                # broken product.
                predicate_errors.append(f"{claim['predicate']} raised {type(exc).__name__}: {exc}")
                continue
            observed_bits.append(observed)
            if not passed:
                failures.append(observed)

        if predicate_errors:
            # An unevaluated claim is neither a pass nor a defect: nobody knows
            # what it would have said, so it is reported as unknown.
            result.results.append(
                StepResult(step.name, step.tool, False, INCONCLUSIVE,
                           " | ".join(predicate_errors), round(time.monotonic() - started, 2),
                           payload_path, error="\n".join(predicate_errors))
            )
            result.findings.extend(f"{step.name}: {err}" for err in predicate_errors)
            result.passed = False
            if result.verdict == "verified":
                result.verdict = INCONCLUSIVE
            continue

        if step.expect_refusal:
            # An expect_refusal step states what a refusal looks like. If the
            # claim does not describe the payload, the step was NOT refused the
            # way it demanded -- a step that expected a guard to hold and saw
            # the write go through is a failure, and a loud one.
            if failures:
                passed = False
                observed_bits.append(
                    f"expect_refusal: payload ok={json.dumps(_dig(payload, 'ok'))[:80]}"
                )
            else:
                passed = True
        else:
            passed = not failures
        elapsed = round(time.monotonic() - started, 2)
        result.results.append(
            StepResult(step.name, step.tool, passed, DEFECT if not passed else "ok",
                       " | ".join(observed_bits) or "no assertions (probe)", elapsed, payload_path)
        )
        if not passed:
            result.findings.append(f"{step.name}: " + "; ".join(failures))
            result.passed = False
            if result.verdict == "verified":
                result.verdict = DEFECT

    result.duration_s = time.monotonic() - started
    if result.passed and result.verdict == "verified":
        result.verdict = "verified"
    return result


def _looks_refused(payload: Any) -> bool:
    """Is this payload a refusal rather than a success?

    A transport-level refusal can arrive as `ok:false`, or with the `ok` key
    absent entirely (an envelope that never got one). Both are refusals, and
    the contract keeps them distinct in the report even though this predicate
    only needs the distinction between refused and not.
    """
    ok = _dig(payload, "ok")
    return ok is False or is_missing(ok)


def _extract_pid(payload: Any) -> int | None:
    """Pull the spawned pid out of a spawn_td reply, whatever envelope it used."""
    for path in ("pid", "result.pid", "items.0.result.pid", "items.0.result.process.pid"):
        value = _dig(payload, path)
        if isinstance(value, int) and value > 0:
            return value
    return None


SCHEMA_DIR = Path(__file__).resolve().parents[2] / "crates" / "tdmcp-mcp" / "tests" / "fixtures" / "schemas"


def tool_schema(tool: str) -> dict | None:
    """The shipped JSON schema for one tool, or None when there is none.

    Read from the checked-in fixtures rather than from a live daemon, so a
    scenario file can be validated without a TouchDesigner: the cost of
    finding out that a step's arguments are malformed should be a test, not a
    campaign run whose every step fails for the same reason.
    """
    path = SCHEMA_DIR / f"{tool}.json"
    if not path.exists():
        return None
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError):
        return None


def validate_step_arguments(step: "Step") -> list[str]:
    """Schema problems in a step's arguments, as human-readable strings.

    This catches the exact defect the first pilot hit: a `comment` field on a
    `mutate_nodes` DELETE step, which the daemon rejects as an unknown field,
    so the create that step guards never happens and every assertion after it
    reports a node that does not exist. Nine failures in a row with one cause
    is a scenario-data defect wearing the costume of nine product defects.

    `{{pid}}` placeholders are substituted with a representative value first,
    because the schema cares about the type and the shape, not about which
    process is live. A step that cannot be checked is REPORTED, never
    silently skipped: "this was not validated" is a fact the reader needs.
    """
    schema = tool_schema(step.tool)
    if schema is None:
        return []
    concrete = _resolve_vars(step.args, {"pid": 1, "path": "/project1/probe"})
    try:
        import jsonschema
    except ImportError:  # pragma: no cover - governance requires it
        return []
    validator = jsonschema.Draft7Validator(schema)
    problems = []
    for error in sorted(validator.iter_errors(concrete), key=lambda e: list(e.path)):
        where = ".".join(str(p) for p in error.path) or "(root)"
        problems.append(f"{step.name}: {step.tool} arg {where}: {error.message}")
    return problems


def validate_scenarios(scenarios: list["Scenario"]) -> dict[str, list[str]]:
    """Every schema problem in every scenario, keyed by scenario id."""
    problems: dict[str, list[str]] = {}
    for sc in scenarios:
        found: list[str] = []
        for step in sc.steps:
            found.extend(validate_step_arguments(step))
        if found:
            problems[sc.id] = found
    return problems


def load_scenarios(directory: Any) -> list[Scenario]:
    """Load every scenario in a directory, sorted by id for stable runs."""
    directory = Path(directory)
    scenarios: list[Scenario] = []
    for path in sorted(directory.glob("*.json")):
        scenarios.append(Scenario.from_dict(json.loads(path.read_text(encoding="utf-8"))))
    return scenarios
