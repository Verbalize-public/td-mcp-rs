#!/usr/bin/env python3
"""Verify an extracted release binary in a temporary installation (no TD needed)."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import socket
import subprocess
import sys
import tempfile
import time
import tomllib
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


def sha256_file(path):
    digest = hashlib.sha256()
    with Path(path).open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def check_third_party_records(directory):
    directory = Path(directory)
    notices = directory / "THIRD_PARTY_NOTICES.txt"
    offer = directory / "SOURCE_OFFER.md"
    third_party = directory / "third-party"
    inventory_path = third_party / "inventory.json"
    for required in (notices, offer, inventory_path, third_party / "licenses", third_party / "sources"):
        if not required.exists():
            raise RuntimeError(f"release package is missing third-party record: {required.name}")

    inventory = json.loads(inventory_path.read_text(encoding="utf-8"))
    if inventory.get("schemaVersion") != 1 or inventory.get("rootPackage") != "tdmcp-daemon":
        raise RuntimeError("third-party inventory has an unsupported schema or root package")
    if not inventory.get("rootVersion") or not inventory.get("rootSourceUrl", "").startswith("https://"):
        raise RuntimeError("third-party inventory has no release source identity")
    packages = inventory.get("packages")
    if not isinstance(packages, list) or not packages:
        raise RuntimeError("third-party inventory has no release dependencies")
    identities = [(item.get("name"), item.get("version"), item.get("source")) for item in packages]
    if identities != sorted(identities):
        raise RuntimeError("third-party inventory dependency order is not deterministic")

    for package in packages:
        if not package.get("source", "").startswith("registry+"):
            raise RuntimeError(f"unsupported third-party source for {package.get('name')}")
        checksum = package.get("cargoLockChecksum", "")
        if len(checksum) != 64:
            raise RuntimeError(f"missing Cargo.lock checksum for {package.get('name')}")
        license_files = package.get("licenseFiles")
        if not license_files:
            raise RuntimeError(f"no applicable license text for {package.get('name')}")
        for record in license_files:
            path = third_party / record["packagedPath"]
            if not path.is_file() or sha256_file(path) != record["sha256"]:
                raise RuntimeError(f"license text mismatch for {package.get('name')}: {path.name}")
        bundled = package.get("bundledSource")
        if bundled:
            path = third_party / bundled["path"]
            if bundled.get("modifiedByTdmcp") is not False or not bundled.get("reason"):
                raise RuntimeError(f"source obligation status is not explicit for {package.get('name')}")
            if not path.is_file() or sha256_file(path) != bundled["sha256"] or bundled["sha256"] != checksum:
                raise RuntimeError(f"bundled source mismatch for {package.get('name')}")

    lock_hash = inventory.get("cargoLockSha256")
    offer_text = offer.read_text(encoding="utf-8")
    if not lock_hash or lock_hash not in notices.read_text(encoding="utf-8") or lock_hash not in offer_text:
        raise RuntimeError("third-party notice/source offer does not identify its Cargo.lock graph")
    if any(package.get("name") == "wayland-protocols-plasma" for package in packages):
        if inventory["rootSourceUrl"] not in offer_text or "Rebuild and relink" not in offer_text:
            raise RuntimeError("LGPL source offer lacks project-source or relinking instructions")
    serialized = inventory_path.read_text(encoding="utf-8")
    if "manifest_path" in serialized or str(Path.home()) in serialized:
        raise RuntimeError("third-party inventory exposes a private local path")


def smoke(binary, *, distribution=False):
    binary = str(Path(binary).resolve(strict=True))
    if distribution:
        check_project_license(Path(binary).parent)
        check_third_party_records(Path(binary).parent)
    with tempfile.TemporaryDirectory(prefix="tdmcp-package-") as temporary:
        root = Path(temporary)
        config = root / "config.toml"
        port = free_port()
        ipc_port = free_port()
        while ipc_port == port:
            ipc_port = free_port()
        config.write_text(f'[server]\nport = {port}\n[daemon]\nkeep_alive = true\nshow_tray = false\n[federation]\ndaemon_id = "package-smoke-stable-id"\n[logging]\nmax_files = 7\nretention_days = 5\ncrash_max_reports = 9\ncrash_tail_lines = 3\n', encoding="utf-8")
        env = {k: v for k, v in os.environ.items() if not k.startswith("TDMCP_")}
        env.update(TDMCP_CONFIG_PATH=str(config), TDMCP_PORT=str(port), TDMCP_IPC_PORT=str(ipc_port), TDMCP_NO_GUI="1")
        data = root / "data"
        for _ in range(2):
            subprocess.run([binary, "install", "--force", "--data-dir", str(data)], env=env, check=True, timeout=30)
        assert 'package-smoke-stable-id' in config.read_text(encoding="utf-8"), "install reset the configuration"
        logging = tomllib.loads(config.read_text(encoding="utf-8"))["logging"]
        for key, value in {"max_files": 7, "retention_days": 5, "crash_max_reports": 9, "crash_tail_lines": 3}.items():
            assert logging[key] == value, f"install reset logging.{key}"
        check_project_license(data)
        check_third_party_records(data)
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
                    fleet = client.call("fleet", {})
                    assert fleet is not None and fleet["processes"] == []
                    docs = client.request("resources/read", {"uri": "tdmcp://docs/operate"})
                    assert docs is not None and docs["contents"][0]["text"], "packaged skills unavailable"
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
    print("Package smoke passed: repeat install, licenses/notices, assets, MCP, shutdown")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary")
    parser.add_argument("--distribution", action="store_true", help="Also verify the extracted archive's license")
    args = parser.parse_args()
    smoke(args.binary, distribution=args.distribution)
