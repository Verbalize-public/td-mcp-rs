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
        tdmcp_bridge._pending_main = tdmcp_bridge.queue.Queue()  # noqa: SLF001
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

    def test_worker_never_dispatches_directly(self) -> None:
        """A request sitting unprocessed must land in the queue, not a response."""
        worker = threading.Thread(
            target=tdmcp_bridge.serve_queued, args=(self.bridge_stream,), daemon=True
        )
        worker.start()
        self._send_request(1, "ping")

        # Give the worker time to read + enqueue; it must NOT answer on its own.
        deadline = time.monotonic() + 1.0
        while tdmcp_bridge._pending_main.qsize() == 0 and time.monotonic() < deadline:
            time.sleep(0.01)
        self.assertEqual(tdmcp_bridge._pending_main.qsize(), 1)

        # Now simulate the main-thread pump (Execute DAT onFrameStart).
        drained = tdmcp_bridge.process_pending()
        self.assertEqual(drained, 1)

        resp = self._recv_response()
        self.assertEqual(resp["id"], 1)
        self.assertEqual(resp["result"], {"ok": True, "pong": True})

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
            tdmcp_bridge._pending_main.put(  # noqa: SLF001
                ({"type": "request", "id": i, "method": "ping", "params": {}}, slot)
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

        deadline = time.monotonic() + 1.0
        while tdmcp_bridge._pending_main.qsize() == 0 and time.monotonic() < deadline:
            time.sleep(0.01)

        tdmcp_bridge.process_pending()
        resp = self._recv_response()
        self.assertEqual(resp["id"], 7)
        self.assertIn("unknown method", resp["error"]["message"])


if __name__ == "__main__":
    unittest.main()
