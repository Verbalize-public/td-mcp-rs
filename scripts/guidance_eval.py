#!/usr/bin/env python3
"""Deterministic mocked interaction harness for guidance A/B/C trials.

This program never connects to TouchDesigner, executes submitted Python, accesses the
network, or writes anywhere except the caller-designated trace file under the system
temporary directory. It is an authoring/pilot harness, not evidence of agent behavior.
"""
from __future__ import annotations

import argparse
import ast
import copy
import hashlib
import json
from pathlib import Path
import random
import re
import sys
import tempfile
from typing import Any

try:
    from jsonschema import Draft202012Validator
except ImportError as exc:  # pragma: no cover - environment setup, not fixture behavior
    raise RuntimeError(
        "guidance_eval requires scripts/requirements-test.txt (run with the project .venv)"
    ) from exc

ROOT = Path(__file__).resolve().parents[1]
DEFAULT_FIXTURES = ROOT / "scripts/fixtures/guidance/scenarios.json"
SCHEMA_DIR = ROOT / "crates/tdmcp-mcp/tests/fixtures/schemas"
ARMS = ("A", "B", "C")
MAX_CARD_BYTES = 1_048_576


class HarnessError(ValueError):
    """A safe, user-facing harness error."""


# Every real TD tool schema comes from the checked-in generated golden catalog.
# Mock-only interaction conveniences are visibly prefixed and separately defined.
REAL_TOOL_SCHEMAS: dict[str, dict[str, Any]] = {
    path.stem: json.loads(path.read_text(encoding="utf-8"))
    for path in sorted(SCHEMA_DIR.glob("*.json"))
}
MOCK_TOOL_SCHEMAS: dict[str, dict[str, Any]] = {
    "mock_ask_user": {
        "type": "object",
        "required": ["question"],
        "properties": {
            "question": {"type": "string"},
            "choices": {"type": "array", "items": {"type": "string"}},
        },
        "additionalProperties": False,
    },
    "mock_read_source": {
        "type": "object",
        "required": ["path"],
        "properties": {
            "path": {"type": "string"},
            "lineStart": {"type": "integer", "minimum": 1},
        },
        "additionalProperties": False,
    },
    "mock_environment_probe": {
        "type": "object",
        "required": ["scope"],
        "properties": {"scope": {"type": "string", "enum": ["session", "process"]}},
        "additionalProperties": False,
    },
    "mock_project_io": {
        "type": "object",
        "required": ["action", "path"],
        "properties": {
            "action": {"type": "string", "enum": ["find", "explain"]},
            "path": {"type": "string"},
        },
        "additionalProperties": False,
    },
}
TOOL_SCHEMAS = {**REAL_TOOL_SCHEMAS, **MOCK_TOOL_SCHEMAS}

def _validate(value: Any, schema: dict[str, Any], at: str = "arguments") -> list[str]:
    """Validate with the same checked-in Draft 2020-12 schema shown to callers."""
    validator = Draft202012Validator(schema)
    errors = sorted(validator.iter_errors(value), key=lambda error: list(error.absolute_path))
    rendered = []
    for error in errors:
        suffix = "".join(
            f"[{part}]" if isinstance(part, int) else f".{part}"
            for part in error.absolute_path
        )
        rendered.append(f"{at}{suffix}: {error.message}")
    return rendered


def _seed_context(scenario_id: str, seed: int) -> dict[str, Any]:
    digest = hashlib.sha256(f"{scenario_id}:{seed}".encode()).hexdigest()
    rng = random.Random(int(digest[:16], 16))
    token = digest[:10]
    root = f"/mock/trials/{token}"
    return {
        "pid": rng.randrange(20_000, 90_000),
        "other_pid": rng.randrange(100_000, 180_000),
        "token": token,
        "root": root,
        "project": f"{root}/scene-{digest[10:16]}.toe",
        "missing_project": f"{root}/missing-{digest[16:22]}.toe",
        "node": f"/project1/unit_{digest[22:28]}",
        "other_node": f"/project1/unrelated_{digest[28:34]}",
        "job": f"job-{digest[34:42]}",
        "palette_a": f"user:Tools/item-{digest[42:48]}",
        "palette_b": f"user:Tools/item-{digest[48:54]}",
    }


def _render(value: Any, context: dict[str, Any]) -> Any:
    if isinstance(value, str):
        for key, replacement in context.items():
            if value == "{{" + key + "}}":
                return copy.deepcopy(replacement)
        for key, replacement in context.items():
            value = value.replace("{{" + key + "}}", str(replacement))
        return value
    if isinstance(value, list):
        return [_render(item, context) for item in value]
    if isinstance(value, dict):
        return {key: _render(item, context) for key, item in value.items()}
    return value


def _deep_merge(base: dict[str, Any], overlay: dict[str, Any]) -> dict[str, Any]:
    result = copy.deepcopy(base)
    for key, value in overlay.items():
        if isinstance(value, dict) and isinstance(result.get(key), dict):
            result[key] = _deep_merge(result[key], value)
        else:
            result[key] = copy.deepcopy(value)
    return result


def load_fixtures(path: Path = DEFAULT_FIXTURES) -> dict[str, Any]:
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        raise HarnessError(f"cannot load fixture file: {exc}") from exc
    validate_fixtures(data)
    return data


def validate_fixtures(data: dict[str, Any]) -> None:
    if data.get("version") != 1 or not isinstance(data.get("scenarios"), list):
        raise HarnessError("fixture version 1 with a scenarios array is required")
    # The set is open-ended so a workflow can be added to it, but it stays
    # SEQUENTIAL: a gap in the numbering means a scenario was lost in an edit
    # or a rebase, which is the silent loss an enumerated list used to
    # prevent. Pinning `range(1, 34)` made every future scenario a contract
    # change to the harness itself -- a poor trade for a property that
    # sequence + uniqueness already give.
    seen: set[str] = set()
    for scenario in data["scenarios"]:
        scenario_id = scenario.get("id")
        if not isinstance(scenario_id, str) or re.fullmatch(r"S\d{2,}", scenario_id) is None:
            raise HarnessError(f"malformed scenario id {scenario_id!r}; expected S01, S02, ...")
        if scenario_id in seen:
            raise HarnessError(f"duplicate scenario {scenario_id}")
        seen.add(scenario_id)
        for field in ("task", "allowed_tools", "initial_state", "expected_actions", "forbidden_actions", "expected_outcomes", "final_status"):
            if field not in scenario:
                raise HarnessError(f"{scenario_id}: missing {field}")
        if not scenario["expected_actions"] or not scenario["forbidden_actions"] or not scenario["expected_outcomes"]:
            raise HarnessError(f"{scenario_id}: expected/forbidden actions and outcomes must be explicit and nonempty")
        unknown = set(scenario["allowed_tools"]) - TOOL_SCHEMAS.keys()
        if unknown:
            raise HarnessError(f"{scenario_id}: unknown tools {sorted(unknown)}")
        action_ids = [row.get("id") for row in scenario["expected_actions"]]
        if len(action_ids) != len(set(action_ids)) or any(not item for item in action_ids):
            raise HarnessError(f"{scenario_id}: expected action ids must be unique and nonempty")
        outcome_ids = [row.get("id") for row in scenario["expected_outcomes"]]
        if len(outcome_ids) != len(set(outcome_ids)) or any(not item for item in outcome_ids):
            raise HarnessError(f"{scenario_id}: outcome ids must be unique and nonempty")
        # A forbidden matcher that CAPTURES a required step makes the scenario
        # permanently unplayable, and nothing else here would notice:
        # `submit_request` tests the forbidden list independently of the
        # expected list, so the required request is answered
        # `mock.policy.forbidden`, never reaches its scripted response, and the
        # session is refused on the missing action. Two matchers are in
        # conflict when either one is a subset of the other -- in that case a
        # request that satisfies the required action also satisfies the
        # forbidden one, in one direction or the other. Checked against
        # REQUIRED expected actions only: an `other_actions` row is by
        # definition a route the harness may serve, so a scenario may
        # legitimately forbid one of those as a discouraged alternative.
        for action in scenario["expected_actions"]:
            if not action.get("required", True):
                continue
            for row in scenario["forbidden_actions"]:
                if _subset(row["match"], action["match"]) or _subset(
                    action["match"], row["match"]
                ):
                    raise HarnessError(
                        f"{scenario_id}: forbidden action {row['id']!r} and required action "
                        f"{action['id']!r} match overlapping requests, so the required step "
                        "can only ever be refused. Make the matchers disjoint."
                    )
        for variant in scenario.get("variants", []):
            overlay = variant.get("overlay", {})
            if not variant.get("id") or not isinstance(overlay, dict):
                raise HarnessError(f"{scenario_id}: malformed variant")
    numbered = sorted(int(scenario_id[1:]) for scenario_id in seen)
    if numbered != list(range(1, len(numbered) + 1)):
        missing = sorted(set(range(1, max(numbered) + 1)) - set(numbered))
        raise HarnessError(
            f"scenario numbering has a gap: missing={missing}. A hole means a scenario was "
            "dropped rather than retired; renumber the tail or restore the scenario."
        )


def resolve_scenario(data: dict[str, Any], scenario_id: str, seed: int) -> tuple[dict[str, Any], dict[str, Any], str | None]:
    raw = next((row for row in data["scenarios"] if row["id"] == scenario_id), None)
    if raw is None:
        raise HarnessError(f"unknown scenario {scenario_id}")
    context = _seed_context(scenario_id, seed)
    resolved = copy.deepcopy(raw)
    variants = resolved.pop("variants", [])
    variant_id = None
    if variants:
        index = int(hashlib.sha256(f"variant:{scenario_id}:{seed}".encode()).hexdigest()[:8], 16) % len(variants)
        variant = variants[index]
        variant_id = variant["id"]
        resolved = _deep_merge(resolved, variant["overlay"])
    return _render(resolved, context), context, variant_id


def _trace_path(raw: str) -> Path:
    path = Path(raw).expanduser()
    if not path.is_absolute():
        raise HarnessError("session trace path must be absolute")
    temp_root = Path(tempfile.gettempdir()).resolve()
    parent = path.parent.resolve()
    if parent != temp_root and temp_root not in parent.parents:
        raise HarnessError(f"session trace must be under {temp_root}")
    if path.suffix != ".json":
        raise HarnessError("session trace must use a .json suffix")
    if path.is_symlink():
        raise HarnessError("session trace cannot be a symbolic link")
    if not parent.is_dir():
        raise HarnessError("session trace parent directory must already exist")
    return path


def _write_trace(path: Path, trace: dict[str, Any]) -> None:
    # A single designated trace is the harness's only write boundary.
    path.write_text(json.dumps(trace, indent=2, sort_keys=True) + "\n", encoding="utf-8")


def _read_trace(path: Path) -> dict[str, Any]:
    try:
        trace = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        raise HarnessError(f"cannot read session trace: {exc}") from exc
    if trace.get("traceVersion") != 1:
        raise HarnessError("unsupported session trace")
    return trace


def _dotted_name(node: ast.AST) -> str | None:
    if isinstance(node, ast.Name):
        return node.id
    if isinstance(node, ast.Attribute):
        prefix = _dotted_name(node.value)
        return f"{prefix}.{node.attr}" if prefix else node.attr
    return None


def _python_matches(spec: dict[str, Any], script: Any) -> bool:
    """Safely classify submitted source with AST inspection; never execute it."""
    if not isinstance(script, str):
        return False
    try:
        tree = ast.parse(script)
    except SyntaxError:
        return False
    imports: set[str] = set()
    names: set[str] = set()
    attributes: set[str] = set()
    calls: set[str] = set()
    strings: set[str] = set()
    assigns_result = False
    for node in ast.walk(tree):
        if isinstance(node, ast.Import):
            imports.update(alias.name.split(".")[0] for alias in node.names)
        elif isinstance(node, ast.ImportFrom) and node.module:
            imports.add(node.module.split(".")[0])
        elif isinstance(node, ast.Name):
            names.add(node.id)
        elif isinstance(node, ast.Attribute):
            dotted = _dotted_name(node)
            if dotted:
                attributes.add(dotted)
                attributes.add(node.attr)
        elif isinstance(node, ast.Call):
            dotted = _dotted_name(node.func)
            if dotted:
                calls.add(dotted)
        elif isinstance(node, ast.Constant) and isinstance(node.value, str):
            strings.add(node.value)
        elif isinstance(node, (ast.Assign, ast.AnnAssign)):
            targets = node.targets if isinstance(node, ast.Assign) else [node.target]
            assigns_result = assigns_result or any(
                isinstance(target, ast.Name) and target.id == "result" for target in targets
            )
    checks = {
        "imports": imports,
        "names": names,
        "attributes": attributes,
        "calls": calls,
        "strings": strings,
    }
    for key, observed in checks.items():
        required = spec.get(key, [])
        if not isinstance(required, list) or not set(required).issubset(observed):
            return False
    absent_imports = spec.get("importsAbsent", [])
    if not isinstance(absent_imports, list) or set(absent_imports) & imports:
        return False
    if "assignsResult" in spec and bool(spec["assignsResult"]) != assigns_result:
        return False
    return True


def _subset(expected: Any, actual: Any) -> bool:
    if isinstance(expected, dict):
        if "$contains" in expected:
            return isinstance(actual, (str, list, dict)) and expected["$contains"] in actual
        if "$setContains" in expected:
            return isinstance(actual, list) and set(expected["$setContains"]).issubset(set(actual))
        if "$in" in expected:
            return isinstance(expected["$in"], list) and actual in expected["$in"]
        if "$python" in expected:
            return isinstance(expected["$python"], dict) and _python_matches(expected["$python"], actual)
        if not isinstance(actual, dict):
            return False
        absent = expected.get("$absent", [])
        if any(key in actual for key in absent):
            return False
        ordinary = {key: value for key, value in expected.items() if key != "$absent"}
        return all(key in actual and _subset(value, actual[key]) for key, value in ordinary.items())
    if isinstance(expected, list):
        return expected == actual
    return expected == actual


def _matches(matcher: dict[str, Any], request: dict[str, Any]) -> bool:
    return _subset(matcher, request)


def _state_get(state: dict[str, Any], dotted: str) -> Any:
    value: Any = state
    for part in dotted.split("."):
        if not isinstance(value, dict) or part not in value:
            return None
        value = value[part]
    return value


def _state_set(state: dict[str, Any], dotted: str, value: Any) -> tuple[Any, Any]:
    parts = dotted.split(".")
    target = state
    for part in parts[:-1]:
        child = target.get(part)
        if not isinstance(child, dict):
            child = {}
            target[part] = child
        target = child
    before = target.get(parts[-1])
    target[parts[-1]] = copy.deepcopy(value)
    return before, value


def _apply_effects(state: dict[str, Any], effects: list[dict[str, Any]]) -> list[dict[str, Any]]:
    observed = []
    for effect in effects:
        if effect.get("op") != "set" or not isinstance(effect.get("path"), str):
            raise HarnessError("fixture contains unsupported effect")
        before, after = _state_set(state, effect["path"], effect.get("value"))
        observed.append({"op": "set", "path": effect["path"], "before": before, "after": after})
    return observed


def _record_schema_exposure(trace: dict[str, Any]) -> None:
    schemas = {name: TOOL_SCHEMAS[name] for name in trace["scenario"]["allowed_tools"]}
    text = json.dumps(schemas, ensure_ascii=False, sort_keys=True, separators=(",", ":"))
    trace["metrics"]["toolSchemaReads"] += 1
    trace["metrics"]["toolSchemaChars"] += len(text)
    trace["metrics"]["toolSchemaBytes"] += len(text.encode("utf-8"))


def _public_session(trace: dict[str, Any]) -> dict[str, Any]:
    scenario = trace["scenario"]
    return {
        "session": {
            "scenario": scenario["id"],
            "variant": trace.get("variant"),
            "arm": trace["arm"],
            "seed": trace["seed"],
            "identity": trace["identity"],
        },
        "task": scenario["task"],
        "stimulus": scenario.get("stimulus", {}),
        "allowedTools": {name: TOOL_SCHEMAS[name] for name in scenario["allowed_tools"]},
        "completionContract": {
            "claimSchema": {
                "status": ["complete", "partial", "blocked"],
                "outcome": "free text, retained for review but not keyword-scored",
                "evidence": "array of concise evidence statements",
                "limitations": "array of concise unresolved limits",
            },
            "tokenMetrics": "unmeasured unless final receives authoritative provider usage",
        },
        "guidance": {
            "arm": trace["arm"],
            "rootBound": True,
            "readCommand": "card",
        },
    }


def start_session(args: argparse.Namespace) -> dict[str, Any]:
    path = _trace_path(args.session)
    if path.exists() and not args.overwrite:
        raise HarnessError("session trace already exists (use --overwrite to replace this trace only)")
    root = Path(args.guidance_root).expanduser().resolve()
    if not root.is_dir():
        raise HarnessError("guidance root must be an existing directory")
    fixtures = load_fixtures(Path(args.fixtures))
    scenario, context, variant = resolve_scenario(fixtures, args.scenario, args.seed)
    trace = {
        "traceVersion": 1,
        "fixtureVersion": fixtures["version"],
        "disclaimer": "Deterministic mock trace; not live TD or agent-behavior acceptance evidence.",
        "scenario": scenario,
        "variant": variant,
        "arm": args.arm,
        "seed": args.seed,
        "identity": context,
        "guidanceRoot": str(root),
        "state": copy.deepcopy(scenario["initial_state"]),
        "events": [],
        "observedActions": [],
        "metrics": {
            "toolRequests": 0,
            "realToolRequests": 0,
            "mockConvenienceRequests": 0,
            "acceptedToolRequests": 0,
            "invalidToolRequests": 0,
            "forbiddenToolRequests": 0,
            "toolSchemaReads": 0,
            "toolSchemaBytes": 0,
            "toolSchemaChars": 0,
            "guidanceCardRequests": 0,
            "guidanceCardBytes": 0,
            "guidanceCardChars": 0,
            "modelInputTokens": None,
            "modelOutputTokens": None,
            "tokenMeasurement": "unmeasured",
        },
        "finished": False,
        "score": None,
    }
    _record_schema_exposure(trace)
    _write_trace(path, trace)
    return _public_session(trace)


def show_session(args: argparse.Namespace) -> dict[str, Any]:
    trace_path = _trace_path(args.session)
    trace = _read_trace(trace_path)
    _record_schema_exposure(trace)
    result = _public_session(trace)
    result["finished"] = trace["finished"]
    result["metrics"] = trace["metrics"]
    _write_trace(trace_path, trace)
    return result


def read_card(args: argparse.Namespace) -> dict[str, Any]:
    trace_path = _trace_path(args.session)
    trace = _read_trace(trace_path)
    root = Path(trace["guidanceRoot"]).resolve()
    relative = Path(args.path)
    if relative.is_absolute() or ".." in relative.parts:
        raise HarnessError("card path must be relative and cannot traverse parents")
    card_path = (root / relative).resolve()
    if root != card_path and root not in card_path.parents:
        raise HarnessError("card path escapes the bound guidance root")
    if not card_path.is_file():
        raise HarnessError("card does not exist under the bound guidance root")
    if args.offset < 0 or not 1 <= args.max_bytes <= MAX_CARD_BYTES:
        raise HarnessError(f"offset must be nonnegative and max-bytes must be 1..{MAX_CARD_BYTES}")
    raw = card_path.read_bytes()
    try:
        full_text = raw.decode("utf-8")
    except UnicodeDecodeError as exc:
        raise HarnessError("guidance cards must be UTF-8 text") from exc
    if args.offset > len(raw):
        raise HarnessError("card byte offset exceeds total bytes")
    end = min(len(raw), args.offset + args.max_bytes)
    while end > args.offset:
        try:
            text = raw[args.offset:end].decode("utf-8")
            break
        except UnicodeDecodeError:
            end -= 1
    else:
        text = ""
    returned_bytes = end - args.offset
    event = {
        "kind": "card_read",
        "path": relative.as_posix(),
        "offsetBytes": args.offset,
        "returnedBytes": returned_bytes,
        "returnedChars": len(text),
        "totalBytes": len(raw),
        "totalChars": len(full_text),
        "sha256": hashlib.sha256(raw).hexdigest(),
        "truncated": end < len(raw),
    }
    trace["events"].append(event)
    trace["metrics"]["guidanceCardRequests"] += 1
    trace["metrics"]["guidanceCardBytes"] += returned_bytes
    trace["metrics"]["guidanceCardChars"] += len(text)
    _write_trace(trace_path, trace)
    return {**event, "content": text, "nextOffsetBytes": end if end < len(raw) else None}


def _request_from_args(args: argparse.Namespace) -> tuple[str, Any, str | None]:
    if args.json is not None and args.stdin:
        raise HarnessError("choose either --json or --stdin")
    raw = sys.stdin.read() if args.stdin else args.json
    if raw is None:
        raise HarnessError("a request is required via --json or --stdin")
    try:
        return raw, json.loads(raw), None
    except json.JSONDecodeError as exc:
        return raw, None, f"request is not JSON: {exc}"


def submit_request(args: argparse.Namespace) -> dict[str, Any]:
    trace_path = _trace_path(args.session)
    trace = _read_trace(trace_path)
    raw_request, parsed_request, parse_error = _request_from_args(args)
    scenario = trace["scenario"]
    event: dict[str, Any] = {
        "kind": "tool_request",
        "sequence": trace["metrics"]["toolRequests"] + 1,
        "request": parsed_request,
        "invalid": False,
        "forbidden": False,
        "action": None,
        "sideEffects": [],
    }
    trace["metrics"]["toolRequests"] += 1
    if parse_error is not None or not isinstance(parsed_request, dict):
        diagnostic = parse_error or "request must be a JSON object"
        event["requestRaw"] = raw_request
        event["invalid"] = True
        event["validationErrors"] = [diagnostic]
        event["response"] = {"ok": False, "code": "mock.request.invalid", "diagnostics": [diagnostic]}
        trace["metrics"]["invalidToolRequests"] += 1
        trace["events"].append(event)
        _write_trace(trace_path, trace)
        return {
            "response": event["response"],
            "requestValid": False,
            "requestForbidden": False,
            "metrics": trace["metrics"],
        }
    request = parsed_request
    tool = request.get("tool")
    arguments = request.get("arguments")
    if isinstance(tool, str) and tool in REAL_TOOL_SCHEMAS:
        trace["metrics"]["realToolRequests"] += 1
    elif isinstance(tool, str) and tool in MOCK_TOOL_SCHEMAS:
        trace["metrics"]["mockConvenienceRequests"] += 1
    errors: list[str] = []
    if set(request) - {"tool", "arguments"}:
        errors.append("request: only tool and arguments fields are allowed")
    if not isinstance(tool, str):
        errors.append("request.tool: expected string")
    elif tool not in scenario["allowed_tools"]:
        errors.append(f"request.tool: {tool!r} is not allowed in this scenario")
    if not isinstance(arguments, dict):
        errors.append("request.arguments: expected object")
    elif isinstance(tool, str) and tool in TOOL_SCHEMAS:
        errors.extend(_validate(arguments, TOOL_SCHEMAS[tool]))
    if errors:
        event["invalid"] = True
        event["validationErrors"] = errors
        trace["metrics"]["invalidToolRequests"] += 1

    forbidden = [row for row in scenario["forbidden_actions"] if _matches(row["match"], request)]
    if forbidden:
        event["forbidden"] = True
        event["forbiddenIds"] = [row["id"] for row in forbidden]
        event["response"] = {
            "ok": False,
            "code": "mock.policy.forbidden",
            "message": forbidden[0].get("reason", "request forbidden by scenario boundary"),
            "validationErrors": errors,
        }
        trace["metrics"]["forbiddenToolRequests"] += 1
    elif errors:
        event["response"] = {"ok": False, "code": "mock.request.invalid", "diagnostics": errors}
    elif trace["finished"]:
        event["response"] = {"ok": False, "code": "mock.session.finished", "message": "trial already finalized"}
    else:
        candidates = [row for row in scenario["expected_actions"] + scenario.get("other_actions", []) if _matches(row["match"], request)]
        ready_unseen = [
            row for row in candidates
            if row["id"] not in trace["observedActions"]
            and all(item in trace["observedActions"] for item in row.get("after", []))
        ]
        action = ready_unseen[0] if ready_unseen else (candidates[0] if candidates else None)
        if action is None:
            event["response"] = {
                "ok": False,
                "code": "mock.no_matching_response",
                "message": "The deterministic backend has no scripted response for this request.",
            }
        else:
            missing = [item for item in action.get("after", []) if item not in trace["observedActions"]]
            if missing:
                event["action"] = action["id"]
                event["response"] = {
                    "ok": False,
                    "code": "mock.precondition_failed",
                    "missingActions": missing,
                }
            else:
                event["action"] = action["id"]
                event["response"] = copy.deepcopy(action["response"])
                event["sideEffects"] = _apply_effects(trace["state"], action.get("effects", []))
                if action.get("required", True) and action["id"] not in trace["observedActions"]:
                    trace["observedActions"].append(action["id"])
                trace["metrics"]["acceptedToolRequests"] += 1
    trace["events"].append(event)
    _write_trace(trace_path, trace)
    return {
        "response": event["response"],
        "requestValid": not event["invalid"],
        "requestForbidden": event["forbidden"],
        "metrics": trace["metrics"],
    }


def _outcome_earned(outcome: dict[str, Any], trace: dict[str, Any]) -> bool:
    if any(action not in trace["observedActions"] for action in outcome.get("requires", [])):
        return False
    return all(_state_get(trace["state"], path) == expected for path, expected in outcome.get("state", {}).items())


def _parse_json_object(raw: str | None, name: str) -> dict[str, Any]:
    if raw is None:
        raise HarnessError(f"{name} is required")
    try:
        value = json.loads(raw)
    except json.JSONDecodeError as exc:
        raise HarnessError(f"{name} is not JSON: {exc}") from exc
    if not isinstance(value, dict):
        raise HarnessError(f"{name} must be a JSON object")
    return value


def finish_session(args: argparse.Namespace) -> dict[str, Any]:
    trace_path = _trace_path(args.session)
    trace = _read_trace(trace_path)
    if trace["finished"]:
        return trace["score"]
    claim = _parse_json_object(args.claim, "claim")
    if set(claim) - {"status", "outcome", "evidence", "limitations"}:
        raise HarnessError("claim accepts only status, outcome, evidence, and limitations")
    if claim.get("status") not in ("complete", "partial", "blocked"):
        raise HarnessError("claim.status must be complete, partial, or blocked")
    if not isinstance(claim.get("outcome"), str):
        raise HarnessError("claim.outcome must be a string")
    for key in ("evidence", "limitations"):
        if not isinstance(claim.get(key, []), list) or not all(isinstance(item, str) for item in claim.get(key, [])):
            raise HarnessError(f"claim.{key} must be an array of strings")

    if args.metrics is not None:
        metrics = _parse_json_object(args.metrics, "metrics")
        allowed = {"modelInputTokens", "modelOutputTokens", "source"}
        if set(metrics) - allowed or not isinstance(metrics.get("source"), str) or not metrics["source"].strip():
            raise HarnessError("metrics require a nonempty source and only authoritative token fields")
        for key in ("modelInputTokens", "modelOutputTokens"):
            value = metrics.get(key)
            if value is not None and (not isinstance(value, int) or isinstance(value, bool) or value < 0):
                raise HarnessError(f"metrics.{key} must be a nonnegative integer or null")
            trace["metrics"][key] = value
        trace["metrics"]["tokenMeasurement"] = {"kind": "authoritative", "source": metrics["source"]}

    scenario = trace["scenario"]
    required = [row["id"] for row in scenario["expected_actions"] if row.get("required", True)]
    missing_actions = [item for item in required if item not in trace["observedActions"]]
    earned = [row["id"] for row in scenario["expected_outcomes"] if _outcome_earned(row, trace)]
    expected_outcomes = [row["id"] for row in scenario["expected_outcomes"]]
    missing_outcomes = [item for item in expected_outcomes if item not in earned]
    forbidden_events = [event for event in trace["events"] if event.get("forbidden")]
    status_correct = claim["status"] == scenario["final_status"]

    action_ratio = (len(required) - len(missing_actions)) / max(1, len(required))
    outcome_ratio = (len(expected_outcomes) - len(missing_outcomes)) / max(1, len(expected_outcomes))
    claim_ratio = 1.0 if status_correct else 0.0
    safety_ratio = 1.0 if not forbidden_events else 0.0
    score_value = round(100 * (0.4 * action_ratio + 0.3 * outcome_ratio + 0.2 * claim_ratio + 0.1 * safety_ratio), 1)
    accepted = not missing_actions and not missing_outcomes and status_correct and not forbidden_events
    score = {
        "accepted": accepted,
        "score": score_value,
        "scenario": scenario["id"],
        "variant": trace.get("variant"),
        "arm": trace["arm"],
        "seed": trace["seed"],
        "claim": claim,
        "checks": {
            "expectedActionCount": len(required),
            "missingExpectedActionCount": len(missing_actions),
            "expectedOutcomeCount": len(expected_outcomes),
            "missingOutcomeCount": len(missing_outcomes),
            "statusCorrect": status_correct,
            "forbiddenAttempts": [{"sequence": row["sequence"]} for row in forbidden_events],
        },
        "metrics": trace["metrics"],
        "disclaimer": "Deterministic fixture result only; not agent behavior acceptance and not a live TD result.",
    }
    trace["events"].append({"kind": "final_claim", "claim": claim, "score": score_value, "accepted": accepted})
    trace["finished"] = True
    trace["score"] = score
    _write_trace(trace_path, trace)
    return score


def list_scenarios(args: argparse.Namespace) -> dict[str, Any]:
    data = load_fixtures(Path(args.fixtures))
    return {
        "scenarios": [{"id": row["id"], "title": row.get("title", ""), "rules": row.get("rules", [])} for row in data["scenarios"]],
        "count": len(data["scenarios"]),
        "disclaimer": "Fixture inventory only; no model or live trials were run.",
    }


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)

    listing = sub.add_parser("list", help="list deterministic scenarios")
    listing.add_argument("--fixtures", default=str(DEFAULT_FIXTURES))
    listing.set_defaults(handler=list_scenarios)

    start = sub.add_parser("start", help="create an isolated trial trace")
    start.add_argument("--session", required=True, help="absolute .json trace path under the system temp directory")
    start.add_argument("--scenario", required=True)
    start.add_argument("--arm", choices=ARMS, required=True)
    start.add_argument("--seed", type=int, required=True)
    start.add_argument("--guidance-root", required=True, help="read-only root bound to this A/B/C arm")
    start.add_argument("--fixtures", default=str(DEFAULT_FIXTURES))
    start.add_argument("--overwrite", action="store_true", help="replace only the designated trace")
    start.set_defaults(handler=start_session)

    show = sub.add_parser("show", help="show task, stimuli, and allowed schemas")
    show.add_argument("--session", required=True)
    show.set_defaults(handler=show_session)

    card = sub.add_parser("card", help="read a bounded UTF-8 guidance card slice")
    card.add_argument("--session", required=True)
    card.add_argument("--path", required=True)
    card.add_argument("--offset", type=int, default=0)
    card.add_argument("--max-bytes", type=int, default=65_536)
    card.set_defaults(handler=read_card)

    request = sub.add_parser("request", help="submit one mocked tool request")
    request.add_argument("--session", required=True)
    source = request.add_mutually_exclusive_group(required=True)
    source.add_argument("--json")
    source.add_argument("--stdin", action="store_true")
    request.set_defaults(handler=submit_request)

    final = sub.add_parser("final", help="finish and score a structured final claim")
    final.add_argument("--session", required=True)
    final.add_argument("--claim", required=True, help='JSON: {"status":...,"outcome":...,"evidence":[],"limitations":[]}')
    final.add_argument("--metrics", help="authoritative provider token usage JSON; otherwise tokens stay unmeasured")
    final.set_defaults(handler=finish_session)
    return parser


def main(argv: list[str] | None = None) -> int:
    parser = build_parser()
    args = parser.parse_args(argv)
    try:
        result = args.handler(args)
    except HarnessError as exc:
        print(json.dumps({"ok": False, "error": str(exc)}, sort_keys=True))
        return 2
    print(json.dumps({"ok": True, "result": result}, indent=2, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
