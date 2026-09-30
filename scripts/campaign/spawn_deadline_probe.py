#!/usr/bin/env python3
"""Is the 60s reply cutoff the tool's own wait, or the SESSION's keep-alive?

    .venv/bin/python scripts/campaign/spawn_deadline_probe.py 25000 90000

Two `spawn_td` calls that take the SAME code path, differing only in how long
the handler is expected to block. The tool's own wait-timeout payload is
well-formed and carries the pid, so a reply at the shorter wait proves the
path works; if the longer wait instead dies at ~60s with no reply at all,
the cutoff belongs to something else -- and the only 60s in the daemon is
`session_manager.session_config.keep_alive` (main.rs:1067).
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

RUN_DIR = REPO / "retroanalysis" / "campaign" / "deadline-probe"


def one(session: Session, wait_ms: int) -> dict:
    before = set(touchdesigner_pids())
    print(f"\n=== spawn_td waitTimeoutMs={wait_ms} ===")
    started = time.monotonic()
    outcome: dict = {"wait_ms": wait_ms}
    try:
        payload = session.call("spawn_td", {"waitTimeoutMs": wait_ms}, timeout_s=wait_ms // 1000 + 90)
        outcome["outcome"] = "reply"
        outcome["elapsed_s"] = round(time.monotonic() - started, 1)
        outcome["payload"] = payload
    except Exception as exc:
        outcome["outcome"] = f"raised {type(exc).__name__}"
        outcome["elapsed_s"] = round(time.monotonic() - started, 1)
        outcome["error"] = str(exc)[:300]
    session._adopt_unreported_td(before)
    outcome["td_running"] = sorted(set(touchdesigner_pids()) - before)
    print(f"  -> {outcome['outcome']} after {outcome['elapsed_s']}s")
    print(f"  -> payload: {json.dumps(outcome.get('payload'), default=str)[:500]}")
    print(f"  -> TD running: {outcome['td_running']}")
    return outcome


def main(argv: list[str]) -> int:
    waits = [int(a) for a in argv] or [25_000, 90_000]
    session = Session(data_dir=RUN_DIR / "daemon-data", run_dir=RUN_DIR)
    session.start_daemon()
    results = []
    try:
        for wait in waits:
            results.append(one(session, wait))
            # Stop whatever this call started so the next measurement is its own.
            session.stop_td()
            time.sleep(3.0)
    finally:
        report = session.teardown()
        print(f"\nteardown: {json.dumps(report)}")
        (RUN_DIR / "results.json").write_text(json.dumps(results, indent=2, default=str))
        interesting = [l for l in session.log_lines if "terminated" in l or "Idle" in l or "error" in l.lower()]
        print("daemon lines mentioning termination/idle/error:")
        for line in interesting[-10:]:
            print(f"    {line[:240]}")
        interesting_noise = [l for l in session.log_noise if "X Error" in l or "GLX" in l or "err:" in l]
        print(f"child-process noise of interest: {interesting_noise[-6:]}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
