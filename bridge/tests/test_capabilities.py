"""Loaded bridge identity/capability and pump-liveness contract tests."""
from __future__ import annotations

import os
import socket
import sys
import threading
import unittest
import uuid
from unittest import mock

sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))

import tdmcp_bridge  # noqa: E402
from tdmcp_bridge import capabilities, state  # noqa: E402


class CapabilityPayloadTest(unittest.TestCase):
    def test_ready_identity_is_cached_and_separates_loaded_from_source(self) -> None:
        first = capabilities.ready_payload()
        second = capabilities.ready_payload()
        self.assertEqual(first["loadGeneration"], second["loadGeneration"])
        self.assertEqual(str(uuid.UUID(first["loadGeneration"])), first["loadGeneration"])
        self.assertEqual(first["protocol"]["version"], tdmcp_bridge.__protocol_version__)
        self.assertEqual(first["loadedCode"]["kind"], "pythonCodeObjects")
        self.assertEqual(first["sourceSnapshot"]["kind"], "sourceSnapshot")
        self.assertEqual(first["bundleSnapshot"]["kind"], "bridgePackageSnapshot")
        self.assertEqual(
            first["sourceSnapshot"]["provenance"],
            "onDiskAtReadyAfterFinalReload",
        )
        self.assertNotEqual(
            first["loadedCode"]["algorithm"], first["sourceSnapshot"]["algorithm"]
        )
        self.assertTrue(first["loadedCode"]["digest"])
        self.assertTrue(first["sourceSnapshot"]["digest"])
        self.assertTrue(first["bundleSnapshot"]["digest"])
        self.assertIn("manifest.json", first["bundleSnapshot"]["files"])

    def test_features_are_only_current_handler_routes(self) -> None:
        self.assertEqual(set(capabilities.SUPPORTED_FEATURES), set(tdmcp_bridge.HANDLERS))
        for future in ("evidence_read", "sample_data", "guarded_writes"):
            self.assertNotIn(future, capabilities.SUPPORTED_FEATURES)
        self.assertTrue(capabilities.supports_required_features(["inspect", "capture"]))
        self.assertFalse(capabilities.supports_required_features(["evidence_read"]))
        with self.assertRaisesRegex(ValueError, "evidence_read"):
            capabilities.require_supported_features(["inspect", "evidence_read"])

    def test_worker_heartbeat_reads_but_never_updates_pump_cache(self) -> None:
        tdmcp_bridge._last_main_thread_pump_monotonic = 10.0  # noqa: SLF001
        with mock.patch.object(state.time, "monotonic", return_value=12.5):
            before = tdmcp_bridge._last_main_thread_pump_monotonic  # noqa: SLF001
            heartbeat = capabilities.heartbeat_payload()
            after = tdmcp_bridge._last_main_thread_pump_monotonic  # noqa: SLF001
        self.assertEqual(heartbeat["bridge"]["mainThreadPumpAgeMs"], 2500)
        self.assertEqual(before, after)

    def test_only_main_thread_pump_marks_responsiveness(self) -> None:
        tdmcp_bridge._last_main_thread_pump_monotonic = None  # noqa: SLF001
        self.assertIsNone(capabilities.heartbeat_payload()["bridge"]["mainThreadPumpAgeMs"])
        tdmcp_bridge.process_pending(max_items=0)
        self.assertIsNotNone(capabilities.heartbeat_payload()["bridge"]["mainThreadPumpAgeMs"])


class ReadyTransportTest(unittest.TestCase):
    def setUp(self) -> None:
        tdmcp_bridge._reset_pending_for_tests()  # noqa: SLF001
        bridge_sock, daemon_sock = socket.socketpair()
        self.bridge_sock = bridge_sock
        self.daemon_sock = daemon_sock
        self.bridge_stream = bridge_sock.makefile("rwb")
        self.daemon_stream = daemon_sock.makefile("rwb")
        self.addCleanup(self.bridge_stream.close)
        self.addCleanup(self.daemon_stream.close)
        self.addCleanup(bridge_sock.close)
        self.addCleanup(daemon_sock.close)

    def _send_ping(self, req_id: int) -> None:
        tdmcp_bridge._write_frame(
            self.daemon_stream,
            {"type": "request", "id": req_id, "method": "ping", "params": {}},
        )

    def test_old_style_serve_has_no_mandatory_ready_frame(self) -> None:
        worker = threading.Thread(
            target=tdmcp_bridge.serve_queued,
            args=(self.bridge_stream,),
            daemon=True,
        )
        worker.start()
        self._send_ping(1)
        first = tdmcp_bridge._read_frame(self.daemon_stream)
        self.assertEqual(first["type"], "response")
        self.assertEqual(first["result"], {"ok": True, "pong": True})

    def test_final_reload_caller_can_emit_ready_before_requests(self) -> None:
        ready = capabilities.ready_event()
        worker = threading.Thread(
            target=tdmcp_bridge.serve_queued,
            args=(self.bridge_stream,),
            kwargs={"initial_event": ready},
            daemon=True,
        )
        worker.start()
        first = tdmcp_bridge._read_frame(self.daemon_stream)
        self.assertEqual(first["type"], "event")
        self.assertEqual(first["name"], "bridge_ready")
        self.assertEqual(first["payload"]["loadGeneration"], ready["payload"]["loadGeneration"])
        self._send_ping(2)
        response = tdmcp_bridge._read_frame(self.daemon_stream)
        self.assertEqual(response["id"], 2)


if __name__ == "__main__":
    unittest.main()
