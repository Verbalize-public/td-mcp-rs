#!/usr/bin/env python3
"""Call the real MCP transport with Python's standard library.

python3 scripts/mcp_probe.py fleet
python3 scripts/mcp_probe.py inspect '{"pid":123,"paths":["/project1"]}'
python3 scripts/mcp_probe.py --resource tdmcp://docs/python-api
Uses TDMCP_DAEMON_URL (HTTP origin) and optional TDMCP_PSK.
"""

import argparse
import json
import os
import urllib.error
import urllib.request


class McpClient:
    def __init__(self, base=None, token=None):
        self.url = (base or os.environ.get("TDMCP_DAEMON_URL", "http://127.0.0.1:9860")).rstrip("/") + "/mcp/rpc"
        self.headers = {"Content-Type": "application/json", "Accept": "application/json, text/event-stream"}
        token = token if token is not None else os.environ.get("TDMCP_PSK")
        if token:
            self.headers["Authorization"] = f"Bearer {token}"
        self.sequence = 0
        initialized = self.request("initialize", {
            "protocolVersion": "2025-06-18", "capabilities": {},
            "clientInfo": {"name": "tdmcp-live-probe", "version": "1"},
        })
        self.headers["Mcp-Protocol-Version"] = initialized["protocolVersion"]
        self.request("notifications/initialized", {}, notification=True)

    def request(self, method, params, notification=False):
        self.sequence += 1
        body = {"jsonrpc": "2.0", "method": method, "params": params}
        if not notification:
            body["id"] = self.sequence
        request = urllib.request.Request(self.url, json.dumps(body).encode(), self.headers)
        with urllib.request.urlopen(request, timeout=90) as response:
            session = response.headers.get("Mcp-Session-Id")
            if session:
                self.headers["Mcp-Session-Id"] = session
            if notification:
                return None
            if response.headers.get_content_type() == "text/event-stream":
                data = []
                for line in response:
                    if line.startswith(b"data:"):
                        data.append(line[5:].strip())
                    elif not line.strip():
                        payload = b"\n".join(data).strip()
                        data.clear()
                        if payload:
                            result = json.loads(payload)
                            if result.get("id") == self.sequence:
                                break
                else:
                    raise RuntimeError("MCP stream ended without a response")
            else:
                result = json.load(response)
        if "error" in result:
            raise RuntimeError(result["error"])
        return result["result"]

    def call(self, name, arguments):
        result = self.request("tools/call", {"name": name, "arguments": arguments})
        if result.get("isError"):
            raise RuntimeError(result)
        if "structuredContent" in result:
            return result["structuredContent"]
        for block in result.get("content", []):
            if block.get("type") == "text":
                try:
                    return json.loads(block["text"])
                except json.JSONDecodeError:
                    pass
        return result

    def close(self):
        try:
            request = urllib.request.Request(self.url, headers=self.headers, method="DELETE")
            with urllib.request.urlopen(request, timeout=3):
                pass
        except urllib.error.URLError:
            pass


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("tool", nargs="?", default="fleet")
    parser.add_argument("arguments", nargs="?", default="{}")
    parser.add_argument("--resource")
    args = parser.parse_args()
    client = McpClient()
    try:
        result = client.request("resources/read", {"uri": args.resource}) if args.resource else client.call(args.tool, json.loads(args.arguments))
        print(json.dumps(result, indent=2))
    finally:
        client.close()


if __name__ == "__main__":
    main()
