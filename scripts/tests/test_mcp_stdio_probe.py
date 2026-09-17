import io
import json
from pathlib import Path
import queue
import sys
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from mcp_stdio_probe import StdioMcpClient


class Process:
    def __init__(self, response):
        self.stdin = io.BytesIO()
        self.stdout = io.BytesIO((json.dumps({"id":1,"result":{"protocolVersion":"2025-06-18"}}) + "\n" + json.dumps({"id":3,"result":response}) + "\n").encode())
    def wait(self, timeout=None):
        return 0


class StdioProbeTests(unittest.TestCase):
    def test_exact_target_and_initialize_notification_framing(self):
        process = Process({"isError":False,"structuredContent":{"ok":True}})
        with patch("mcp_stdio_probe.subprocess.Popen", return_value=process):
            client = StdioMcpClient("owned-daemon")
            try:
                self.assertEqual(client.call("capture", {"pid":42,"path":"/project1/out"}), {"ok":True})
                frames = [json.loads(x) for x in process.stdin.getvalue().splitlines()]
                self.assertEqual(frames[0]["method"], "initialize")
                self.assertNotIn("id", frames[1])
                self.assertEqual(frames[2]["params"]["arguments"]["pid"], 42)
                self.assertEqual(len(frames), 3)
            finally:
                client.close()

    def test_raw_tool_error_is_retained_for_failure_probes(self):
        response = {"isError":True,"structuredContent":{"ok":False,"items":[{"code":"tdmcp.args.missing_field"}]}}
        with patch("mcp_stdio_probe.subprocess.Popen", return_value=Process(response)):
            with StdioMcpClient("owned-daemon") as client:
                self.assertEqual(client.call_raw("capture", {}), response)

    def test_timeout_does_not_replay_mutation(self):
        client = object.__new__(StdioMcpClient)
        client.sequence = 0
        client.timeout = 0
        client.messages = queue.Queue()
        client.process = Process({})
        with self.assertRaises(TimeoutError):
            client.request("tools/call", {"name":"mutate_nodes","arguments":{"pid":42}})
        self.assertEqual(len(client.process.stdin.getvalue().splitlines()), 1)
