#!/usr/bin/env python3
"""Is the handshake failure about the CAMPAIGN's environment or the host's?

    .venv/bin/python scripts/campaign/handshake_isolate.py

Two daemon sessions, same data dir shape, different environments:

  A. as the campaign starts it  (TDMCP_IPC_PORT + TDMCP_NO_GUI + --no-gui)
  B. as a plain interactive shell would (nothing exported)

The question is not "does it work" but WHERE it breaks: if A handshakes and
B does not, the campaign's own environment is the cause and the fix belongs in
the harness. If both fail the same way, the cause is upstream of the campaign
(the host's display / Wine / prefix) and the harness is innocent -- which is
the difference between a defect in my code and a fact about the machine.
"""
from __future__ import annotations

import json
import os
import subprocess
import sys
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
REPO = HERE.parents[1]
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(REPO / "scripts"))

import harness  # noqa: E402
from preflight import touchdesigner_pids  # noqa: E402
from scenario import _dig  # noqa: E402

RUN_DIR = REPO / "retroanalysis" / "campaign" / "handshake-isolate"


def fleet_bridge_states(session: harness.Session, pid: int) -> list[dict]:
    try:
        fleet = session.call("fleet", {"include": ["process", "bridge"]}, 30)
    except Exception as exc:
        return [{"error": f"{type(exc).__name__}: {exc}"}]
    rows = harness._dig(fleet, "processes")
    if not isinstance(rows, list):
        return [{"error": f"fleet payload has no process list: {str(fleet)[:200]}"}]
    return [r for r in rows if isinstance(r, dict) and r.get("pid") == pid]


def attempt(label: str, *, campaign_env: bool, budget_s: float = 150.0) -> dict:
    before = set(touchdesigner_pids())
    session = harness.Session(data_dir=RUN_DIR / f"data-{label}", run_dir=RUN_DIR / label)
    if campaign_env:
        session.start_daemon()
    else:
        # Plain environment: no TDMCP_IPC_PORT, so the bridge port falls back to
        # its built-in default. This is the honest "what does a user get" arm.
        os.environ.pop("TDMCP_IPC_PORT", None)
        os.environ.pop("TDMCP_NO_GUI", None)
        session.start_daemon()
    result: dict = {"label": label, "campaign_env": campaign_env}
    try:
        payload = session.call("spawn_td", {}, timeout_s=budget_s + 60)
        result["spawn_reply_s"] = round(budget_s, 1)
        result["spawn_payload"] = payload
        pid = harness._extract_pid(payload)
        result["pid"] = pid
        deadline = time.monotonic() + budget_s
        while pid and time.monotonic() < deadline:
            rows = fleet_bridge_states(session, pid)
            if any(str(r.get("bridge")) == "connected" for r in rows):
                result["connected_after_s"] = round(budget_s - (deadline - time.monotonic()), 1)
                break
            time.sleep(3.0)
        result["final_rows"] = fleet_bridge_states(session, pid) if pid else []
        result["bridge"] = str(_dig(result["final_rows"], "0.bridge")) if result["final_rows"] else None
    except Exception as exc:
        result["error"] = f"{type(exc).__name__}: {exc}"
    finally:
        report = session.teardown()
        result["teardown"] = report
    result["td_after"] = sorted(set(touchdesigner_pids()) - before)
    return result


def main() -> int:
    harness.BRIDGE_PORT = 9971
    harness.MCP_PORT = 9972
    out = []
    for label, env_on in (("campaign-env", True), ("plain-env", False)):
        print(f"\n=== {label} (campaign_env={env_on}) ===", flush=True)
        r = attempt(label, campaign_env=env_on)
        print(json.dumps(r, indent=2, default=str)[:1500], flush=True)
        out.append(r)
    (RUN_DIR / "results.json").write_text(json.dumps(out, indent=2, default=str))
    print("\nTD left running:", touchdesigner_pids())
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
