#!/usr/bin/env python3
"""Verify response guards through real MCP and TD; remove the temporary COMP.

Use a scratch TD process: this creates a COMP beside the supplied output and
puts 33 MiB in a custom string parameter to exceed the aggregate reply budget.
Usage: python scripts/live_bridge_limits_smoke.py PID /project1/output
"""

import json
import secrets
import sys

from mcp_probe import McpClient


def smoke(pid, path):
    client = McpClient()
    fixture = path.rsplit("/", 1)[0] + "/tdmcp_limits_" + secrets.token_hex(6)
    created = False
    try:
        client.call("inspect", {"pid": pid, "paths": [path]})
        for script in [
            "result = float('nan')",
            "result = float('inf')",
            "result = 1 << 64",
            "result = '\\ud800'",
            "result = 0\nfor _ in range(64): result = [result]",
        ]:
            reply = client.request("tools/call", {"name": "execute_python", "arguments": {"pid": pid, "script": script}})
            assert reply.get("isError") and "tdmcp.bridge.response_invalid" in json.dumps(reply), reply
            assert client.call("execute_python", {"pid": pid, "script": "result = 42"})["result"] == 42
        client.call("mutate_nodes", {"pid": pid, "steps": [{
            "op": "create", "path": fixture, "opType": "baseCOMP",
            "comment": "Temporary bridge response-size fixture; removed by live smoke",
        }]})
        created = True
        script = f"node = op({fixture!r})\nnode.appendCustomPage('Audit').appendStr('Payload')[0].val = 'x' * (33 * 1024 * 1024)\nresult = len(node.par.Payload.eval())"
        assert client.call("execute_python", {"pid": pid, "script": script})["result"] == 33 * 1024 * 1024
        reply = client.request("tools/call", {"name": "inspect", "arguments": {"pid": pid, "paths": [fixture], "include": ["params"]}})
        assert reply.get("isError") and "tdmcp.bridge.response_too_large" in json.dumps(reply), reply
        result = client.call("execute_python", {"pid": pid, "script": f"result = len(op({fixture!r}).par.Payload.eval())"})
        assert result["result"] == 33 * 1024 * 1024, "Response guard changed TD data"
        client.call("inspect", {"pid": pid, "paths": [path]})
    finally:
        try:
            if created:
                client.call("mutate_nodes", {"pid": pid, "steps": [{"op": "delete", "path": fixture}]})
        finally:
            client.close()
    print("Live bridge limits passed: NaN/infinity, large integer, invalid Unicode, nesting, 33 MiB parameter reply; subsequent calls survived and fixture removed")


if __name__ == "__main__":
    smoke(int(sys.argv[1]), sys.argv[2])
