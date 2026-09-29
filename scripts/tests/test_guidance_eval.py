"""Deterministic D4 mock tests; these are not model-behavior acceptance."""
import argparse
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest

from jsonschema import Draft202012Validator

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location("guidance_eval", ROOT / "scripts/guidance_eval.py")
assert SPEC is not None and SPEC.loader is not None
guidance_eval = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(guidance_eval)


def ns(**values):
    return argparse.Namespace(**values)


class GuidanceEvalTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.cards = self.root / "cards"
        self.cards.mkdir()
        (self.cards / "operate.md").write_text("alpha βeta\n", encoding="utf-8")

    def start(self, scenario="S03", arm="C", seed=17, name="session.json"):
        session = self.root / name
        result = guidance_eval.start_session(ns(
            session=str(session), scenario=scenario, arm=arm, seed=seed,
            guidance_root=str(self.cards), fixtures=str(guidance_eval.DEFAULT_FIXTURES),
            overwrite=False,
        ))
        return session, result

    def request(self, session, payload):
        return guidance_eval.submit_request(ns(
            session=str(session), json=json.dumps(payload), stdin=False,
        ))

    def finish(self, session, status, metrics=None):
        return guidance_eval.finish_session(ns(
            session=str(session),
            claim=json.dumps({
                "status": status,
                "outcome": "Concise model-supplied outcome; not keyword-scored.",
                "evidence": ["See recorded tool responses."],
                "limitations": [],
            }),
            metrics=json.dumps(metrics) if metrics is not None else None,
        ))

    @staticmethod
    def concrete(value):
        """Create one schema-valid representative for the fixture matching DSL."""
        if isinstance(value, dict):
            if "$contains" in value:
                return value["$contains"]
            if "$setContains" in value:
                return value["$setContains"]
            if "$in" in value:
                return value["$in"][0]
            if "$python" in value:
                spec = value["$python"]
                lines = [f"import {name}" for name in spec.get("imports", [])]
                lines.extend(repr(text) for text in spec.get("strings", []))
                lines.extend(f"{call}()" for call in spec.get("calls", []))
                lines.extend(
                    f"_ = obj.{name}" for name in spec.get("attributes", []) if "." not in name
                )
                lines.extend(f"_ = {name}" for name in spec.get("names", []))
                if spec.get("assignsResult"):
                    lines.append("result = None")
                return "\n".join(lines) or "result = None"
            return {
                key: GuidanceEvalTests.concrete(item)
                for key, item in value.items()
                if key != "$absent"
            }
        if isinstance(value, list):
            return [GuidanceEvalTests.concrete(item) for item in value]
        return value

    @staticmethod
    def all_resolved_scenarios():
        data = guidance_eval.load_fixtures()
        for raw in data["scenarios"]:
            wanted = [None] if not raw.get("variants") else [item["id"] for item in raw["variants"]]
            seeds = {}
            for seed in range(500):
                resolved, _identity, variant = guidance_eval.resolve_scenario(data, raw["id"], seed)
                if variant in wanted and variant not in seeds:
                    seeds[variant] = (seed, resolved)
                if len(seeds) == len(wanted):
                    break
            if set(seeds) != set(wanted):
                raise AssertionError(f"could not select every variant for {raw['id']}")
            for variant, (seed, resolved) in seeds.items():
                yield raw["id"], variant, seed, resolved

    def test_fixture_covers_all_scenarios_with_explicit_oracles(self):
        data = guidance_eval.load_fixtures()
        # Open-ended, but still SEQUENTIAL -- `validate_fixtures` enforces that.
        # The assertion is about the property rather than a pinned count: a
        # hard-coded `range(1, 34)` made every scenario added after this one a
        # change to the test, and a test that must be edited before it can
        # accept a new case tends to stop being written at all.
        ids = [row["id"] for row in data["scenarios"]]
        self.assertEqual(
            [f"S{index:02d}" for index in range(1, len(ids) + 1)], ids
        )
        for row in data["scenarios"]:
            with self.subTest(row=row["id"]):
                self.assertTrue(row["expected_actions"])
                self.assertTrue(row["forbidden_actions"])
                self.assertTrue(row["expected_outcomes"])
                resolved, identity, _variant = guidance_eval.resolve_scenario(data, row["id"], 918_273)
                self.assertIsInstance(identity["pid"], int)
                self.assertNotIn("{{", json.dumps(resolved))

    def test_all_real_tools_use_checked_in_generated_schemas(self):
        schema_paths = sorted(guidance_eval.SCHEMA_DIR.glob("*.json"))
        self.assertEqual(set(guidance_eval.REAL_TOOL_SCHEMAS), {path.stem for path in schema_paths})
        for path in schema_paths:
            self.assertEqual(
                guidance_eval.REAL_TOOL_SCHEMAS[path.stem],
                json.loads(path.read_text(encoding="utf-8")),
            )
        for _sid, _variant, _seed, scenario in self.all_resolved_scenarios():
            for tool in scenario["allowed_tools"]:
                self.assertTrue(
                    tool in guidance_eval.REAL_TOOL_SCHEMAS or tool.startswith("mock_"),
                    f"non-TD convenience must be mock_-prefixed: {tool}",
                )

    def test_every_expected_real_request_validates_against_generated_schema(self):
        checked = 0
        for sid, variant, _seed, scenario in self.all_resolved_scenarios():
            for action in scenario["expected_actions"]:
                request = self.concrete(action["match"])
                tool = request["tool"]
                if tool not in guidance_eval.REAL_TOOL_SCHEMAS:
                    continue
                errors = sorted(
                    Draft202012Validator(guidance_eval.REAL_TOOL_SCHEMAS[tool]).iter_errors(
                        request["arguments"]
                    ),
                    key=lambda error: list(error.absolute_path),
                )
                self.assertEqual([], [error.message for error in errors], (sid, variant, action["id"]))
                checked += 1
        self.assertGreater(checked, 50)

    def test_real_tool_mock_responses_use_current_outer_shapes(self):
        for sid, variant, _seed, scenario in self.all_resolved_scenarios():
            for action in scenario["expected_actions"]:
                tool = action["match"]["tool"]
                response = action["response"]
                with self.subTest(scenario=sid, variant=variant, action=action["id"]):
                    if tool == "inspect" and response.get("ok") is True:
                        self.assertIsInstance(response.get("nodes"), list)
                        self.assertNotIn("params", response)
                        for node in response["nodes"]:
                            for parameter in node.get("params", []):
                                self.assertIn("val", parameter)
                                self.assertIn("evaluation", parameter)
                                self.assertNotIn("value", parameter)
                    elif tool == "mutate_nodes":
                        self.assertIn("applied", response)
                        self.assertIn("failedAt", response)
                        self.assertIsInstance(response.get("steps"), list)
                    elif tool == "api_help" and response.get("ok") is True:
                        self.assertIn("results", response)
                        self.assertNotIn("queries", response)
                    elif tool == "fleet":
                        self.assertIn("processes", response)
                    elif tool == "capture" and response.get("ok") is True:
                        action_name = action["match"].get("arguments", {}).get("action", "start")
                        if "timing" in action["match"].get("arguments", {}) or action_name in ("status", "cancel"):
                            self.assertIn("jobId", response)
                            self.assertIn("state", response)
                        else:
                            self.assertIn("path", response)
                            self.assertIn("bytes", response)
                            self.assertEqual(response.get("mimeType"), "image/png")

    def test_every_scenario_variant_oracle_path_is_independently_executable(self):
        serial = 0
        for sid, variant, seed, _resolved in self.all_resolved_scenarios():
            serial += 1
            with self.subTest(scenario=sid, variant=variant):
                session, _public = self.start(sid, seed=seed, name=f"oracle-{serial}.json")
                scenario = json.loads(session.read_text())["scenario"]
                for action in scenario["expected_actions"]:
                    visible = self.request(session, self.concrete(action["match"]))
                    self.assertNotIn("action", visible)
                    self.assertNotIn("currentState", visible)
                    trace = json.loads(session.read_text())
                    self.assertEqual(trace["events"][-1]["action"], action["id"])
                score = self.finish(session, scenario["final_status"])
                self.assertTrue(score["accepted"], score)

    def test_public_surface_hides_internal_state_and_oracle(self):
        session, public = self.start("S03", seed=1)
        self.assertNotIn("currentState", public)
        self.assertNotIn("oracle", public)
        self.assertNotIn("outcomes", public["completionContract"]["claimSchema"])
        shown = guidance_eval.show_session(ns(session=str(session)))
        self.assertNotIn("currentState", shown)
        self.assertNotIn("oracle", shown)
        self.assertEqual(shown["metrics"]["toolSchemaReads"], 2)

    def test_successful_scalar_flow_accepts_equivalent_include_and_real_mutation(self):
        session, public = self.start("S03", seed=41)
        pid = public["session"]["identity"]["pid"]
        path = public["session"]["identity"]["node"]
        read = {
            "tool": "inspect",
            "arguments": {
                "pid": pid, "paths": [path],
                "include": ["params", "errors", "warnings"], "paramNames": ["Gain"],
            },
        }
        mutate = {
            "tool": "mutate_nodes",
            "arguments": {"pid": pid, "steps": [{"op": "set", "path": path, "values": {"Gain": 0.8}}]},
        }
        self.assertTrue(self.request(session, read)["response"]["ok"])
        changed = self.request(session, mutate)
        self.assertEqual(changed["response"]["applied"], 1)
        self.assertTrue(self.request(session, read)["response"]["ok"])
        trace = json.loads(session.read_text())
        self.assertEqual(trace["state"]["target"]["gain"], 0.8)
        score = self.finish(session, "complete")
        self.assertTrue(score["accepted"])
        self.assertEqual(score["score"], 100.0)
        self.assertEqual(score["metrics"]["tokenMeasurement"], "unmeasured")

    def test_missing_actions_make_complete_claim_fail_without_prose_heuristic(self):
        session, _public = self.start("S03", seed=2)
        score = self.finish(session, "complete")
        self.assertFalse(score["accepted"])
        self.assertGreater(score["checks"]["missingExpectedActionCount"], 0)
        self.assertGreater(score["checks"]["missingOutcomeCount"], 0)

    def test_forbidden_invalid_attempt_is_logged_with_no_side_effect(self):
        session, public = self.start("S05", seed=9)
        path = public["session"]["identity"]["node"]
        visible = self.request(session, {"tool": "capture", "arguments": {"path": path, "mode": "preview"}})
        self.assertTrue(visible["requestForbidden"])
        self.assertFalse(visible["requestValid"])
        self.assertEqual(visible["response"]["code"], "mock.policy.forbidden")
        trace = json.loads(session.read_text())
        event = trace["events"][-1]
        self.assertTrue(event["forbidden"])
        self.assertTrue(event["invalid"])
        self.assertEqual(event["sideEffects"], [])
        self.assertEqual(trace["state"]["captures"], 0)

    def test_malformed_json_attempt_is_still_logged(self):
        session, _public = self.start("S03", seed=10)
        visible = guidance_eval.submit_request(ns(session=str(session), json="{bad", stdin=False))
        self.assertFalse(visible["requestValid"])
        self.assertEqual(visible["response"]["code"], "mock.request.invalid")
        trace = json.loads(session.read_text())
        self.assertEqual(trace["metrics"]["toolRequests"], 1)
        self.assertEqual(trace["events"][-1]["requestRaw"], "{bad")

    def test_partial_effect_recovery_repairs_only_remainder(self):
        session, public = self.start("S10", seed=123)
        pid = public["session"]["identity"]["pid"]
        path = public["session"]["identity"]["node"]
        inspect = {"tool": "inspect", "arguments": {"pid": pid, "paths": [path], "include": ["params"], "paramNames": ["Gain", "Bypass"]}}
        repair = {"tool": "mutate_nodes", "arguments": {"pid": pid, "steps": [{"op": "set", "path": path, "values": {"Bypass": True}}]}}
        self.request(session, inspect)
        self.request(session, repair)
        self.request(session, inspect)
        trace = json.loads(session.read_text())
        self.assertEqual(trace["state"], {"gain": 0.6, "bypass": True, "scriptReplays": 0})
        self.assertTrue(self.finish(session, "complete")["accepted"])

    def test_python_ast_matching_accepts_equivalent_imports_and_never_executes(self):
        session, public = self.start("S08", seed=33)
        pid = public["session"]["identity"]["pid"]
        equivalent = {
            "tool": "execute_python",
            "arguments": {
                "pid": pid,
                "script": "import json\nimport math\nvalue = math.sqrt(9)\nresult = json.dumps({'value': value})",
            },
        }
        self.assertTrue(self.request(session, equivalent)["response"]["ok"])

        other, other_public = self.start("S08", seed=34, name="no-exec.json")
        sentinel = self.root / "must-not-exist"
        payload = {
            "tool": "execute_python",
            "arguments": {
                "pid": other_public["session"]["identity"]["pid"],
                "script": f"open({str(sentinel)!r}, 'w').write('bad')",
            },
        }
        result = self.request(other, payload)
        self.assertEqual(result["response"]["code"], "mock.no_matching_response")
        self.assertFalse(sentinel.exists())

    def test_card_reads_are_bounded_and_metrics_distinguish_bytes_chars_sources(self):
        session, _public = self.start("S01", seed=7)
        result = guidance_eval.read_card(ns(session=str(session), path="operate.md", offset=0, max_bytes=1024))
        self.assertEqual(result["content"], "alpha βeta\n")
        self.assertGreater(result["returnedBytes"], result["returnedChars"])
        trace = json.loads(session.read_text())
        self.assertEqual(trace["metrics"]["guidanceCardBytes"], result["returnedBytes"])
        self.assertEqual(trace["metrics"]["guidanceCardChars"], result["returnedChars"])
        self.assertEqual(trace["metrics"]["toolSchemaReads"], 1)
        self.assertGreater(trace["metrics"]["toolSchemaBytes"], 0)
        self.assertGreaterEqual(trace["metrics"]["toolSchemaBytes"], trace["metrics"]["toolSchemaChars"])
        self.assertEqual(trace["metrics"]["mockConvenienceRequests"], 0)
        with self.assertRaisesRegex(guidance_eval.HarnessError, "cannot traverse"):
            guidance_eval.read_card(ns(session=str(session), path="../outside.md", offset=0, max_bytes=1024))

    def test_arm_changes_guidance_only_not_mock_backend(self):
        publics = [self.start("S03", arm=arm, seed=555, name=f"{arm}.json")[1] for arm in ("A", "B", "C")]
        self.assertEqual([item["allowedTools"] for item in publics], [publics[0]["allowedTools"]] * 3)
        self.assertEqual([item["session"]["identity"] for item in publics], [publics[0]["session"]["identity"]] * 3)

    def test_authoritative_tokens_are_optional_and_source_labeled(self):
        session, public = self.start("S01", seed=71)
        project = public["session"]["identity"]["project"]
        self.request(session, {"tool": "mock_project_io", "arguments": {"action": "explain", "path": project}})
        score = self.finish(session, "complete", {
            "modelInputTokens": 123, "modelOutputTokens": 45, "source": "provider usage object"
        })
        self.assertTrue(score["accepted"])
        self.assertEqual(score["metrics"]["modelInputTokens"], 123)
        self.assertEqual(score["metrics"]["tokenMeasurement"]["kind"], "authoritative")
        self.assertEqual(score["metrics"]["mockConvenienceRequests"], 1)
        self.assertEqual(score["metrics"]["realToolRequests"], 0)

    def test_trace_must_be_designated_under_system_temp(self):
        with self.assertRaisesRegex(guidance_eval.HarnessError, "must be under"):
            guidance_eval._trace_path(str(ROOT / "not-a-session.json"))


if __name__ == "__main__":
    unittest.main()
