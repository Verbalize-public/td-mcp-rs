"""Frame boundary regressions: malformed replies must not lose the session."""

import ast
import io
import json
from pathlib import Path
import re
import socket
import struct
import sys
import threading
import time
import unittest
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from tdmcp_bridge import transport
import tdmcp_bridge


class FrameLimitsTest(unittest.TestCase):
    def test_rust_and_python_limits_match(self):
        source = (Path(__file__).resolve().parents[2] / "crates/tdmcp-ipc/src/framing.rs").read_text()
        value = re.search(r"const MAX_FRAME: usize = ([0-9 *]+);", source)
        self.assertIsNotNone(value)
        # Only integer factors, never evaluate arbitrary Rust/Python source.
        factors = [ast.literal_eval(part.strip()) for part in value[1].split("*")]
        size = 1
        for factor in factors:
            size *= factor
        self.assertEqual(transport.MAX_FRAME_BYTES, size)

    def test_oversized_header_rejected_without_reading_body(self):
        stream = mock.Mock()
        stream.read.return_value = struct.pack("<I", transport.MAX_FRAME_BYTES + 1)
        with self.assertRaises(transport.FrameTooLarge):
            transport._read_frame(stream)
        stream.read.assert_called_once_with(4)

    def test_exact_wire_budget_is_accepted(self):
        msg = {"text": "€" * 40}
        size = len(json.dumps(msg, ensure_ascii=False).encode("utf-8"))
        with mock.patch.object(transport, "MAX_FRAME_BYTES", size):
            stream = io.BytesIO()
            transport._write_frame(stream, msg)
            self.assertEqual(len(stream.getvalue()), size + 4)
            stream.seek(0)
            self.assertEqual(transport._read_frame(stream), msg)
        with mock.patch.object(transport, "MAX_FRAME_BYTES", size - 1):
            stream = io.BytesIO()
            with self.assertRaises(transport.FrameTooLarge):
                transport._write_frame(stream, msg)
            self.assertEqual(stream.getvalue(), b"")

    def test_oversized_response_replaced_before_writing_and_next_frame_survives(self):
        # Individually small fields can still form an oversized aggregate.
        msg = {"type": "response", "id": "large", "result": ["€" * 100] * 4}
        with mock.patch.object(transport, "MAX_FRAME_BYTES", 1024):
            stream = io.BytesIO()
            transport._write_frame(stream, msg)
            next_msg = {"type": "response", "id": "next", "result": {"pong": True}}
            transport._write_frame(stream, next_msg)
            stream.seek(0)
            result = transport._read_frame(stream)
            self.assertEqual(result["id"], "large")
            self.assertEqual(result["error"]["code"], "tdmcp.bridge.response_too_large")
            self.assertIn("before retrying any mutation", result["error"]["message"])
            self.assertNotIn("result", result)
            self.assertEqual(transport._read_frame(stream), next_msg)

    def test_non_json_responses_replaced_and_next_frame_survives(self):
        recursive = []
        recursive.append(recursive)
        for result in [float("nan"), float("inf"), -float("inf"), object(), recursive, "\ud800", 1 << 64, -(1 << 63) - 1]:
            with self.subTest(value_type=type(result).__name__):
                stream = io.BytesIO()
                transport._write_frame(stream, {"type": "response", "id": "bad", "result": result})
                transport._write_frame(stream, {"type": "response", "id": "next", "result": 42})
                stream.seek(0)
                response = transport._read_frame(stream)
                self.assertEqual(response["id"], "bad")
                self.assertEqual(response["error"]["code"], "tdmcp.bridge.response_invalid")
                self.assertEqual(transport._read_frame(stream)["result"], 42)

    def test_supported_integer_boundaries_remain_exact(self):
        values = [-(1 << 63), (1 << 64) - 1]
        stream = io.BytesIO()
        transport._write_frame(stream, {"type": "response", "id": "ints", "result": values})
        stream.seek(0)
        self.assertEqual(transport._read_frame(stream)["result"], values)

    def test_nesting_budget_includes_response_envelope(self):
        value = 1
        for _ in range(transport.MAX_JSON_DEPTH - 1):
            value = [value]
        msg = {"type": "response", "id": "depth", "result": value}
        stream = io.BytesIO()
        transport._write_frame(stream, msg)
        stream.seek(0)
        self.assertEqual(transport._read_frame(stream), msg)
        msg["result"] = [value]
        stream = io.BytesIO()
        transport._write_frame(stream, msg)
        stream.seek(0)
        self.assertEqual(transport._read_frame(stream)["error"]["code"], "tdmcp.bridge.response_invalid")

    def test_invalid_non_response_is_not_recast_as_a_response(self):
        stream = io.BytesIO()
        with self.assertRaises(ValueError):
            transport._write_frame(stream, {"type": "event", "payload": float("nan")})
        self.assertEqual(stream.getvalue(), b"")

    def test_queued_handler_failure_keeps_worker_alive(self):
        tdmcp_bridge._reset_pending_for_tests()
        bridge_sock, daemon_sock = socket.socketpair()
        bridge = transport._TcpStream(bridge_sock)
        daemon = transport._TcpStream(daemon_sock)
        daemon.set_read_timeout(2)
        worker = threading.Thread(target=tdmcp_bridge.serve_queued, args=(bridge,), daemon=True)
        try:
            with mock.patch.dict(tdmcp_bridge.HANDLERS, {"inspect": lambda _: {"bad": float("nan")}}):
                worker.start()
                transport._write_frame(daemon, {"type": "request", "id": "bad", "method": "inspect"})
                deadline = time.monotonic() + 2
                while not tdmcp_bridge.pending_count() and time.monotonic() < deadline:
                    time.sleep(0.01)
                self.assertEqual(tdmcp_bridge.process_pending(), 1)
                response = transport._read_frame(daemon)
                self.assertEqual(response["error"]["code"], "tdmcp.bridge.response_invalid")
                transport._write_frame(daemon, {"type": "request", "id": "ping", "method": "ping"})
                self.assertEqual(transport._read_frame(daemon)["result"], {"ok": True, "pong": True})
                self.assertTrue(worker.is_alive())
        finally:
            daemon.cancel_pending_io(None)
            daemon.close()
            worker.join(timeout=2)
            bridge.close()
            tdmcp_bridge._reset_pending_for_tests()
        self.assertFalse(worker.is_alive())

    def test_non_object_and_non_json_envelopes_rejected(self):
        for body in [b"null", b"[]", b"0", b'{"value": NaN}', b'{"value": Infinity}']:
            with self.subTest(body=body):
                with self.assertRaises(ValueError):
                    transport._read_frame(io.BytesIO(struct.pack("<I", len(body)) + body))


if __name__ == "__main__":
    unittest.main()
