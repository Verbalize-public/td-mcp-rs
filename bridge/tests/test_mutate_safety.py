"""Mutation intent safety; fakes do not establish native TD replacement semantics."""
import sys
from pathlib import Path
from types import SimpleNamespace as NS

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from tdmcp_bridge.mutate import apply_step, run_mutate_steps
from test_mutate import FakeCtx, FakeNode


def graph():
    ctx = FakeCtx()
    a = ctx.track(FakeNode("/project1/a"))
    b = ctx.track(FakeNode("/project1/b"))
    dst = ctx.track(FakeNode("/project1/merge", n_inputs=3))
    return ctx, a, b, dst


def connect(ctx, source, dst, **options):
    return apply_step(ctx, {"op": "connect", "src": source.path, "dst": dst.path, **options})


def test_safe_policy_preserves_occupied_input():
    ctx, a, b, dst = graph()
    connect(ctx, a, dst)
    before = list(dst.inputConnectors[0].connections)
    rejected = connect(ctx, b, dst, onOccupied="error")
    assert rejected["code"] == "tdmcp.wire.input_occupied"
    assert not rejected["ok"]
    assert dst.inputConnectors[0].connections == before
    assert b.outputConnectors[0].connections == []
    assert rejected["previousConnections"] == [{"path": a.path}]


def test_internal_checked_connect_cannot_downgrade_to_legacy_policy():
    ctx, a, b, dst = graph()
    connect(ctx, a, dst)
    out = apply_step(ctx, {"op": "connect_checked", "src": b.path, "dst": dst.path,
                           "onOccupied": "replace"})
    assert not out["ok"] and out["code"] == "tdmcp.wire.input_occupied"
    assert dst.inputConnectors[0].connections == [a.outputConnectors[0]]


def test_explicit_indices_and_retries_are_unambiguous():
    ctx, a, b, dst = graph()
    first = connect(ctx, a, dst, onOccupied="error")
    second = connect(ctx, b, dst, dstInput=1, onOccupied="error")
    assert first["dstInput"] == 0 and second["dstInput"] == 1
    assert second["src"] == b.path
    repeated = connect(ctx, a, dst, onOccupied="error")
    assert repeated["unchanged"]
    assert len(dst.inputConnectors[0].connections) == 1
    assert len(dst.inputConnectors[1].connections) == 1


def test_native_connector_wrappers_use_owner_path_and_output_index():
    ctx, a, _, dst = graph()
    peer = NS(owner=NS(path=a.path), index=0)
    dst.inputConnectors[0].connections = [peer]
    assert peer != a.outputConnectors[0]
    out = connect(ctx, a, dst, onOccupied="error")
    assert out["ok"] and out["unchanged"]
    assert a.outputConnectors[0].connections == []  # no new connect call


def test_same_owner_different_output_is_not_an_identical_connection():
    ctx, a, _, dst = graph()
    dst.inputConnectors[0].connections = [NS(owner=NS(path=a.path), index=1)]
    out = connect(ctx, a, dst, onOccupied="error")
    assert not out["ok"] and out["code"] == "tdmcp.wire.input_occupied"


def test_legacy_policy_reports_occupied_input_without_auto_append():
    ctx, a, b, dst = graph()
    connect(ctx, a, dst)
    out = connect(ctx, b, dst)
    assert out["ok"] and out["dstInput"] == 0
    assert out["lints"][0]["code"] == "tdmcp.wire.input_occupied"
    assert dst.inputConnectors[1].connections == []


def test_unreadable_occupancy_fails_closed_only_in_safe_mode():
    ctx, a, _, dst = graph()
    class Input:
        @property
        def connections(self):
            raise RuntimeError("unreadable")
    target = Input()
    dst.inputConnectors = [target]
    calls = []
    a.outputConnectors[0].connect = lambda peer: calls.append(peer)
    out = connect(ctx, a, dst, onOccupied="error")
    assert out["code"] == "tdmcp.wire.occupancy_unknown"
    assert calls == []
    out = connect(ctx, a, dst)
    assert out["ok"] and calls == [target]
    assert not out["connectionObservation"]["available"]


@pytest.mark.parametrize("options", [{"onOccupied": "append"}, {"dstInput": -1}, {"dstInput": 99}])
def test_invalid_intent_never_changes_connections(options):
    ctx, a, _, dst = graph()
    assert not connect(ctx, a, dst, **options)["ok"]
    assert a.outputConnectors[0].connections == []


def test_safe_failure_stops_dependent_steps_without_replaying():
    ctx, a, b, dst = graph()
    connect(ctx, a, dst)
    out = run_mutate_steps(ctx, [
        {"op": "connect", "src": b.path, "dst": dst.path, "onOccupied": "error"},
        {"op": "delete", "path": a.path},
    ])
    assert out["applied"] == 0 and out["failedAt"] == 0
    assert out["steps"][1]["skipped"]
    assert ctx.resolve(a.path) is a


def test_nested_create_uses_new_parent_without_touching_existing_comp():
    ctx = FakeCtx()
    ctx.op_types["baseCOMP"] = type("baseCOMP", (), {})
    original = ctx.track(FakeNode("/project1/fx"))
    def track_children(parent):
        create = parent.create
        def create_tracked(op_type, name):
            child = ctx.track(create(op_type, name))
            track_children(child)
            return child
        parent.create = create_tracked
    track_children(ctx.nodes["/project1"])
    out = run_mutate_steps(ctx, [
        {"op": "create", "path": "fx", "opType": "baseCOMP"},
        {"op": "create", "path": "fx/sub", "opType": "baseCOMP"},
        {"op": "create", "path": "fx/sub/child", "opType": "noiseTOP"},
        {"op": "create", "path": "sibling", "opType": "noiseTOP"},
    ], context_path="/project1")
    assert out["ok"]
    assert out["steps"][2]["path"] == "/project1/fx2/sub/child"
    assert ctx.resolve("/project1/fx2/sub/child") is not None
    assert ctx.resolve("/project1/fx/sub/child") is None
    assert ctx.resolve("/project1/sibling") is not None  # context never moves
    assert original._children == {}


def test_descendant_aliases_use_longest_segment_prefix_only():
    from tdmcp_bridge.mutate import _rewrite_step_aliases
    aliases = {"/project1/fx": "/project1/fx2",
               "/project1/fx/sub": "/project1/fx2/sub2"}
    original = {"op": "connect", "src": "fx/sub/child", "dst": "fx20/out"}
    out = _rewrite_step_aliases(original, aliases, "/project1")
    assert out["src"] == "/project1/fx2/sub2/child"
    assert out["dst"] == "fx20/out"
    assert original["src"] == "fx/sub/child"


def test_batch_descendants_and_superseded_parent_aliases():
    from unittest.mock import patch
    from tdmcp_bridge import mutate
    paths = []
    actuals = iter(["/project1/fx2", "/project1/fx2/sub2", "/project1/fx3",
                    "/project1/fx3/sub/child"])
    def apply(ctx, step, **kw):
        paths.append(step["path"])
        return {"ok": True, "path": next(actuals)}
    steps = [{"op": "create", "path": p, "opType": "baseCOMP"}
             for p in ("fx", "fx/sub", "fx", "fx/sub/child")]
    with patch.object(mutate, "apply_step", apply):
        out = run_mutate_steps(None, steps, context_path="/project1")
    assert out["ok"]
    assert paths == ["fx", "/project1/fx2/sub", "/project1/fx2", "/project1/fx3/sub/child"]


def test_skipped_descendants_use_canonical_parent_without_new_context():
    from unittest.mock import patch
    from tdmcp_bridge import mutate
    replies = iter([{"ok": True, "path": "/project1/fx2"}, {"ok": False}])
    steps = [{"op": "create", "path": "fx", "opType": "baseCOMP"},
             {"op": "set", "path": "missing"},
             {"op": "create", "path": "fx/child", "opType": "noiseTOP"}]
    with patch.object(mutate, "apply_step", lambda *a, **kw: next(replies)):
        out = run_mutate_steps(None, steps, context_path="/project1")
    assert out["steps"][2]["path"] == "/project1/fx2/child"
    assert out["steps"][2]["skipped"]

