"""`project_snapshot`: save a project or a component, and say what was written.

# The measurements this is built on

Everything here comes from TouchDesigner 2025.32460, and two of the facts
would have produced a lying receipt had they been assumed rather than measured.

**The returned path is the only trustworthy identity.** Both save paths take a
path and return something that may not be it:

* `COMP.save(filepath, createFolders=False, password=None) -> str` returns the
  path it used. Saving twice to the same path returned the SAME path both times
  with a DIFFERENT sha256 — a silent in-place overwrite, with nothing to say so.
* `td.project.save(path, saveExternalToxs=False) -> bool` returns a bool, not a
  path. Saving to an existing path did NOT overwrite: it wrote a second file,
  `name.toe` alongside `name.1.toe`, byte-identical. The doc says the filename
  is "incremented" when a path already exists.

So a caller can never assume the file it asked for is the file that was written.
The receipt therefore reports the path TD actually returned, and a stat of that
path, and nothing is reported as saved until that file has been observed.

**`project.save` returning `False` does not mean nothing was written.** Its doc
says `False` covers "if the file exists, and when prompted, the user selects to
not overwrite" — and on this build a project save could also *wedge the MCP
stream* entirely, leaving the outcome unknown. A blocked save is not a failed
save: claiming "no file was written" when one may well have been is the worst
available answer. So the three outcomes are kept apart: written, declined, and
**unknown**, and an unknown outcome is never reported as either of the other
two and never invites a retry that could double-write.

**`project.modified` is a path list, not a bool.** Measured: `['/',
'/project1/tdmcp_rs', '/local/time', '/local/maps/master', ...]`. A count is
not a summary of that, so the paths themselves are reported, bounded and
flagged when bounded.

# What a save is not

This is a write to the user's disk, so it is not retried, not repeated, and not
attempted when the destination's state is not known. Nothing here repacks a
project, expands a directory, or touches a `.tox` that is not the one named.
"""

from __future__ import annotations

import hashlib
import os
import re
from typing import Any

from .constants import (
    SNAPSHOT_MAX_INCREMENT,
    SNAPSHOT_MAX_MODIFIED_PATHS,
    SNAPSHOT_MAX_PATH_CHARS,
)
from .paths import resolve_op

_INCREMENT_SUFFIX = re.compile(r"^(?P<stem>.+?)(?P<n>\d+)(?P<ext>\.[^.]*)$")


def _err(code: str, message: str, **extra: Any) -> dict[str, Any]:
    out: dict[str, Any] = {"ok": False, "code": code, "message": message}
    out.update(extra)
    return out


def _text(value: Any, limit: int = SNAPSHOT_MAX_PATH_CHARS) -> str:
    if value is None:
        return ""
    try:
        return str(value)[:limit]
    except Exception:  # noqa: BLE001
        return ""


def _stat_of(path: str) -> dict[str, Any]:
    """What is actually on disk at `path`.

    The identity of an artifact is its CONTENT, not the promise that a write
    happened. A receipt that reported a path without reading it would be
    reporting an intention.
    """
    try:
        data = open(path, "rb").read()
    except FileNotFoundError:
        return {"exists": False, "path": path}
    except Exception as exc:  # noqa: BLE001
        return {
            "exists": False,
            "path": path,
            "statError": f"{type(exc).__name__}: {exc}"[:200],
        }
    return {
        "exists": True,
        "path": path,
        "bytes": len(data),
        # Full length: a truncated digest is a weaker identity than the receipt
        # claims to give, and a consumer cannot tell it was cut short.
        "sha256": hashlib.sha256(data).hexdigest(),
    }


def _requested_vs_written(requested: str, written: str) -> dict[str, Any]:
    """How far the file that was written is from the one that was asked for.

    MEASURED: `project.save` increments rather than overwriting, so the written
    path is often `name.1.toe` when `name.toe` was requested. That is a
    legitimate outcome, but it is not the same file, and a caller that assumed
    otherwise would read a stale artifact and believe it was current.
    """
    if not written or written == requested:
        return {"asRequested": True}
    match = _INCREMENT_SUFFIX.match(written)
    if not match:
        return {"asRequested": False, "reason": "the written path is unrelated to the request"}
    n = int(match.group("n"))
    if n < 1 or n > SNAPSHOT_MAX_INCREMENT:
        return {
            "asRequested": False,
            "reason": (
                f"the written path is {n} increments from the request, past the "
                f"reported cap of {SNAPSHOT_MAX_INCREMENT}"
            ),
        }
    return {
        "asRequested": False,
        "reason": (
            "the destination already existed, so TouchDesigner wrote the next "
            "free name instead of overwriting"
        ),
        "increment": n,
    }


def _modified_paths() -> tuple[list[str], bool]:
    """`project.modified` as a path list.

    MEASURED: a list of paths, not a flag. Bounded, and `truncated` says so,
    because a count is not a summary of which operators are unsaved.
    """
    import td  # type: ignore

    try:
        raw = list(getattr(td.project, "modified", []) or [])
    except Exception:  # noqa: BLE001
        return [], False
    paths = [_text(p) for p in raw]
    truncated = len(paths) > SNAPSHOT_MAX_MODIFIED_PATHS
    return paths[:SNAPSHOT_MAX_MODIFIED_PATHS], truncated


def _before_state(target: str) -> dict[str, Any]:
    """What is at the destination BEFORE a write, so an overwrite is visible."""
    return _stat_of(target)


def _project_unsaved() -> dict[str, Any]:
    """`project.modified`, for a receipt about the PROJECT.

    MEASURED: a list of operator paths, not a flag, so a count is not a summary
    of it. Bounded, and `truncated` says so. A save never claims to have made
    these clean -- TouchDesigner does not report that, and a receipt that
    implied otherwise would be asserting something nobody checked.
    """
    paths, truncated = _modified_paths()
    return {
        "available": True,
        "scope": "project",
        "modifiedPaths": paths,
        "modifiedCount": len(paths),
        "truncated": truncated,
        "explanation": (
            "TouchDesigner reports unsaved state as a list of operator paths, "
            "not a flag; this save does not claim to have made them clean"
        ),
    }


def _component_unsaved(node: Any, path: str) -> dict[str, Any]:
    """Unsaved state for a COMPONENT receipt.

    Deliberately reports `available: false`. TouchDesigner's Python surface
    exposes no per-operator dirty flag -- measured: `isModified` reads None on
    every child of root, and `td.root.dirty` is a single whole-tree bool with
    nothing per component.

    An earlier version put the PROJECT's modified paths here, which was wrong
    twice over: a component save does not save the project, so those paths
    describe something this call never touched; and an empty list would have
    read as "nothing is unsaved", which is the opposite of what is known.
    """
    return {
        "available": False,
        "scope": "component",
        "explanation": (
            "TouchDesigner exposes no per-component dirty flag, so this receipt "
            "says nothing about whether this component has unsaved changes; "
            "the project's own modified paths are reported by action=project"
        ),
    }


def _snapshot_component(node: Any, path: str, params: dict[str, Any]) -> dict[str, Any]:
    """Save one COMPONENT, and report the file TD actually wrote."""
    target = _text(params.get("target") or params.get("targetPath"))
    if not target:
        return _err(
            "tdmcp.snapshot.target_required",
            "a component snapshot needs target: an explicit path, because the "
            "default would land in project.folder under a generated name",
            path=path,
        )
    overwrite = str(params.get("overwrite") or "fail").lower()
    if overwrite not in ("fail", "replace"):
        return _err(
            "tdmcp.snapshot.bad_overwrite",
            f"overwrite must be 'fail' or 'replace', got {overwrite!r}",
            path=path,
        )

    before = _before_state(target)
    if before["exists"] and overwrite == "fail":
        # Refusing here is the whole point of the policy: a caller who did not
        # ask to replace someone's file must not have it replaced.
        return _err(
            "tdmcp.snapshot.target_exists",
            f"{target} already exists and overwrite is 'fail'; pass "
            "overwrite='replace' to replace it deliberately",
            path=path,
            target=target,
            existing=before,
        )

    # The previous content is kept so a failed save can be reported against
    # something the caller can check, and so a successful overwrite is legible.
    saved_to: str | None = None
    try:
        returned = node.save(target)
    except Exception as exc:  # noqa: BLE001
        after = _stat_of(target)
        return {
            "ok": False,
            "code": "tdmcp.snapshot.save_failed",
            "message": f"{type(exc).__name__}: {exc}",
            "outcome": "failed",
            "path": path,
            "target": target,
            "before": before,
            "after": after,
            "unsaved": _component_unsaved(node, path),
            "explanation": (
                "the save raised; whether anything was written is reported as "
                "observed, not assumed"
            ),
        }

    saved_to = _text(returned)
    if not saved_to:
        # An empty return is not a path, and must not be stat'ed as one.
        return {
            "ok": False,
            "code": "tdmcp.snapshot.no_path_returned",
            "message": "the component save returned no path, so no artifact identity is available",
            "outcome": "unknown",
            "path": path,
            "target": target,
            "before": before,
            "unsaved": _component_unsaved(node, path),
        }

    after = _stat_of(saved_to)
    if not after["exists"]:
        # TD said it saved, and the file is not there. Claiming success here
        # would be a receipt for a write nobody can find.
        return {
            "ok": False,
            "code": "tdmcp.snapshot.not_written",
            "message": f"the save reported {saved_to} but no file is there",
            "outcome": "unknown",
            "path": path,
            "target": target,
            "written": saved_to,
            "before": before,
            "unsaved": _component_unsaved(node, path),
        }

    return {
        "ok": True,
        "outcome": "written",
        "kind": "component",
        "path": path,
        "target": target,
        "written": saved_to,
        "artifact": after,
        "before": before,
        "overwrote": bool(before["exists"] and saved_to == target),
        "destination": _requested_vs_written(target, saved_to),
        "unsaved": _component_unsaved(node, path),
        "explanation": (
            "the artifact is identified by the path TouchDesigner returned and "
            "the digest of what is on disk, not by the path that was requested"
        ),
    }


def _snapshot_project(params: dict[str, Any]) -> dict[str, Any]:
    """Save the whole project, and be honest when the outcome is unknown.

    MEASURED, and the reason this half is not the simple one:

    * `project.save` returns a BOOL, not a path. `False` covers "the file
      exists and the user declined to overwrite" — so a False is a decline,
      not a failure, and saying otherwise would send a caller looking for a
      problem that does not exist.
    * Saving to an existing path did NOT overwrite: it wrote `name.1.toe`
      alongside `name.toe`, byte-identical. The requested path is therefore
      not a reliable identity, and the file is found by looking for what
      appeared.
    * A project save could WEDGE the MCP stream, leaving the outcome unknown:
      neither written nor declined. `unknown` is a first-class outcome here,
      and it is never reported as the other two, because claiming "nothing was
      written" when a file may exist is the worst available answer — and it
      invites a retry that double-writes.
    """
    import td  # type: ignore

    target = _text(params.get("target") or params.get("targetPath"))
    if not target:
        return _err(
            "tdmcp.snapshot.target_required",
            "a project snapshot needs target: an explicit path. The default is "
            "an incremented name under project.folder, which no receipt could "
            "predict",
        )
    overwrite = str(params.get("overwrite") or "fail").lower()
    if overwrite not in ("fail", "replace"):
        return _err(
            "tdmcp.snapshot.bad_overwrite",
            f"overwrite must be 'fail' or 'replace', got {overwrite!r}",
        )

    before = _before_state(target)
    siblings_before = _sibling_digests(target)
    if before["exists"] and overwrite == "fail":
        return _err(
            "tdmcp.snapshot.target_exists",
            f"{target} already exists and overwrite is 'fail'; pass "
            "overwrite='replace' to save alongside or over it deliberately",
            target=target,
            existing=before,
        )

    unsaved = _project_unsaved()
    try:
        returned = bool(td.project.save(target))
    except Exception as exc:  # noqa: BLE001
        return {
            "ok": False,
            "code": "tdmcp.snapshot.save_failed",
            "message": f"{type(exc).__name__}: {exc}",
            "outcome": "unknown",
            "target": target,
            "before": before,
            "unsaved": unsaved,
            "explanation": (
                "the save raised; whether a file was written is not known, so "
                "this is reported as unknown and not retried"
            ),
        }

    if not returned:
        # A decline, not a failure. The doc is explicit that False covers the
        # "file exists and the user chose not to overwrite" case.
        return {
            "ok": False,
            "code": "tdmcp.snapshot.not_saved",
            "message": (
                "TouchDesigner declined to save (the file exists and was not "
                "replaced, or the save was declined); this is not an error in "
                "the project"
            ),
            "outcome": "declined",
            "target": target,
            "before": before,
            "unsaved": unsaved,
        }

    # The bool said yes, but which file? MEASURED: the requested path may be
    # untouched while an incremented sibling was written, so the artifact is
    # found by what changed rather than assumed from the request.
    written = _find_written(target, siblings_before)
    if written is None:
        return {
            "ok": False,
            "code": "tdmcp.snapshot.not_written",
            "message": (
                f"the save reported success but neither {target} nor any "
                "incremented sibling appeared, so no artifact identity is "
                "available"
            ),
            "outcome": "unknown",
            "target": target,
            "before": before,
            "unsaved": unsaved,
        }

    after = _stat_of(written)
    return {
        "ok": True,
        "outcome": "written",
        "kind": "project",
        "target": target,
        "written": written,
        "artifact": after,
        "before": before,
        "destination": _requested_vs_written(target, written),
        "unsaved": unsaved,
        "explanation": (
            "the project save returns a bool, not a path; the artifact is "
            "identified by the file that actually appeared, since an existing "
            "destination is incremented rather than overwritten"
        ),
    }


def _sibling_digests(target: str) -> dict[str, str]:
    """Digests of `target.1`, `target.2`, ... before the save.

    MEASURED: an existing destination is not overwritten, so the file that
    appears is often a sibling. Recording the siblings first is what lets the
    difference be attributed to THIS save rather than to something else that
    happened to be on disk.
    """
    found: dict[str, str] = {}
    match = re.match(r"^(?P<stem>.+?)(?P<ext>\.[^.]*)$", target)
    if not match:
        return found
    stem, ext = match.group("stem"), match.group("ext")
    for n in range(1, SNAPSHOT_MAX_INCREMENT + 1):
        candidate = f"{stem}.{n}{ext}"
        try:
            data = open(candidate, "rb").read()
        except FileNotFoundError:
            continue
        except Exception:  # noqa: BLE001
            continue
        found[candidate] = hashlib.sha256(data).hexdigest()
    return found


def _find_written(target: str, siblings_before: dict[str, str]) -> str | None:
    """The path a save actually wrote, by difference rather than by request.

    Checked in the order a caller would reason about: the requested path first
    (an overwrite, or a first write), then the incremented siblings. A sibling
    that already existed AND is unchanged is not this save's work.
    """
    if _stat_of(target)["exists"]:
        return target
    for candidate, digest_before in siblings_before.items():
        after = _stat_of(candidate)
        if after["exists"] and after.get("sha256") != digest_before:
            return candidate
    for n in range(1, SNAPSHOT_MAX_INCREMENT + 1):
        match = re.match(r"^(?P<stem>.+?)(?P<ext>\.[^.]*)$", target)
        if not match:
            return None
        candidate = f"{match.group('stem')}.{n}{match.group('ext')}"
        if _stat_of(candidate)["exists"]:
            return candidate
    return None


def handle_project_snapshot(params: dict[str, Any]) -> dict[str, Any]:
    """`project_snapshot`: save a project or a named component, with a receipt.

    Actions:
      project   save the whole project to `target`
      component save one COMP to `target`
      status    report unsaved state and the project file, writing nothing

    Every save reports `written` — the path TouchDesigner actually produced —
    and `artifact`, a digest of the bytes on disk. The requested path is
    reported separately and is never treated as the identity.
    """
    action = str(params.get("action") or "project")

    if action == "status":
        return {
            "ok": True,
            "outcome": "reported",
            "kind": "status",
            "name": _text(getattr(_project(), "name", None), 256),
            "folder": _text(getattr(_project(), "folder", None)),
            "unsaved": _project_unsaved(),
            "explanation": "writes nothing; this is the current unsaved state",
        }

    if action == "component":
        path = params.get("path") or ""
        node = resolve_op(path, params.get("contextPath"))
        if node is None or not getattr(node, "valid", False):
            return {"ok": False, "code": "tdmcp.op.not_found", "path": path}
        if _text(getattr(node, "family", None), 16) != "COMP":
            return _err(
                "tdmcp.snapshot.unsupported_family",
                "a component snapshot saves a COMP; "
                f"resolved family is {_text(getattr(node, 'family', None), 16)!r}",
                path=path,
            )
        return _snapshot_component(node, path, params)

    if action == "project":
        return _snapshot_project(params)

    return _err(
        "tdmcp.snapshot.unknown_action",
        f"unknown project_snapshot action {action!r}; use project, component or status",
    )


def _project() -> Any:
    import td  # type: ignore

    return td.project
