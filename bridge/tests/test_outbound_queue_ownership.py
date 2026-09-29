"""A superseded worker must not touch the live session's outbound queue.

`_event_queue` was a module global read by both sides of the event uplink: the
producer (`enqueue_event`) and the consumer (`_drain_outbound`, called from
`serve_queued`'s loop). Module globals are reset by `importlib.reload`, and a
bounded teardown (`_await_shutdown`) deliberately proceeds to dial the next
session while a slow previous worker is still running. Those two facts put two
live workers in one file, and a global read by both sides cannot tell them
apart.

The measured failure, before this fix: an event produced for the LIVE session
was drained by the ABANDONED worker and written onto the dead stream, so the
frame was both lost and stolen from the session the daemon was waiting on. The
drain reported healthy.

The queue is now published on the thread that owns it, and the session is
resolved by identity (`task_queue.active_event_queue`).
"""

from __future__ import annotations

import importlib
import os
import queue
import socket
import sys
import threading
import time

import pytest

sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))

import tdmcp_bridge  # noqa: E402 — the path shim above must run first
from tdmcp_bridge import task_queue as tq  # noqa: E402

# Sockets backing live workers, closed when a test finishes. A leaked worker
# holding a queue is indistinguishable from a real session to the next test in
# this process, so nothing may outlive its test.
stop_socks: list = []


class RecordingStream:
    """A dead stream that only records what a worker tried to write to it."""

    def __init__(self) -> None:
        self.frames: list[bytes] = []

    def write(self, data):
        self.frames.append(bytes(data))
        return len(data)

    def flush(self) -> None:
        pass


def _live_worker(name: str, stop: threading.Event) -> threading.Thread:
    """A REAL `serve_queued` worker, still running, over a `socketpair()`.

    Two properties force the real worker rather than a stub thread: the product
    decides queue ownership inside `serve_queued` (not at the call site), and
    ownership is only observable once a thread is ALIVE. A thread that returns
    immediately is not `is_alive()`, so the session resolution would report
    "no session" for a state the product never produces.

    Returns `(worker, peer_file)` — the peer end is how a test cycles the
    worker's loop, because a `makefile()` stream blocks in `_read_frame` and
    the drain at the top of the loop only runs when a frame arrives.
    """
    sock, peer = socket.socketpair()
    stream = sock.makefile("rwb")
    stop_socks.append((sock, peer))
    t = threading.Thread(
        target=tq.serve_queued,
        args=(stream,),
        kwargs={"idle_dead_s": 30.0},
        name=name,
        daemon=True,
    )
    t.start()
    deadline = time.monotonic() + 5.0
    while getattr(t, "tdmcp_event_queue", None) is None and time.monotonic() < deadline:
        time.sleep(0.01)
    assert getattr(t, "tdmcp_event_queue", None) is not None, (
        "serve_queued never published its outbound queue on its own thread"
    )
    return t, peer.makefile("rwb")


def _ping(peer, req_id: int = 1) -> None:
    """Send one request so the peer's worker cycles back to the drain point."""
    tq._write_frame(peer, {"type": "request", "id": req_id, "method": "ping", "params": {}})


@pytest.fixture(autouse=True)
def no_session():
    """Every test starts and ends with no session and no leaked worker."""
    stop_socks.clear()
    tdmcp_bridge._active_thread = None
    tdmcp_bridge._active_stream = None
    tdmcp_bridge._task_queue._event_queue = None
    try:
        yield
    finally:
        for pair in stop_socks:
            for s in pair:
                try:
                    s.close()
                except OSError:
                    pass
        stop_socks.clear()
        tdmcp_bridge._active_thread = None
        tdmcp_bridge._active_stream = None
        tdmcp_bridge._task_queue._event_queue = None


def test_the_second_session_takes_ownership_from_the_first():
    """The measured regression, through the REAL `serve_queued` ownership path.

    Session A is still running — exactly the state `_await_shutdown` permits
    when its bounded wait times out — when session B starts. Measured before
    the fix: A kept ownership, so an event produced for the live session B
    landed in A's queue and was written to A's abandoned stream.
    """
    a, _peer_a = _live_worker("session-a", threading.Event())
    assert tdmcp_bridge._active_thread is a, "the first worker must claim the session"

    b, _peer_b = _live_worker("session-b", threading.Event())
    assert a.is_alive(), "the previous worker must still be running for this test to mean anything"
    assert b.is_alive()

    assert tdmcp_bridge._active_thread is b, (
        "the newest session did not take ownership; the live session is now "
        "unreachable to every producer"
    )
    assert tq.active_event_queue() is b.tdmcp_event_queue

    assert tq.enqueue_event({"type": "event", "name": "for-b"}) is True
    assert b.tdmcp_event_queue.qsize() == 1, "the event did not reach the live session"
    assert a.tdmcp_event_queue.qsize() == 0, (
        "the superseded session's queue took the event for the live session"
    )


def test_a_superseded_worker_writes_nothing_to_its_abandoned_stream():
    """The drain-time half: ownership is re-checked, not decided once.

    An event is queued for session A, then session B supersedes it while A is
    still in its loop. A must not write that event to its abandoned stream even
    though the event is sitting in the queue A owns. This is the guard that
    `owns_queue()` provides on every pass; a startup-only ownership decision
    would let A keep draining and the event would be written to a stream the
    daemon has already abandoned.
    """
    a, peer_a = _live_worker("session-a", threading.Event())
    qa = a.tdmcp_event_queue

    # An event for session A, queued and waiting in A's own queue.
    assert tdmcp_bridge._active_thread is a
    assert tq.enqueue_event({"type": "event", "name": "for-a"}) is True
    assert qa.qsize() == 1

    # Session B supersedes A while A is still running.
    b, _peer_b = _live_worker("session-b", threading.Event())
    assert tdmcp_bridge._active_thread is b
    assert a.is_alive(), "A must still be running for the supersession to mean anything"

    # Cycle A's loop so it ACTUALLY reaches the drain point. Without this the
    # test is vacuous: a worker blocked in `_read_frame` never drains, so it
    # would leave the event alone whether or not the guard exists.
    _ping(peer_a)
    time.sleep(0.3)
    assert a.is_alive(), "A exited; the test did not exercise a superseded live worker"
    assert qa.qsize() == 1, (
        "a superseded worker drained the queue it can still see: the event was "
        "written to a stream the daemon has already abandoned, so it is both "
        "lost and stolen from the live session"
    )


def test_a_worker_that_is_not_the_session_does_not_resolve_to_it():
    """A live worker with a published queue is not automatically the session.

    This is the case the measured bug turned on: a leftover worker from a
    previous session is alive and has a queue, but the recorded session is
    another thread. Resolving by "any live worker" would be the guess that
    reintroduces cross-session delivery.
    """
    other, _ = _live_worker("other", threading.Event())
    session, _ = _live_worker("session", threading.Event())

    tdmcp_bridge._active_thread = session

    assert tq.active_event_queue() is session.tdmcp_event_queue
    assert tq.active_event_queue() is not other.tdmcp_event_queue


def test_the_queue_a_worker_drains_is_the_one_it_published():
    """Ownership is by identity: the worker drains the queue it published.

    A worker that finds a live session already recorded claims the new queue
    but must NOT drain the old one — that is exactly how an event is lost.
    """
    first_q = queue.Queue(maxsize=4)
    second_q = queue.Queue(maxsize=4)
    first_q.put_nowait({"type": "event", "name": "stale"})
    second_q.put_nowait({"type": "event", "name": "mine"})

    stream = RecordingStream()
    assert tq._drain_outbound(stream, second_q) is True
    assert stream.frames, "the worker drained nothing"
    assert first_q.qsize() == 1, "a superseded queue was drained by the wrong worker"
    assert second_q.qsize() == 0


def test_draining_without_a_queue_is_a_programming_error_not_a_silent_drop():
    """`_drain_outbound` must not quietly succeed with no queue.

    Silently returning True would let a future caller that forgets to pass the
    worker's queue look exactly like a healthy session with an idle uplink: no
    frame is ever written and nothing reports a problem. The parameter is
    required for that reason, and the refusal has to be loud.
    """
    stream = RecordingStream()
    try:
        tq._drain_outbound(stream, None)  # type: ignore[arg-type]
    except TypeError as exc:
        assert "own queue" in str(exc)
    else:
        pytest.fail(
            "_drain_outbound(stream, None) returned without refusing: a caller "
            "with no queue is indistinguishable from a healthy session with "
            "nothing to send"
        )


def test_a_dead_session_thread_resolves_to_no_queue():
    """A session whose worker has exited is not a session.

    `active_event_queue` must not hand a producer the queue of a thread that
    is no longer serving, or every event is silently dropped while `enqueue`
    reports success.
    """
    dead = threading.Thread(target=lambda: None, daemon=True)
    dead.start()
    dead.join(timeout=5)
    assert not dead.is_alive(), "this test needs a thread that has EXITED"
    dead.tdmcp_event_queue = queue.Queue(maxsize=4)

    tdmcp_bridge._active_thread = dead
    assert tq.active_event_queue() is None
    assert tq.enqueue_event({"type": "event", "name": "log"}) is False


def test_the_session_queue_survives_a_module_reload():
    """A reload resets module globals; the session must not be forgotten.

    This is the half that first motivated thread-carried state for the stream.
    The queue rides on the same thread for the same reason.
    """
    worker, _ = _live_worker(tdmcp_bridge._WORKER_THREAD_NAME, threading.Event())
    q = queue.Queue(maxsize=4)
    worker.tdmcp_event_queue = q
    worker.tdmcp_stream = RecordingStream()
    tdmcp_bridge._active_thread = worker
    tdmcp_bridge._active_stream = worker.tdmcp_stream

    importlib.reload(tdmcp_bridge)
    tdmcp_bridge._recover_session()

    assert tdmcp_bridge._active_thread is worker, "reload forgot the live session"
    assert tq.active_event_queue() is q, "reload stranded the session's outbound queue"
    assert tq.enqueue_event({"type": "event", "name": "log"}) is True
    assert q.qsize() == 1
