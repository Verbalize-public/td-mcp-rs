"""A teardown still in flight is not licence to dial a replacement on top of it.

`is_shutting_down()` exists to say "the session is gone, but the previous
worker is still draining the socket it was serving on". Its own docstring
names the consequence it was built for:

    every caller that sees `False` treats it as licence to dial a replacement

That sentence describes a caller, and until this file there was none. Both
connect paths in `tox_callbacks` — the per-frame one and the 2 s wall-clock
watchdog — decided whether to dial from `is_connected()` alone, so a
`disconnect()` whose bounded join expired (the state round 33 manufactured and
round 34 could reach) was reported as a plain disconnect, and the very next
pass started a second worker on the port the first still held.

The daemon sees that as a new session for a pid it already has, supersedes the
generation it is serving, and cancels whatever call was in flight. The pilot
measured exactly that (`tdmcp.bridge.lost` on a healthy, uncontended session),
so the cost is a dropped call, not a cosmetic race.

These tests drive the REAL `tox_callbacks` namespace. Only the bridge module
object and the dial are substituted, and the teardown predicate itself is
never re-implemented here: a local copy of the rule under test can agree with
itself while the shipped code is wrong, which is how the first two tests here
let a frame-path mutant through.
"""
import sys
from pathlib import Path
from types import SimpleNamespace

CALLBACKS = Path(__file__).resolve().parents[1] / 'tox_callbacks.py'


def _load_callbacks():
    namespace = {'__name__': 'test_callbacks'}
    exec(compile(CALLBACKS.read_text(), str(CALLBACKS), 'exec'), namespace)
    return namespace


def _bridge(*, connected, shutting_down=False):
    """A bridge module reporting a known state.

    `shutting_down` is a plain bool -> a current package that can answer.
    `'absent'` -> an older package predating the predicate entirely.
    `'raises'` -> a bridge mid-reload, reading incoherent thread state.
    """
    mod = SimpleNamespace(is_connected=lambda: connected)
    if shutting_down == 'absent':
        pass  # the attribute is simply not there
    elif shutting_down == 'raises':
        def _boom():
            raise RuntimeError("bridge is mid-reload")

        mod.is_shutting_down = _boom
    else:
        mod.is_shutting_down = lambda: bool(shutting_down)
    return mod


def _wire(monkeypatch, mod, dials, **frame_globals):
    """Load the callbacks with a bridge module, a counted dial, a stub clock."""
    namespace = _load_callbacks()
    monkeypatch.setitem(sys.modules, 'td', SimpleNamespace(run=lambda script, **kw: None))
    namespace['_bridge_mod'] = lambda: mod
    namespace['_run_bootstrap'] = lambda: dials.append(1)
    namespace['_comp'] = lambda: SimpleNamespace(par=SimpleNamespace(eval=lambda: False))
    namespace['_par_bool'] = lambda *a: True
    namespace['_refresh_face'] = lambda *a, **k: None
    namespace['_td_delay_ref'] = lambda: None
    namespace.update(
        {
            '_ui_ready': True,
            '_prev_connect': True,  # no rising edge: exercise the retry path
            '_prev_cancel_pulse': False,
            '_had_connected': True,
            'process_pending': lambda: None,
            'task_snapshot': lambda: [],
            'pending_count': lambda: 0,
        }
    )
    namespace.update(frame_globals)
    return namespace


# ── the defect: a teardown in flight is not a free port ───────────────


def test_the_watchdog_does_not_dial_while_the_previous_worker_is_still_exiting(
    monkeypatch,
):
    """The defect itself, on the paused-timeline path.

    `is_connected()` is False here — the session really is gone from the
    caller's point of view, and that is not in dispute. The third fact, that
    the worker is still draining the socket, is what the watchdog has to read
    before it treats the port as free.
    """
    dials = []
    namespace = _wire(monkeypatch, _bridge(connected=False, shutting_down=True), dials)

    namespace['_reconnect_watchdog']()

    assert dials == [], (
        "the watchdog dialled while the previous worker was still holding the "
        "socket -- the exact state that supersedes the daemon's generation and "
        "cancels the call in flight"
    )


def test_the_per_frame_path_does_not_dial_onto_a_draining_teardown(monkeypatch):
    """The same defect on the playing path, which is a separate decision.

    Two gates, one defect. A fix applied to only one of them leaves a bridge
    that reconnects perfectly while the timeline runs and stalls the moment it
    is paused — the harder half to notice, because the working case is the one
    under ordinary use.
    """
    dials = []
    namespace = _wire(monkeypatch, _bridge(connected=False, shutting_down=True), dials)

    namespace['onFrameStart'](None)

    assert dials == [], "the per-frame path dialled onto a socket still being drained"


def test_the_per_frame_path_reports_reconnecting_rather_than_connected(monkeypatch):
    """And it must not claim to be serving while it is deliberately not dialing.

    Falling through to the connected path would run the pump against a dead
    session and report a phase the operator reads as healthy. The phase is the
    part a dial counter cannot see.
    """
    dials = []
    namespace = _wire(monkeypatch, _bridge(connected=False, shutting_down=True), dials)

    namespace['onFrameStart'](None)

    assert namespace['_phase'] == "Re-connecting", (
        f"expected an honest 'still reconnecting' phase, got {namespace['_phase']!r}"
    )


# ── the other direction: a gate that never opens is its own defect ────


def test_the_watchdog_still_dials_when_the_teardown_has_actually_finished(
    monkeypatch,
):
    """A real disconnect must still recover, or the fix is a dead bridge."""
    dials = []
    namespace = _wire(
        monkeypatch, _bridge(connected=False, shutting_down=False), dials
    )

    namespace['_reconnect_watchdog']()

    assert dials == [1], "a genuinely finished session must still be redialed"


def test_the_per_frame_path_still_connects_once_the_teardown_is_done(monkeypatch):
    """The veto direction for the frame path, for the same reason."""
    dials = []
    namespace = _wire(
        monkeypatch, _bridge(connected=False, shutting_down=False), dials
    )

    namespace['onFrameStart'](None)

    assert dials == [1], "a genuinely finished session must still be redialed"


def test_a_live_session_is_never_dialed_onto(monkeypatch):
    """Sanity: the ordinary connected path is unchanged by any of this."""
    dials = []
    namespace = _wire(monkeypatch, _bridge(connected=True, shutting_down=False), dials)

    namespace['_reconnect_watchdog']()
    namespace['onFrameStart'](None)

    assert dials == []


# ── the predicate must fail OPEN, or older packages strand ────────────


def test_an_older_bridge_without_the_predicate_is_not_stranded(monkeypatch):
    """`is_shutting_down` is new; a package predating it must still dial.

    The bridge package is reloaded from the handshake's `bridgePackageDir`
    while the tox callbacks are baked separately, so a user can genuinely have
    a `tox_callbacks` newer than its `tdmcp_bridge`. Failing closed on a
    missing predicate reads that as "a teardown is in flight forever" and
    leaves such an installation permanently disconnected — a fix that breaks
    the only case it cannot observe.
    """
    dials = []
    namespace = _wire(
        monkeypatch, _bridge(connected=False, shutting_down='absent'), dials
    )

    namespace['onFrameStart'](None)

    assert dials == [1], "a package with no teardown predicate must still be dialed"


def test_a_raising_predicate_does_not_forbid_dialing_either(monkeypatch):
    """`is_shutting_down` reads thread state, and a mid-reload bridge is exactly
    when that state is briefly incoherent. An exception must not become
    "dialing is forbidden" — that turns a transient blip into a dead session.
    """
    dials = []
    namespace = _wire(
        monkeypatch, _bridge(connected=False, shutting_down='raises'), dials
    )

    namespace['onFrameStart'](None)

    assert dials == [1], "an exception from the predicate must not forbid dialing"
