#!/usr/bin/env python3
"""Check the actual release graphs separately; do not mix platform-only edges."""

import json
from pathlib import Path
import shutil
import subprocess
import sys


ROOT = Path(__file__).resolve().parent.parent
TARGETS = (
    "x86_64-unknown-linux-gnu",
    "x86_64-pc-windows-msvc",
    "aarch64-apple-darwin",
    "x86_64-apple-darwin",
)


def check(run=subprocess.run):
    failed = False
    for target in TARGETS:
        print(f"Dependency check: {target}", flush=True)
        result = run(
            ["cargo", "deny", "--locked", "--target", target, "--format", "json", "check"],
            cwd=ROOT, capture_output=True, text=True, timeout=180,
        )
        failed |= result.returncode != 0
        for line in (result.stdout + "\n" + result.stderr).splitlines():
            try:
                event = json.loads(line)
            except json.JSONDecodeError:
                if line.strip():
                    print(line)
                continue
            fields = event.get("fields", {})
            if event.get("type") == "summary":
                print(json.dumps(fields, sort_keys=True))
            elif fields.get("severity") == "error":
                crates = ", ".join(
                    f'{g["Krate"]["name"]} {g["Krate"]["version"]}'
                    for g in fields.get("graphs", []) if "Krate" in g
                )
                print(f'{fields.get("code", "error")}: {crates}: {fields.get("message", "")}')
                for label in fields.get("labels", []):
                    if label.get("message"):
                        print(f'  {label.get("span", "")}: {label["message"]}')
                for note in fields.get("notes", [])[:2]:
                    print(f"  {note}")
    return int(failed)


if __name__ == "__main__":
    if shutil.which("cargo-deny") is None:
        sys.exit("cargo-deny is required; install it before running dependency checks")
    try:
        sys.exit(check())
    except (OSError, subprocess.TimeoutExpired) as error:
        sys.exit(f"Could not run cargo-deny: {error}")
