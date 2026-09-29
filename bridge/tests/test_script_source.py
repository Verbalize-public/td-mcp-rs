"""Named Python sources: registration, immutable refs, and recorded runs.

The properties pinned here are the ones whose failure is silent, plus the two
the plan names as acceptance: registration must never execute, and stale
dependency behaviour must be explicit.

No live TouchDesigner: the registry is pure, and `run` is asserted through a
patched `handle_execute_python` so the test proves the *forwarding* rather than
re-testing execution.
"""

from __future__ import annotations

import os
import sys
import unittest
from types import SimpleNamespace
from typing import Any
from unittest import mock

sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))

import tdmcp_bridge  # noqa: E402
from tdmcp_bridge import script_source  # noqa: E402
from tdmcp_bridge.script_source import SourceRegistry, source_id_for  # noqa: E402

DESTRUCTIVE = "op('/project1/never_touch_this').destroy()"


def fresh_registry() -> SourceRegistry:
    """A registry of its own, so tests cannot depend on each other's state."""
    return SourceRegistry()


class RegistrationTests(unittest.TestCase):
    def test_registering_returns_a_ref_and_never_executes(self) -> None:
        store = fresh_registry()
        # A script whose only effect would be to destroy an operator. If
        # registration executed it, the scene would change and nothing would
        # say so -- so execution is asserted absent, not merely unobserved.
        with mock.patch.object(script_source, "resolve_op") as resolve:
            out = store.register(DESTRUCTIVE, name="danger")
        self.assertTrue(out["ok"])
        self.assertFalse(out["executed"])
        resolve.assert_not_called()
        self.assertTrue(out["sourceId"].startswith("src_"))
        self.assertEqual(out["byteLength"], len(DESTRUCTIVE.encode("utf-8")))

    def test_a_ref_is_the_content_address_of_its_source(self) -> None:
        # The whole reason a ref cannot go stale: its identity IS its bytes, so
        # a holder can reconstruct the source from any peer without trusting a
        # registry index.
        store = fresh_registry()
        first = store.register("result = 1")
        second = store.register("result = 1")
        third = store.register("result = 2")
        self.assertEqual(first["sourceId"], second["sourceId"])
        self.assertNotEqual(first["sourceId"], third["sourceId"])
        self.assertEqual(first["sourceId"], source_id_for("result = 1"))
        self.assertEqual(store.code_for(first["sourceId"]), "result = 1")

    def test_reregistering_identical_bytes_keeps_the_original_registration(self) -> None:
        # Reporting a second registration as new would let a caller believe
        # they had versioned something they had not.
        store = fresh_registry()
        first = store.register("result = 1", name="original")
        again = store.register("result = 1", name="renamed")
        self.assertTrue(again["alreadyRegistered"])
        self.assertEqual(again["registeredAt"], first["registeredAt"])
        # The name recorded first is kept; the ref names content, not metadata.
        self.assertEqual(store.describe(first["sourceId"])["source"]["name"], "original")

    def test_the_registry_cap_is_a_limit_not_a_policy(self) -> None:
        store = fresh_registry()
        # Patched where it is USED: `from .constants import X` binds the value
        # at import time, so patching the constants module would be a no-op.
        with mock.patch.object(script_source, "SCRIPT_SOURCE_MAX_SOURCES", 2):
            self.assertTrue(store.register("a = 1")["ok"])
            self.assertTrue(store.register("b = 2")["ok"])
            refused = store.register("c = 3")
        self.assertFalse(refused["ok"])
        self.assertEqual(refused["code"], "tdmcp.script.source_limit")

    def test_a_dependency_cap_is_enforced(self) -> None:
        store = fresh_registry()
        deps = [f"/project1/node{index}" for index in range(4)]
        with mock.patch.object(script_source, "SCRIPT_SOURCE_MAX_DEPENDENCIES", 2):
            out = store.register("result = 1", dependencies=deps)
        self.assertFalse(out["ok"])
        self.assertEqual(out["code"], "tdmcp.script.too_many_dependencies")

    def test_a_bare_string_dependency_is_read_as_an_op_path(self) -> None:
        store = fresh_registry()
        out = store.register("result = 1", dependencies=["/project1/a"])
        self.assertEqual(out["dependencies"], [{"kind": "op", "target": "/project1/a"}])

    def test_a_malformed_dependency_is_refused_at_registration(self) -> None:
        store = fresh_registry()
        for bad, code in (
            (["/ok", ""], "tdmcp.script.bad_dependency"),
            ([{"kind": "module"}], "tdmcp.script.bad_dependency"),
            ([{"kind": "widget", "target": "x"}], "tdmcp.script.bad_dependency"),
            ("not-a-list", "tdmcp.script.bad_dependency"),
            ([42], "tdmcp.script.bad_dependency"),
        ):
            out = store.register("result = 1", dependencies=bad)
            self.assertFalse(out["ok"], bad)
            self.assertEqual(out["code"], code, bad)


class RosterTests(unittest.TestCase):
    def test_a_listing_never_echoes_the_source_body(self) -> None:
        # A listing is a roster. Echoing every script back would defeat the
        # point of naming a source instead of resending it.
        store = fresh_registry()
        store.register("result = 41 + 1")
        listing = store.list_sources()
        self.assertEqual(len(listing), 1)
        self.assertNotIn("code", listing[0])
        # Assert on the source text, not a substring: the sourceId is a hash and
        # can contain any digits, so a numeric probe is a coin flip.
        self.assertNotIn("result", str(listing))

    def test_describe_returns_the_body_because_that_is_what_it_is_for(self) -> None:
        store = fresh_registry()
        source_id = store.register("result = 1")["sourceId"]
        described = store.describe(source_id)
        self.assertTrue(described["ok"])
        self.assertEqual(described["source"]["code"], "result = 1")

    def test_an_unknown_ref_says_it_is_a_content_address(self) -> None:
        store = fresh_registry()
        out = store.describe("src_deadbeef")
        self.assertFalse(out["ok"])
        self.assertEqual(out["code"], "tdmcp.script.unknown_source")
        self.assertIn("content address", out["message"])

    def test_delete_forgets_the_registry_not_the_fact_it_ran(self) -> None:
        store = fresh_registry()
        source_id = store.register("result = 1")["sourceId"]
        out = store.delete(source_id)
        self.assertTrue(out["ok"])
        self.assertIn("nothing here undoes a previous run", out["note"])
        self.assertIsNone(store.get(source_id))
        self.assertFalse(store.delete(source_id)["ok"])


class DependencyTests(unittest.TestCase):
    def test_a_present_dependency_is_reported_present(self) -> None:
        store = fresh_registry()
        node = SimpleNamespace(valid=True, path="/project1/thing")
        source_id = store.register("result = 1", dependencies=["/project1/thing"])["sourceId"]
        with mock.patch.object(script_source, "resolve_op", return_value=node):
            deps = store.check_dependencies(source_id)
        self.assertEqual(deps[0]["status"], "present")

    def test_a_missing_dependency_is_reported_missing_not_assumed_present(self) -> None:
        store = fresh_registry()
        source_id = store.register("result = 1", dependencies=["/project1/gone"])["sourceId"]
        with mock.patch.object(script_source, "resolve_op", return_value=None):
            deps = store.check_dependencies(source_id)
        self.assertEqual(deps[0]["status"], "missing")

    def test_a_dependency_that_moved_is_reported_retargeted_not_satisfied(self) -> None:
        # "Something is at a path with that name" and "the thing I meant" are
        # different answers. A run that silently received the second one would
        # be working from a premise nobody checked.
        store = fresh_registry()
        source_id = store.register("result = 1", dependencies=["/project1/a"])["sourceId"]
        moved = SimpleNamespace(valid=True, path="/project1/renamed")
        with mock.patch.object(script_source, "resolve_op", return_value=moved):
            deps = store.check_dependencies(source_id)
        self.assertEqual(deps[0]["status"], "retargeted")
        self.assertEqual(deps[0]["resolvedPath"], "/project1/renamed")

    def test_an_invalid_node_counts_as_missing(self) -> None:
        store = fresh_registry()
        source_id = store.register("result = 1", dependencies=["/project1/a"])["sourceId"]
        with mock.patch.object(
            script_source, "resolve_op", return_value=SimpleNamespace(valid=False, path="")
        ):
            deps = store.check_dependencies(source_id)
        self.assertEqual(deps[0]["status"], "missing")

    def test_a_module_dependency_is_resolved_through_the_importer(self) -> None:
        store = fresh_registry()
        source_id = store.register(
            "import json", dependencies=[{"kind": "module", "target": "json"}]
        )["sourceId"]
        deps = store.check_dependencies(source_id)
        self.assertEqual(deps[0]["status"], "present")
        source_id = store.register(
            "import nope", dependencies=[{"kind": "module", "target": "tdmcp_no_such"}]
        )["sourceId"]
        self.assertEqual(store.check_dependencies(source_id)[0]["status"], "missing")

    def test_an_unknown_source_has_no_dependencies_to_check(self) -> None:
        self.assertEqual(fresh_registry().check_dependencies("src_nope"), [])


class RunTests(unittest.TestCase):
    def _registry_patch(self):
        """Point the handler at a registry this test owns."""
        store = fresh_registry()
        return store, mock.patch.object(
            script_source, "registry", return_value=store
        )

    def test_run_executes_through_the_inline_path_and_returns_its_result_unchanged(self) -> None:
        store, patch = self._registry_patch()
        source_id = store.register("result = 1")["sourceId"]
        inline = {"ok": True, "result": 1, "logs": "hello"}
        with patch, mock.patch(
            "tdmcp_bridge.execute.handle_execute_python", return_value=dict(inline)
        ) as run:
            out = script_source.handle_script_source({"action": "run", "sourceId": source_id})
        # The registered source reached the inline entry point as a script, with
        # no new global and no widened builtins.
        self.assertEqual(run.call_args.args[0]["script"], "result = 1")
        # Result, logs and ok all survive: a run is not a re-interpretation.
        self.assertEqual(out["result"], 1)
        self.assertEqual(out["logs"], "hello")
        self.assertTrue(out["ok"])

    def test_run_does_not_leak_registry_only_fields_into_the_script(self) -> None:
        store, patch = self._registry_patch()
        source_id = store.register("result = 1")["sourceId"]
        with patch, mock.patch(
            "tdmcp_bridge.execute.handle_execute_python", return_value={"ok": True, "result": 1}
        ) as run:
            script_source.handle_script_source(
                {
                    "action": "run",
                    "sourceId": source_id,
                    "name": "should not be forwarded",
                    "dependencies": ["/project1/a"],
                    "includeLogs": False,
                }
            )
        forwarded = run.call_args.args[0]
        for leaked in ("action", "sourceId", "name", "dependencies"):
            self.assertNotIn(leaked, forwarded, leaked)
        # A genuine execute_python option still passes through unchanged.
        self.assertIs(forwarded["includeLogs"], False)

    def test_a_failed_run_reports_an_unknown_effect_rather_than_none(self) -> None:
        # The exception says the script stopped, not that nothing happened
        # before it stopped.
        store, patch = self._registry_patch()
        source_id = store.register("op('/project1/x').destroy()")["sourceId"]
        failure = {"ok": False, "error": "boom", "traceback": "..."}
        with patch, mock.patch(
            "tdmcp_bridge.execute.handle_execute_python", return_value=dict(failure)
        ):
            out = script_source.handle_script_source({"action": "run", "sourceId": source_id})
        self.assertFalse(out["ok"])
        self.assertEqual(out["effect"], "unknown")
        self.assertEqual(out["error"], "boom")
        self.assertEqual(out["traceback"], "...")

    def test_a_completed_run_reports_an_unobserved_effect(self) -> None:
        # Nothing here inspects the scene afterwards, so "it ran" is not
        # "it changed the scene" and is not reported as if it were.
        store, patch = self._registry_patch()
        source_id = store.register("result = 1")["sourceId"]
        for extra in ({}, {"resultTruncated": True}):
            with patch, mock.patch(
                "tdmcp_bridge.execute.handle_execute_python",
                return_value={"ok": True, "result": 1, **extra},
            ):
                out = script_source.handle_script_source(
                    {"action": "run", "sourceId": source_id}
                )
            self.assertEqual(out["effect"], "unobserved", extra)

    def test_a_run_reports_dependency_staleness_beside_its_result(self) -> None:
        # Not withheld from the run -- refusing would be a policy this tool has
        # no basis for, and hiding it would be the dishonest option.
        store, patch = self._registry_patch()
        source_id = store.register("result = 1", dependencies=["/project1/gone"])["sourceId"]
        with patch, mock.patch(
            "tdmcp_bridge.execute.handle_execute_python", return_value={"ok": True, "result": 1}
        ), mock.patch.object(script_source, "resolve_op", return_value=None):
            out = script_source.handle_script_source({"action": "run", "sourceId": source_id})
        self.assertTrue(out["ok"])
        self.assertTrue(out["source"]["dependenciesStale"])
        self.assertEqual(len(out["source"]["missing"]), 1)

    def test_running_an_unknown_ref_is_refused_and_executes_nothing(self) -> None:
        with mock.patch("tdmcp_bridge.execute.handle_execute_python") as run:
            out = script_source.handle_script_source({"action": "run", "sourceId": "src_nope"})
        self.assertFalse(out["ok"])
        self.assertEqual(out["code"], "tdmcp.script.unknown_source")
        run.assert_not_called()


class HandlerActionTests(unittest.TestCase):
    def _registry_patch(self):
        store = fresh_registry()
        return store, mock.patch.object(
            script_source, "registry", return_value=store
        )

    def test_register_through_the_handler_never_executes(self) -> None:
        store, patch = self._registry_patch()
        with patch, mock.patch("tdmcp_bridge.execute.handle_execute_python") as run:
            out = script_source.handle_script_source(
                {"action": "register", "source": DESTRUCTIVE}
            )
        self.assertTrue(out["ok"])
        self.assertFalse(out["executed"])
        run.assert_not_called()

    def test_registration_is_the_default_action(self) -> None:
        # A caller that omits the action gets a registration, which is the
        # only non-executing default available here.
        store, patch = self._registry_patch()
        with patch, mock.patch("tdmcp_bridge.execute.handle_execute_python") as run:
            out = script_source.handle_script_source({"source": "result = 1"})
        self.assertTrue(out["ok"])
        self.assertFalse(out["executed"])
        run.assert_not_called()

    def test_check_reports_staleness_without_refusing(self) -> None:
        store, patch = self._registry_patch()
        source_id = store.register(
            "result = 1", dependencies=["/project1/a", "/project1/b"]
        )["sourceId"]
        def resolve(path, _ctx):
            if path == "/project1/a":
                return SimpleNamespace(valid=True, path="/project1/a")
            return None
        with patch, mock.patch.object(script_source, "resolve_op", side_effect=resolve):
            out = script_source.handle_script_source(
                {"action": "check", "sourceId": source_id}
            )
        self.assertTrue(out["ok"])
        self.assertTrue(out["stale"])
        self.assertEqual(len(out["missing"]), 1)
        self.assertEqual(out["retargeted"], [])

    def test_check_on_an_unknown_source_is_refused(self) -> None:
        store, patch = self._registry_patch()
        with patch:
            out = script_source.handle_script_source(
                {"action": "check", "sourceId": "src_nope"}
            )
        self.assertFalse(out["ok"])
        self.assertEqual(out["code"], "tdmcp.script.unknown_source")

    def test_an_action_needing_a_ref_says_so_when_it_is_missing(self) -> None:
        store, patch = self._registry_patch()
        with patch:
            for action in ("run", "describe", "delete", "check"):
                out = script_source.handle_script_source({"action": action})
                self.assertFalse(out["ok"], action)
                self.assertEqual(out["code"], "tdmcp.script.missing_source_id", action)

    def test_an_unknown_action_is_refused_by_name(self) -> None:
        store, patch = self._registry_patch()
        with patch:
            out = script_source.handle_script_source({"action": "compile"})
        self.assertFalse(out["ok"])
        self.assertEqual(out["code"], "tdmcp.script.unknown_action")
        self.assertIn("register", out["message"])

    def test_list_and_delete_go_through_the_handler(self) -> None:
        store, patch = self._registry_patch()
        source_id = store.register("result = 1")["sourceId"]
        with patch:
            listing = script_source.handle_script_source({"action": "list"})
            self.assertEqual(len(listing["sources"]), 1)
            described = script_source.handle_script_source(
                {"action": "describe", "sourceId": source_id}
            )
            self.assertEqual(described["source"]["code"], "result = 1")
            deleted = script_source.handle_script_source(
                {"action": "delete", "sourceId": source_id}
            )
        self.assertTrue(deleted["ok"])
        self.assertIsNone(store.get(source_id))

    def test_script_source_is_a_registered_route(self) -> None:
        # A capability is claimed from a route that exists.
        self.assertIn("script_source", tdmcp_bridge.HANDLERS)
        self.assertIs(
            tdmcp_bridge.HANDLERS["script_source"],
            script_source.handle_script_source,
        )

    def test_inline_control_still_works_through_its_own_route(self) -> None:
        # W8's acceptance includes "inline control still works". Registering a
        # source must not have changed the inline path in any way.
        self.assertIn("execute_python", tdmcp_bridge.HANDLERS)
        self.assertIs(
            tdmcp_bridge.HANDLERS["execute_python"],
            tdmcp_bridge.HANDLERS["execute_python"],
        )
        import tdmcp_bridge.execute as _execute

        self.assertIs(
            tdmcp_bridge.HANDLERS["execute_python"], _execute.handle_execute_python
        )


class RegistrySingletonTests(unittest.TestCase):
    def test_the_registry_is_shared_per_bridge_package(self) -> None:
        # So a source registered through one call is resolvable from the next,
        # which is the entire point of naming it.
        first = script_source.registry()
        second = script_source.registry()
        self.assertIs(first, second)

    def test_a_test_can_replace_the_registry_without_disturbing_others(self) -> None:
        # `state` is imported inside `registry()`, so the seam is the package
        # attribute it reads -- which is the same monkeypatch-friendly shape
        # `state.py` itself uses.
        import tdmcp_bridge as _pkg
        from tdmcp_bridge import state as _state

        original = getattr(_state._pkg(), "_script_sources", None)
        replacement = SourceRegistry()
        setattr(_state._pkg(), "_script_sources", replacement)
        try:
            self.assertIs(script_source.registry(), replacement)
        finally:
            if original is None:
                try:
                    delattr(_state._pkg(), "_script_sources")
                except AttributeError:
                    pass
            else:
                setattr(_state._pkg(), "_script_sources", original)

    def test_a_ref_survives_because_the_bytes_do(self) -> None:
        # The reconstruction property that makes a ref trustworthy: any peer can
        # recompute the same id from the same source, so nothing about the
        # registry needs to be believed.
        source = "result = 6 * 7"
        self.assertEqual(source_id_for(source), source_id_for(source))
        self.assertNotEqual(source_id_for(source), source_id_for(source + "\n"))
        # And the digest is a real content address, not a counter.
        self.assertRegex(source_id_for(source), r"^src_[0-9a-f]{32}$")


if __name__ == "__main__":
    unittest.main()
