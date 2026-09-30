#!/usr/bin/env python3
"""Why does a healthy bridge session get superseded 2.7 s in?

Reproduces the campaign's `superseding bridge session` signature on ONE
task-owned TouchDesigner with ONE task-owned daemon, and asks the deciding
question: what did `is_connected()` see on the tick that decided to redial?

The probe is deliberately read-only against the product. It installs an
`importlib.reload` wrapper INSIDE TouchDesigner, because the bridge package is
what gets reloaded on every bootstrap, so a patch on `importlib` itself survives
every one of them. The wrapper records the pre-reload `is_connected()`, the
live worker threads, and the post-reload value.

If the record shows a healthy worker reported as NOT connected, the watchdog
redialed a session that was still there -- the supersede is a false negative in
`is_connected()`, not a real disconnect. If the record shows the worker already
dead, the supersede is recovery and the real question becomes why it died.

Usage:
    .venv/bin/python scripts/campaign/reload_probe.py
"""
from __future__ import annotations

import json
import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import harness  # noqa: E402

REPO = Path(__file__).resolve().parents[2]

# Installed INSIDE TD by this script. The bridge package is reloaded on every
# bootstrap; `importlib` is not, so a patch applied here outlives every reload
# and observes each one.
PATCH = r'''
import io as _io, importlib as _il, json as _json, threading as _th, time as _th_time

TDMCP_RELOAD_LOG = "/home/acorbeau/Repos/td-mcp-rs/retroanalysis/campaign/reload-probe.log"

def _tdmcp_probe_snapshot(tag):
    rec = {"tag": tag, "t": _th_time.monotonic()}
    try:
        rec["stack"] = [(f.name, f.lineno) for f in _th.stack()[:6]]
    except Exception:
        rec["stack"] = None
    try:
        import tdmcp_bridge as p
        rec["is_connected_fn"] = repr(getattr(p, "is_connected", None))
        rec["active_thread"] = repr(getattr(p, "_active_thread", None))
        rec["active_stream"] = repr(getattr(p, "_active_stream", None))
        rec["shutting_down"] = repr(getattr(p, "_shutting_down_thread", None))
        try:
            rec["is_connected"] = bool(p.is_connected())
        except Exception as e:
            rec["is_connected"] = "raised " + type(e).__name__ + ": " + str(e)
        try:
            rec["is_shutting_down"] = bool(p.is_shutting_down())
        except Exception as e:
            rec["is_shutting_down"] = "raised " + type(e).__name__ + ": " + str(e)
        rec["pkg_file"] = getattr(p, "__file__", None)
    except Exception as e:
        rec["pkg_error"] = type(e).__name__ + ": " + str(e)
    rec["workers"] = [
        {"name": t.name, "alive": t.is_alive(), "stream": repr(getattr(t, "tdmcp_stream", None)),
         "shutting_down": getattr(t, "tdmcp_shutting_down", False)}
        for t in _th.enumerate() if t.name == "tdmcp-bridge-worker"
    ]
    try:
        rec["caller"] = repr(getattr(_th.current_thread(), "name", None))
    except Exception:
        rec["caller"] = None
    try:
        with _io.open(TDMCP_RELOAD_LOG, "a", encoding="utf-8") as fh:
            fh.write(_json.dumps(rec) + "\n")
    except Exception:
        pass
    return rec

if not getattr(_il, "_tdmcp_probe_installed", False):
    _tdmcp_probe_installed = True
    _orig_reload = _il.reload

    def _probe_reload(module, *a, **kw):
        name = getattr(module, "__name__", str(module))
        if name == "tdmcp_bridge" or name.startswith("tdmcp_bridge."):
            _tdmcp_probe_snapshot("pre:" + name)
            out = _orig_reload(module, *a, **kw)
            _tdmcp_probe_snapshot("post:" + name)
            return out
        return _orig_reload(module, *a, **kw)

    _il.reload = _probe_reload
'''


def main() -> int:
    log = REPO / "retroanalysis" / "campaign" / "reload-probe.log"
    log.parent.mkdir(parents=True, exist_ok=True)
    if log.exists():
        log.unlink()

    run_dir = REPO / "retroanalysis" / "campaign" / f"reload-probe-{int(time.time())}"
    run_dir.mkdir(parents=True, exist_ok=True)

    session = harness.Session(
        data_dir=run_dir / "daemon-data",
        run_dir=run_dir,
    )
    call = None
    try:
        session.start_daemon()
        call = session.call
        print(f"daemon up: {harness.MCP_PORT}", flush=True)

        toe = REPO / "retroanalysis" / "campaign" / "reload-probe.toe"
        toe.parent.mkdir(parents=True, exist_ok=True)
        import shutil

        shutil.copy2(REPO / "crates" / "tdmcp-daemon" / "embedded" / "template.toe", toe)
        pid = session.spawn_td(project_path=str(toe))
        print(f"td pid {pid}, bridge connected")

        # Install the probe AFTER the handshake, so the log records only what
        # happens afterwards -- a reconnect, not the initial connect.
        out = call(
            "execute_python",
            {"pid": pid, "script": PATCH},
            60,
        )
        print(f"patch installed: ok={out.get('ok')}")
        if not out.get("ok"):
            print(json.dumps(out, indent=1)[:2000])
            return 1

        print("\n--- now sit still and let the watchdog decide (12 s) ---")
        for i in range(6):
            time.sleep(2.0)
            fleet = call("fleet", {}, 30)
            row = harness._bridge_connected(fleet, pid)
            state = "?"
            procs = fleet.get("result", fleet)
            if isinstance(procs, dict):
                for p in procs.get("processes", []) or []:
                    if p.get("pid") == pid:
                        state = p.get("bridge")
            print(f"  t+{(i + 1) * 2:>2}s  bridge={state}  ({row})")

        # Ask TD directly what it thinks, right now.
        out = call(
            "execute_python",
            {
                "pid": pid,
                "script": (
                    "import tdmcp_bridge as p, threading, json\n"
                    "res = {\n"
                    "  'is_connected': p.is_connected(),\n"
                    "  'is_shutting_down': p.is_shutting_down(),\n"
                    "  'active_thread': repr(p._active_thread),\n"
                    "  'active_stream': repr(p._active_stream),\n"
                    "  'workers': [(t.name, t.is_alive(), getattr(t,'tdmcp_shutting_down',False))\n"
                    "              for t in threading.enumerate() if t.name=='tdmcp-bridge-worker'],\n"
                    "}\n"
                    "print('TDSAYS ' + json.dumps(res))\n"
                ),
            },
            60,
        )
        logs = out.get("logs") or []
        for line in logs:
            if "TDSAYS" in str(line):
                print("\nTD says:", str(line).split("TDSAYS", 1)[1].strip()[:1200])

        print(f"\n--- reload log ({log}) ---")
        if log.exists():
            for line in log.read_text(encoding="utf-8", errors="replace").splitlines():
                try:
                    print(json.dumps(json.loads(line), indent=1))
                except Exception:
                    print(line[:400])
        else:
            print("(no reload recorded -- the package was never reloaded)")
        return 0
    finally:
        report = session.teardown()
        print(f"\nteardown: {json.dumps(report)}")
        print(f"TD after: {report.get('leftovers')}")


if __name__ == "__main__":
    raise SystemExit(main())
