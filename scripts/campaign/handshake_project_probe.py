#!/usr/bin/env python3
"""Prove the handshake on a project that actually carries the bridge.

    .venv/bin/python -u scripts/campaign/handshake_project_probe.py

The bare spawn cannot handshake -- it opens a project with no bridge in it,
which is a known property, not a defect. This probe stages the daemon's own
template (the one `createIfMissing` would produce), spawns with that project,
and reports every observed value: the reply, the pid's row in `fleet`, and
what the bridge's own connection state says over time.

Two arms, because the difference between them is the whole question:

  template  -- the shipped template.toe, unmodified
  installed -- the user's `Untitled.toe` (the same file a human's session has)

If both fail identically, the bridge cannot run on this host at all. If one
connects, the difference is in the project, and that is a real finding.
"""
from __future__ import annotations

import json
import shutil
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

RUN_DIR = REPO / "retroanalysis" / "campaign" / "handshake-project"
TEMPLATE = REPO / "crates" / "tdmcp-daemon" / "embedded" / "template.toe"
USER_PROJECT = Path.home() / ".local/share/tdmcp-rs/Untitled.toe"


def rows_for(session: harness.Session, pid: int) -> list[dict]:
    try:
        fleet = session.call("fleet", {"pids": [pid], "include": ["process", "bridge", "tasks"]}, 30)
    except Exception as exc:
        return [{"error": f"{type(exc).__name__}: {exc}"}]
    rows = _dig(fleet, "processes")
    if not isinstance(rows, list):
        return [{"error": f"no process list: {str(fleet)[:200]}"}]
    return [r for r in rows if isinstance(r, dict) and r.get("pid") == pid]


def arm(label: str, source: Path, port_mcp: int) -> dict:
    """Stage the project, spawn it, and watch the bridge state for the budget."""
    work = RUN_DIR / label
    work.mkdir(parents=True, exist_ok=True)
    staged = work / f"{label}.toe"
    shutil.copy2(source, staged)
    # The staged path must be visible to Wine, so it lives under the repo's
    # Z:-mapped tree rather than /tmp (Wine maps Z: to /).
    zed = REPO / "retroanalysis" / "campaign" / "handshake-project" / f"{label}.toe"
    shutil.copy2(source, zed)

    before = set(touchdesigner_pids())
    harness.MCP_PORT = port_mcp
    harness.BRIDGE_PORT = port_mcp - 1
    session = harness.Session(data_dir=work / "data", run_dir=work)
    out: dict = {"label": label, "source": str(source), "staged": str(zed)}
    try:
        session.start_daemon()
        out["spawn_reply"] = session.call("spawn_td", {"projectPath": str(zed)}, 120)
        pid = harness._extract_pid(out["spawn_reply"])
        out["pid"] = pid
        if pid is None:
            return out
        deadline = time.monotonic() + 120
        while time.monotonic() < deadline:
            rows = rows_for(session, pid)
            bridge = str(_dig(rows, "0.bridge"))
            out["last_bridge"] = bridge
            if bridge == "connected":
                out["connected_after_s"] = round(120 - (deadline - time.monotonic()), 1)
                break
            time.sleep(3.0)
        out["final_rows"] = rows_for(session, pid)
    except Exception as exc:
        out["error"] = f"{type(exc).__name__}: {exc}"
    finally:
        out["teardown"] = session.teardown()
    out["td_after"] = sorted(set(touchdesigner_pids()) - before)
    return out


def main() -> int:
    results = []
    for label, source, port in (("template", TEMPLATE, 9981), ("installed", USER_PROJECT, 9983)):
        if not source.exists():
            print(f"skip {label}: {source} does not exist", flush=True)
            continue
        print(f"\n=== {label}: {source} ===", flush=True)
        r = arm(label, source, port)
        print(json.dumps(r, indent=2, default=str)[:2000], flush=True)
        results.append(r)
    RUN_DIR.mkdir(parents=True, exist_ok=True)
    (RUN_DIR / "results.json").write_text(json.dumps(results, indent=2, default=str))
    print("\nTD left running:", touchdesigner_pids())
    return 0


if __name__ == "__main__":
    raise SystemExit(main())