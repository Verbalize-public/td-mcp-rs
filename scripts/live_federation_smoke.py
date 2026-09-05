#!/usr/bin/env python3
"""Exercise federation against an existing bridged TD PID; restore its daemon role.

Run only against a development daemon: this temporarily changes timeouts,
creates and removes a DAT beside the output, and joins an isolated coordinator
after testing a deliberately rejected coordinator key.
Usage: python scripts/live_federation_smoke.py PID /project1/output
"""

import json
import os
from pathlib import Path
import secrets
import subprocess
import sys
import tempfile
import time
import urllib.request

from mcp_probe import McpClient
from package_smoke import free_port


def admin(base, path, body=None):
    request = urllib.request.Request(base + path, headers={"Content-Type": "application/json"})
    if body is not None:
        request.data = json.dumps(body).encode()
    with urllib.request.urlopen(request, timeout=3) as response:
        return json.load(response)


def eventually(check, timeout=20):
    deadline = time.monotonic() + timeout
    while True:
        try:
            result = check()
            if result:
                return result
        except OSError:
            pass
        if time.monotonic() >= deadline:
            raise AssertionError("Timed out waiting for federation state")
        time.sleep(0.1)


def smoke(pid, path):
    base = os.environ.get("TDMCP_DAEMON_URL", "http://127.0.0.1:9860")
    before = admin(base, "/admin/config")
    status = admin(base, "/admin/status")
    assert before["federation"]["role"] == "standalone", "Use a standalone test daemon, not an existing fleet"
    local = McpClient(base)
    try:
        eventually(lambda: any(
            row["pid"] == pid and row.get("bridge") == "connected"
            for row in local.call("fleet", {})["processes"]
        ))
        local.call("inspect", {"pid": pid, "paths": [path]})
        preview_path = path.rsplit("/", 1)[0] + "/tdmcp_smoke_" + secrets.token_hex(6)
        created = False
        try:
            reply = local.call("mutate_nodes", {"pid": pid, "steps": [{
                "op": "create", "path": preview_path, "opType": "textDAT",
                "comment": "Temporary DAT preview regression fixture; removed by live smoke",
            }]})
            assert reply.get("ok"), reply
            created = True
            source = f"op({preview_path!r}).text = 'x' * 65535 + '€' + 'tail'\nresult = True"
            local.call("execute_python", {"pid": pid, "script": source})
            result = local.call("inspect", {"pid": pid, "paths": [preview_path], "include": ["content"]})
            content = result["nodes"][0]["content"]
            assert content["text"] == "x" * 65535
            assert content["bytes"] == 65535 and content["totalBytes"] == 65542
            assert content["textTruncation"]["code"] == "tdmcp.op.content_truncated"
            untouched = local.call("execute_python", {"pid": pid, "script": f"result = op({preview_path!r}).text[65535:]"})
            assert untouched["result"] == "€tail", "Inspect changed the original DAT"
        finally:
            if created:
                local.call("mutate_nodes", {"pid": pid, "steps": [{"op": "delete", "path": preview_path}]})
        try:
            admin(base, "/admin/config", {"bridge": {"scriptTimeoutSecs": 1}})
            script = {"pid": pid, "script": "import time\ntime.sleep(2)\nresult = 42"}
            timed_out = local.request("tools/call", {"name": "execute_python", "arguments": script})
            assert timed_out.get("isError"), "One-second script budget was not applied"
            assert "tdmcp.bridge.timeout" in json.dumps(timed_out), timed_out
            admin(base, "/admin/config", {"bridge": {"scriptTimeoutSecs": 5}})
            assert local.call("execute_python", script)["result"] == 42, "Increased live budget was not applied"
            assert admin(base, "/admin/status")["pid"] == status["pid"], "Changing budgets restarted the daemon"
        finally:
            admin(base, "/admin/config", {"bridge": {"scriptTimeoutSecs": before["bridge"]["script_timeout_secs"]}})
    finally:
        local.close()
    with tempfile.TemporaryDirectory(prefix="tdmcp-live-federation-") as directory:
        root = Path(directory)
        port, ipc = free_port(), free_port()
        while ipc == port:
            ipc = free_port()
        key = secrets.token_hex(24)
        config = root / "config.toml"
        config.write_text(f'[server]\nport = {port}\n[bridge]\nport = {ipc}\n[auth]\nmode = "psk"\npsk = "{key}"\n[daemon]\nshow_tray = false\nkeep_alive = true\n[federation]\nrole = "master"\ndaemon_id = "live-smoke-coordinator"\n', encoding="utf-8")
        env = {k: v for k, v in os.environ.items() if not k.startswith("TDMCP_")}
        env["TDMCP_CONFIG_PATH"] = str(config)
        master = f"http://127.0.0.1:{port}"
        binary = Path("target/debug/tdmcp-daemon").resolve()
        with (root / "daemon.log").open("wb") as log:
            child = subprocess.Popen([str(binary), "start", "--no-gui", "--data-dir", str(root / "data")], env=env, stdout=log, stderr=log)
            try:
                eventually(lambda: admin(master, "/admin/status")["ok"])
                reply = admin(base, "/admin/config", {"federation": {"role": "slave", "masterUrl": master, "masterPsk": "wrong-key"}})
                assert not reply["restartRequired"]
                eventually(lambda: "key rejected" in admin(base, "/admin/status")["federationConnection"])
                admin(base, "/admin/config", {"federation": {"masterPsk": key}})
                eventually(lambda: admin(base, "/admin/status")["federationConnection"] == "Connected to coordinator")
                client = McpClient(master, token=key)
                try:
                    rows = client.call("fleet", {})["processes"]
                    assert any(row["pid"] == pid and row.get("daemonId") == status["daemonId"] for row in rows), rows
                    target = {"pid": pid, "daemonId": status["daemonId"]}
                    client.call("inspect", {**target, "paths": [path]})
                    result = client.call("execute_python", {**target, "script": "result = 6 * 7"})
                    assert result.get("result") == 42, result
                    result = client.request("tools/call", {"name": "capture", "arguments": {**target, "path": path}})
                    assert not result.get("isError"), result
                    assert any(block["type"] == "image" for block in result["content"]), "No remote image"
                finally:
                    client.close()
                assert admin(base, "/admin/status")["pid"] == status["pid"], "Joining restarted the daemon"
            except BaseException:
                print((root / "daemon.log").read_text(encoding="utf-8", errors="replace"), file=sys.stderr)
                raise
            finally:
                try:
                    original = {k: v for k, v in before["federation"].items() if k != "daemon_id"}
                    admin(base, "/admin/config", {"federation": original})
                finally:
                    child.terminate()
                    try:
                        child.wait(timeout=5)
                    except subprocess.TimeoutExpired:
                        child.kill()
                        child.wait()
    print("Live smoke passed: UTF-8 DAT preview, live timeout decrease/increase, rejected key, live join, remote inspect/Python/image, unchanged daemon PID; temporary DAT removed and original settings restored")


if __name__ == "__main__":
    smoke(int(sys.argv[1]), sys.argv[2])
