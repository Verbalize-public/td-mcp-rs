"""`project_snapshot`: a save receipt that names what was written.

Every fact pinned here was measured against TouchDesigner 2025.32460, and two
of them would have produced a lying receipt had they been assumed:

  * `COMP.save` returns the path it used, but saving twice to the same path
    returned that same path both times with a DIFFERENT digest — a silent
    in-place overwrite.
  * `td.project.save` returns a BOOL, and saving to an existing path wrote
    `name.toe` AND `name.1.toe`, byte-identical, because the filename is
    incremented rather than overwritten.

So the requested path is never the identity, and a receipt that reported it as
one would point a caller at a stale artifact.

The tests write to a temp directory, never to a project, because this is a save
and the point of a test is not to overwrite anything real.
"""

from __future__ import annotations

import os
import sys
import tempfile
import unittest
from types import SimpleNamespace
from typing import Any
from unittest import mock

sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))

import tdmcp_bridge  # noqa: E402
from tdmcp_bridge import project_snapshot as snap  # noqa: E402

DIGEST_A = "a" * 64
DIGEST_B = "b" * 64


class TempDirCase(unittest.TestCase):
    def setUp(self) -> None:
        self._dir = tempfile.TemporaryDirectory()
        self.addCleanup(self._dir.cleanup)
        self.dir = self._dir.name

    def target(self, name: str = "snap.tox") -> str:
        return os.path.join(self.dir, name)

    def write(self, path: str, content: bytes) -> str:
        with open(path, "wb") as handle:
            handle.write(content)
        return path


class StatTests(TempDirCase):
    def test_a_missing_file_is_reported_absent_not_as_zero_bytes(self) -> None:
        stat = snap._stat_of(self.target())
        self.assertFalse(stat["exists"])
        self.assertNotIn("sha256", stat)

    def test_a_present_file_is_identified_by_its_digest(self) -> None:
        path = self.write(self.target(), b"hello")
        stat = snap._stat_of(path)
        self.assertTrue(stat["exists"])
        self.assertEqual(stat["bytes"], 5)
        # Full length: a truncated digest is a weaker identity than the receipt
        # claims to give, and a consumer cannot tell it was cut short.
        self.assertEqual(len(stat["sha256"]), 64)


class DestinationTests(unittest.TestCase):
    def test_the_requested_path_is_the_destination_when_touched(self) -> None:
        self.assertTrue(
            snap._requested_vs_written("/a/snap.tox", "/a/snap.tox")["asRequested"]
        )

    def test_an_incremented_destination_is_reported_as_not_requested(self) -> None:
        # MEASURED: a project save to an existing path does not overwrite, it
        # increments. A caller assuming otherwise would read a stale file.
        out = snap._requested_vs_written("/a/snap.tox", "/a/snap.1.toe")
        self.assertFalse(out["asRequested"])
        self.assertEqual(out["increment"], 1)
        self.assertIn("next free name", out["reason"])

    def test_an_unrelated_destination_is_reported_as_such(self) -> None:
        out = snap._requested_vs_written("/a/snap.tox", "/b/other.toe")
        self.assertFalse(out["asRequested"])
        self.assertIn("unrelated", out["reason"])

    def test_an_increment_past_the_cap_is_not_silently_accepted(self) -> None:
        from tdmcp_bridge import constants as C

        far = f"/a/snap.{C.SNAPSHOT_MAX_INCREMENT + 5}.toe"
        out = snap._requested_vs_written("/a/snap.toe", far)
        self.assertFalse(out["asRequested"])
        self.assertIn("cap", out["reason"])


class ComponentSnapshotTests(TempDirCase):
    def _comp(self, *, save_result: Any = "auto", raise_on_save: Exception | None = None):
        path = self.target()

        def save(target: str, **_kwargs: Any) -> Any:
            if raise_on_save is not None:
                raise raise_on_save
            used = target if save_result == "auto" else save_result
            # Only write when there IS a path. An empty return models a save
            # that reported nothing, and attempting a write to "" would raise --
            # turning an `unknown` outcome into a `failed` one in the fake and
            # testing the harness instead of the code.
            if isinstance(used, str) and used:
                self.write(used, b"tox-bytes")
            return used

        return SimpleNamespace(family="COMP", path=path, valid=True, save=save)

    def test_a_fresh_save_reports_the_artifact_it_wrote(self) -> None:
        node = self._comp()
        out = snap._snapshot_component(node, node.path, {"target": self.target()})
        self.assertTrue(out["ok"])
        self.assertEqual(out["outcome"], "written")
        self.assertEqual(out["kind"], "component")
        self.assertEqual(out["written"], self.target())
        self.assertTrue(out["artifact"]["exists"])
        self.assertEqual(len(out["artifact"]["sha256"]), 64)
        self.assertFalse(out["overwrote"])
        self.assertTrue(out["destination"]["asRequested"])

    def test_an_existing_target_is_refused_under_the_default_policy(self) -> None:
        # The point of the default: a caller who did not ask to replace
        # someone's file must not have it replaced.
        self.write(self.target(), b"original")
        node = self._comp()
        out = snap._snapshot_component(node, node.path, {"target": self.target()})
        self.assertFalse(out["ok"])
        self.assertEqual(out["code"], "tdmcp.snapshot.target_exists")
        self.assertTrue(out["existing"]["exists"])
        # And the file is untouched.
        with open(self.target(), "rb") as handle:
            self.assertEqual(handle.read(), b"original")

    def test_replace_reports_that_it_overwrote(self) -> None:
        self.write(self.target(), b"original")
        node = self._comp()
        out = snap._snapshot_component(
            node, node.path, {"target": self.target(), "overwrite": "replace"}
        )
        self.assertTrue(out["ok"])
        self.assertTrue(out["overwrote"])
        self.assertTrue(out["before"]["exists"])

    def test_a_target_is_required_rather_than_defaulted(self) -> None:
        # The default lands in project.folder under a generated, incremented
        # name, which no receipt could predict.
        node = self._comp()
        out = snap._snapshot_component(node, node.path, {})
        self.assertFalse(out["ok"])
        self.assertEqual(out["code"], "tdmcp.snapshot.target_required")

    def test_an_unknown_overwrite_policy_is_refused(self) -> None:
        node = self._comp()
        out = snap._snapshot_component(
            node, node.path, {"target": self.target(), "overwrite": "clobber"}
        )
        self.assertFalse(out["ok"])
        self.assertEqual(out["code"], "tdmcp.snapshot.bad_overwrite")

    def test_a_save_that_raises_is_reported_as_failed_with_the_observed_state(self) -> None:
        node = self._comp(raise_on_save=RuntimeError("disk full"))
        out = snap._snapshot_component(node, node.path, {"target": self.target()})
        self.assertFalse(out["ok"])
        self.assertEqual(out["outcome"], "failed")
        # The after-state is observed, not assumed.
        self.assertFalse(out["after"]["exists"])

    def test_a_save_returning_no_path_is_unknown_not_a_success(self) -> None:
        # An empty return is not a path and must not be stat'ed as one.
        node = self._comp(save_result="")
        out = snap._snapshot_component(node, node.path, {"target": self.target()})
        self.assertFalse(out["ok"])
        self.assertEqual(out["outcome"], "unknown")
        self.assertEqual(out["code"], "tdmcp.snapshot.no_path_returned")

    def test_a_save_that_wrote_nothing_is_reported_not_silently_successful(self) -> None:
        # TD said it saved and no file is there. Claiming success would be a
        # receipt for a write nobody can find.
        node = SimpleNamespace(
            family="COMP",
            path=self.target(),
            valid=True,
            save=lambda target, **_k: target,
        )
        out = snap._snapshot_component(node, node.path, {"target": self.target()})
        self.assertFalse(out["ok"])
        self.assertEqual(out["outcome"], "unknown")
        self.assertEqual(out["code"], "tdmcp.snapshot.not_written")


class UnsavedStateTests(unittest.TestCase):
    def test_project_modified_state_is_reported_as_paths_not_a_count_alone(self) -> None:
        # MEASURED: `project.modified` is a PATH LIST, not a flag.
        fake_td = SimpleNamespace(project=SimpleNamespace(modified=["/a", "/b", "/c"]))
        with mock.patch.dict(sys.modules, {"td": fake_td}):
            report = snap._project_unsaved()
        self.assertTrue(report["available"])
        self.assertEqual(report["scope"], "project")
        self.assertEqual(report["modifiedPaths"], ["/a", "/b", "/c"])
        self.assertFalse(report["truncated"])
        self.assertIn("list of operator paths", report["explanation"])

    def test_a_long_modified_list_is_bounded_and_says_so(self) -> None:
        from tdmcp_bridge import constants as C

        many = [f"/op{index}" for index in range(C.SNAPSHOT_MAX_MODIFIED_PATHS + 10)]
        fake_td = SimpleNamespace(project=SimpleNamespace(modified=many))
        with mock.patch.dict(sys.modules, {"td": fake_td}):
            report = snap._project_unsaved()
        self.assertEqual(len(report["modifiedPaths"]), C.SNAPSHOT_MAX_MODIFIED_PATHS)
        self.assertTrue(report["truncated"])

    def test_a_component_receipt_does_not_carry_the_project_s_unsaved_state(self) -> None:
        # A component save does not save the project, so those paths describe
        # something the call never touched.
        fake_td = SimpleNamespace(project=SimpleNamespace(modified=["/a"]))
        with mock.patch.dict(sys.modules, {"td": fake_td}):
            report = snap._component_unsaved(SimpleNamespace(), "/p")
        self.assertFalse(report["available"])
        self.assertEqual(report["scope"], "component")
        self.assertNotIn("modifiedPaths", report)

    def test_a_component_receipt_does_not_claim_nothing_is_unsaved(self) -> None:
        # TouchDesigner exposes no per-component dirty flag, so an empty list
        # would read as "nothing unsaved" -- the opposite of what is known.
        report = snap._component_unsaved(SimpleNamespace(), "/p")
        self.assertNotIn("modifiedCount", report)
        self.assertIn("no per-component dirty flag", report["explanation"])


class HandlerTests(TempDirCase):
    def test_an_unknown_action_is_refused_by_name(self) -> None:
        out = snap.handle_project_snapshot({"action": "rewrite"})
        self.assertFalse(out["ok"])
        self.assertEqual(out["code"], "tdmcp.snapshot.unknown_action")

    def test_status_writes_nothing(self) -> None:
        fake_td = SimpleNamespace(project=SimpleNamespace(name="p", folder=self.dir, modified=[]))
        with mock.patch.dict(sys.modules, {"td": fake_td}):
            out = snap.handle_project_snapshot({"action": "status"})
        self.assertTrue(out["ok"])
        self.assertEqual(out["outcome"], "reported")
        self.assertEqual(os.listdir(self.dir), [], "status must not write")

    def test_a_non_comp_is_refused_by_family(self) -> None:
        node = SimpleNamespace(family="TOP", path="/project1/t", valid=True)
        with mock.patch.object(snap, "resolve_op", return_value=node):
            out = snap.handle_project_snapshot(
                {"action": "component", "path": "/project1/t", "target": self.target()}
            )
        self.assertFalse(out["ok"])
        self.assertEqual(out["code"], "tdmcp.snapshot.unsupported_family")

    def test_an_unresolvable_path_is_not_found(self) -> None:
        with mock.patch.object(snap, "resolve_op", return_value=None):
            out = snap.handle_project_snapshot(
                {"action": "component", "path": "/p", "target": self.target()}
            )
        self.assertFalse(out["ok"])
        self.assertEqual(out["code"], "tdmcp.op.not_found")

    def test_project_snapshot_is_a_registered_route(self) -> None:
        # A capability is claimed from a route that exists.
        self.assertIn("project_snapshot", tdmcp_bridge.HANDLERS)
        self.assertIs(
            tdmcp_bridge.HANDLERS["project_snapshot"], snap.handle_project_snapshot
        )


if __name__ == "__main__":
    unittest.main()
