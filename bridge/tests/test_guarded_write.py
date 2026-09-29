"""Guarded writes: a precondition rechecked immediately before the write.

The properties pinned here:

* The guard is re-read at write time, not taken from a caller's earlier
  baseline. A guard that reused a cached value would pass on stale evidence,
  which is the same failure as having no guard at all.
* A mismatch refuses the step and does NOT write.
* A value that cannot be read fails closed — the write is refused rather than
  applied on the assumption that an unreadable field is unchanged.
* `null`, `false`, `0` and `""` are real expectations, distinct from absence.
* There is no whole-scene atomicity and no exactly-once claim, and no retry
  path of any kind.
"""

from __future__ import annotations

import os
import sys
import unittest
from types import SimpleNamespace as NS
from typing import Any

sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))
sys.path.insert(0, os.path.join(os.path.dirname(__file__), "."))

from tdmcp_bridge.mutate import apply_step, run_mutate_steps  # noqa: E402
from test_mutate import FakeCtx, FakeNode, FakePar, FakeParGroup  # noqa: E402


def target(**pars: Any) -> FakeNode:
    node = FakeNode("/project1/a")
    node.par = FakeParGroup({name: FakePar(value) for name, value in pars.items()})
    node.opType = "constantTOP"
    return node


def ctx_with(node: FakeNode) -> FakeCtx:
    ctx = FakeCtx()
    ctx.nodes[node.path] = node
    return ctx


def set_step(
    node: FakeNode,
    values: dict[str, Any],
    expect: dict[str, Any] | None = None,
):
    step: dict[str, Any] = {"op": "set", "path": node.path, "values": values}
    if expect is not None:
        step["expect"] = expect
    return step


class GuardedWriteTests(unittest.TestCase):
    def test_a_satisfied_guard_writes(self):
        node = target(t=1.0)
        out = apply_step(ctx_with(node), set_step(node, {"t": 5.0}, {"fields": {"par.t": 1.0}}))
        self.assertTrue(out["ok"], out)
        self.assertEqual(node.par.t.val, 5.0)

    def test_a_mismatched_guard_refuses_and_does_not_write(self):
        node = target(t=1.0)
        out = apply_step(ctx_with(node), set_step(node, {"t": 5.0}, {"fields": {"par.t": 9.0}}))
        self.assertFalse(out["ok"])
        self.assertEqual(out["code"], "tdmcp.mutate.precondition_failed")
        self.assertEqual(out["field"], "par.t")
        # The value is untouched — refusing must not be a partial write.
        self.assertEqual(node.par.t.val, 1.0)
        # The caller is shown both sides so it can decide what to do.
        self.assertEqual(out["expected"]["value"], 9.0)
        self.assertEqual(out["observed"]["value"], 1.0)

    def test_the_guard_is_reread_not_taken_from_a_baseline(self):
        """The decisive one: the field changes AFTER the caller formed a plan."""
        node = target(t=1.0)
        ctx = ctx_with(node)
        # A caller read 1.0 and planned on that basis.
        plan = {"fields": {"par.t": 1.0}}
        # ...and then something else in the project moved the value.
        node.par.t.val = 2.0
        out = apply_step(ctx, set_step(node, {"t": 5.0}, plan))
        self.assertFalse(out["ok"])
        self.assertEqual(out["code"], "tdmcp.mutate.precondition_failed")
        self.assertEqual(out["observed"]["value"], 2.0)
        self.assertEqual(node.par.t.val, 2.0)

    def test_a_callback_change_between_guard_and_write_is_not_claimed_to_be_prevented(self):
        """No whole-scene atomicity: the docstring must say so, and must not
        overstate. The guard narrows the window; it does not close it."""
        node = target(t=1.0)
        ctx = ctx_with(node)

        class RacingCtx(FakeCtx):
            def resolve(self, path: str) -> Any | None:
                # Every read after the first sees a value that moved — exactly
                # what a timeline or a user script does mid-write.
                node.par.t.val = 7.0
                return self.nodes.get(path)

        racing = RacingCtx()
        racing.nodes[node.path] = node
        out = apply_step(racing, set_step(node, {"t": 5.0}, {"fields": {"par.t": 1.0}}))
        # The guard read 7.0, so it refused. The point is that the guard read
        # *at all*; there is no cached answer it could have used instead.
        self.assertFalse(out["ok"])
        self.assertEqual(out["observed"]["value"], 7.0)

    def test_an_unreadable_precondition_fails_closed(self):
        node = target(t=1.0)
        out = apply_step(
            ctx_with(node), set_step(node, {"t": 5.0}, {"fields": {"par.nope": 1.0}})
        )
        self.assertFalse(out["ok"])
        self.assertEqual(out["code"], "tdmcp.mutate.precondition_failed")
        self.assertFalse(out["readable"])
        # Refusing is the whole point: writing on an unconfirmed field would be
        # the exact silent clobber the guard exists to prevent.
        self.assertEqual(node.par.t.val, 1.0)

    def test_a_guard_on_a_missing_node_is_refused_not_skipped(self):
        node = target(t=1.0)
        ctx = ctx_with(node)
        step = set_step(node, {"t": 5.0}, {"fields": {"par.t": 1.0}})
        step["path"] = "/project1/gone"
        out = apply_step(ctx, step)
        self.assertFalse(out["ok"])
        self.assertIn(out["code"], ("tdmcp.mutate.precondition_unreadable", "tdmcp.op.not_found"))

    def test_falsy_expectations_are_real_expectations(self):
        for value in (None, False, 0, ""):
            with self.subTest(value=value):
                node = target(t=value)
                out = apply_step(
                    ctx_with(node), set_step(node, {"t": 9.0}, {"fields": {"par.t": value}})
                )
                self.assertTrue(out["ok"], f"{value!r} was not honoured as an expectation")
                self.assertEqual(node.par.t.val, 9.0)

    def test_falsy_but_different_values_do_not_pass(self):
        node = target(t=0)
        out = apply_step(ctx_with(node), set_step(node, {"t": 9.0}, {"fields": {"par.t": False}}))
        self.assertFalse(out["ok"], "False must not match 0 as a 'falsy' expectation")
        self.assertEqual(node.par.t.val, 0)

    def test_a_guard_never_retries(self):
        """One refusal, one attempt. A retried write on an unknown effect is
        how one intent becomes two mutations."""
        node = target(t=1.0)
        ctx = ctx_with(node)
        step = set_step(node, {"t": 5.0}, {"fields": {"par.t": 9.0}})
        first = apply_step(ctx, step)
        second = apply_step(ctx, step)
        self.assertEqual(first["code"], second["code"])
        self.assertEqual(node.par.t.val, 1.0)

    def test_a_step_without_expect_is_unchanged_in_behaviour(self):
        node = target(t=1.0)
        out = apply_step(ctx_with(node), {"op": "set", "path": node.path, "values": {"t": 5.0}})
        self.assertTrue(out["ok"], out)
        self.assertEqual(node.par.t.val, 5.0)

    def test_guards_are_per_step_inside_a_batch(self):
        """A later step's guard is checked when that step runs, not at the
        batch boundary — earlier steps in the batch are allowed to move the
        very field a later step guards."""
        node = target(t=1.0)
        ctx = ctx_with(node)
        out = run_mutate_steps(
            ctx,
            [
                set_step(node, {"t": 2.0}),
                set_step(node, {"t": 5.0}, {"fields": {"par.t": 2.0}}),
            ],
        )
        self.assertTrue(out["ok"], out)
        self.assertEqual(node.par.t.val, 5.0)

    def test_a_failed_guard_stops_dependent_later_steps(self):
        node = target(t=1.0)
        ctx = ctx_with(node)
        out = run_mutate_steps(
            ctx,
            [
                set_step(node, {"t": 5.0}, {"fields": {"par.t": 9.0}}),
                set_step(node, {"t": 7.0}),
            ],
        )
        self.assertFalse(out["ok"])
        # The first step refused, so the second never ran.
        self.assertEqual(node.par.t.val, 1.0)
        self.assertTrue(out["steps"][1].get("skipped"))

    def test_create_cannot_be_guarded_because_there_is_nothing_to_expect(self):
        node = target()
        ctx = ctx_with(node)
        out = apply_step(
            ctx,
            {"op": "create", "path": "/project1/new", "opType": "noiseTOP",
             "expect": {"fields": {"par.t": 1.0}}},
        )
        self.assertTrue(out["ok"], out)


if __name__ == "__main__":
    unittest.main()
