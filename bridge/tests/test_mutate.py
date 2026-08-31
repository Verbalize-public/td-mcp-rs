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
        # Set by the ctx that owns this node, so a rename re-keys the registry
        # the way TD re-paths an operator.
        self._registry: dict[str, "FakeNode"] | None = None
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

    @property
    def name(self) -> str:
        return self.path.rsplit("/", 1)[-1]

    @name.setter
    def name(self, new: str) -> None:
        # TD moves the operator when you rename it; the fake must too, or
        # placement tests would silently pass on a stale path.
        old_path = self.path
        parent, _ = old_path.rsplit("/", 1)
        wanted = f"{parent or '/'}/{new}"
        if self._registry is not None:
            occupant = self._registry.get(wanted)
            if occupant is not None and occupant is not self:
                # Name taken: TD keeps the operator where it is, and the caller
                # learns the real path from the returned `tdmcp.op.renamed` lint.
                return
            self._registry.pop(old_path, None)
            self._registry[wanted] = self
        self.path = wanted

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
        # Echo create opType for api_help diagnostic refs (matches live TD).
        child.opType = getattr(op_cls, "__name__", None) or str(op_cls)
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
        self.op_types: dict[str, Any] = {"noiseTOP": type("noiseTOP", (), {})}
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
        node._registry = self.nodes
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
        self.assertEqual(out.get("opType"), "noiseTOP")
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

    def test_is_op_type_name(self) -> None:
        self.assertTrue(tdmcp_bridge._is_op_type_name("noiseTOP"))
        self.assertTrue(tdmcp_bridge._is_op_type_name("geometryCOMP"))
        self.assertFalse(tdmcp_bridge._is_op_type_name("math"))
        self.assertFalse(tdmcp_bridge._is_op_type_name("_private"))
        self.assertFalse(tdmcp_bridge._is_op_type_name("TOP"))

    def test_suggest_op_types_keep_list_and_garbage(self) -> None:
        roster = [
            "hsvadjustTOP",
            "noiseTOP",
            "blurTOP",
            "levelTOP",
            "nullTOP",
            "geometryCOMP",
            "nullCOMP",
            "nullCHOP",
            "mathCHOP",
        ]
        self.assertEqual(
            tdmcp_bridge._suggest_op_types(
                "hsvAdjustTOP", ["hsvadjustTOP", "noiseTOP"]
            ),
            ["hsvadjustTOP"],
        )
        self.assertEqual(
            tdmcp_bridge._suggest_op_types("noizeTOP", ["noiseTOP", "blurTOP"]),
            ["noiseTOP"],
        )
        bare = tdmcp_bridge._suggest_op_types("noise", ["noiseTOP", "blurTOP"])
        self.assertIn("noiseTOP", bare)
        self.assertEqual(tdmcp_bridge._suggest_op_types("fooTOP", roster), [])
        self.assertEqual(tdmcp_bridge._suggest_op_types("xyzTOP", roster), [])
        self.assertEqual(tdmcp_bridge._suggest_op_types("geo", roster), [])

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

    def test_create_garbage_op_type_no_similar_lint(self) -> None:
        ctx = FakeCtx()
        out = tdmcp_bridge.apply_step(
            ctx,
            {"op": "create", "path": "/project1/x", "opType": "fooTOP"},
        )
        self.assertFalse(out["ok"])
        self.assertEqual(out["code"], "tdmcp.op.unknown_type")
        self.assertNotIn("lints", out)
        self.assertNotIn("did you mean", out["message"])

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


# --- text writes + shader lint (docs/SHADER_LINT.md §4) ----------------------

_SUCCESS_RESULT = (
    "Vertex Shader Compile Results:\n\nCompiled Successfully\n\n"
    "=============\nPixel Shader Compile Results:\n\nCompiled Successfully\n"
)
_FAILURE_RESULT = (
    "Vertex Shader Compile Results:\n\n"
    "ERROR: /project1/probe/shader:5: '' : syntax error, unexpected RIGHT_BRACE\n"
    "ERROR: 1 compilation errors.  No code generated.\n"
)


def _mut_dat(path: str, text: str = "") -> SimpleNamespace:
    return SimpleNamespace(
        path=path,
        family="DAT",
        isDAT=True,
        isText=True,
        isTable=False,
        opType="textDAT",
        text=text,
    )


class _StagePar:
    def __init__(self, target: Any) -> None:
        self._target = target

    def eval(self) -> Any:
        return self._target


class _GlslConsumer(SimpleNamespace):
    """GLSL op whose stage par evaluates to a DAT (consumer-scan match)."""


def _glsl_consumer(
    path: str,
    op_type: str,
    dat: Any,
    compile_result: Any,
    stage_par: str = "pixeldat",
) -> _GlslConsumer:
    return _GlslConsumer(
        path=path,
        opType=op_type,
        compileResult=compile_result,
        par=SimpleNamespace(**{stage_par: _StagePar(dat)}),
    )


class LintCapableCtx(FakeCtx):
    def __init__(self) -> None:
        super().__init__()
        self._by_type: dict[str, list[Any]] = {}

    def register(self, type_name: str, *ops: Any) -> None:
        self._by_type.setdefault(type_name, []).extend(ops)

    def find_children(self, root: Any, type_name: str) -> list[Any]:
        return list(self._by_type.get(type_name, []))


class MutateTextWriteTest(unittest.TestCase):
    def test_set_text_on_dat_writes_and_attaches_error_lint(self) -> None:
        ctx = LintCapableCtx()
        dat = ctx.track(_mut_dat("/project1/probe/shader"))
        glsl = _glsl_consumer("/project1/fx/glsl1", "glslTOP", dat, _FAILURE_RESULT)
        ctx.register("glslTOP", glsl)
        out = tdmcp_bridge.run_mutate_steps(
            ctx, [{"op": "set", "path": "/project1/probe/shader", "text": "void m(){}"}]
        )
        self.assertTrue(out["ok"])
        step = out["steps"][0]
        self.assertTrue(step["ok"])
        self.assertEqual(dat.text, "void m(){}")
        diags = step["shaderDiagnostics"]
        self.assertEqual(len(diags), 1)
        self.assertEqual(diags[0]["severity"], "error")
        self.assertEqual(diags[0]["code"], "tdmcp.shader.compile_failed")
        self.assertEqual(diags[0]["consumer"], glsl.path)
        self.assertEqual(diags[0]["role"], "pixel")
        self.assertEqual(out["shaderErrors"], 1)

    def test_set_text_note_lint_and_summary_count(self) -> None:
        ctx = LintCapableCtx()
        dat = ctx.track(_mut_dat("/project1/probe/shader"))
        ctx.register(
            "glslTOP",
            _glsl_consumer("/project1/fx/glsl1", "glslTOP", dat, _SUCCESS_RESULT),
        )
        out = tdmcp_bridge.run_mutate_steps(
            ctx, [{"op": "set", "path": "/project1/probe/shader", "text": "x"}]
        )
        self.assertEqual(out["steps"][0]["shaderDiagnostics"][0]["code"],
                         "tdmcp.shader.compiled")
        self.assertEqual(out["shaderNotes"], 1)
        self.assertNotIn("shaderErrors", out)

    def test_set_text_on_non_dat_hard_error_skips_rest(self) -> None:
        ctx = FakeCtx()
        top = ctx.track(FakeNode("/project1/nz"))  # no family/isDAT → not a DAT
        out = tdmcp_bridge.run_mutate_steps(
            ctx,
            [
                {"op": "set", "path": "/project1/nz", "text": "x"},
                {"op": "delete", "path": "/project1/nz"},
            ],
        )
        self.assertFalse(out["ok"])
        self.assertEqual(out["failedAt"], 0)
        self.assertEqual(out["steps"][0]["code"], "tdmcp.mutate.not_dat")
        self.assertTrue(out["steps"][1].get("skipped"))
        self.assertIsNone(getattr(top, "text", None))

    def test_create_with_text_writes_body(self) -> None:
        ctx = LintCapableCtx()
        parent = ctx.track(FakeNode("/project1/probe"))
        ctx.op_types["textDAT"] = type("textDAT", (), {})
        dat = _mut_dat("/project1/probe/shader")
        parent.create = lambda op_cls, name: dat
        out = tdmcp_bridge.run_mutate_steps(
            ctx,
            [{
                "op": "create",
                "path": "/project1/probe/shader",
                "opType": "textDAT",
                "text": "void main() {}",
            }],
        )
        self.assertTrue(out["ok"])
        self.assertEqual(dat.text, "void main() {}")

    def test_create_non_dat_text_rolls_back(self) -> None:
        ctx = LintCapableCtx()
        parent = ctx.track(FakeNode("/project1/probe"))
        created = FakeNode("/project1/probe/nz")  # plain OP → not a DAT
        parent.create = lambda op_cls, name: created
        out = tdmcp_bridge.run_mutate_steps(
            ctx,
            [{
                "op": "create",
                "path": "/project1/probe/nz",
                "opType": "noiseTOP",
                "text": "x",
            }],
        )
        self.assertFalse(out["ok"])
        self.assertEqual(out["steps"][0]["code"], "tdmcp.mutate.not_dat")
        self.assertTrue(created._destroyed, "rollback must destroy the node")

    def test_text_applies_before_values(self) -> None:
        ctx = LintCapableCtx()
        dat = ctx.track(_mut_dat("/project1/probe/shader"))
        glsl = _glsl_consumer("/project1/fx/glsl1", "glslTOP", dat, _FAILURE_RESULT)
        ctx.register("glslTOP", glsl)
        out = tdmcp_bridge.run_mutate_steps(
            ctx,
            [{
                "op": "set",
                "path": "/project1/probe/shader",
                "text": "body",
                "values": {"bogusPar": 1},
            }],
        )
        # text lands first; the unknown-par error still fails the step.
        self.assertEqual(dat.text, "body")
        self.assertFalse(out["ok"])
        self.assertEqual(out["steps"][0]["code"], "tdmcp.par.unknown")
        # lint still rides the failure envelope because the text write landed.
        diags = out["steps"][0]["shaderDiagnostics"]
        self.assertEqual(len(diags), 1)
        self.assertEqual(diags[0]["code"], "tdmcp.shader.compile_failed")
        self.assertEqual(diags[0]["consumer"], glsl.path)
        self.assertEqual(out["shaderErrors"], 1)

    def test_detailed_echoes_length_not_body(self) -> None:
        ctx = LintCapableCtx()
        ctx.track(_mut_dat("/project1/probe/shader"))
        body = "void main() {}"
        out = tdmcp_bridge.run_mutate_steps(
            ctx,
            [{"op": "set", "path": "/project1/probe/shader", "text": body}],
            detail_level="detailed",
        )
        step = out["steps"][0]
        self.assertEqual(step["textLength"], len(body))
        self.assertNotIn("text", step)


class MutateCommentTest(unittest.TestCase):
    """`comment` is a first-class step field on create/set (OP.comment)."""

    def test_create_sets_comment(self) -> None:
        ctx = FakeCtx()
        parent = ctx.enable_create_tracking()
        self.assertIs(parent, ctx.nodes["/project1"])
        out = tdmcp_bridge.apply_step(
            ctx,
            {
                "op": "create",
                "path": "/project1/noise1",
                "opType": "noiseTOP",
                "comment": "base plate noise for the displacement chain",
            },
        )
        self.assertTrue(out["ok"])
        node = ctx.nodes["/project1/noise1"]
        self.assertEqual(
            node.comment, "base plate noise for the displacement chain"
        )

    def test_set_sets_and_clears_comment(self) -> None:
        ctx = FakeCtx()
        node = ctx.track(FakeNode("/project1/noise1"))
        out = tdmcp_bridge.apply_step(
            ctx,
            {"op": "set", "path": "/project1/noise1", "comment": "why this exists"},
        )
        self.assertTrue(out["ok"])
        self.assertEqual(node.comment, "why this exists")
        # Empty string clears — distinct from omitting the field.
        out = tdmcp_bridge.apply_step(
            ctx, {"op": "set", "path": "/project1/noise1", "comment": ""}
        )
        self.assertTrue(out["ok"])
        self.assertEqual(node.comment, "")

    def test_set_without_comment_leaves_existing(self) -> None:
        ctx = FakeCtx()
        node = ctx.track(FakeNode("/project1/noise1"))
        node.comment = "keep me"
        out = tdmcp_bridge.apply_step(
            ctx, {"op": "set", "path": "/project1/noise1", "values": {}}
        )
        self.assertTrue(out["ok"])
        self.assertEqual(node.comment, "keep me")

    def test_create_rolls_back_when_comment_write_fails(self) -> None:
        ctx = FakeCtx()
        parent = ctx.nodes["/project1"]
        created = FakeNode("/project1/nz")

        class _NoComment(FakeNode):
            @property
            def comment(self) -> str:
                return ""

        created = _NoComment("/project1/nz")  # read-only property → setattr raises
        parent.create = lambda op_cls, name: created
        out = tdmcp_bridge.apply_step(
            ctx,
            {
                "op": "create",
                "path": "/project1/nz",
                "opType": "noiseTOP",
                "comment": "boom",
            },
        )
        self.assertFalse(out["ok"])
        self.assertEqual(out["code"], "tdmcp.mutate.step_failed")
        self.assertEqual(out["field"], "comment")
        self.assertTrue(created._destroyed, "rollback must destroy the node")

    def test_detailed_echoes_comment(self) -> None:
        ctx = FakeCtx()
        ctx.track(FakeNode("/project1/noise1"))
        out = tdmcp_bridge.apply_step(
            ctx,
            {"op": "set", "path": "/project1/noise1", "comment": "echoed"},
            detail_level="detailed",
        )
        self.assertEqual(out["comment"], "echoed")

    def test_summary_does_not_echo_comment(self) -> None:
        ctx = FakeCtx()
        ctx.track(FakeNode("/project1/noise1"))
        out = tdmcp_bridge.apply_step(
            ctx, {"op": "set", "path": "/project1/noise1", "comment": "quiet"}
        )
        self.assertNotIn("comment", out)


if __name__ == "__main__":
    unittest.main()


class FakePaletteCtx(FakeCtx):
    """FakeCtx plus a ``load_tox`` that mimics ``COMP.loadTox``.

    TD returns the loaded component as a *child* of the receiver
    (``docs/DEV_ENV.md`` `root.loadTox(KIT)`), under the name baked into the
    `.tox` — which is why `_step_place` renames afterwards.
    """

    def __init__(self, *, loaded_name: str = "loaded", fail: bool = False) -> None:
        super().__init__()
        self.loaded_name = loaded_name
        self.fail = fail
        self.loads: list[str] = []
        self.returns_none = False

    def _materialize(self, parent: Any, leaf: str, op_type: str) -> FakeNode:
        """A child of ``parent`` whose ``.name`` setter moves it, as TD's does."""
        path = f"{parent.path.rstrip('/')}/{leaf}"
        node = FakeNode(path, op_types=self.op_types)
        node.opType = op_type
        node.par = FakeParGroup({"Birthrate": FakePar(1000)})
        node.family = "COMP"
        parent._children[leaf] = node
        self.track(node)
        return node

    def copy_ops(self, parent: Any, ops: list[Any]) -> list[Any]:
        # TD suffixes the copy when the source name is taken in the destination.
        out = [
            self._materialize(parent, f"{o.name}1", getattr(o, "opType", None))
            for o in ops
        ]
        self.copied = out
        return out

    def load_tox(self, parent: Any, tox_path: str) -> Any | None:
        self.loads.append(tox_path)
        if self.fail:
            raise RuntimeError("bad tox build")
        if self.returns_none:
            return None
        return self._materialize(parent, self.loaded_name, "baseCOMP")


class MutatePlaceTest(unittest.TestCase):
    def test_place_loads_renames_and_applies_values(self) -> None:
        ctx = FakePaletteCtx()
        out = tdmcp_bridge.apply_step(
            ctx,
            {
                "op": "place",
                "path": "/project1/parts",
                "paletteId": "builtin:Tools/particlesGpu",
                "toxPath": "/palette/Tools/particlesGpu.tox",
                "comment": "stock GPU particles",
                "values": {"Birthrate": 2000},
            },
        )
        self.assertTrue(out["ok"], out)
        self.assertEqual(out["path"], "/project1/parts")
        self.assertEqual(out["paletteId"], "builtin:Tools/particlesGpu")
        self.assertEqual(ctx.loads, ["/palette/Tools/particlesGpu.tox"])
        placed = ctx.nodes["/project1/parts"]
        self.assertEqual(placed.par.Birthrate.val, 2000)
        self.assertEqual(placed.comment, "stock GPU particles")
        # Renamed to the requested leaf, so no rename lint.
        self.assertNotIn("lints", out)

    def test_place_reports_a_rename_when_td_keeps_its_own_name(self) -> None:
        ctx = FakePaletteCtx()
        # A node already occupies the requested leaf name, so the rename is
        # refused and TD keeps what it loaded.
        placed_holder = FakeNode("/project1/parts", op_types=ctx.op_types)
        ctx.track(placed_holder)

        def stubborn_load(parent: Any, tox_path: str) -> Any:
            child = FakeNode("/project1/particlesGpu", op_types=ctx.op_types)
            child.opType = "baseCOMP"
            ctx.track(child)
            return child

        ctx.load_tox = stubborn_load  # type: ignore[method-assign]
        out = tdmcp_bridge.apply_step(
            ctx,
            {
                "op": "place",
                "path": "/project1/parts",
                "toxPath": "/palette/Tools/particlesGpu.tox",
            },
        )
        self.assertTrue(out["ok"], out)
        self.assertEqual(out["path"], "/project1/particlesGpu")
        self.assertEqual(out["lints"][0]["code"], "tdmcp.op.renamed")
        self.assertEqual(
            out["lints"][0]["suggestion"]["opPath"], "/project1/particlesGpu"
        )

    def test_place_rolls_back_when_a_value_fails(self) -> None:
        ctx = FakePaletteCtx()
        out = tdmcp_bridge.apply_step(
            ctx,
            {
                "op": "place",
                "path": "/project1/parts",
                "toxPath": "/palette/x.tox",
                "values": {"NoSuchPar": 1},
            },
        )
        self.assertFalse(out["ok"])
        placed = ctx.nodes["/project1/parts"]
        self.assertTrue(placed._destroyed, "a failed place must not leave debris")

    def test_place_surfaces_a_load_failure_as_palette_load_failed(self) -> None:
        ctx = FakePaletteCtx(fail=True)
        out = tdmcp_bridge.apply_step(
            ctx,
            {"op": "place", "path": "/project1/parts", "toxPath": "/palette/x.tox"},
        )
        self.assertFalse(out["ok"])
        self.assertEqual(out["code"], "tdmcp.palette.load_failed")
        self.assertIn("bad tox build", out["message"])

    def test_place_reports_a_load_that_produced_nothing(self) -> None:
        ctx = FakePaletteCtx()
        ctx.returns_none = True
        out = tdmcp_bridge.apply_step(
            ctx,
            {"op": "place", "path": "/project1/parts", "toxPath": "/palette/x.tox"},
        )
        self.assertFalse(out["ok"])
        self.assertEqual(out["code"], "tdmcp.palette.load_failed")

    def test_place_requires_a_resolved_tox_path(self) -> None:
        # The daemon resolves paletteId; a step arriving without toxPath is a
        # contract break, not something to guess at.
        ctx = FakePaletteCtx()
        out = tdmcp_bridge.apply_step(
            ctx,
            {"op": "place", "path": "/project1/parts", "paletteId": "builtin:Tools/x"},
        )
        self.assertFalse(out["ok"])
        self.assertIn("toxPath", out["message"])

    def test_place_fails_when_the_parent_is_missing(self) -> None:
        ctx = FakePaletteCtx()
        out = tdmcp_bridge.apply_step(
            ctx,
            {"op": "place", "path": "/nope/parts", "toxPath": "/palette/x.tox"},
        )
        self.assertFalse(out["ok"])
        self.assertEqual(out["code"], "tdmcp.op.not_found")

    def test_a_renamed_placement_remaps_later_steps_in_the_batch(self) -> None:
        ctx = FakePaletteCtx()
        blocker = FakeNode("/project1/parts", op_types=ctx.op_types)
        ctx.track(blocker)
        target = FakeNode("/project1/out1", op_types=ctx.op_types)
        ctx.track(target)

        def stubborn_load(parent: Any, tox_path: str) -> Any:
            child = FakeNode("/project1/particlesGpu", op_types=ctx.op_types)
            child.opType = "baseCOMP"
            ctx.track(child)
            return child

        ctx.load_tox = stubborn_load  # type: ignore[method-assign]
        out = tdmcp_bridge.run_mutate_steps(
            ctx,
            [
                {
                    "op": "place",
                    "path": "/project1/parts",
                    "toxPath": "/palette/x.tox",
                },
                {"op": "connect", "src": "/project1/parts", "dst": "/project1/out1"},
            ],
            context_path="/project1",
            detail_level="detailed",
        )
        self.assertTrue(out["ok"], out)
        # The connect followed the placement to its real path.
        self.assertEqual(out["steps"][1]["src"], "/project1/particlesGpu")
        placed = ctx.nodes["/project1/particlesGpu"]
        self.assertEqual(
            placed.outputConnectors[0].connections,
            [target.inputConnectors[0]],
        )


if __name__ == "__main__":
    unittest.main()


class PlaceUnwrapsPaletteWrapperTest(unittest.TestCase):
    """A stock palette `.tox` is a wrapper (icon + help + the real component).

    Placing the wrapper would drop a parameterless baseCOMP with an icon into
    the network — verified live against TouchDesigner before this was fixed.
    """

    def ctx_with_wrapper(self) -> FakePaletteCtx:
        ctx = FakePaletteCtx(loaded_name="particlesGpu")
        orig = ctx.load_tox

        def load_wrapper(parent: Any, tox_path: str) -> Any:
            wrapper = orig(parent, tox_path)
            payload = FakeNode(f"{wrapper.path}/particlesGpu", op_types=ctx.op_types)
            payload.opType = "containerCOMP"
            payload.family = "COMP"
            payload.__dict__["name"] = "particlesGpu"
            payload.par = FakeParGroup({"Birthrate": FakePar(1000)})
            payload.customPars = [SimpleNamespace(name="Birthrate")]
            icon = FakeNode(f"{wrapper.path}/icon", op_types=ctx.op_types)
            icon.opType = "nullTOP"
            icon.__dict__["name"] = "icon"
            wrapper.children = [icon, payload]
            wrapper.customPars = []
            return wrapper

        ctx.load_tox = load_wrapper  # type: ignore[method-assign]
        return ctx

    def test_place_lifts_the_component_out_and_drops_the_wrapper(self) -> None:
        ctx = self.ctx_with_wrapper()
        out = tdmcp_bridge.apply_step(
            ctx,
            {
                "op": "place",
                "path": "/project1/parts",
                "paletteId": "builtin:Tools/particlesGpu",
                "toxPath": "/palette/Tools/particlesGpu.tox",
                "values": {"Birthrate": 2000},
            },
            detail_level="detailed",
        )
        self.assertTrue(out["ok"], out)
        self.assertTrue(out["unwrapped"])
        self.assertEqual(out["opType"], "containerCOMP", "the payload, not the wrapper")
        self.assertEqual(out["path"], "/project1/parts")
        placed = ctx.nodes["/project1/parts"]
        self.assertEqual(placed.par.Birthrate.val, 2000)
        # The wrapper (and its icon) must not survive in the user's network.
        wrapper = ctx.nodes["/project1/particlesGpu"]
        self.assertTrue(wrapper._destroyed)

    def test_a_copy_failure_leaves_no_wrapper_behind(self) -> None:
        ctx = self.ctx_with_wrapper()

        def boom(parent: Any, ops: list[Any]) -> list[Any]:
            raise RuntimeError("copyOPs refused")

        ctx.copy_ops = boom  # type: ignore[method-assign]
        out = tdmcp_bridge.apply_step(
            ctx,
            {"op": "place", "path": "/project1/parts", "toxPath": "/palette/x.tox"},
        )
        self.assertFalse(out["ok"])
        self.assertEqual(out["code"], "tdmcp.palette.load_failed")
        self.assertTrue(ctx.nodes["/project1/particlesGpu"]._destroyed)

    def test_an_empty_copy_is_a_load_failure_not_a_silent_success(self) -> None:
        ctx = self.ctx_with_wrapper()
        ctx.copy_ops = lambda parent, ops: []  # type: ignore[method-assign]
        out = tdmcp_bridge.apply_step(
            ctx,
            {"op": "place", "path": "/project1/parts", "toxPath": "/palette/x.tox"},
        )
        self.assertFalse(out["ok"])
        self.assertEqual(out["code"], "tdmcp.palette.load_failed")

    def test_a_plain_user_tox_is_placed_as_is(self) -> None:
        ctx = FakePaletteCtx(loaded_name="myThing")
        out = tdmcp_bridge.apply_step(
            ctx,
            {"op": "place", "path": "/project1/mine", "toxPath": "/mine/myThing.tox"},
            detail_level="detailed",
        )
        self.assertTrue(out["ok"], out)
        self.assertFalse(out["unwrapped"], "nothing to unwrap; no copy round-trip")
