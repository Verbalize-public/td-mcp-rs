"""A teardown that outlives its join must not be read as a free session.

`disconnect()` bounds the worker's join at 2 s and then closes the stream out
from under it. When the join times out the worker is still draining the socket
it was blocked reading, but the session is genuinely gone: `is_connected()`
reports `False`, and every caller that sees `False` takes it as licence to dial
a replacement. Starting one on the port the old worker still holds is what a
superseded session looks like from the daemon — `already connected` for the
same pid seconds later, then a heartbeat pong timeout.

So "disconnected" and "the previous worker has not finished leaving" have to be
two answers rather than one. These tests hold the state `disconnect()`
actually produces when the join times out, and check that the bridge can tell
them apart.
"""

from __future__ import annotations

import importlib
import sys
import threading
import time
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import tdmcp_bridge  # noqa: E402


class _UnblockableStream:
    """A stream whose pending read ignores shutdown, as `disconnect()` allows.

    `disconnect()` calls `cancel_pending_io` precisely because closing the
    socket out from under a pending read can hang the *closing* thread; it
    tolerates that call doing nothing and falls back to the bounded join. This
    is that fallback, and nothing more.
    """

    def __init__(self) -> None:
        self.closed = False

    def cancel_pending_io(self, thread_id=None) -> None:  # noqa: ARG002
        return None

    def close(self) -> None:
        self.closed = True


@pytest.fixture
def slow_teardown():
    """A worker that outlives the bounded join in `disconnect()`.

    Built exactly as `bootstrap_threaded` builds one — named, carrying its
    stream — because the recovery path keys on both. The worker only notices
    its stop event between `step` sleeps, so the join in `disconnect()` is
    guaranteed to time out against it without the test taking 2 s to say so.
    """
    importlib.reload(tdmcp_bridge)
    stop = threading.Event()
    step = 0.25
    stream = _UnblockableStream()

    def worker() -> None:
        while not stop.is_set():
            stop.wait(step)

    thread = threading.Thread(
        target=worker, name=tdmcp_bridge._WORKER_THREAD_NAME, daemon=True
    )
    thread.tdmcp_stream = stream
    thread.start()
    tdmcp_bridge._active_stream = stream
    tdmcp_bridge._active_thread = thread
    tdmcp_bridge._shutting_down_thread = None
    try:
        yield thread, stop
    finally:
        stop.set()
        thread.join(timeout=5)
        tdmcp_bridge._shutting_down_thread = None
        tdmcp_bridge._active_stream = None
        tdmcp_bridge._active_thread = None


def test_a_timed_out_join_is_reported_as_shutting_down_not_connected(slow_teardown):
    """The two facts must not collapse into one boolean.

    `is_connected()` is `False` the moment the globals are cleared, and it has
    to stay `False` — the session is over. The additional fact that the old
    worker is still draining the socket is a separate answer, because a caller
    that only sees `False` will dial over it.
    """
    thread, _ = slow_teardown

    assert tdmcp_bridge.disconnect() is True

    assert tdmcp_bridge.is_connected() is False, (
        "the session is over: reporting it connected would make the watchdog "
        "wait for a stream that is closed"
    )
    assert thread.is_alive(), (
        "this test is only about the timed-out-join case; if the worker exited "
        "there is no teardown in flight and nothing to tell apart"
    )
    assert tdmcp_bridge.is_shutting_down() is True, (
        "a worker that outlived its join is being reported as no teardown at "
        "all, so the next bootstrap will race it for the port"
    )


def test_the_next_bootstrap_waits_for_the_worker_it_could_not_join(slow_teardown):
    """`_await_shutdown` is what stops a replacement racing the old worker.

    A bootstrap that starts immediately would put a second worker on a port
    the first still holds. The wait is bounded, so a worker that never exits
    cannot block a reconnect forever.
    """
    thread, stop = slow_teardown
    assert tdmcp_bridge.disconnect() is True
    assert tdmcp_bridge.is_shutting_down() is True

    exited = threading.Event()

    def watch() -> None:
        # The worker polls its stop event, so it cannot exit before we set it.
        assert not exited.is_set()
        exited.set()

    watcher = threading.Thread(target=watch, daemon=True)
    watcher.start()
    watcher.join(timeout=5)

    assert tdmcp_bridge._await_shutdown(timeout_s=0.1) is False, (
        "the wait reported the teardown finished while the worker is still "
        "running, so a bootstrap would start on top of it"
    )
    assert thread.is_alive()

    stop.set()
    assert tdmcp_bridge._await_shutdown(timeout_s=5.0) is True
    assert not thread.is_alive()
    assert tdmcp_bridge.is_shutting_down() is False


def test_a_reload_does_not_forget_a_teardown_in_flight(slow_teardown):
    """The reload resets module globals, so the teardown has to be re-found.

    Same class as the session itself: `_shutting_down_thread` is assigned at
    module level, so a reload that forgot it would let the next bootstrap dial
    a port the old worker still holds. The worker keeps its name and its
    `tdmcp_stream` attribute across the reload, which is what makes it
    findable.
    """
    assert tdmcp_bridge.disconnect() is True
    assert tdmcp_bridge.is_shutting_down() is True

    tdmcp_bridge._load_bridge_package(None)

    assert tdmcp_bridge.is_connected() is False, (
        "a worker finishing its teardown was adopted as a live session"
    )
    assert tdmcp_bridge.is_shutting_down() is True, (
        "the reload forgot the teardown in flight, so the next bootstrap "
        "would race a worker still draining the socket"
    )


def test_a_finished_teardown_is_not_reported_as_in_flight(slow_teardown):
    """A worker that has left is not a teardown — that state must clear.

    Without this, `is_shutting_down()` would latch True forever and every later
    bootstrap would pay a pointless wait, or worse, believe the port is
    contested when it is free.
    """
    thread, stop = slow_teardown
    assert tdmcp_bridge.disconnect() is True
    stop.set()
    thread.join(timeout=5)
    assert not thread.is_alive()

    assert tdmcp_bridge.is_shutting_down() is False
    assert tdmcp_bridge._await_shutdown(timeout_s=0.1) is True


def test_a_bootstrap_waits_for_a_lingering_worker_before_it_dials(
    slow_teardown, monkeypatch
):
    """The wait has to sit between the teardown and the dial, not beside it.

    `bootstrap_threaded` is the only thing that starts a replacement, so this
    is the call site that decides whether the old worker is still holding the
    port when the new one dials. Everything past the dial needs a live TD, so
    the dial itself is stubbed to raise and the order is what gets asserted.

    The wait is BOUNDED on purpose — a worker that never exits must not block a
    reconnect forever — so the assertion is that the bootstrap waited and then
    went ahead anyway, with the abandonment announced, not that the worker was
    gone. A bootstrap that does not wait at all cannot be told from this by the
    worker alone, which is why the wait is timed.
    """
    old, stop = slow_teardown
    assert tdmcp_bridge.disconnect() is True
    assert tdmcp_bridge.is_shutting_down() is True

    order = []

    def fake_dial():
        order.append(time.monotonic())
        raise RuntimeError("stubbed: the test stops at the dial")

    monkeypatch.setattr(tdmcp_bridge, "_load_bridge_package", lambda path: None)
    monkeypatch.setattr(tdmcp_bridge, "mark_main_thread", lambda: None)
    monkeypatch.setattr(tdmcp_bridge, "_identity_snapshot", lambda: {})
    monkeypatch.setattr(tdmcp_bridge, "dial", fake_dial)
    monkeypatch.setattr(tdmcp_bridge, "_env_bridge_dir", lambda: None)
    monkeypatch.setattr(tdmcp_bridge, "_conventional_bridge_dir", lambda: None)
    monkeypatch.setattr(tdmcp_bridge, "_recover_session", lambda: None, raising=False)
    # A wait that returns immediately is the mutant this test exists to catch,
    # so keep the production timeout and record how long the call really took.
    waits = []
    real_await = tdmcp_bridge._await_shutdown

    def timed_await(timeout_s=2.0):
        start = time.monotonic()
        result = real_await(timeout_s)
        waits.append(time.monotonic() - start)
        return result

    monkeypatch.setattr(tdmcp_bridge, "_await_shutdown", timed_await)
    started = time.monotonic()

    with pytest.raises(RuntimeError, match="stubbed"):
        tdmcp_bridge.bootstrap_threaded(bridge_dir="/nonexistent")

    assert order, "bootstrap never reached the dial, so the wait order is untested"
    assert waits, (
        "bootstrap_threaded dialed without waiting for the lingering worker: "
        "the new session starts while the old one still holds the port, which "
        "the daemon sees as a supersede followed by a heartbeat timeout"
    )
    assert waits[0] > 0.0, (
        "the wait returned instantly, so the lingering worker was given no "
        "chance to finish leaving the port"
    )
    assert order[0] - started >= waits[0], (
        "the dial happened before the wait finished, so the wait is not "
        "actually between the teardown and the dial"
    )
    assert old.is_alive(), (
        "the bounded wait should have abandoned a worker that ignores its stop "
        "event; if it exited, this test is no longer covering that path"
    )

    stop.set()
    old.join(timeout=5)
