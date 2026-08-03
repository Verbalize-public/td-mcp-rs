"""Regression coverage for the worker-thread/main-thread queue split.

`tdmcp_bridge.serve_queued` must never call `dispatch()` itself (that would
run handler code — which touches `td.*` in real TD — off the main thread).
This test drives the real framing + queue + pump code over a `socketpair()`,
simulating the daemon on one end and TD's Execute DAT `onFrameStart` pump on
the "main thread" (here: the test thread, after joining the worker's enqueue).

Run: `python -m unittest bridge/tests/test_bridge_queue.py` (no TD, no deps).
"""

from __future__ import annotations

import os
import socket
import sys
import threading
import time
import unittest

sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))

import tdmcp_bridge  # noqa: E402


class QueuedServeTest(unittest.TestCase):
    def setUp(self) -> None:
        # Fresh module-level queue per test — tests run in one process.
        tdmcp_bridge._reset_pending_for_tests()  # noqa: SLF001
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


class IdleDeadTest(unittest.TestCase):
    """serve_queued exits after inbound silence when the stream supports timeouts."""

    def setUp(self) -> None:
        tdmcp_bridge._reset_pending_for_tests()  # noqa: SLF001
        bridge_sock, daemon_sock = socket.socketpair()
        self.daemon_sock = daemon_sock
        self.addCleanup(bridge_sock.close)
        self.addCleanup(daemon_sock.close)
        self.stream = tdmcp_bridge._UdsStream(bridge_sock)  # noqa: SLF001

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
    "primitive. See WindowsPipeDisconnectTest for the Windows equivalent.",
)
class DisconnectTest(unittest.TestCase):
    """Regression for the close-while-blocked-read freeze (POSIX/UDS path).

    `disconnect()` must not block: the worker thread is parked in a blocking
    `read()` with nothing to read (no pending job), and `disconnect()` has to
    unstick it (`cancel_pending_io` -> POSIX `shutdown`) rather than closing
    the handle out from under it.
    """

    def setUp(self) -> None:
        tdmcp_bridge._reset_pending_for_tests()  # noqa: SLF001
        bridge_sock, daemon_sock = socket.socketpair()
        self.daemon_sock = daemon_sock
        self.addCleanup(daemon_sock.close)

        stream = tdmcp_bridge._UdsStream(bridge_sock)  # noqa: SLF001
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


@unittest.skipUnless(sys.platform.startswith("win"), "named-pipe path is Windows-only")
class WindowsPipeDisconnectTest(unittest.TestCase):
    """Regression for the CloseHandle-while-blocked-ReadFile freeze (named-pipe path).

    This is the exact bug hit live against TouchDesigner: `disconnect()`
    calling `CloseHandle` while the worker thread had a pending synchronous
    `ReadFile` on the same handle froze the *calling* thread indefinitely —
    which froze TD itself, since the call originated from a script running on
    TD's main thread. Verified manually that `CancelSynchronousIo` targeting
    the worker's OS thread id aborts the pending `ReadFile` with
    `ERROR_OPERATION_ABORTED` immediately; this test locks that in.
    """

    PIPE_NAME = r"\\.\pipe\tdmcp-test-disconnect"

    def setUp(self) -> None:
        import ctypes

        self.kernel32 = ctypes.windll.kernel32  # type: ignore[attr-defined]
        pipe_access_duplex = 0x3
        server = self.kernel32.CreateNamedPipeW(
            self.PIPE_NAME, pipe_access_duplex, 0, 1, 65536, 65536, 0, None
        )
        client_handle = self.kernel32.CreateFileW(
            self.PIPE_NAME, 0x80000000 | 0x40000000, 0, None, 3, 0, None
        )
        self.kernel32.ConnectNamedPipe(server, None)
        self.addCleanup(lambda: self.kernel32.CloseHandle(server))

        tdmcp_bridge._reset_pending_for_tests()  # noqa: SLF001
        stream = tdmcp_bridge._NamedPipeStream(client_handle)  # noqa: SLF001
        thread = threading.Thread(target=tdmcp_bridge.serve_queued, args=(stream,), daemon=True)
        thread.start()
        time.sleep(0.1)  # let the worker enter its blocking ReadFile
        tdmcp_bridge._active_stream = stream  # noqa: SLF001
        tdmcp_bridge._active_thread = thread  # noqa: SLF001

    def test_disconnect_does_not_hang_and_joins_worker(self) -> None:
        thread = tdmcp_bridge._active_thread  # noqa: SLF001
        self.assertTrue(tdmcp_bridge.is_connected())

        started = time.monotonic()
        ok = tdmcp_bridge.disconnect()
        elapsed = time.monotonic() - started

        self.assertTrue(ok)
        self.assertLess(elapsed, 2.0, "disconnect() should not block on a pending ReadFile")
        self.assertFalse(thread.is_alive(), "worker thread must exit after disconnect()")
        self.assertFalse(tdmcp_bridge.is_connected())


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
                raise TimeoutError("named pipe read timed out")

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


@unittest.skipUnless(sys.platform.startswith("win"), "named-pipe WriteFile loop is Windows-only")
class NamedPipeWriteLoopTest(unittest.TestCase):
    """_NamedPipeStream.write must loop until all bytes are sent."""

    def test_write_loops_on_partial_writefile(self) -> None:
        from ctypes import wintypes

        stream = tdmcp_bridge._NamedPipeStream.__new__(tdmcp_bridge._NamedPipeStream)  # noqa: SLF001
        stream._handle = 1  # noqa: SLF001
        stream._wintypes = wintypes  # noqa: SLF001
        calls = {"n": 0}

        class FakeKernel:
            def WriteFile(self, _h, _data, length, written_ref, _ov):
                calls["n"] += 1
                take = min(3, int(length))
                written_ref._obj.value = take
                return 1

        stream._kernel32 = FakeKernel()  # noqa: SLF001
        payload = b"abcdefghij"  # 10 bytes → at least 4 WriteFile calls
        n = stream.write(payload)
        self.assertEqual(n, len(payload))
        self.assertGreaterEqual(calls["n"], 4)


if __name__ == "__main__":
    unittest.main()
