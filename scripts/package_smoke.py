#!/usr/bin/env python3
"""Verify an extracted release binary in a temporary installation (no TD needed)."""

import argparse
import json
import os
from pathlib import Path
import socket
import subprocess
import sys
import tempfile
import time
import urllib.request

from mcp_probe import McpClient


def free_port():
    with socket.socket() as listener:
        listener.bind(("127.0.0.1", 0))
        return listener.getsockname()[1]


def check_project_license(directory):
    license_path = Path(directory) / "LICENSE"
    expected = Path(__file__).resolve().parents[1] / "LICENSE"
    if not license_path.is_file() or license_path.read_text(encoding="utf-8") != expected.read_text(encoding="utf-8"):
        raise RuntimeError("release package is missing the matching project LICENSE")


def smoke(binary, *, distribution=False):
    binary = str(Path(binary).resolve(strict=True))
    if distribution:
        check_project_license(Path(binary).parent)
    with tempfile.TemporaryDirectory(prefix="tdmcp-package-") as temporary:
        root = Path(temporary)
        config = root / "config.toml"
        port = free_port()
        ipc_port = free_port()
        while ipc_port == port:
            ipc_port = free_port()
        config.write_text(f'[server]\nport = {port}\n[daemon]\nkeep_alive = true\nshow_tray = false\n[federation]\ndaemon_id = "package-smoke-stable-id"\n', encoding="utf-8")
        env = {k: v for k, v in os.environ.items() if not k.startswith("TDMCP_")}
        env.update(TDMCP_CONFIG_PATH=str(config), TDMCP_PORT=str(port), TDMCP_IPC_PORT=str(ipc_port), TDMCP_NO_GUI="1")
        data = root / "data"
        for _ in range(2):
            subprocess.run([binary, "install", "--force", "--data-dir", str(data)], env=env, check=True, timeout=30)
        assert 'package-smoke-stable-id' in config.read_text(encoding="utf-8"), "install reset the configuration"
        check_project_license(data)
        for asset in ["bootstrap.tox", "bridge/tdmcp_bridge/__init__.py", "diagnostics/catalog.yaml", "skills/touchdesigner/SKILL.md"]:
            assert (data / asset).is_file(), f"missing packaged asset: {asset}"
        installed = data / "bin" / Path(binary).name
        subprocess.run([str(installed), "--version"], env=env, check=True, timeout=5)
        with (root / "daemon.log").open("wb") as log:
            child = subprocess.Popen([str(installed), "start", "--data-dir", str(data), "--no-gui"], env=env, stdout=log, stderr=log)
            try:
                base = f"http://127.0.0.1:{port}"
                deadline = time.monotonic() + 20
                while True:
                    assert child.poll() is None, "packaged daemon exited during startup"
                    try:
                        with urllib.request.urlopen(base + "/admin/status", timeout=1) as response:
                            assert json.load(response)["daemonId"] == "package-smoke-stable-id"
                        break
                    except OSError:
                        assert time.monotonic() < deadline, "packaged daemon did not become healthy"
                        time.sleep(0.1)
                client = McpClient(base)
                try:
                    assert client.call("fleet", {})["processes"] == []
                    docs = client.request("resources/read", {"uri": "tdmcp://docs/operate"})
                    assert docs["contents"][0]["text"], "packaged skills unavailable"
                finally:
                    client.close()
                request = urllib.request.Request(base + "/admin/shutdown", data=b"", method="POST")
                with urllib.request.urlopen(request, timeout=3):
                    pass
                assert child.wait(timeout=10) == 0, "packaged daemon did not shut down cleanly"
            except BaseException:
                print((root / "daemon.log").read_text(encoding="utf-8", errors="replace"), file=sys.stderr)
                raise
            finally:
                if child.poll() is None:
                    child.kill()
                child.wait()
    print("Package smoke passed: repeat install, preserved config, assets, MCP, shutdown")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary")
    parser.add_argument("--distribution", action="store_true", help="Also verify the extracted archive's license")
    args = parser.parse_args()
    smoke(args.binary, distribution=args.distribution)
