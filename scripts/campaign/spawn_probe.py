#!/usr/bin/env python3
"""Diagnose a campaign spawn by hand, with the daemon kept alive for probing.

    .venv/bin/python scripts/campaign/spawn_probe.py [wait_ms]

Prints every observed value. The point is to see what the daemon's fleet says
about a spawn whose RPC reply never arrived, because a run that cannot learn
the pid it spawned cannot clean up after itself.
"""
from __future__ import annotations

import json
import sys
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
REPO = HERE.parents[1]
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(REPO / "scripts"))

from harness import Session  # noqa: E402
from preflight import touchdesigner_pids  # noqa: E402

RUN_DIR = REPO / "retroanalysis" / "campaign" / "spawn-probe"


def show(label: str, value) -> None:
    print(f"  {label}: {json.dumps(value, default=str)[:600]}")


def main(argv: list[str]) -> int:
    wait_ms = int(argv[0]) if argv else 60000
    before = touchdesigner_pids()
    print(f"TD before: {before}")
    session = Session(data_dir=RUN_DIR / "daemon-data", run_dir=RUN_DIR)
    session.start_daemon()
    try:
        print(f"\nspawn_td (waitTimeoutMs={wait_ms}) ...")
        started = time.monotonic()
        try:
            payload = session.call("spawn_td", {"waitTimeoutMs": wait_ms}, timeout_s=wait_ms // 1000 + 120)
            show(f"reply after {time.monotonic() - started:.1f}s", payload)
        except Exception as exc:
            print(f"  reply raised after {time.monotonic() - started:.1f}s: {type(exc).__name__}: {exc}")

        print("\nfleet:")
        try:
            show("fleet", session.call("fleet", {"include": ["process", "bridge", "cancelled"]}, 30))
        except Exception as exc:
            print(f"  fleet raised: {type(exc).__name__}: {exc}")

        print(f"\nTD now: {touchdesigner_pids()}")
        print(f"daemon log tail:")
        for line in session.log_lines[-25:]:
            print(f"    {line[:300]}")
    finally:
        report = session.teardown()
        print(f"\nteardown: {json.dumps(report)}")
        print(f"TD after: {touchdesigner_pids()}  (baseline {before})")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
