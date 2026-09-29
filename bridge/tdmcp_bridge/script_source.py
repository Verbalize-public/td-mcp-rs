"""Named Python sources: registration, immutable refs, and recorded runs.

`execute_python` takes a script inline and runs it. That is the right control
surface and it stays exactly as it was. This is the other half: a script
registered once and referenced by identity afterwards, so a caller can say "run
that" without resending the source, and so what ran can be identified later.

# The rules, and why each exists

**Registration never executes.** Registering `op('/project1/x').destroy()` returns
a ref and leaves the scene untouched. A registry that executed on register would
be a remote-execution primitive wearing a bookkeeping costume, and "register" is
exactly the verb a caller would not think to guard.

**A ref is its own content, so it cannot go stale.** `sourceId` is derived from
the SHA-256 of the source. Two consequences, both the point:

* A ref's identity *is* its bytes. A holder can reconstruct the exact source from
  any peer without trusting a registry index.
* There is no "is my ref current?" question, because a ref cannot name anything
  but itself. A registry that let one id hold new code would make every cached
  ref silently wrong.

So staleness is reported where it actually exists: on **dependencies**. A source
declares what it needs, and each is reported present, missing, or retargeted at
run time. A run whose dependency moved is reported, not quietly executed against
whatever now occupies the name.

**No authority expansion.** A registered source runs through the same entry point,
with the same globals and the same bounds, as an inline `execute_python`. This
module adds no global, does not widen `__builtins__`, and reaches nothing the
inline path cannot. It holds no capability of its own: it is a name for a string
and a record of what happened.

**No output loss.** Logs, result and truncation markers travel through unchanged,
and an oversized result is reported as truncated rather than dropped -- the run
happened and its effect is unaffected by the size of its return value.

**Effects are reported, never assumed.** A run that returned is not a run whose
effect was observed.
"""
from __future__ import annotations

import hashlib
import threading
import time
from typing import Any

from .constants import SCRIPT_SOURCE_MAX_DEPENDENCIES, SCRIPT_SOURCE_MAX_SOURCES
from .paths import resolve_op


def _err(code: str, message: str, **extra: Any) -> dict[str, Any]:
    out: dict[str, Any] = {"ok": False, "code": code, "message": message}
    out.update(extra)
    return out


def source_id_for(code: str) -> str:
    """The content address of `code`.

    Full SHA-256 truncated to 16 bytes (32 hex chars). 128 bits of a
    cryptographic digest is far past collision risk for the number of scripts one
    user registers, and 64 hex characters is unpleasant to read in a log line.
    """
    return "src_" + hashlib.sha256(code.encode("utf-8")).hexdigest()[:32]


def _normalise_dependencies(raw: Any) -> list[dict[str, str]]:
    """Coerce declared dependencies into `{"kind", "target"}` records.

    A bare string is read as an OP path, which is the common case. Two kinds are
    recognised and no more: `op` and `module`. A wider vocabulary here would mean
    inventing semantics this bridge does not have.
    """
    if raw is None:
        return []
    if not isinstance(raw, list):
        raise ValueError("dependencies must be a list")
    out: list[dict[str, str]] = []
    for item in raw:
        if isinstance(item, str):
            text = item.strip()
            if not text:
                raise ValueError("a dependency may not be empty")
            out.append({"kind": "op", "target": text})
            continue
        if isinstance(item, dict):
            kind = str(item.get("kind") or "op")
            target = str(item.get("target") or "").strip()
            if not target:
                raise ValueError("a dependency needs a non-empty target")
            if kind not in ("op", "module"):
                raise ValueError(
                    f"unsupported dependency kind {kind!r}; use 'op' or 'module'"
                )
            out.append({"kind": kind, "target": target})
            continue
        raise ValueError("a dependency must be a string or an object")
    return out


class SourceRegistry:
    """Registered sources, keyed by content address.

    One instance per bridge package, held in `state`. A lock rather than the
    GIL alone: registration can come from the main thread and a lookup from a
    worker thread, and a half-built record would be worse than a lock.
    """

    def __init__(self) -> None:
        self._lock = threading.Lock()
        self._sources: dict[str, dict[str, Any]] = {}

    def register(
        self,
        code: Any,
        name: str | None = None,
        dependencies: Any = None,
    ) -> dict[str, Any]:
        """Store `code` and return an immutable reference. **Does not execute it.**

        Re-registering identical bytes is idempotent and returns the same ref,
        because the id *is* the content: a second registration of the same source
        is not a new source, and reporting it as one would let a caller believe
        they had versioned something they had not.
        """
        text = code if isinstance(code, str) else str(code)
        try:
            deps = _normalise_dependencies(dependencies)
        except ValueError as exc:
            return _err("tdmcp.script.bad_dependency", str(exc))
        if len(deps) > SCRIPT_SOURCE_MAX_DEPENDENCIES:
            return _err(
                "tdmcp.script.too_many_dependencies",
                f"{len(deps)} dependencies declared, cap is "
                f"{SCRIPT_SOURCE_MAX_DEPENDENCIES}",
            )

        source_id = source_id_for(text)
        now = time.time()
        with self._lock:
            existing = self._sources.get(source_id)
            if existing is not None:
                # Same bytes. Report the ORIGINAL registration rather than
                # moving its clock or its declared dependencies underneath a
                # caller who is already holding this ref.
                return {
                    "ok": True,
                    "sourceId": source_id,
                    "alreadyRegistered": True,
                    "registeredAt": existing["registeredAt"],
                    "byteLength": existing["byteLength"],
                    "dependencies": existing["dependencies"],
                    "executed": False,
                }
            if len(self._sources) >= SCRIPT_SOURCE_MAX_SOURCES:
                return _err(
                    "tdmcp.script.source_limit",
                    f"registry holds {len(self._sources)} sources, cap is "
                    f"{SCRIPT_SOURCE_MAX_SOURCES}",
                )
            record = {
                "sourceId": source_id,
                "name": name,
                "code": text,
                "byteLength": len(text.encode("utf-8")),
                "dependencies": deps,
                "registeredAt": now,
            }
            self._sources[source_id] = record
        return {
            "ok": True,
            "sourceId": source_id,
            "alreadyRegistered": False,
            "registeredAt": now,
            "byteLength": record["byteLength"],
            "dependencies": deps,
            # Stated rather than implied: this call did not run the code.
            "executed": False,
        }

    def get(self, source_id: str) -> dict[str, Any] | None:
        with self._lock:
            record = self._sources.get(source_id)
            return dict(record) if record is not None else None

    def code_for(self, source_id: str) -> str | None:
        with self._lock:
            record = self._sources.get(source_id)
            return None if record is None else record["code"]

    def describe(self, source_id: str) -> dict[str, Any]:
        record = self.get(source_id)
        if record is None:
            return _err(
                "tdmcp.script.unknown_source",
                f"no registered source {source_id!r}; a ref is a content address, "
                "so this source was never registered here or its bytes differ",
                sourceId=source_id,
            )
        return {"ok": True, "source": record}

    def list_sources(self) -> list[dict[str, Any]]:
        with self._lock:
            records = [dict(record) for record in self._sources.values()]
        # A listing is a roster, so the body is never echoed back: that would
        # defeat the point of naming a source instead of resending it.
        for record in records:
            record.pop("code", None)
        records.sort(key=lambda record: record["registeredAt"], reverse=True)
        return records

    def delete(self, source_id: str) -> dict[str, Any]:
        """Forget a source.

        Only the *registry* forgets it. A ref already in a caller's hands still
        carries the bytes, and forgetting is not a claim that the script never
        ran -- which is why the reply says so.
        """
        with self._lock:
            record = self._sources.pop(source_id, None)
        if record is None:
            return _err(
                "tdmcp.script.unknown_source",
                f"no registered source {source_id!r}",
                sourceId=source_id,
            )
        return {
            "ok": True,
            "sourceId": source_id,
            "byteLength": record["byteLength"],
            "note": (
                "forgotten by the registry; a ref already held still carries "
                "these bytes, and nothing here undoes a previous run"
            ),
        }

    def check_dependencies(self, source_id: str) -> list[dict[str, Any]]:
        """Resolve each declared dependency and report what actually answered.

        This is where staleness becomes visible. A declared OP that resolves to
        a different path than the one declared is reported `retargeted`, not
        satisfied: "something is at a path with that name" and "the thing I
        meant" are different answers, and a run that silently received the second
        one would be working from a premise nobody checked.
        """
        record = self.get(source_id)
        if record is None:
            return []
        out: list[dict[str, Any]] = []
        for dep in record["dependencies"]:
            entry = dict(dep)
            if dep["kind"] == "module":
                import importlib.util

                try:
                    found = importlib.util.find_spec(dep["target"]) is not None
                except (ImportError, ValueError):
                    found = False
                entry["status"] = "present" if found else "missing"
            else:
                node = resolve_op(dep["target"], None)
                if node is None or not getattr(node, "valid", False):
                    entry["status"] = "missing"
                else:
                    resolved = str(getattr(node, "path", "") or "")
                    entry["resolvedPath"] = resolved
                    entry["status"] = (
                        "present" if resolved == dep["target"] else "retargeted"
                    )
            out.append(entry)
        return out


def registry() -> SourceRegistry:
    """The bridge's source registry, created on first use.

    Held on the package rather than a module global so a test can swap it, which
    is the same monkeypatch-friendly shape `state.py` uses for everything else.
    """
    from . import state as _state

    existing = getattr(_state._pkg(), "_script_sources", None)
    if existing is None:
        existing = SourceRegistry()
        setattr(_state._pkg(), "_script_sources", existing)
    return existing


def _effect_of(outcome: dict[str, Any]) -> str:
    """What is honestly known about a run's effect on the scene.

    Two answers, and never a third:

    * a run that raised is `unknown` -- the exception says the script stopped,
      not that nothing happened before it stopped;
    * a run that returned is `unobserved` -- the caller can do anything, and
      nothing here inspects the scene afterwards to find out what.

    Truncation does not change either. A shortened *report* is not a different
    kind of run: an earlier version branched on `resultTruncated` and returned
    the same value on both sides, which read as a distinction that did not
    exist. An oversized result is still a completed run.
    """
    return "unknown" if not outcome.get("ok") else "unobserved"


def handle_script_source(params: dict[str, Any]) -> dict[str, Any]:
    """`script_source`: register, inspect, and run named Python sources.

    Actions:
      register  store `source`, return an immutable content-addressed ref
      describe  return one source, including its body
      list      the roster, without source bodies
      delete    forget a source in the registry
      run       execute a registered source through the inline path
      check     resolve a source's declared dependencies, reporting staleness

    `run` takes the same options as `execute_python` and returns its result
    unchanged, with a `source` block naming what ran. Registration itself
    never runs anything.
    """
    action = params.get("action") or "register"
    store = registry()

    if action == "register":
        return store.register(
            params.get("source") or "",
            params.get("name"),
            params.get("dependencies"),
        )

    if action == "list":
        return {"ok": True, "sources": store.list_sources()}

    # Validate the verb BEFORE asking for a ref. Otherwise an unknown action is
    # reported as a missing sourceId, which tells the caller to fix something
    # that was not the problem.
    known_ref_actions = ("describe", "delete", "check", "run")
    if action not in known_ref_actions:
        return _err(
            "tdmcp.script.unknown_action",
            f"unknown script_source action {action!r}; use register, describe, "
            "list, delete, run or check",
        )

    source_id = params.get("sourceId") or params.get("source_id") or ""
    if not isinstance(source_id, str) or not source_id:
        return _err(
            "tdmcp.script.missing_source_id",
            "sourceId is required; it is the content address returned by register",
        )

    if action == "describe":
        return store.describe(source_id)

    if action == "delete":
        return store.delete(source_id)

    if action == "check":
        deps = store.check_dependencies(source_id)
        if not deps and store.get(source_id) is None:
            return _err(
                "tdmcp.script.unknown_source",
                f"no registered source {source_id!r}",
                sourceId=source_id,
            )
        missing = [dep for dep in deps if dep["status"] == "missing"]
        moved = [dep for dep in deps if dep["status"] == "retargeted"]
        return {
            "ok": True,
            "sourceId": source_id,
            "dependencies": deps,
            "missing": missing,
            "retargeted": moved,
            # Stated so a caller can decide, not so a caller is stopped: a
            # missing dependency is information, and whether it matters is the
            # caller's judgement.
            "stale": bool(missing or moved),
            "explanation": (
                "each declared dependency was resolved now; 'missing' means "
                "nothing is there and 'retargeted' means something else is"
            ),
        }

    code = store.code_for(source_id)
    if code is None:
        return _err(
            "tdmcp.script.unknown_source",
            f"no registered source {source_id!r}; a ref is a content address, "
            "so this source was never registered here or its bytes differ",
            sourceId=source_id,
        )

    # The inline entry point, unchanged. Whatever it accepts it also accepts
    # here: no new global, no widened builtins, no extra authority.
    from .execute import handle_execute_python

    dependencies = store.check_dependencies(source_id)
    forward = {
        key: value
        for key, value in params.items()
        if key
        not in ("action", "sourceId", "source_id", "source", "name", "dependencies")
    }
    forward["script"] = code
    outcome = handle_execute_python(forward)
    outcome["source"] = {
        "sourceId": source_id,
        "byteLength": store.get(source_id)["byteLength"],
        "dependencies": dependencies,
        "missing": [dep for dep in dependencies if dep["status"] == "missing"],
        "retargeted": [
            dep for dep in dependencies if dep["status"] == "retargeted"
        ],
        # A dependency that moved or vanished is reported with the run, not
        # withheld from it. Refusing would be a policy this tool has no basis
        # for; hiding it would be the dishonest option.
        "dependenciesStale": bool(
            any(dep["status"] != "present" for dep in dependencies)
        ),
    }
    outcome["effect"] = _effect_of(outcome)
    return outcome
