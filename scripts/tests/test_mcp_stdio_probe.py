import io
import json
from pathlib import Path
import queue
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from mcp_stdio_probe import StdioMcpClient, main, measure


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


class MeasureTests(unittest.TestCase):
    def test_counts_structured_text_and_image_bytes_not_tokens(self):
        result = {
            "isError": False,
            "structuredContent": {"ok": True, "items": ["a", "b"]},
            "content": [
                {"type": "text", "text": "{\"ok\":true}"},
                {"type": "text", "text": "héllo"},
                {"type": "image", "data": "AAEC", "mimeType": "image/png"},
            ],
        }
        m = measure(result)
        self.assertEqual(m["image_count"], 1)
        self.assertEqual(m["encoded_image_bytes"], 4)
        self.assertEqual(m["text_bytes"], 11 + 6)
        self.assertEqual(m["structured_bytes"],
                         len(json.dumps({"ok": True, "items": ["a", "b"]},
                                        sort_keys=True, separators=(",", ":"),
                                        ensure_ascii=False).encode("utf-8")))
        self.assertEqual(m["canonical_bytes"],
                         len(json.dumps(result, sort_keys=True, separators=(",", ":"),
                                        ensure_ascii=False).encode("utf-8")))

    def test_resources_count_only_image_blobs_and_include_embedded_text(self):
        result = {"contents": [
            {"uri": "tdmcp://x", "text": "é"},
            {"uri": "tdmcp://y", "blob": "AA==", "mimeType": "image/png"},
            {"uri": "tdmcp://z", "blob": "AAAA", "mimeType": "application/pdf"},
        ], "content": [
            {"type": "resource", "resource": {"uri": "tdmcp://a", "text": "𐐀"}},
            {"type": "resource", "resource": {
                "uri": "tdmcp://b", "blob": "AQ==", "mimeType": "image/jpeg"}},
        ]}
        m = measure(result)
        self.assertEqual(m["text_bytes"], 6)
        self.assertEqual(m["image_count"], 2)
        self.assertEqual(m["encoded_image_bytes"], 8)
        self.assertEqual(m["structured_bytes"], 0)

    def test_empty_and_catalog_results(self):
        self.assertEqual(measure({}), {
            "canonical_bytes": 2, "structured_bytes": 0, "text_bytes": 0,
            "image_count": 0, "encoded_image_bytes": 0,
        })
        result = {"tools": [{"name": "fleet", "description": "é"}], "nextCursor": "next"}
        m = measure(result)
        self.assertEqual(m["text_bytes"], 0)
        self.assertGreater(m["canonical_bytes"], 2)
        self.assertEqual(m, measure(dict(reversed(list(result.items())))))

    def test_canonical_unicode_escaping_and_explicit_null(self):
        self.assertEqual(measure({"structuredContent": {"é": "𐐀\n"}}), {
            "canonical_bytes": len('{"structuredContent":{"é":"𐐀\\n"}}'.encode("utf-8")),
            "structured_bytes": len('{"é":"𐐀\\n"}'.encode("utf-8")),
            "text_bytes": 0, "image_count": 0, "encoded_image_bytes": 0,
        })
        self.assertEqual(measure({"structuredContent": None})["structured_bytes"], 4)


class MainDispatchTests(unittest.TestCase):
    def run_main(self, argv, response):
        with patch("sys.argv", ["mcp_stdio_probe.py", "--binary", "owned-daemon"] + argv), \
                patch("mcp_stdio_probe.StdioMcpClient") as factory, \
                patch("sys.stdout", new_callable=io.StringIO) as output:
            client = factory.return_value.__enter__.return_value
            client.call_raw.return_value = response
            client.request.return_value = response
            main()
            factory.assert_called_once_with("owned-daemon")
            return client, output.getvalue()

    def test_list_tools_requests_tools_list(self):
        response = {"tools": [{"name": "fleet"}, {"name": "capture"}]}
        for flags in ([], ["--measure"]):
            with self.subTest(flags=flags):
                client, output = self.run_main(["--list-tools"] + flags, response)
                client.request.assert_called_once_with("tools/list", {})
                client.call_raw.assert_not_called()
                self.assertEqual(json.loads(output), measure(response) if flags else response)

    def test_measure_prints_only_metrics_json_including_tool_errors(self):
        for is_error in (False, True):
            with self.subTest(is_error=is_error):
                response = {"isError": is_error, "structuredContent": {"ok": not is_error},
                            "content": [{"type": "text", "text": "{}"}]}
                client, output = self.run_main(["capture", '{"pid":42}', "--measure"], response)
                client.call_raw.assert_called_once_with("capture", {"pid": 42})
                client.request.assert_not_called()
                self.assertEqual(json.loads(output), measure(response))
                self.assertEqual(len(output.splitlines()), 1)

    def test_default_tool_and_resource_output_are_preserved(self):
        response = {"structuredContent": {"ok": True}, "content": []}
        client, output = self.run_main([], response)
        client.call_raw.assert_called_once_with("fleet", {})
        self.assertEqual(output, json.dumps(response, indent=2) + "\n")
        for flags in ([], ["--measure"]):
            with self.subTest(flags=flags):
                client, output = self.run_main(["--resource", "tdmcp://docs/test"] + flags, response)
                client.request.assert_called_once_with("resources/read", {"uri": "tdmcp://docs/test"})
                client.call_raw.assert_not_called()
                self.assertEqual(json.loads(output), measure(response) if flags else response)

    def test_out_saves_full_evidence_before_measurement_and_preserves_legacy_path(self):
        response = {"content": [{"type": "image", "data": "AA==", "mimeType": "image/png"}]}
        for flags in ([], ["--measure"]):
            with self.subTest(flags=flags), tempfile.TemporaryDirectory() as directory:
                out = Path(directory) / "nested" / "evidence.json"

                def measure_saved(result):
                    self.assertEqual(out.read_text(encoding="utf-8"), json.dumps(response, indent=2) + "\n")
                    return measure(result)

                with patch("mcp_stdio_probe.measure", side_effect=measure_saved) as measurement:
                    _, output = self.run_main(["--out", str(out)] + flags, response)
                self.assertEqual(json.loads(out.read_text(encoding="utf-8")), response)
                if flags:
                    measurement.assert_called_once_with(response)
                    self.assertEqual(json.loads(output), measure(response))
                else:
                    measurement.assert_not_called()
                    self.assertEqual(output, str(out) + "\n")

    def test_existing_output_is_rejected_before_client_creation(self):
        with tempfile.TemporaryDirectory() as directory:
            out = Path(directory) / "evidence.json"
            out.write_text("prior evidence", encoding="utf-8")
            with patch("sys.argv", ["probe", "--measure", "--list-tools", "--out", str(out)]), \
                    patch("mcp_stdio_probe.StdioMcpClient") as factory, \
                    patch("sys.stderr", new_callable=io.StringIO), \
                    self.assertRaises(SystemExit) as error:
                main()
            self.assertEqual(error.exception.code, 2)
            factory.assert_not_called()
            self.assertEqual(out.read_text(encoding="utf-8"), "prior evidence")

    def test_resource_and_list_tools_are_mutually_exclusive(self):
        with patch("sys.argv", ["probe", "--resource", "tdmcp://x", "--list-tools"]), \
                patch("mcp_stdio_probe.StdioMcpClient") as factory, \
                patch("sys.stderr", new_callable=io.StringIO), \
                self.assertRaises(SystemExit) as error:
            main()
        self.assertEqual(error.exception.code, 2)
        factory.assert_not_called()

    def test_failed_evidence_save_does_not_print_measurement(self):
        with patch("mcp_stdio_probe.Path.exists", return_value=False), \
                patch("mcp_stdio_probe.Path.mkdir"), \
                patch("mcp_stdio_probe.Path.open", side_effect=FileExistsError), \
                patch("mcp_stdio_probe.measure") as measurement, \
                patch("builtins.print") as printing, \
                self.assertRaises(FileExistsError):
            self.run_main(["--out", "evidence.json", "--measure"], {})
        measurement.assert_not_called()
        printing.assert_not_called()
