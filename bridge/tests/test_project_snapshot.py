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


class OverwriteRecoveryTests(TempDirCase):
    """Replacing a file must not destroy the only copy of what was there.

    Without a set-aside, a save that fails partway leaves a half-written
    destination and no copy of the original -- and the receipt's `before`
    digest would describe bytes that no longer exist anywhere.
    """

    def _node(self, *, raise_on_save: Exception | None = None, write: str = "tox-bytes"):
        def save(target: str, **_kwargs: Any) -> str:
            if raise_on_save is not None:
                # A save that fails PARTWAY: clobber the destination, then
                # raise. This is the case recovery exists for.
                with open(target, "wb") as handle:
                    handle.write(b"half-written")
                raise raise_on_save
            with open(target, "wb") as handle:
                handle.write(write.encode())
            return target

        return SimpleNamespace(family="COMP", path="/p", valid=True, save=save)

    def test_a_replace_keeps_a_verified_copy_of_the_displaced_file(self) -> None:
        self.write(self.target(), b"original")
        out = snap._snapshot_component(
            self._node(), "/p", {"target": self.target(), "overwrite": "replace"}
        )
        self.assertTrue(out["ok"])
        recovery = out["recovery"]
        self.assertTrue(recovery["stashed"])
        self.assertTrue(recovery["attempted"])
        # The stash is a SIBLING, so the restore is a move within one directory.
        self.assertEqual(
            recovery["stash"], self.target().replace(".tox", ".replaced.tox")
        )
        with open(recovery["stash"], "rb") as handle:
            self.assertEqual(handle.read(), b"original", "the original survives")

    def test_a_successful_replace_does_not_delete_the_stash(self) -> None:
        # A caller who replaced a file is entitled to the copy they displaced.
        self.write(self.target(), b"original")
        out = snap._snapshot_component(
            self._node(), "/p", {"target": self.target(), "overwrite": "replace"}
        )
        self.assertTrue(out["ok"])
        self.assertTrue(snap._stat_of(out["recovery"]["stash"])["exists"])

    def test_a_save_that_raises_partway_restores_the_original(self) -> None:
        self.write(self.target(), b"original")
        out = snap._snapshot_component(
            self._node(raise_on_save=RuntimeError("disk full")),
            "/p",
            {"target": self.target(), "overwrite": "replace"},
        )
        self.assertFalse(out["ok"])
        self.assertEqual(out["outcome"], "failed")
        # The destination holds the ORIGINAL bytes again, not the half-written
        # ones the failed save left behind.
        with open(self.target(), "rb") as handle:
            self.assertEqual(handle.read(), b"original")
        self.assertTrue(out["recovery"]["restore"]["restored"])

    def test_the_restore_is_verified_against_the_digest_that_was_set_aside(self) -> None:
        # A restore that reports success without checking would be reporting a
        # file nobody can verify.
        self.write(self.target(), b"original")
        stash = snap._stash(self.target())
        self.assertTrue(stash["stashed"])
        # A wrong expected digest must be refused, not accepted.
        bad = snap._restore(stash["stash"], self.target(), "f" * 64)
        self.assertFalse(bad["restored"])
        self.assertIn("does not match", bad["reason"])

    def test_a_recovery_that_refuses_stops_the_save_before_it_destroys_anything(self) -> None:
        # Proceeding without a good copy would destroy the only one.
        self.write(self.target(), b"original")
        node = self._node()
        with mock.patch.object(snap, "_stash", return_value={"stashed": False, "reason": "no room"}):
            out = snap._snapshot_component(
                node, "/p", {"target": self.target(), "overwrite": "replace"}
            )
        self.assertFalse(out["ok"])
        self.assertEqual(out["outcome"], "declined")
        self.assertEqual(out["code"], "tdmcp.snapshot.recovery_failed")
        # The original is untouched, and no save was attempted.
        with open(self.target(), "rb") as handle:
            self.assertEqual(handle.read(), b"original")

    def test_a_first_save_never_stashes_anything(self) -> None:
        # Nothing is displaced on a first write, so there is nothing to keep.
        out = snap._snapshot_component(self._node(), "/p", {"target": self.target()})
        self.assertTrue(out["ok"])
        self.assertFalse(out["recovery"].get("stashed", False))
        self.assertFalse(out["overwrote"])


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
        # MEASURED: COMP.dirty EXISTS and never flips, so the reason is "it
        # cannot move", not "there is no such flag". A flag that always reads
        # False is worse than none, because it looks like an answer.
        self.assertIn("never changes value", report["reason"])
        self.assertIn("always False", report["explanation"])
        self.assertIn("would look like an answer", report["explanation"])


class LoadPlanTests(TempDirCase):
    """The plan asks for a "separate fresh-load workflow".

    Measured: TouchDesigner offers no safe in-place load on this build, so
    this reports the plan and performs none of it. A tool that offered an
    unverifiable, irreversible, session-destroying load would be a data-loss
    primitive wearing a recovery tool's name.
    """

    def _td(self, name: str = "other.toe", folder: str = "/elsewhere") -> Any:
        return SimpleNamespace(project=SimpleNamespace(name=name, folder=folder))

    def test_a_plan_loads_nothing_and_says_so(self) -> None:
        path = self.write(self.target("candidate.toe"), b"toe-bytes")
        with mock.patch.dict(sys.modules, {"td": self._td()}):
            out = snap._load_plan(path)
        self.assertTrue(out["ok"])
        self.assertEqual(out["outcome"], "planned")
        self.assertFalse(out["verified"])
        self.assertFalse(out["undone"])
        # The file is still the only one that was there.
        self.assertEqual(os.listdir(self.dir), ["candidate.toe"])

    def test_a_plan_identifies_the_candidate_by_digest(self) -> None:
        path = self.write(self.target("candidate.toe"), b"toe-bytes")
        with mock.patch.dict(sys.modules, {"td": self._td()}):
            out = snap._load_plan(path)
        self.assertTrue(out["artifact"]["exists"])
        self.assertEqual(len(out["artifact"]["sha256"]), 64)
        self.assertEqual(out["artifact"]["bytes"], 9)

    def test_a_missing_candidate_is_reported_absent_not_as_a_plan(self) -> None:
        # Planning a load of a file that is not there would be reporting on
        # something that does not exist.
        with mock.patch.dict(sys.modules, {"td": self._td()}):
            out = snap._load_plan(self.target("nope.toe"))
        self.assertTrue(out["ok"])
        self.assertFalse(out["artifactPresent"])
        self.assertFalse(out["artifact"]["exists"])

    def test_a_candidate_that_is_already_loaded_is_named(self) -> None:
        # "Would loading this replace what I have open?" is the question a
        # caller actually has, and it needs the current project named.
        folder = self.dir
        name = "current.toe"
        self.write(os.path.join(folder, name), b"current")
        candidate = os.path.join(folder, name)
        with mock.patch.dict(sys.modules, {"td": self._td(name=name, folder=folder)}):
            out = snap._load_plan(candidate)
        self.assertTrue(out["isCurrentProject"])
        self.assertTrue(out["artifactPresent"])
        self.assertEqual(out["currentProject"]["name"], name)

    def _loaded_child(self) -> Any:
        """A loaded COMP child, in the shape `loadTox` returns.

        The parameters carry the SAME predicates export classifies on, so the
        read-back in `_load_component` is genuinely exercised rather than
        asserted against a bare object.
        """
        def par(name: str, kind: str) -> Any:
            node = SimpleNamespace(name=name, label=name, isFloat=False, isInt=False,
                                   isMenu=False, isPulse=False, isToggle=False,
                                   isString=False, isMomentary=False, isSequence=False,
                                   isPython=False, isFolder=False, isOP=False)
            setattr(node, {"float": "isFloat", "int": "isInt", "menu": "isMenu",
                           "pulse": "isPulse", "toggle": "isToggle",
                           "string": "isString"}[kind], True)
            return node

        page = SimpleNamespace(name="Controls",
                               pars=[par("Speed", "float"), par("Count", "int")])
        return SimpleNamespace(path="/project1/host/loaded", family="COMP",
                               customPages=[page])

    def test_the_refusal_names_what_to_do_instead(self) -> None:
        # A refusal with no route forward is a dead end; the fresh-TD route is
        # genuinely fresh and already owned by this codebase.
        path = self.write(self.target("candidate.toe"), b"toe-bytes")
        with mock.patch.dict(sys.modules, {"td": self._td()}):
            out = snap._load_plan(path)
        refusal = out["refusal"]
        self.assertEqual(refusal["code"], "tdmcp.snapshot.load_not_offered")
        self.assertIn("spawn_td", refusal["instead"])
        self.assertIn("revert", refusal["reason"])

    def test_the_current_project_comparison_survives_mixed_separators(self) -> None:
        # TouchDesigner runs on Windows under Wine and reports BACKSLASH paths
        # for the very same file a Linux host bridge receives with forward
        # slashes. Hardcoding either separator made "is this what I already have
        # open?" answer false for the file that IS open.
        root = "/home/acorbeau/project"
        backslashed = root.replace("/", "\\")
        self.assertTrue(
            snap._same_path(root, "scratch.toe", backslashed + "\\scratch.toe")
        )
        self.assertTrue(
            snap._same_path(backslashed, "scratch.toe", root + "/scratch.toe")
        )
        # And a different file is still different.
        self.assertFalse(snap._same_path(root, "scratch.toe", root + "/other.toe"))
        # A partial path is not a match, and empty parts are not a wildcard.
        self.assertFalse(snap._same_path(root, "scratch.toe", root))
        self.assertFalse(snap._same_path("", "", root + "/scratch.toe"))

    def test_the_comparison_is_case_insensitive(self) -> None:
        # TD reports paths with its own casing conventions, and the host may
        # send them either way.
        self.assertTrue(snap._same_path("/Home/Proj", "A.toe", "/home/proj/a.toe"))

    def test_a_wine_drive_prefix_is_not_treated_as_the_same_file(self) -> None:
        # TD reports `z:/home/...` where the host sees `/home/...`. That mapping
        # is real, but stripping a drive prefix would make a DIFFERENT mounted
        # path compare equal, so the mismatch is left visible for a caller that
        # knows the mapping rather than guessed away here.
        self.assertFalse(
            snap._same_path(
                "z:/home/acorbeau/project", "a.toe", "/home/acorbeau/project/a.toe"
            )
        )

    def test_the_load_action_requires_a_target(self) -> None:
        with mock.patch.dict(sys.modules, {"td": self._td()}):
            out = snap.handle_project_snapshot({"action": "load"})
        self.assertFalse(out["ok"])
        self.assertEqual(out["code"], "tdmcp.snapshot.target_required")

    def test_a_component_load_works_and_reports_the_copied_interface(self) -> None:
        # MEASURED: `loadTox` DOES exist and round-trips. My earlier claim that a
        # .tox could not be loaded into an empty COMP was a load-shaped guess;
        # the method is `loadTox`, not `load`.
        path = self.write(self.target("dep.tox"), b"tox-bytes")
        child = self._loaded_child()
        host = SimpleNamespace(family="COMP", path="/project1/host", valid=True,
                              loadTox=lambda _p, **_k: child)
        out = snap._load_component(path, host, "/project1/host")
        self.assertTrue(out["ok"])
        self.assertEqual(out["outcome"], "loaded")
        self.assertEqual(out["kind"], "componentLoad")
        self.assertEqual(out["loaded"]["path"], "/project1/host/loaded")
        # The interface is READ BACK, so a load that lost a page is visible.
        self.assertEqual(out["loaded"]["pageCount"], 1)
        self.assertEqual(
            [p["name"] for p in out["loaded"]["pages"][0]["pars"]],
            ["Speed", "Count"],
        )
        self.assertEqual(
            [p["type"] for p in out["loaded"]["pages"][0]["pars"]],
            ["float", "int"],
        )

    def test_a_component_load_states_that_it_is_a_copy(self) -> None:
        # The whole caveat: after a load, host.dirty and root.dirty stay False,
        # externalTimeStamp stays 0, project.paths stays empty and isLibrary is
        # None. So the .tox is NOT referenced, editing it later changes nothing
        # here, and no dependency flag will ever report the divergence.
        path = self.write(self.target("dep.tox"), b"tox-bytes")
        host = SimpleNamespace(family="COMP", path="/h", valid=True,
                              loadTox=lambda _p, **_k: self._loaded_child())
        out = snap._load_component(path, host, "/h")
        self.assertEqual(out["linkage"], "copy")
        caveat = out["linkageCaveat"]
        self.assertIn("COPIES", caveat)
        self.assertIn("externalTimeStamp", caveat)
        self.assertIn("no external-dependency flag will report", caveat)

    def test_a_missing_component_file_is_refused_before_loading(self) -> None:
        host = SimpleNamespace(family="COMP", path="/h", valid=True)
        out = snap._load_component(self.target("nope.tox"), host, "/h")
        self.assertFalse(out["ok"])
        self.assertEqual(out["code"], "tdmcp.snapshot.target_missing")

    def test_a_load_that_raises_is_failed_with_observed_state(self) -> None:
        path = self.write(self.target("dep.tox"), b"tox-bytes")

        def boom(_p: str, **_k: Any) -> Any:
            raise RuntimeError("bad tox")

        host = SimpleNamespace(family="COMP", path="/h", valid=True, loadTox=boom)
        out = snap._load_component(path, host, "/h")
        self.assertFalse(out["ok"])
        self.assertEqual(out["outcome"], "failed")
        self.assertIn("observed, not assumed", out["explanation"])

    def test_a_load_returning_nothing_is_unknown_not_a_success(self) -> None:
        path = self.write(self.target("dep.tox"), b"tox-bytes")
        host = SimpleNamespace(family="COMP", path="/h", valid=True,
                              loadTox=lambda _p, **_k: None)
        out = snap._load_component(path, host, "/h")
        self.assertFalse(out["ok"])
        self.assertEqual(out["outcome"], "unknown")

    def test_a_load_with_a_host_path_loads_and_without_one_only_plans(self) -> None:
        path = self.write(self.target("dep.tox"), b"tox-bytes")
        host = SimpleNamespace(family="COMP", path="/project1/host", valid=True,
                              loadTox=lambda _p, **_k: self._loaded_child())
        with mock.patch.object(snap, "resolve_op", return_value=host):
            loaded = snap.handle_project_snapshot(
                {"action": "load", "target": path, "path": "/project1/host"}
            )
        self.assertEqual(loaded["kind"], "componentLoad")
        with mock.patch.dict(sys.modules, {"td": self._td()}):
            planned = snap.handle_project_snapshot({"action": "load", "target": path})
        self.assertEqual(planned["kind"], "loadPlan")

    def test_a_component_load_into_a_non_comp_is_refused_by_family(self) -> None:
        path = self.write(self.target("dep.tox"), b"tox-bytes")
        host = SimpleNamespace(family="TOP", path="/project1/t", valid=True)
        with mock.patch.object(snap, "resolve_op", return_value=host):
            out = snap.handle_project_snapshot(
                {"action": "load", "target": path, "path": "/project1/t"}
            )
        self.assertFalse(out["ok"])
        self.assertEqual(out["code"], "tdmcp.snapshot.unsupported_family")

    def test_the_load_action_routes_to_the_plan(self) -> None:
        path = self.write(self.target("candidate.toe"), b"toe-bytes")
        with mock.patch.dict(sys.modules, {"td": self._td()}):
            out = snap.handle_project_snapshot({"action": "load", "target": path})
        self.assertEqual(out["kind"], "loadPlan")
        self.assertEqual(out["outcome"], "planned")


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
