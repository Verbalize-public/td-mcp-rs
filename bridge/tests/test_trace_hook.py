"""The redial decision has to record itself, because nothing else can see it.

Measured while chasing the recurring supersede in the campaign runs: the daemon
logs `pid handshake — already connected` → `superseding bridge session` →
`bridge session ended — superseded, queue cleared`, and the caller's payload is
`tdmcp.bridge.lost`. Everything visible happens AFTER the bridge decided to
dial. The decision itself was unobservable.

Four attempts to observe it from outside all failed identically:
`execute_python` is itself delivered over the bridge being lost, so a probe
that installs itself through that bridge is cancelled by the event it exists
to catch — the decision log stayed 0 bytes while the supersede fired at
seq 33-37. A trivial `os.path.exists` probe call was lost the same way, which
rules the probe out as the trigger.

So the trace hook below is the fix for the OBSERVABILITY, not for the bug.
The bug is still open; these tests pin only what the hook promises.
"""
from __future__ import annotations

import json
import os
import sys
import types

import pytest

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from tdmcp_bridge import is_shutting_down  # noqa: F401  (package presence check)


def _load_callbacks():
    """Load `bridge/tox_callbacks.py` the way TouchDesigner does."""
    import importlib.util

    here = os.path.dirname(os.path.abspath(__file__))
    path = os.path.join(os.path.dirname(here), "tox_callbacks.py")
    spec = importlib.util.spec_from_file_location("__tdmcp_callbacks__", path)
    mod = importlib.util.module_from_spec(spec)
    sys.modules["__tdmcp_callbacks__"] = mod
    spec.loader.exec_module(mod)
    return mod


@pytest.fixture(scope="module")
def cb():
    return _load_callbacks()


def test_nothing_is_written_unless_the_environment_asks_for_it(cb, tmp_path):
    """Off by default: one env lookup, no file, no side effect."""
    trace = tmp_path / "trace.jsonl"
    env = {k: v for k, v in os.environ.items() if k != "TDMCP_BRIDGE_TRACE"}
    cb.os.environ.clear()
    cb.os.environ.update(env)
    cb._trace("watchdog_dialing", connected=False)
    assert not trace.exists(), "a trace file appeared with no env var set"


def test_the_hook_writes_one_json_object_per_decision(cb, tmp_path):
    trace = tmp_path / "trace.jsonl"
    cb.os.environ["TDMCP_BRIDGE_TRACE"] = str(trace)
    try:
        cb._trace("watchdog_dialing", connected=False, active_thread=None)
        cb._trace("watchdog_no_action", connected=True)
        lines = trace.read_text().splitlines()
    finally:
        cb.os.environ.pop("TDMCP_BRIDGE_TRACE", None)

    assert len(lines) == 2
    first = json.loads(lines[0])
    assert first["event"] == "watchdog_dialing"
    assert first["connected"] is False
    assert first["active_thread"] is None
    assert isinstance(first["t"], float), "a decision with no timestamp is not evidence"


def test_a_failure_to_trace_never_breaks_the_reconnect_decision(cb, tmp_path):
    """The hook must not be able to become a new reason the bridge is down.

    An unwritable path is the easy case; the required one is a value that
    cannot be serialised by `json.dumps`, which a real snapshot can contain
    (a thread repr is a str, but `default=repr` is what keeps that true for
    anything else a future caller passes).
    """
    unwritable = tmp_path / "no-such-dir" / "trace.jsonl"
    cb.os.environ["TDMCP_BRIDGE_TRACE"] = str(unwritable)
    try:
        cb._trace("watchdog_dialing")  # must not raise

        class Unserialisable:
            def __repr__(self):
                raise RuntimeError("no text form")

        cb._trace("frame_dialing", value=Unserialisable())  # must not raise
    finally:
        cb.os.environ.pop("TDMCP_BRIDGE_TRACE", None)


def test_the_snapshot_reports_exactly_what_the_predicates_decided_on(cb):
    """The trace is only evidence if it agrees with the branch it explains.

    This pins the whole point: the `watchdog_dialing` record carries
    `connected` and `shutting_down` from the SAME predicates that chose the
    branch, so "it dialled while the old worker was still draining" is
    readable from one line — and a future edit that changes what is recorded
    without changing the decision would fail here.
    """
    mod = types.SimpleNamespace(
        is_connected=lambda: False,
        is_shutting_down=lambda: True,
        _active_thread="thread-obj",
        _active_stream="stream-obj",
    )
    snap = cb._bridge_snapshot(mod)
    assert snap == {
        "connected": False,
        "shutting_down": True,
        # `repr`, not the value: this is what tells a real thread
        # (`<Thread(...)>`) from the STRING "None", and that difference is
        # the whole question when a later line claims the port was free.
        "active_thread": "'thread-obj'",
        "active_stream": "'stream-obj'",
    }

    absent = cb._bridge_snapshot(None)
    assert absent == {"mod": None}, "a missing module must say so, not look clean"

    # A bridge package with neither predicate must still produce a snapshot.
    bare = types.SimpleNamespace()
    assert cb._bridge_snapshot(bare)["connected"] is False
    assert cb._bridge_snapshot(bare)["shutting_down"] is False
    assert cb._bridge_snapshot(bare)["active_thread"] == "None"


def test_the_watchdog_records_which_branch_it_took(cb, tmp_path):
    """Both dialling branches trace, and the draining one says it waited.

    The supersede is caused by the `watchdog_dialing` line specifically. If
    this test only checked that SOMETHING was written, removing the dial
    branch's trace entirely would survive it.
    """
    trace = tmp_path / "trace.jsonl"
    cb.os.environ["TDMCP_BRIDGE_TRACE"] = str(trace)

    calls: list[str] = []
    mod = types.SimpleNamespace(
        is_connected=lambda: False,
        is_shutting_down=lambda: False,
        _active_thread=None,
        _active_stream=None,
    )
    comp = types.SimpleNamespace(
        par=types.SimpleNamespace(
            Connect=types.SimpleNamespace(eval=lambda: True),
            Autoconnect=types.SimpleNamespace(eval=lambda: True),
        )
    )

    # One pass: not connected, not draining -> dial.
    orig_bootstrap = cb._run_bootstrap
    orig_comp = cb._comp
    orig_mod = cb._bridge_mod
    orig_face = cb._refresh_face
    orig_sched = cb._reconnect_watchdog_scheduled
    try:
        cb._run_bootstrap = lambda: calls.append("bootstrap")
        cb._comp = lambda: comp
        cb._bridge_mod = lambda: mod
        cb._refresh_face = lambda *a, **k: None
        cb._reconnect_watchdog_scheduled = False
        cb.td = types.SimpleNamespace(run=lambda *a, **k: None)
        cb._reconnect_watchdog()
        assert calls == ["bootstrap"]
        first = json.loads(trace.read_text().splitlines()[0])
        assert first["event"] == "watchdog_dialing"

        # Same pass, but a teardown is in flight: wait, and do NOT dial.
        trace.write_text("")
        calls.clear()
        mod.is_shutting_down = lambda: True
        cb._reconnect_watchdog()
        assert calls == [], "a draining teardown must not dial"
        second = json.loads(trace.read_text().splitlines()[0])
        assert second["event"] == "watchdog_wait_draining"
        assert second["shutting_down"] is True

        # Connected: neither branch, but still recorded.
        trace.write_text("")
        mod.is_connected = lambda: True
        cb._reconnect_watchdog()
        third = json.loads(trace.read_text().splitlines()[0])
        assert third["event"] == "watchdog_no_action"
        assert third["connected"] is True
    finally:
        cb.os.environ.pop("TDMCP_BRIDGE_TRACE", None)
        cb._run_bootstrap = orig_bootstrap
        cb._comp = orig_comp
        cb._bridge_mod = orig_mod
        cb._refresh_face = orig_face
        cb._reconnect_watchdog_scheduled = orig_sched
