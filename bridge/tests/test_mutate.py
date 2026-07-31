"""Unit tests for mutate_nodes apply_step seam (no live TD required)."""

from __future__ import annotations

import os
import sys
import unittest
from types import SimpleNamespace
from typing import Any

sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))

import tdmcp_bridge  # noqa: E402


class FakePar:
    def __init__(self, val: Any = None) -> None:
        self.val = val
        self.mode: Any = "CONSTANT"
        self.expr: str | None = None
        self._pulsed = 0

    def pulse(self) -> None:
        self._pulsed += 1


class FakeParGroup:
    def __init__(self, names: dict[str, FakePar] | None = None) -> None:
        self._pars = names or {}
        for k, v in self._pars.items():
            setattr(self, k, v)

    def __getattr__(self, name: str) -> Any:
        if name.startswith("_"):
            raise AttributeError(name)
        return None


class FakeConnector:
    def __init__(self, owner: "FakeNode", *, kind: str, index: int) -> None:
        self.owner = owner
        self.kind = kind
        self.index = index
        self.connections: list[Any] = []

    def connect(self, target: Any) -> None:
        self.connections.append(target)
        # Mirror TD: connecting an output to an input also records on the input.
        if self.kind == "out" and isinstance(target, FakeConnector):
            target.connections.append(self)

    def disconnect(self) -> None:
        self.connections.clear()


class FakeNode:
    def __init__(
        self,
        path: str,
        *,
        op_types: dict[str, Any] | None = None,
        n_inputs: int = 1,
        n_outputs: int = 1,
    ) -> None:
        self.path = path
        self.par = FakeParGroup()
        self._destroyed = False
        self._children: dict[str, FakeNode] = {}
        self._op_types = op_types or {}
        self.valid = True
        self.inputConnectors = [
            FakeConnector(self, kind="in", index=i) for i in range(n_inputs)
        ]
        self.outputConnectors = [
            FakeConnector(self, kind="out", index=i) for i in range(n_outputs)
        ]

    def pars(self) -> list[Any]:
        return [SimpleNamespace(name=k) for k in self.par._pars]

    def create(self, op_cls: Any, name: str) -> FakeNode:
        # Name taken → allocate a different leaf (simple counter; not full TD parity).
        actual = name
        if actual in self._children:
            base = name.rstrip("0123456789") or name
            n = 2
            while True:
                candidate = f"{base}{n}"
                if candidate not in self._children:
                    actual = candidate
                    break
                n += 1
        child_path = f"{self.path.rstrip('/')}/{actual}"
        child = FakeNode(child_path, op_types=self._op_types)
        # Seed a few common pars for set tests.
        child.par = FakeParGroup(
            {
                "resolutionw": FakePar(256),
                "resolutionh": FakePar(256),
                "pulse1": FakePar(0),
            }
        )
        self._children[actual] = child
        return child

    def destroy(self) -> None:
        if self._destroyed:
            raise RuntimeError("already destroyed")
        self._destroyed = True


class FakeCtx(tdmcp_bridge.MutateContext):
    def __init__(self) -> None:
        self.nodes: dict[str, FakeNode] = {}
        self.op_types: dict[str, Any] = {"noiseTOP": object()}
        root = FakeNode("/project1", op_types=self.op_types)
        self.nodes["/project1"] = root

    def resolve(self, path: str) -> Any | None:
        node = self.nodes.get(path)
        if node is not None and getattr(node, "_destroyed", False):
            return None
        return node

    def get_op_type(self, op_type: str) -> Any | None:
        return self.op_types.get(op_type)

    def list_op_type_names(self) -> list[str]:
        return list(self.op_types.keys())

    def expression_mode(self) -> Any:
        return "EXPRESSION"

    def track(self, node: FakeNode) -> FakeNode:
        self.nodes[node.path] = node
        parent_path, leaf = tdmcp_bridge._parent_and_name(node.path)
        parent = self.nodes.get(parent_path)
        if parent is not None and leaf:
            parent._children[leaf] = node
        return node

    def enable_create_tracking(self) -> FakeNode:
        """Wrap root create so children are registered for later resolve."""
        parent = self.nodes["/project1"]
        orig = parent.create

        def create_and_track(op_cls: Any, name: str) -> FakeNode:
            return self.track(orig(op_cls, name))

        parent.create = create_and_track  # type: ignore[method-assign]
        return parent


class MutateCreateTest(unittest.TestCase):
    def test_create_ok(self) -> None:
        ctx = FakeCtx()
        # Wrap create so the child is registered for later resolve.
        parent = ctx.nodes["/project1"]
        orig_create = parent.create

        def create_and_track(op_cls: Any, name: str) -> FakeNode:
            child = orig_create(op_cls, name)
            return ctx.track(child)

        parent.create = create_and_track  # type: ignore[method-assign]

        out = tdmcp_bridge.apply_step(
            ctx,
            {"op": "create", "path": "/project1/noise1", "opType": "noiseTOP"},
        )
        self.assertTrue(out["ok"])
        self.assertEqual(out["path"], "/project1/noise1")
        self.assertIn("/project1/noise1", ctx.nodes)

    def test_create_unknown_type(self) -> None:
        ctx = FakeCtx()
        out = tdmcp_bridge.apply_step(
            ctx,
            {"op": "create", "path": "/project1/x", "opType": "notARealOP"},
        )
        self.assertFalse(out["ok"])
        self.assertEqual(out["code"], "tdmcp.op.unknown_type")

    def test_create_parent_missing(self) -> None:
        ctx = FakeCtx()
        out = tdmcp_bridge.apply_step(
            ctx,
            {"op": "create", "path": "/missing/child", "opType": "noiseTOP"},
        )
        self.assertFalse(out["ok"])
        self.assertEqual(out["code"], "tdmcp.op.not_found")

    def test_create_with_values(self) -> None:
        ctx = FakeCtx()
        parent = ctx.nodes["/project1"]
        orig = parent.create

        def create_and_track(op_cls: Any, name: str) -> FakeNode:
            return ctx.track(orig(op_cls, name))

        parent.create = create_and_track  # type: ignore[method-assign]
        out = tdmcp_bridge.apply_step(
            ctx,
            {
                "op": "create",
                "path": "noise1",
                "opType": "noiseTOP",
                "values": {"resolutionw": 128},
            },
            context_path="/project1",
            detail_level="detailed",
        )
        self.assertTrue(out["ok"])
        self.assertEqual(out["values"], {"resolutionw": 128})
        self.assertEqual(ctx.nodes["/project1/noise1"].par.resolutionw.val, 128)

    def test_create_with_flags(self) -> None:
        ctx = FakeCtx()
        parent = ctx.nodes["/project1"]
        orig = parent.create

        def create_and_track(op_cls: Any, name: str) -> FakeNode:
            return ctx.track(orig(op_cls, name))

        parent.create = create_and_track  # type: ignore[method-assign]
        out = tdmcp_bridge.apply_step(
            ctx,
            {
                "op": "create",
                "path": "noise1",
                "opType": "noiseTOP",
                "flags": {"viewer": True, "display": True},
            },
            context_path="/project1",
            detail_level="detailed",
        )
        self.assertTrue(out["ok"])
        self.assertEqual(out["flags"], {"viewer": True, "display": True})
        node = ctx.nodes["/project1/noise1"]
        self.assertTrue(node.viewer)
        self.assertTrue(node.display)

    def test_create_rename_emits_lint(self) -> None:
        ctx = FakeCtx()
        ctx.enable_create_tracking()
        occupant = FakeNode("/project1/null1")
        ctx.track(occupant)
        out = tdmcp_bridge.apply_step(
            ctx,
            {"op": "create", "path": "/project1/null1", "opType": "noiseTOP"},
        )
        self.assertTrue(out["ok"])
        self.assertEqual(out["path"], "/project1/null2")
        self.assertNotEqual(out["path"], "/project1/null1")
        lints = out.get("lints") or []
        self.assertEqual(len(lints), 1)
        self.assertEqual(lints[0]["code"], "tdmcp.op.renamed")
        self.assertIn("requested", lints[0]["message"])
        self.assertEqual(lints[0]["suggestion"]["opPath"], "/project1/null2")
        self.assertEqual(lints[0]["suggestion"]["replace"], "/project1/null2")
        self.assertIn("/project1/null2", ctx.nodes)
        self.assertIs(ctx.nodes["/project1/null1"], occupant)

    def test_create_no_rename_no_lints(self) -> None:
        ctx = FakeCtx()
        ctx.enable_create_tracking()
        out = tdmcp_bridge.apply_step(
            ctx,
            {"op": "create", "path": "/project1/noise1", "opType": "noiseTOP"},
        )
        self.assertTrue(out["ok"])
        self.assertEqual(out["path"], "/project1/noise1")
        self.assertNotIn("lints", out)

    def test_create_bad_values_rolls_back(self) -> None:
        ctx = FakeCtx()
        parent = ctx.nodes["/project1"]
        orig = parent.create
        created_holder: list[FakeNode] = []

        def create_and_track(op_cls: Any, name: str) -> FakeNode:
            child = ctx.track(orig(op_cls, name))
            created_holder.append(child)
            return child

        parent.create = create_and_track  # type: ignore[method-assign]
        out = tdmcp_bridge.apply_step(
            ctx,
            {
                "op": "create",
                "path": "noise1",
                "opType": "noiseTOP",
                "values": {"resolutionw": 128, "nope": 1},
            },
            context_path="/project1",
        )
        self.assertFalse(out["ok"])
        self.assertEqual(out["code"], "tdmcp.par.unknown")
        self.assertEqual(out["field"], "nope")
        self.assertEqual(len(created_holder), 1)
        self.assertTrue(created_holder[0]._destroyed)
        self.assertIsNone(ctx.resolve("/project1/noise1"))

    def test_create_bad_flags_rolls_back(self) -> None:
        ctx = FakeCtx()
        parent = ctx.nodes["/project1"]
        orig = parent.create
        created_holder: list[FakeNode] = []

        def create_and_track(op_cls: Any, name: str) -> FakeNode:
            child = ctx.track(orig(op_cls, name))
            created_holder.append(child)
            return child

        parent.create = create_and_track  # type: ignore[method-assign]
        out = tdmcp_bridge.apply_step(
            ctx,
            {
                "op": "create",
                "path": "noise1",
                "opType": "noiseTOP",
                "flags": {"selected": True},
            },
            context_path="/project1",
        )
        self.assertFalse(out["ok"])
        self.assertEqual(out["code"], "tdmcp.flag.unknown")
        self.assertTrue(created_holder[0]._destroyed)
        self.assertIsNone(ctx.resolve("/project1/noise1"))

    def test_create_rollback_destroy_failure_keeps_original_error(self) -> None:
        ctx = FakeCtx()
        parent = ctx.nodes["/project1"]
        orig = parent.create

        def create_and_track(op_cls: Any, name: str) -> FakeNode:
            child = ctx.track(orig(op_cls, name))

            def boom() -> None:
                raise RuntimeError("destroy blew up")

            child.destroy = boom  # type: ignore[method-assign]
            return child

        parent.create = create_and_track  # type: ignore[method-assign]
        out = tdmcp_bridge.apply_step(
            ctx,
            {
                "op": "create",
                "path": "noise1",
                "opType": "noiseTOP",
                "values": {"nope": 1},
            },
            context_path="/project1",
        )
        self.assertFalse(out["ok"])
        self.assertEqual(out["code"], "tdmcp.par.unknown")
        self.assertEqual(out["field"], "nope")
        self.assertNotIn("destroy blew up", out.get("message", ""))


class MutateSetTest(unittest.TestCase):
    def _node(self) -> tuple[FakeCtx, FakeNode]:
        ctx = FakeCtx()
        node = FakeNode("/project1/noise1")
        node.par = FakeParGroup(
            {
                "resolutionw": FakePar(256),
                "pulse1": FakePar(0),
            }
        )
        ctx.track(node)
        return ctx, node

    def test_set_values(self) -> None:
        ctx, node = self._node()
        out = tdmcp_bridge.apply_step(
            ctx,
            {"op": "set", "path": "/project1/noise1", "values": {"resolutionw": 64}},
        )
        self.assertTrue(out["ok"])
        self.assertEqual(node.par.resolutionw.val, 64)

    def test_set_expressions_sets_mode(self) -> None:
        ctx, node = self._node()
        out = tdmcp_bridge.apply_step(
            ctx,
            {
                "op": "set",
                "path": "/project1/noise1",
                "expressions": {"resolutionw": "absTime.seconds*4"},
            },
        )
        self.assertTrue(out["ok"])
        self.assertEqual(node.par.resolutionw.mode, "EXPRESSION")
        self.assertEqual(node.par.resolutionw.expr, "absTime.seconds*4")

    def test_set_pulse(self) -> None:
        ctx, node = self._node()
        out = tdmcp_bridge.apply_step(
            ctx,
            {"op": "set", "path": "/project1/noise1", "pulse": ["pulse1"]},
        )
        self.assertTrue(out["ok"])
        self.assertEqual(node.par.pulse1._pulsed, 1)

    def test_set_unknown_param(self) -> None:
        ctx, _node = self._node()
        out = tdmcp_bridge.apply_step(
            ctx,
            {"op": "set", "path": "/project1/noise1", "values": {"nope": 1}},
        )
        self.assertFalse(out["ok"])
        self.assertEqual(out["code"], "tdmcp.par.unknown")
        self.assertEqual(out["message"], "unknown parameter: nope")
        self.assertNotIn("lints", out)

    def test_set_flag_name_under_values_hints_wrong_collection(self) -> None:
        ctx, _node = self._node()
        out = tdmcp_bridge.apply_step(
            ctx,
            {
                "op": "set",
                "path": "/project1/noise1",
                "values": {"viewer": True},
            },
        )
        self.assertFalse(out["ok"])
        self.assertEqual(out["code"], "tdmcp.par.unknown")
        self.assertEqual(out["field"], "viewer")
        self.assertIn("exists as flag", out["message"])
        self.assertEqual(len(out.get("lints", [])), 1)
        lint = out["lints"][0]
        self.assertEqual(lint["code"], "tdmcp.par.wrong_collection")
        self.assertEqual(lint["suggestion"]["replace"], "flags.viewer")

    def test_set_node_missing(self) -> None:
        ctx = FakeCtx()
        out = tdmcp_bridge.apply_step(
            ctx,
            {"op": "set", "path": "/project1/missing", "values": {"resolutionw": 1}},
        )
        self.assertFalse(out["ok"])
        self.assertEqual(out["code"], "tdmcp.op.not_found")

    def test_set_flags_ok(self) -> None:
        ctx, node = self._node()
        out = tdmcp_bridge.apply_step(
            ctx,
            {
                "op": "set",
                "path": "/project1/noise1",
                "flags": {"bypass": True},
            },
            detail_level="detailed",
        )
        self.assertTrue(out["ok"])
        self.assertEqual(out["flags"], {"bypass": True})
        self.assertTrue(node.bypass)
        out2 = tdmcp_bridge.apply_step(
            ctx,
            {
                "op": "set",
                "path": "/project1/noise1",
                "flags": {"bypass": False},
            },
        )
        self.assertTrue(out2["ok"])
        self.assertFalse(node.bypass)

    def test_set_flags_unknown(self) -> None:
        ctx, _node = self._node()
        out = tdmcp_bridge.apply_step(
            ctx,
            {
                "op": "set",
                "path": "/project1/noise1",
                "flags": {"selected": True},
            },
        )
        self.assertFalse(out["ok"])
        self.assertEqual(out["code"], "tdmcp.flag.unknown")
        self.assertEqual(out["field"], "selected")
        self.assertEqual(out["message"], "unknown flag: selected")
        self.assertNotIn("lints", out)

    def test_set_param_name_under_flags_hints_wrong_collection(self) -> None:
        ctx, _node = self._node()
        out = tdmcp_bridge.apply_step(
            ctx,
            {
                "op": "set",
                "path": "/project1/noise1",
                "flags": {"resolutionw": 64},
            },
        )
        self.assertFalse(out["ok"])
        self.assertEqual(out["code"], "tdmcp.flag.unknown")
        self.assertEqual(out["field"], "resolutionw")
        self.assertIn("exists as parameter", out["message"])
        self.assertEqual(len(out.get("lints", [])), 1)
        lint = out["lints"][0]
        self.assertEqual(lint["code"], "tdmcp.flag.wrong_collection")
        self.assertEqual(lint["suggestion"]["replace"], "values.resolutionw")

    def test_collection_hint_enrich_failure_returns_base(self) -> None:
        """Enrichment exceptions must not change the base diagnostic."""

        class BoomErr(dict):
            def get(self, key: Any, default: Any = None) -> Any:
                raise RuntimeError("enrich boom")

        base: dict[str, Any] = BoomErr(
            ok=False,
            code="tdmcp.par.unknown",
            path="/project1/noise1",
            message="unknown parameter: viewer",
            field="viewer",
        )
        out = tdmcp_bridge._with_collection_hint(
            base, FakeNode("/project1/noise1"), as_param=True
        )
        self.assertIs(out, base)
        self.assertEqual(out["code"], "tdmcp.par.unknown")
        self.assertEqual(out["message"], "unknown parameter: viewer")
        self.assertNotIn("lints", out)

    def test_suggest_names_case_and_near_miss(self) -> None:
        self.assertEqual(
            tdmcp_bridge._suggest_names("hsvAdjustTOP", ["hsvadjustTOP", "noiseTOP"]),
            ["hsvadjustTOP"],
        )
        near = tdmcp_bridge._suggest_names(
            "satmult", ["saturationmult", "hueoffset", "valuemult"]
        )
        self.assertIn("saturationmult", near)
        self.assertEqual(tdmcp_bridge._suggest_names("zzzz", ["amp", "freq"]), [])

    def test_set_similar_param_name_hint(self) -> None:
        ctx = FakeCtx()
        node = FakeNode("/project1/hsv1")
        node.par = FakeParGroup(
            {
                "saturationmult": FakePar(1.0),
                "hueoffset": FakePar(0.0),
            }
        )
        ctx.nodes[node.path] = node
        out = tdmcp_bridge.apply_step(
            ctx,
            {"op": "set", "path": "/project1/hsv1", "values": {"satmult": 1.3}},
        )
        self.assertFalse(out["ok"])
        self.assertEqual(out["code"], "tdmcp.par.unknown")
        self.assertIn("saturationmult", out["message"])
        self.assertEqual(out["lints"][0]["code"], "tdmcp.par.similar_name")
        self.assertEqual(out["lints"][0]["suggestion"]["replace"], "saturationmult")

    def test_create_similar_op_type_hint(self) -> None:
        ctx = FakeCtx()
        ctx.op_types["hsvadjustTOP"] = object()
        out = tdmcp_bridge.apply_step(
            ctx,
            {"op": "create", "path": "/project1/x", "opType": "hsvAdjustTOP"},
        )
        self.assertFalse(out["ok"])
        self.assertEqual(out["code"], "tdmcp.op.unknown_type")
        self.assertIn("hsvadjustTOP", out["message"])
        self.assertEqual(out["lints"][0]["code"], "tdmcp.op.similar_type")
        self.assertEqual(out["lints"][0]["suggestion"]["replace"], "hsvadjustTOP")

    def test_similar_param_hint_enrich_failure_returns_base(self) -> None:
        class BoomNode:
            def pars(self) -> list[Any]:
                raise RuntimeError("pars boom")

        base = {
            "ok": False,
            "code": "tdmcp.par.unknown",
            "path": "/project1/x",
            "message": "unknown parameter: satmult",
            "field": "satmult",
        }
        out = tdmcp_bridge._with_similar_param_hint(base, BoomNode())
        self.assertEqual(out["code"], "tdmcp.par.unknown")
        self.assertEqual(out["message"], "unknown parameter: satmult")
        self.assertNotIn("lints", out)

    def test_set_flags_and_values_together(self) -> None:
        ctx, node = self._node()
        out = tdmcp_bridge.apply_step(
            ctx,
            {
                "op": "set",
                "path": "/project1/noise1",
                "values": {"resolutionw": 64},
                "flags": {"viewer": True, "display": True},
            },
            detail_level="detailed",
        )
        self.assertTrue(out["ok"])
        self.assertEqual(out["values"], {"resolutionw": 64})
        self.assertEqual(out["flags"], {"viewer": True, "display": True})
        self.assertEqual(node.par.resolutionw.val, 64)
        self.assertTrue(node.viewer)
        self.assertTrue(node.display)


class MutateWireTest(unittest.TestCase):
    def _pair(self) -> tuple[FakeCtx, FakeNode, FakeNode]:
        ctx = FakeCtx()
        src = FakeNode("/project1/noise1")
        dst = FakeNode("/project1/null1")
        ctx.track(src)
        ctx.track(dst)
        return ctx, src, dst

    def test_connect_ok(self) -> None:
        ctx, src, dst = self._pair()
        out = tdmcp_bridge.apply_step(
            ctx,
            {
                "op": "connect",
                "src": "/project1/noise1",
                "dst": "/project1/null1",
            },
            detail_level="detailed",
        )
        self.assertTrue(out["ok"])
        self.assertEqual(out["path"], "/project1/null1")
        self.assertEqual(out["src"], "/project1/noise1")
        self.assertEqual(out["srcOutput"], 0)
        self.assertEqual(out["dstInput"], 0)
        self.assertIn(dst.inputConnectors[0], src.outputConnectors[0].connections)

    def test_disconnect_ok(self) -> None:
        ctx, src, dst = self._pair()
        src.outputConnectors[0].connect(dst.inputConnectors[0])
        out = tdmcp_bridge.apply_step(
            ctx,
            {"op": "disconnect", "path": "/project1/null1", "input": 0},
            detail_level="detailed",
        )
        self.assertTrue(out["ok"])
        self.assertEqual(out["path"], "/project1/null1")
        self.assertEqual(out["input"], 0)
        self.assertEqual(dst.inputConnectors[0].connections, [])

    def test_connect_src_missing(self) -> None:
        ctx = FakeCtx()
        ctx.track(FakeNode("/project1/null1"))
        out = tdmcp_bridge.apply_step(
            ctx,
            {
                "op": "connect",
                "src": "/project1/missing",
                "dst": "/project1/null1",
            },
        )
        self.assertFalse(out["ok"])
        self.assertEqual(out["code"], "tdmcp.op.not_found")

    def test_connect_dst_missing(self) -> None:
        ctx = FakeCtx()
        ctx.track(FakeNode("/project1/noise1"))
        out = tdmcp_bridge.apply_step(
            ctx,
            {
                "op": "connect",
                "src": "/project1/noise1",
                "dst": "/project1/missing",
            },
        )
        self.assertFalse(out["ok"])
        self.assertEqual(out["code"], "tdmcp.op.not_found")

    def test_connect_bad_index(self) -> None:
        ctx, _src, _dst = self._pair()
        out = tdmcp_bridge.apply_step(
            ctx,
            {
                "op": "connect",
                "src": "/project1/noise1",
                "dst": "/project1/null1",
                "dstInput": 99,
            },
        )
        self.assertFalse(out["ok"])
        self.assertEqual(out["code"], "tdmcp.wire.bad_index")

    def test_disconnect_bad_index(self) -> None:
        ctx, _src, dst = self._pair()
        out = tdmcp_bridge.apply_step(
            ctx,
            {"op": "disconnect", "path": dst.path, "input": 99},
        )
        self.assertFalse(out["ok"])
        self.assertEqual(out["code"], "tdmcp.wire.bad_index")

    def test_connect_relative_context(self) -> None:
        ctx, src, dst = self._pair()
        out = tdmcp_bridge.apply_step(
            ctx,
            {"op": "connect", "src": "noise1", "dst": "null1"},
            context_path="/project1",
        )
        self.assertTrue(out["ok"])
        self.assertIn(dst.inputConnectors[0], src.outputConnectors[0].connections)

    def test_batch_connect_fail_skips(self) -> None:
        ctx = FakeCtx()
        parent = ctx.nodes["/project1"]
        orig = parent.create

        def create_and_track(op_cls: Any, name: str) -> FakeNode:
            return ctx.track(orig(op_cls, name))

        parent.create = create_and_track  # type: ignore[method-assign]
        out = tdmcp_bridge.run_mutate_steps(
            ctx,
            [
                {"op": "create", "path": "/project1/noise1", "opType": "noiseTOP"},
                {
                    "op": "connect",
                    "src": "/project1/noise1",
                    "dst": "/project1/missing",
                },
                {"op": "delete", "path": "/project1/noise1"},
            ],
        )
        self.assertFalse(out["ok"])
        self.assertEqual(out["applied"], 1)
        self.assertEqual(out["failedAt"], 1)
        self.assertEqual(out["steps"][1]["code"], "tdmcp.op.not_found")
        self.assertTrue(out["steps"][2].get("skipped"))
        self.assertEqual(out["steps"][2]["code"], "tdmcp.batch.skipped_dependent")

    def test_skipped_connect_path_is_absolutized(self) -> None:
        ctx = FakeCtx()
        ctx.track(FakeNode("/project1/zone/a"))
        out = tdmcp_bridge.run_mutate_steps(
            ctx,
            [
                {
                    "op": "connect",
                    "src": "missing_src",
                    "dst": "null1",
                },
                {"op": "disconnect", "path": "null1"},
            ],
            context_path="/project1/zone",
        )
        self.assertFalse(out["ok"])
        self.assertEqual(out["failedAt"], 0)
        self.assertTrue(out["steps"][1].get("skipped"))
        self.assertEqual(out["steps"][1]["path"], "/project1/zone/null1")

    def test_batch_connect_after_rename_wires_new_node(self) -> None:
        ctx = FakeCtx()
        ctx.enable_create_tracking()
        occupant = FakeNode("/project1/null1")
        ctx.track(occupant)
        out = tdmcp_bridge.run_mutate_steps(
            ctx,
            [
                {"op": "create", "path": "/project1/null1", "opType": "noiseTOP"},
                {"op": "create", "path": "/project1/noiseX", "opType": "noiseTOP"},
                {
                    "op": "connect",
                    "src": "/project1/null1",
                    "dst": "/project1/noiseX",
                },
            ],
            detail_level="detailed",
        )
        self.assertTrue(out["ok"])
        self.assertEqual(out["applied"], 3)
        create_step = out["steps"][0]
        self.assertEqual(create_step["path"], "/project1/null2")
        self.assertEqual(create_step["lints"][0]["code"], "tdmcp.op.renamed")
        renamed = ctx.nodes["/project1/null2"]
        dst = ctx.nodes["/project1/noiseX"]
        self.assertIn(
            dst.inputConnectors[0], renamed.outputConnectors[0].connections
        )
        self.assertEqual(occupant.outputConnectors[0].connections, [])
        self.assertEqual(out["steps"][2]["src"], "/project1/null2")

    def test_batch_remap_relative_paths(self) -> None:
        ctx = FakeCtx()
        ctx.enable_create_tracking()
        occupant = FakeNode("/project1/null1")
        ctx.track(occupant)
        out = tdmcp_bridge.run_mutate_steps(
            ctx,
            [
                {"op": "create", "path": "null1", "opType": "noiseTOP"},
                {"op": "create", "path": "peer1", "opType": "noiseTOP"},
                {"op": "connect", "src": "null1", "dst": "peer1"},
            ],
            context_path="/project1",
            detail_level="detailed",
        )
        self.assertTrue(out["ok"])
        self.assertEqual(out["steps"][0]["path"], "/project1/null2")
        renamed = ctx.nodes["/project1/null2"]
        peer = ctx.nodes["/project1/peer1"]
        self.assertIn(
            peer.inputConnectors[0], renamed.outputConnectors[0].connections
        )
        self.assertEqual(occupant.outputConnectors[0].connections, [])
        self.assertEqual(out["steps"][2]["src"], "/project1/null2")


class MutateDeleteTest(unittest.TestCase):
    def test_delete_ok(self) -> None:
        ctx = FakeCtx()
        node = FakeNode("/project1/noise1")
        ctx.track(node)
        out = tdmcp_bridge.apply_step(
            ctx, {"op": "delete", "path": "/project1/noise1"}
        )
        self.assertTrue(out["ok"])
        self.assertTrue(node._destroyed)

    def test_delete_missing(self) -> None:
        ctx = FakeCtx()
        out = tdmcp_bridge.apply_step(
            ctx, {"op": "delete", "path": "/project1/missing"}
        )
        self.assertFalse(out["ok"])
        self.assertEqual(out["code"], "tdmcp.op.not_found")

    def test_delete_exception(self) -> None:
        ctx = FakeCtx()
        node = FakeNode("/project1/noise1")
        node.destroy = lambda: (_ for _ in ()).throw(RuntimeError("locked"))  # type: ignore[method-assign]
        ctx.track(node)
        out = tdmcp_bridge.apply_step(
            ctx, {"op": "delete", "path": "/project1/noise1"}
        )
        self.assertFalse(out["ok"])
        self.assertEqual(out["code"], "tdmcp.mutate.step_failed")


class MutateBatchTest(unittest.TestCase):
    def test_sequential_stop_and_skip(self) -> None:
        ctx = FakeCtx()
        parent = ctx.nodes["/project1"]
        orig = parent.create

        def create_and_track(op_cls: Any, name: str) -> FakeNode:
            return ctx.track(orig(op_cls, name))

        parent.create = create_and_track  # type: ignore[method-assign]
        out = tdmcp_bridge.run_mutate_steps(
            ctx,
            [
                {"op": "create", "path": "/project1/noise1", "opType": "noiseTOP"},
                {"op": "set", "path": "/project1/noise1", "values": {"nope": 1}},
                {"op": "delete", "path": "/project1/noise1"},
            ],
        )
        self.assertFalse(out["ok"])
        self.assertEqual(out["applied"], 1)
        self.assertEqual(out["failedAt"], 1)
        self.assertTrue(out["steps"][0]["ok"])
        self.assertEqual(out["steps"][1]["code"], "tdmcp.par.unknown")
        self.assertTrue(out["steps"][2].get("skipped"))
        self.assertEqual(out["steps"][2]["code"], "tdmcp.batch.skipped_dependent")

    def test_all_ok(self) -> None:
        ctx = FakeCtx()
        parent = ctx.nodes["/project1"]
        orig = parent.create

        def create_and_track(op_cls: Any, name: str) -> FakeNode:
            return ctx.track(orig(op_cls, name))

        parent.create = create_and_track  # type: ignore[method-assign]
        out = tdmcp_bridge.run_mutate_steps(
            ctx,
            [
                {"op": "create", "path": "/project1/noise1", "opType": "noiseTOP"},
                {
                    "op": "set",
                    "path": "/project1/noise1",
                    "values": {"resolutionw": 128},
                },
                {"op": "delete", "path": "/project1/noise1"},
            ],
        )
        self.assertTrue(out["ok"])
        self.assertEqual(out["applied"], 3)
        self.assertIsNone(out["failedAt"])

    def test_unknown_op(self) -> None:
        ctx = FakeCtx()
        out = tdmcp_bridge.apply_step(ctx, {"op": "rename", "path": "/x"})
        self.assertFalse(out["ok"])
        self.assertEqual(out["code"], "tdmcp.mutate.step_failed")


class MutateSummarizeTest(unittest.TestCase):
    def test_summarize_mutate(self) -> None:
        s = tdmcp_bridge.summarize_request(
            {
                "method": "mutate_nodes",
                "params": {
                    "steps": [
                        {"op": "create", "path": "/project1/a", "opType": "noiseTOP"},
                        {"op": "set", "path": "/project1/a"},
                    ]
                },
            }
        )
        self.assertIn("create", s)
        self.assertIn("2", s)


if __name__ == "__main__":
    unittest.main()
