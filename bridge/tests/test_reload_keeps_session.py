"""A package reload must not orphan a live session.

`_load_bridge_package` re-executes the `tdmcp_bridge` module body on every
bootstrap, and module-level state is re-initialized when it does — so
`_active_stream` / `_active_thread` (set at the end of `bootstrap_threaded`)
are reset to `None` while the worker thread is still running and still holding
the socket. `is_connected()` then reports False for a healthy session, the
2 s `_reconnect_watchdog` decides the session is dead, and it bootstraps a
SECOND stream; the first worker is orphaned, still reading the old socket. The
observable signature on the daemon is `pid handshake — already connected`
followed by a heartbeat pong timeout.

`logtap` already solves exactly this class of bug — see
`_unwrap_stale_tee`, "a reload rebinds module state but an older instance keeps
its class's original __globals__". These tests pin that the session globals got
the same treatment, using a real thread and a real module reload rather than
mocking either.
"""
from __future__ import annotations

import importlib
import os
import sys
import threading

import pytest

sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))

import tdmcp_bridge  # noqa: E402 — the path shim above must run first


@pytest.fixture
def live_session():
    """A worker thread set up exactly as `bootstrap_threaded` sets one up.

    Yields `(thread, stop)` so a test can end the worker's life: the recovery
    path recognises a session by the thread's name and the stream parked on it,
    so a bare unnamed thread would describe a state the product never produces
    — and the test would pass for the wrong reason.
    """
    importlib.reload(tdmcp_bridge)
    stop = threading.Event()
    stream = object()
    thread = threading.Thread(
        target=stop.wait, name=tdmcp_bridge._WORKER_THREAD_NAME, daemon=True
    )
    thread.tdmcp_stream = stream
    thread.start()
    tdmcp_bridge._active_stream = stream
    tdmcp_bridge._active_thread = thread
    try:
        yield thread, stop
    finally:
        stop.set()
        thread.join(timeout=5)
        tdmcp_bridge._active_stream = None
        tdmcp_bridge._active_thread = None


def test_a_live_session_survives_a_package_reload(live_session):
    """The load-bootstrap path reloads; a connected session must stay connected.

    Without the carry-over, `bootstrap_threaded` sees `_active_stream is None`,
    skips `disconnect()` (nothing to disconnect, as far as it knows), and opens
    a second stream while the first worker still owns the first socket.
    """
    thread, _ = live_session
    stream = tdmcp_bridge._active_stream
    assert tdmcp_bridge.is_connected() is True

    tdmcp_bridge._load_bridge_package(None)

    assert tdmcp_bridge.is_connected() is True, (
        "a package reload dropped the live session: `is_connected()` went "
        "False while the worker thread is still alive, so the reconnect "
        "watchdog will dial a second stream and orphan the first"
    )
    assert tdmcp_bridge._active_thread is thread, (
        "the module lost the reference to its own running worker"
    )
    assert tdmcp_bridge._active_stream is stream, (
        "the re-adopted session lost its stream, so `disconnect()` would have "
        "nothing to shut down and the socket would leak"
    )
    assert thread.is_alive()


def test_a_reload_during_worker_startup_keeps_the_session(live_session):
    """The reload must not lose a worker that is still starting.

    The real race is the reverse direction from the dead-worker case: a worker
    that is alive but whose `tdmcp_stream` has not been parked yet is a worker
    the bridge is in the middle of creating. Recovery must skip it (there is no
    stream to adopt) and leave the session empty, so the bootstrap that follows
    completes the wiring rather than inheriting a half-built one.
    """
    thread, _ = live_session
    del thread.tdmcp_stream
    tdmcp_bridge._active_stream = None
    tdmcp_bridge._active_thread = None

    tdmcp_bridge._recover_session()

    assert tdmcp_bridge.is_connected() is False
    assert tdmcp_bridge._active_thread is None
    assert tdmcp_bridge._active_stream is None


def test_a_second_bridge_worker_is_not_adopted(live_session):
    """Adoption keys on the one worker this bridge names and parks a stream on.

    A thread that merely shares the name is not this bridge's session. The
    check that matters is the `tdmcp_stream` payload, so a worker without one
    must not be adopted.
    """
    thread, _ = live_session
    del thread.tdmcp_stream

    tdmcp_bridge._recover_session()

    assert tdmcp_bridge.is_connected() is False, (
        "a worker with no stream was adopted; `disconnect()` would then have a "
        "thread but nothing to shut down, and the session would look live"
    )


def test_a_foreign_live_thread_is_not_adopted(live_session):
    """Only the bridge's OWN named worker may be adopted, not any live thread.

    The name check is the only thing separating them: some other part of the
    host (TD itself, a user's script) can hold live threads, and adopting one
    of those as the session would hand `disconnect()` a thread that is not
    reading our socket. The impostor is deliberately live and does carry a
    `tdmcp_stream` attribute, so neither the liveness check nor the payload
    check can catch it.
    """
    thread, stop = live_session
    stop.set()
    thread.join(timeout=5)
    tdmcp_bridge._active_stream = None
    tdmcp_bridge._active_thread = None

    impostor_stop = threading.Event()
    impostor = threading.Thread(
        target=impostor_stop.wait, name="not-the-bridge", daemon=True
    )
    impostor.tdmcp_stream = object()
    impostor.start()
    try:
        tdmcp_bridge._recover_session()

        assert tdmcp_bridge.is_connected() is False, (
            "a foreign live thread was adopted as the bridge session: "
            "`disconnect()` would shut down a stream nobody is serving"
        )
    finally:
        impostor_stop.set()
        impostor.join(timeout=5)
