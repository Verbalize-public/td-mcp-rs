"""Regression coverage for the worker-thread/main-thread queue split.

`tdmcp_bridge.serve_queued` must never call `dispatch()` itself (that would
run handler code — which touches `td.*` in real TD — off the main thread).
This test drives the real framing + queue + pump code over a `socketpair()`,
simulating the daemon on one end and TD's Execute DAT `onFrameStart` pump on
the "main thread" (here: the test thread, after joining the worker's enqueue).

Run: `python -m unittest bridge/tests/test_bridge_queue.py` (no TD, no deps).
"""

from __future__ import annotations

import contextlib
import io
import os
import queue
import socket
import sys
import threading
import time
import unittest
from unittest import mock

sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))

import tdmcp_bridge  # noqa: E402


class QueuedServeTest(unittest.TestCase):
    def setUp(self) -> None:
        # Fresh module-level queue per test — tests run in one process.
        tdmcp_bridge._reset_pending_for_tests()  # noqa: SLF001
        tdmcp_bridge._task_queue._event_queue = None  # noqa: SLF001
        bridge_sock, daemon_sock = socket.socketpair()
        self.bridge_stream = bridge_sock.makefile("rwb")
        self.daemon_stream = daemon_sock.makefile("rwb")
        self.addCleanup(bridge_sock.close)
        self.addCleanup(daemon_sock.close)

    def _send_request(self, req_id: int, method: str, params: dict | None = None) -> None:
        tdmcp_bridge._write_frame(
            self.daemon_stream,
            {"type": "request", "id": req_id, "method": method, "params": params or {}},
        )

    def _recv_response(self) -> dict:
        return tdmcp_bridge._read_frame(self.daemon_stream)

    def _wait_pending(self, n: int = 1, timeout: float = 1.0) -> None:
        deadline = time.monotonic() + timeout
        while tdmcp_bridge.pending_count() < n and time.monotonic() < deadline:
            time.sleep(0.01)

    def test_worker_never_dispatches_td_methods_directly(self) -> None:
        """TD API methods must land in the queue, not run on the worker."""
        worker = threading.Thread(
            target=tdmcp_bridge.serve_queued, args=(self.bridge_stream,), daemon=True
        )
        worker.start()
        self._send_request(1, "execute_python", {"script": "result = 1"})

        # Give the worker time to read + enqueue; it must NOT answer on its own.
        self._wait_pending(1)
        self.assertEqual(tdmcp_bridge.pending_count(), 1)

        # Now simulate the main-thread pump (Execute DAT onFrameStart).
        drained = tdmcp_bridge.process_pending()
        self.assertEqual(drained, 1)

        resp = self._recv_response()
        self.assertEqual(resp["id"], 1)
        self.assertTrue(resp["result"]["ok"])
        self.assertEqual(resp["result"]["result"], 1)

    def test_ping_answered_on_worker_without_process_pending(self) -> None:
        """Idle heartbeat ping must not depend on the main-thread pump."""
        worker = threading.Thread(
            target=tdmcp_bridge.serve_queued, args=(self.bridge_stream,), daemon=True
        )
        worker.start()
        self._send_request(2, "ping")
        # Blocking recv — must complete without calling process_pending.
        resp = self._recv_response()
        self.assertEqual(resp["id"], 2)
        self.assertEqual(resp["result"], {"ok": True, "pong": True})
        self.assertEqual(tdmcp_bridge.pending_count(), 0)

    def test_multiple_requests_drained_in_one_pump(self) -> None:
        """Batch-drain semantics, independent of I/O timing.

        The real daemon<->bridge protocol is strictly one-in-flight per
        connection (`serve_queued` only reads the next frame after the
        current request's response is written), so this exercises
        `process_pending`'s own batching directly rather than depending on
        stream pipelining.
        """
        slots = [tdmcp_bridge.queue.Queue(maxsize=1) for _ in range(3)]
        for i, slot in enumerate(slots, start=1):
            tdmcp_bridge._enqueue_pending(  # noqa: SLF001
                {"type": "request", "id": i, "method": "ping", "params": {}},
                slot,
            )

        drained = tdmcp_bridge.process_pending()
        self.assertEqual(drained, 3)
        for slot in slots:
            self.assertEqual(slot.get_nowait()["result"], {"ok": True, "pong": True})

    def test_unknown_method_returns_error_without_crashing_pump(self) -> None:
        worker = threading.Thread(
            target=tdmcp_bridge.serve_queued, args=(self.bridge_stream,), daemon=True
        )
        worker.start()
        self._send_request(7, "not_a_method")

        self._wait_pending(1)

        tdmcp_bridge.process_pending()
        resp = self._recv_response()
        self.assertEqual(resp["id"], 7)
        self.assertIn("unknown method", resp["error"]["message"])

    def test_write_failure_on_superseded_connection_is_quiet_not_a_traceback(
        self,
    ) -> None:
        """Root-cause fix (found live against a real TD session, mid-M3
        debugging): a write failure here is the *routine* way this loop
        discovers it has been superseded by a newer connection — the daemon
        closes its end, and the next write is what surfaces that as an
        OSError. It used to dump a full traceback to stderr (the Textport,
        in real TD) on every single reconnect; it must now log one quiet
        line and exit cleanly instead."""

        class _WriteFailsStream:
            def __init__(self, inner):
                self._inner = inner

            def read(self, *a, **kw):
                return self._inner.read(*a, **kw)

            def write(self, _data):
                raise OSError(6, "WriteFile failed (WinError 6)")

            def flush(self):
                pass

            def close(self):
                self._inner.close()

        failing_stream = _WriteFailsStream(self.bridge_stream)
        stderr_capture = io.StringIO()

        worker = threading.Thread(
            target=tdmcp_bridge.serve_queued, args=(failing_stream,), daemon=True
        )
        with contextlib.redirect_stderr(stderr_capture):
            worker.start()
            self._send_request(1, "ping")
            worker.join(timeout=2.0)

        self.assertFalse(worker.is_alive(), "worker must exit, not hang, on write failure")
        output = stderr_capture.getvalue()
        self.assertIn("stream closed", output)
        self.assertNotIn("Traceback (most recent call last)", output)

    def test_task_snapshot_empty_then_fields(self) -> None:
        self.assertEqual(tdmcp_bridge.task_snapshot(), [])
        self.assertEqual(tdmcp_bridge.pending_count(), 0)
        slot = tdmcp_bridge.queue.Queue(maxsize=1)
        tdmcp_bridge._enqueue_pending(  # noqa: SLF001
            {
                "type": "request",
                "id": 9,
                "method": "inspect",
                "params": {"path": "/project1/e2e_kit"},
            },
            slot,
        )
        snap = tdmcp_bridge.task_snapshot()
        self.assertEqual(len(snap), 1)
        self.assertEqual(snap[0]["state"], "queued")
        self.assertEqual(snap[0]["method"], "inspect")
        self.assertEqual(snap[0]["summarize"], "/project1/e2e_kit")
        self.assertEqual(snap[0]["id"], 9)

    def test_cancel_queued_clears_and_unblocks(self) -> None:
        slots = [tdmcp_bridge.queue.Queue(maxsize=1) for _ in range(2)]
        for i, slot in enumerate(slots, start=1):
            tdmcp_bridge._enqueue_pending(  # noqa: SLF001
                {"type": "request", "id": i, "method": "ping", "params": {}},
                slot,
            )
        self.assertEqual(tdmcp_bridge.pending_count(), 2)
        n = tdmcp_bridge.cancel_queued()
        self.assertEqual(n, 2)
        self.assertEqual(tdmcp_bridge.pending_count(), 0)
        self.assertEqual(tdmcp_bridge.task_snapshot(), [])
        for i, slot in enumerate(slots, start=1):
            resp = slot.get_nowait()
            self.assertEqual(resp["id"], i)
            self.assertEqual(resp["error"]["code"], "tdmcp.bridge.cancelled")

    def test_summarize_execute_python_first_line(self) -> None:
        s = tdmcp_bridge.summarize_request(
            {
                "method": "execute_python",
                "params": {"script": "\n# comment\nresult = 1\n"},
            }
        )
        self.assertEqual(s, "# comment")

    def test_enqueue_event_reaches_wire_via_worker_thread(self) -> None:
        """M2 log uplink: `enqueue_event` frames are written by the
        connection's own `serve_queued` worker — never a second writer
        racing `_write_frame` on the same stream.

        The stream here (a plain `socketpair().makefile()`) has no read
        timeout support, so the worker's blocking read never wakes up on its
        own to drain — a real request is what cycles the loop back to the
        top-of-loop drain point, same as the real TCP stream does every
        ``_READ_POLL_S`` while idle.
        """
        worker = threading.Thread(
            target=tdmcp_bridge.serve_queued, args=(self.bridge_stream,), daemon=True
        )
        worker.start()
        deadline = time.monotonic() + 1.0
        while tdmcp_bridge._task_queue._event_queue is None and time.monotonic() < deadline:  # noqa: SLF001
            time.sleep(0.01)
        self.assertIsNotNone(
            tdmcp_bridge._task_queue._event_queue,  # noqa: SLF001
            "serve_queued must install its outbound queue before serving",
        )

        ok = tdmcp_bridge.enqueue_event(
            {
                "type": "event",
                "name": "log",
                "payload": {"records": [{"level": "info", "target": "t", "msg": "hi"}]},
            }
        )
        self.assertTrue(ok)

        # Unblock the worker's current read so its loop cycles back to the
        # drain point; a normal request/response must still work cleanly.
        self._send_request(1, "ping")
        resp = self._recv_response()
        self.assertEqual(resp["result"], {"ok": True, "pong": True})

        frame = tdmcp_bridge._read_frame(self.daemon_stream)  # noqa: SLF001
        self.assertEqual(frame["type"], "event")
        self.assertEqual(frame["name"], "log")
        self.assertEqual(frame["payload"]["records"][0]["msg"], "hi")

    def test_enqueue_event_without_active_connection_drops_silently(self) -> None:
        self.assertIsNone(tdmcp_bridge._task_queue._event_queue)  # noqa: SLF001
        self.assertFalse(tdmcp_bridge.enqueue_event({"type": "event", "name": "log", "payload": {}}))


class IdleDeadTest(unittest.TestCase):
    """serve_queued exits after inbound silence when the stream supports timeouts."""

    def setUp(self) -> None:
        tdmcp_bridge._reset_pending_for_tests()  # noqa: SLF001
        bridge_sock, daemon_sock = socket.socketpair()
        self.daemon_sock = daemon_sock
        self.addCleanup(bridge_sock.close)
        self.addCleanup(daemon_sock.close)
        self.stream = tdmcp_bridge._TcpStream(bridge_sock)  # noqa: SLF001

    def test_idle_dead_exits_serve_queued(self) -> None:
        done = threading.Event()

        def run() -> None:
            tdmcp_bridge.serve_queued(self.stream, idle_dead_s=0.25)
            done.set()

        worker = threading.Thread(target=run, daemon=True)
        worker.start()
        # No frames written — worker should exit on idle_dead.
        self.assertTrue(done.wait(2.0), "serve_queued should exit after idle_dead")
        worker.join(timeout=1.0)
        self.assertFalse(worker.is_alive())


@unittest.skipIf(
    sys.platform.startswith("win"),
    "Windows' socket.socketpair() emulation doesn't propagate shutdown() to a "
    "concurrent blocking recv() the way real POSIX sockets do (verified "
    "manually — recv() never unblocks); this path only ships on macOS/Linux, "
    "where shutdown() is the documented, reliable cross-thread cancellation "
    "primitive.",
)
class DisconnectTest(unittest.TestCase):
    """Regression for the close-while-blocked-read freeze (TCP stream path).

    `disconnect()` must not block: the worker thread is parked in a blocking
    `read()` with nothing to read (no pending job), and `disconnect()` has to
    unstick it (`cancel_pending_io` -> POSIX `shutdown`) rather than closing
    the socket out from under it.
    """

    def setUp(self) -> None:
        tdmcp_bridge._reset_pending_for_tests()  # noqa: SLF001
        bridge_sock, daemon_sock = socket.socketpair()
        self.daemon_sock = daemon_sock
        self.addCleanup(daemon_sock.close)

        stream = tdmcp_bridge._TcpStream(bridge_sock)  # noqa: SLF001
        thread = threading.Thread(target=tdmcp_bridge.serve_queued, args=(stream,), daemon=True)
        thread.start()
        # Let the worker actually enter its blocking read before we disconnect.
        time.sleep(0.05)
        tdmcp_bridge._active_stream = stream  # noqa: SLF001
        tdmcp_bridge._active_thread = thread  # noqa: SLF001

    def test_disconnect_does_not_hang_and_joins_worker(self) -> None:
        thread = tdmcp_bridge._active_thread  # noqa: SLF001

        started = time.monotonic()
        ok = tdmcp_bridge.disconnect()
        elapsed = time.monotonic() - started

        self.assertTrue(ok)
        self.assertLess(elapsed, 2.0, "disconnect() should not block on a pending read")
        self.assertFalse(thread.is_alive(), "worker thread must exit after disconnect()")
        self.assertIsNone(tdmcp_bridge._active_stream)  # noqa: SLF001
        self.assertIsNone(tdmcp_bridge._active_thread)  # noqa: SLF001

    def test_disconnect_on_idle_module_is_a_noop(self) -> None:
        tdmcp_bridge.disconnect()  # drain the fixture's connection first
        self.assertFalse(tdmcp_bridge.disconnect())

    def test_is_connected_true_while_worker_alive_false_after_disconnect(self) -> None:
        self.assertTrue(tdmcp_bridge.is_connected())
        tdmcp_bridge.disconnect()
        self.assertFalse(tdmcp_bridge.is_connected())


class IsConnectedIdleTest(unittest.TestCase):
    def test_is_connected_false_when_idle(self) -> None:
        tdmcp_bridge._active_stream = None  # noqa: SLF001
        tdmcp_bridge._active_thread = None  # noqa: SLF001
        self.assertFalse(tdmcp_bridge.is_connected())


class ResolveEndpointTest(unittest.TestCase):
    """T-8 endpoint resolution: TDMCP_IPC_ENDPOINT > TDMCP_IPC_PORT > default."""

    def test_default_loopback_port(self) -> None:
        with mock.patch.dict(os.environ):
            os.environ.pop("TDMCP_IPC_ENDPOINT", None)
            os.environ.pop("TDMCP_IPC_PORT", None)
            self.assertEqual(tdmcp_bridge.resolve_endpoint(), ("127.0.0.1", 9861))

    def test_port_env_overrides_port_only(self) -> None:
        with mock.patch.dict(os.environ):
            os.environ.pop("TDMCP_IPC_ENDPOINT", None)
            os.environ["TDMCP_IPC_PORT"] = "9000"
            self.assertEqual(tdmcp_bridge.resolve_endpoint(), ("127.0.0.1", 9000))

    def test_endpoint_env_beats_port_env(self) -> None:
        with mock.patch.dict(os.environ):
            os.environ["TDMCP_IPC_ENDPOINT"] = "10.1.2.3:7000"
            os.environ["TDMCP_IPC_PORT"] = "9000"
            self.assertEqual(tdmcp_bridge.resolve_endpoint(), ("10.1.2.3", 7000))

    def test_malformed_endpoint_env_raises_clear_error(self) -> None:
        with mock.patch.dict(os.environ, {"TDMCP_IPC_ENDPOINT": "no-port-here"}):
            with self.assertRaises(ValueError) as ctx:
                tdmcp_bridge.resolve_endpoint()
        self.assertIn("TDMCP_IPC_ENDPOINT", str(ctx.exception))

    def test_malformed_port_env_raises_clear_error(self) -> None:
        with mock.patch.dict(os.environ, {"TDMCP_IPC_PORT": "not-a-port"}):
            os.environ.pop("TDMCP_IPC_ENDPOINT", None)
            with self.assertRaises(ValueError) as ctx:
                tdmcp_bridge.resolve_endpoint()
        self.assertIn("TDMCP_IPC_PORT", str(ctx.exception))


class DialTcpSmokeTest(unittest.TestCase):
    """T-8: dial() connects over TCP and frames both directions (no daemon)."""

    def test_dial_connects_and_frames_over_tcp(self) -> None:
        server = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        server.bind(("127.0.0.1", 0))
        server.listen(1)
        self.addCleanup(server.close)
        port = server.getsockname()[1]

        accepted: list[socket.socket] = []

        def accept_one() -> None:
            conn, _ = server.accept()
            accepted.append(conn)

        accepter = threading.Thread(target=accept_one, daemon=True)
        accepter.start()

        stream = tdmcp_bridge.dial(f"127.0.0.1:{port}")
        self.addCleanup(stream.close)
        accepter.join(timeout=2.0)
        self.assertEqual(len(accepted), 1, "server must accept the dial")
        daemon_stream = accepted[0].makefile("rwb")
        self.addCleanup(daemon_stream.close)
        self.addCleanup(accepted[0].close)

        tdmcp_bridge._write_frame(  # noqa: SLF001
            stream,
            {"type": "request", "id": 1, "method": "ping", "params": {}},
        )
        req = tdmcp_bridge._read_frame(daemon_stream)  # noqa: SLF001
        self.assertEqual(req["method"], "ping")
        tdmcp_bridge._write_frame(  # noqa: SLF001
            daemon_stream,
            {"type": "response", "id": 1, "result": {"ok": True, "pong": True}},
        )
        resp = tdmcp_bridge._read_frame(stream)  # noqa: SLF001
        self.assertEqual(resp["id"], 1)
        self.assertEqual(resp["result"], {"ok": True, "pong": True})


class MidFrameTimeoutServeTest(unittest.TestCase):
    """serve_queued must disconnect on mid-frame stall, not silently continue."""

    def setUp(self) -> None:
        tdmcp_bridge._reset_pending_for_tests()  # noqa: SLF001

    def test_mid_frame_timeout_exits_cleanly(self) -> None:
        closed = {"n": 0}

        class BoomStream:
            def set_read_timeout(self, _seconds: float | None) -> None:
                return None

            def read(self, _n: int) -> bytes:
                raise tdmcp_bridge.MidFrameTimeout("partial body")

            def write(self, data: bytes) -> int:
                return len(data)

            def close(self) -> None:
                closed["n"] += 1

        # Must return (not hang) when the first read raises MidFrameTimeout.
        tdmcp_bridge.serve_queued(BoomStream(), idle_dead_s=30.0)
        self.assertEqual(closed["n"], 1, "serve_queued must close stream on exit")

    def test_clean_idle_timeout_continues_until_idle_dead(self) -> None:
        calls = {"n": 0}
        closed = {"n": 0}

        class IdleStream:
            def set_read_timeout(self, _seconds: float | None) -> None:
                return None

            def read(self, _n: int) -> bytes:
                calls["n"] += 1
                raise TimeoutError("read timed out")

            def write(self, data: bytes) -> int:
                return len(data)

            def close(self) -> None:
                closed["n"] += 1

        started = time.monotonic()
        tdmcp_bridge.serve_queued(IdleStream(), idle_dead_s=0.15)
        elapsed = time.monotonic() - started
        self.assertGreaterEqual(calls["n"], 2)
        self.assertLess(elapsed, 2.0)
        self.assertEqual(closed["n"], 1)

    def test_mid_frame_progress_retries_then_completes(self) -> None:
        """Short stalls mid-body must not disconnect while bytes keep arriving."""
        import json
        import struct

        body = json.dumps(
            {"type": "request", "id": 1, "method": "ping", "params": {}}
        ).encode("utf-8")
        frame = struct.pack("<I", len(body)) + body
        # Deliver header, then body one byte at a time with TimeoutError gaps.
        chunks: list[bytes | None] = [frame[:4]]
        for i, b in enumerate(frame[4:]):
            if i > 0:
                chunks.append(None)  # one poll stall (progress was recent)
            chunks.append(bytes([b]))
        state = {"i": 0, "writes": 0, "closed": 0}

        class ProgressStream:
            def set_read_timeout(self, _seconds: float | None) -> None:
                return None

            def read(self, n: int) -> bytes:
                if state["i"] >= len(chunks):
                    # After the ping response, idle-exit the serve loop.
                    raise TimeoutError("idle")
                item = chunks[state["i"]]
                state["i"] += 1
                if item is None:
                    raise TimeoutError("brief stall")
                return item[:n]

            def write(self, data: bytes) -> int:
                state["writes"] += 1
                return len(data)

            def flush(self) -> None:
                return None

            def close(self) -> None:
                state["closed"] += 1

        tdmcp_bridge.serve_queued(ProgressStream(), idle_dead_s=0.3)
        self.assertGreaterEqual(state["writes"], 1, "ping response should be written")
        self.assertEqual(state["closed"], 1)

    def test_mid_frame_zero_progress_exits_after_idle_dead(self) -> None:
        """Header then silence must raise MidFrameTimeout path and exit."""
        import struct

        state = {"phase": 0, "closed": 0}

        class StallStream:
            def set_read_timeout(self, _seconds: float | None) -> None:
                return None

            def read(self, n: int) -> bytes:
                if state["phase"] == 0:
                    state["phase"] = 1
                    return struct.pack("<I", 16)[:n]
                raise TimeoutError("no body progress")

            def write(self, data: bytes) -> int:
                return len(data)

            def close(self) -> None:
                state["closed"] += 1

        started = time.monotonic()
        tdmcp_bridge.serve_queued(StallStream(), idle_dead_s=0.2)
        elapsed = time.monotonic() - started
        self.assertGreaterEqual(elapsed, 0.15)
        self.assertLess(elapsed, 2.0)
        self.assertEqual(state["closed"], 1)

    def test_decode_exception_exits_cleanly(self) -> None:
        closed = {"n": 0}

        class GarbageStream:
            def __init__(self) -> None:
                self._phase = 0

            def set_read_timeout(self, _seconds: float | None) -> None:
                return None

            def read(self, n: int) -> bytes:
                import struct

                if self._phase == 0:
                    self._phase = 1
                    return struct.pack("<I", 4)
                # Valid length, garbage UTF-8 body → JSONDecodeError in _read_frame.
                return b"\xff\xff\xff\xff"[:n]

            def write(self, data: bytes) -> int:
                return len(data)

            def close(self) -> None:
                closed["n"] += 1

        tdmcp_bridge.serve_queued(GarbageStream(), idle_dead_s=30.0)
        self.assertEqual(closed["n"], 1)


class IdleDeadFromHandshakeTest(unittest.TestCase):
    """Handshake idleDeadSecs maps to serve_queued budget with safe fallbacks."""

    def test_present_positive(self) -> None:
        self.assertEqual(tdmcp_bridge.idle_dead_from_handshake({"idleDeadSecs": 30}), 30.0)
        self.assertEqual(tdmcp_bridge.idle_dead_from_handshake({"idleDeadSecs": 12.5}), 12.5)

    def test_missing_or_invalid_falls_back(self) -> None:
        self.assertEqual(tdmcp_bridge.idle_dead_from_handshake({}), tdmcp_bridge.IDLE_DEAD_S)
        self.assertEqual(tdmcp_bridge.idle_dead_from_handshake(None), tdmcp_bridge.IDLE_DEAD_S)
        self.assertEqual(
            tdmcp_bridge.idle_dead_from_handshake({"idleDeadSecs": 0}),
            tdmcp_bridge.IDLE_DEAD_S,
        )
        self.assertEqual(
            tdmcp_bridge.idle_dead_from_handshake({"idleDeadSecs": -1}),
            tdmcp_bridge.IDLE_DEAD_S,
        )
        self.assertEqual(
            tdmcp_bridge.idle_dead_from_handshake({"idleDeadSecs": "nope"}),
            tdmcp_bridge.IDLE_DEAD_S,
        )


class MaxCallWaitFromHandshakeTest(unittest.TestCase):
    def test_present_and_fallback(self) -> None:
        self.assertEqual(
            tdmcp_bridge.max_call_wait_from_handshake({"maxCallWaitSecs": 45}), 45.0
        )
        self.assertEqual(
            tdmcp_bridge.max_call_wait_from_handshake({}),
            tdmcp_bridge.DEFAULT_MAX_CALL_WAIT_S,
        )


class MainThreadWaitTimeoutTest(QueuedServeTest):
    """Worker unwedged when process_pending never runs (hung / paused timeline)."""

    def test_response_slot_timeout_writes_error_and_continues(self) -> None:
        worker = threading.Thread(
            target=tdmcp_bridge.serve_queued,
            args=(self.bridge_stream,),
            kwargs={"idle_dead_s": 5.0, "max_call_wait_s": 0.2},
            daemon=True,
        )
        worker.start()
        self._send_request(90, "execute_python", {"script": "result = 1"})
        resp = self._recv_response()
        self.assertEqual(resp["id"], 90)
        self.assertIn("error", resp)
        self.assertEqual(resp["error"]["code"], "tdmcp.bridge.main_thread_timeout")

        # Stream still usable for ping (worker continued).
        self._send_request(91, "ping")
        pong = self._recv_response()
        self.assertEqual(pong["id"], 91)
        self.assertEqual(pong["result"], {"ok": True, "pong": True})


class _FakeTd:
    """Stand-in for TouchDesigner's ``td`` module — records ``run()`` calls."""

    run_calls: list[tuple[object, int, object | None]] = []
    TDResources = object()

    class op:  # noqa: N801 — mirrors td.op
        TDResources = None  # set in setUp

    @staticmethod
    def run(script_or_callable, delayMilliSeconds: int = 0, delayRef=None, **_kwargs) -> None:
        _FakeTd.run_calls.append((script_or_callable, delayMilliSeconds, delayRef))


class _AliveThread:
    def is_alive(self) -> bool:
        return True


class PumpTest(unittest.TestCase):
    """Pause-resilient ``td.run`` main-thread pump (no live TD)."""

    def setUp(self) -> None:
        tdmcp_bridge._reset_pending_for_tests()  # noqa: SLF001
        _FakeTd.run_calls.clear()
        _FakeTd.op.TDResources = _FakeTd.TDResources
        self._prev_td = sys.modules.get("td")
        sys.modules["td"] = _FakeTd  # type: ignore[assignment]
        self._prev_thread = tdmcp_bridge._active_thread  # noqa: SLF001
        tdmcp_bridge._active_thread = _AliveThread()  # noqa: SLF001
        self.addCleanup(self._restore)

    def _restore(self) -> None:
        tdmcp_bridge._active_thread = self._prev_thread  # noqa: SLF001
        if self._prev_td is None:
            sys.modules.pop("td", None)
        else:
            sys.modules["td"] = self._prev_td
        tdmcp_bridge._reset_pending_for_tests()  # noqa: SLF001

    def test_pump_processes_and_reschedules(self) -> None:
        # Arrange
        slot: queue.Queue = queue.Queue()
        tdmcp_bridge._enqueue_pending(  # noqa: SLF001
            {"type": "request", "id": 1, "method": "ping", "params": {}},
            slot,
        )

        # Act
        tdmcp_bridge._pump()  # noqa: SLF001

        # Assert
        self.assertEqual(tdmcp_bridge.pending_count(), 0)
        self.assertEqual(len(_FakeTd.run_calls), 1)
        target, delay, delay_ref = _FakeTd.run_calls[0]
        self.assertIs(target, tdmcp_bridge._pump)
        self.assertEqual(delay, 50)
        self.assertIs(delay_ref, _FakeTd.TDResources)
        resp = slot.get_nowait()
        self.assertEqual(resp["id"], 1)
        self.assertEqual(resp["result"], {"ok": True, "pong": True})

    def test_pump_stops_when_disconnected(self) -> None:
        # Arrange
        tdmcp_bridge._active_thread = None  # noqa: SLF001
        tq = sys.modules["tdmcp_bridge.task_queue"]
        tq._set_pump_scheduled(True)  # noqa: SLF001

        # Act
        tdmcp_bridge._pump()  # noqa: SLF001

        # Assert
        self.assertFalse(tdmcp_bridge._pump_scheduled)  # noqa: SLF001
        self.assertEqual(_FakeTd.run_calls, [])

    def test_pump_survives_dispatch_exception(self) -> None:
        # Arrange — patch the module binding ``_pump`` actually calls
        tq = sys.modules["tdmcp_bridge.task_queue"]
        real_process = tq.process_pending

        def boom(**_kwargs):
            raise ValueError("dispatch boom")

        tq.process_pending = boom  # type: ignore[assignment]
        self.addCleanup(lambda: setattr(tq, "process_pending", real_process))

        # Act
        tdmcp_bridge._pump()  # noqa: SLF001

        # Assert — pump still rescheduled after exception
        self.assertEqual(len(_FakeTd.run_calls), 1)
        self.assertEqual(_FakeTd.run_calls[0][1], 50)

    def test_start_pump_idempotent(self) -> None:
        # Act
        tdmcp_bridge.start_pump()
        tdmcp_bridge.start_pump()

        # Assert — one initial schedule (delay 0), not two
        self.assertEqual(len(_FakeTd.run_calls), 1)
        self.assertEqual(_FakeTd.run_calls[0][1], 0)
        self.assertIs(_FakeTd.run_calls[0][0], tdmcp_bridge._pump)
        self.assertIs(_FakeTd.run_calls[0][2], _FakeTd.TDResources)
        self.assertTrue(tdmcp_bridge._pump_scheduled)  # noqa: SLF001

    def test_pump_rate_limits_burst(self) -> None:
        # Arrange — first call schedules; subsequent immediate calls must not
        tdmcp_bridge._pump()  # noqa: SLF001
        first_n = len(_FakeTd.run_calls)
        self.assertEqual(first_n, 1)

        # Act — rapid burst within the 50 ms window
        for _ in range(100):
            tdmcp_bridge._pump()  # noqa: SLF001

        # Assert
        self.assertEqual(len(_FakeTd.run_calls), 1)

    def test_start_pump_no_td_module(self) -> None:
        # Arrange
        sys.modules.pop("td", None)
        # Force ImportError on ``import td``
        import builtins

        real_import = builtins.__import__

        def block_td(name, *args, **kwargs):
            if name == "td" or name.startswith("td."):
                raise ImportError("no td")
            return real_import(name, *args, **kwargs)

        builtins.__import__ = block_td  # type: ignore[assignment]
        self.addCleanup(lambda: setattr(builtins, "__import__", real_import))

        # Act
        tdmcp_bridge.start_pump()

        # Assert
        self.assertFalse(tdmcp_bridge._pump_scheduled)  # noqa: SLF001
        self.assertEqual(_FakeTd.run_calls, [])


class WriteFrameTest(unittest.TestCase):
    """_write_frame must require a full write (guards Windows partial WriteFile)."""

    def test_short_write_raises(self) -> None:
        class ShortWriter:
            def write(self, data: bytes) -> int:
                return max(0, len(data) - 1)

            def flush(self) -> None:
                return None

        with self.assertRaises(OSError):
            tdmcp_bridge._write_frame(ShortWriter(), {"type": "response", "id": 1})

    def test_full_write_ok(self) -> None:
        class FullWriter:
            def __init__(self) -> None:
                self.buf = bytearray()

            def write(self, data: bytes) -> int:
                self.buf += data
                return len(data)

            def flush(self) -> None:
                return None

        w = FullWriter()
        tdmcp_bridge._write_frame(w, {"ok": True})
        self.assertGreater(len(w.buf), 4)


if __name__ == "__main__":
    unittest.main()
