#!/usr/bin/env python3
"""What does the bridge decide, and why, at the moment it redials?

Two clean runs (`r39-verify`, `r40-verify`) show the same thing on a healthy
host: a pid handshakes once, serves four calls, and then ~440 ms after a
`compare_nodes` REFUSAL the daemon logs

    pid handshake — already connected
    superseding bridge session  prev_generation 1

so something on the bridge side decided the port was free while a worker was
still holding it. Earlier rounds blamed the `importlib.reload` of the package
root; `reload_probe.py` showed a reload does wipe `_active_thread`, but also
that runs which never reloaded never superseded. Neither measurement says
which code path dials here.

This probe answers it by instrumenting the DECISION, not the state: it wraps
`_bridge_connected`, `_bridge_shutting_down` and `_run_bootstrap` inside TD,
so every call records its arguments, its answer, and the thread and stack it
came from. That distinguishes the three possibilities directly:

  * `_bridge_connected` returns False while a live worker exists  -> false
    negative in `is_connected()`.
  * `_run_bootstrap` is called while the old worker is alive      -> the gate
    let it through, and the question is why the gate agreed.
  * no decision at all, and a second worker just appears         -> the dial is
    not coming from tox callbacks, and the whole premise is wrong.

The wrappers are installed on the `tdmcp_bridge` and `__tdmcp_callbacks__`
module objects rather than on the functions they wrap, so a reload replaces the
function but leaves the attribute pointing at the wrapper's closure re-bound to
whatever the fresh lookup finds. `_rebind` re-installs after each reload.

Read-only against the product: nothing is written to the repo or the data dir,
and the log is appended, never truncated, so a supersede already in progress
still shows what preceded it.

Usage:
    .venv/bin/python scripts/campaign/decision_probe.py [--seconds 40]
"""
from __future__ import annotations

import json
import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import harness  # noqa: E402

REPO = Path(__file__).resolve().parents[2]
LOG = REPO / "retroanalysis/campaign/decision-probe.log"

#: Installed inside TD. Appends one JSON line per observation.
PATCH = r'''
import io as _io, json as _json, os as _os, threading as _th, time as _th_time

TDMCP_DECISION_LOG = __LOG__

def _tdmcp_dec_write(rec):
    try:
        rec["t"] = round(_th_time.monotonic(), 4)
        rec["thread"] = _th.current_thread().name
        rec["stack"] = [f"{f.name}:{f.lineno}" for f in _th.stack()[:7]]
        with _io.open(TDMCP_DECISION_LOG, "a", encoding="utf-8") as fh:
            fh.write(_json.dumps(rec, default=repr) + "\n")
            fh.flush()
    except Exception as exc:
        pass

def _tdmcp_workers():
    out = []
    for t in _th.enumerate():
        rec = {"name": t.name, "alive": t.is_alive(),
               "daemon": t.daemon, "shutting_down": getattr(t, "tdmcp_shutting_down", None),
               "has_stream": hasattr(t, "tdmcp_stream")}
        out.append(rec)
    return out

def _tdmcp_state():
    try:
        import tdmcp_bridge as p
    except Exception as exc:
        return {"import_error": repr(exc)}
    rec = {"active_thread": repr(getattr(p, "_active_thread", None)),
           "active_stream": repr(getattr(p, "_active_stream", None)),
           "shutting_down": repr(getattr(p, "_shutting_down_thread", None)),
           "workers": _tdmcp_workers()}
    for name in ("is_connected", "is_shutting_down"):
        fn = getattr(p, name, None)
        if callable(fn):
            try:
                rec[name] = bool(fn())
            except Exception as exc:
                rec[name] = "raised " + repr(exc)
        else:
            rec[name] = "<absent>"
    return rec

def _tdmcp_install():
    """Wrap the three decision functions in tox callbacks."""
    import sys as _sys
    mod = _sys.modules.get("__tdmcp_callbacks__")
    if mod is None:
        _tdmcp_dec_write({"event": "callbacks_module_absent",
                          "sys_modules": sorted(n for n in _sys.modules if "tdmcp" in n or "callback" in n)})
        return False
    _tdmcp_dec_write({"event": "install", "module": getattr(mod, "__file__", "?")})

    for name in ("_bridge_connected", "_bridge_shutting_down"):
        original = getattr(mod, name, None)
        if original is None or getattr(original, "_tdmcp_wrapped", False):
            continue
        def make(fn, label):
            def wrapper(arg=None, *a, **k):
                answer = fn(arg, *a, **k) if arg is not None else fn(*a, **k)
                _tdmcp_dec_write({"event": label, "answer": answer, "state": _tdmcp_state()})
                return answer
            wrapper._tdmcp_wrapped = True
            return wrapper
        setattr(mod, name, make(original, name))

    original_boot = getattr(mod, "_run_bootstrap", None)
    if original_boot is not None and not getattr(original_boot, "_tdmcp_wrapped", False):
        def boot_wrapper(*a, **k):
            _tdmcp_dec_write({"event": "_run_bootstrap:CALL", "state": _tdmcp_state()})
            result = original_boot(*a, **k)
            _tdmcp_dec_write({"event": "_run_bootstrap:RETURN", "state": _tdmcp_state()})
            return result
        boot_wrapper._tdmcp_wrapped = True
        mod._run_bootstrap = boot_wrapper
    return True

if not _tdmcp_install():
    raise SystemExit("callbacks module not found")
_tdmcp_dec_write({"event": "armed", "state": _tdmcp_state()})

# Re-arm after every reload of the callbacks module.
try:
    import __tdmcp_callbacks__ as _cb
    if not hasattr(_cb, "_tdmcp_install_wired"):
        _tdmcp_install_wired = True
        def _tdmcp_rearm():
            try:
                _tdmcp_install()
            except Exception:
                pass
            try:
                import td
                # The product's own re-arm shape: a module-level function,
                # re-scheduled by path, with a delayRef so the delay still
                # advances while the timeline is paused.
                ref = _cb._td_delay_ref() if hasattr(_cb, "_td_delay_ref") else None
                kwargs = {"delayMilliSeconds": 1000, "wallTime": True}
                if ref is not None:
                    kwargs["delayRef"] = ref
                td.run(
                    f"op('{_cb.me.path}').module._tdmcp_rearm()", **kwargs
                )
            except Exception:
                pass
        _cb._tdmcp_rearm = _tdmcp_rearm
        td.run(
            f"op('{_cb.me.path}').module._tdmcp_rearm()",
            delayMilliSeconds=500,
            wallTime=True,
            delayRef=_cb._td_delay_ref() if hasattr(_cb, "_td_delay_ref") else td.TDResources(0),
        )
        _tdmcp_dec_write({"event": "watchdog_wired"})
except Exception as exc:
    _tdmcp_dec_write({"event": "watchdog_wire_failed", "error": repr(exc)})

print("tdmcp decision probe armed")
'''.replace("__LOG__", repr(str(LOG)))


def main() -> int:
    seconds = 40.0
    args = sys.argv[1:]
    if "--seconds" in args:
        seconds = float(args[args.index("--seconds") + 1])

    # Truncated here, unlike an in-TD append, so a stale file from a previous
    # probe run cannot be read as this run's evidence.
    LOG.write_text("", encoding="utf-8")

    run_dir = REPO / "retroanalysis/campaign" / f"decision-probe-{int(time.time())}"
    run_dir.mkdir(parents=True, exist_ok=True)
    session = harness.Session(data_dir=run_dir / "daemon-data", run_dir=run_dir)
    try:
        session.start_daemon()
        print("daemon up; spawning a task-owned TouchDesigner")
        import shutil

        toe = run_dir / "decision-probe.toe"
        shutil.copy2(REPO / "crates/tdmcp-daemon/embedded/template.toe", toe)
        td_pid = session.spawn_td(project_path=str(toe))
        if not td_pid:
            print("spawn_td returned no pid")
            return 2
        print(f"td pid {td_pid}")
        # `spawn_td` with no projectPath opens a project with no bridge DATs,
        # so no handshake can ever arrive. Measured, twice, at ~2.5 s per check:
        # a bare spawn is accepted by the tool and then reports "no connected
        # bridge" forever. The probe needs a LIVE bridge, so it loads one.
        connect_deadline = time.monotonic() + 90.0
        # `bootstrap.py` imports the bridge from the CONVENTIONAL data dir
        # (`~/.local/share/tdmcp-rs/bridge`), not from the per-run `--data-dir`
        # copy the daemon materialises, because `bridge_dir` only reaches TD
        # through `TDMCP_BRIDGE_DIR` and `bootstrap_threaded(bridge_dir=...)`.
        # So this probe's edits are only in TD if the CONVENTIONAL copy is
        # current — proven, not assumed, by reading the file back out of TD.
        connected = False
        while time.monotonic() < connect_deadline:
            fleet = session.call("fleet", {"pids": [td_pid]}, 30)
            if harness._bridge_connected(fleet, td_pid):
                connected = True
                break
            time.sleep(1.0)
        if not connected:
            print(f"bridge not connected for {td_pid} within 90s; refusing to arm")
            return 2
        print("bridge connected; checking TD is running the code under test")

        # Read back what TD actually imported. The daemon's own handshake
        # answer is not proof: `bootstrap.py` puts the CONVENTIONAL data dir on
        # `sys.path` and hands `bootstrap_threaded` no override, so a per-run
        # `--data-dir` copy never reaches TD. Measured: a fixed `execute.py`
        # in the repo, in the per-run copy, and in the global install all left
        # TD running the stale version.
        probe = session.call(
            "execute_python",
            {
                "pid": td_pid,
                "script": (
                    "import tdmcp_bridge.execute as e\n"
                    "result = {\n"
                    "    'file': e.__file__,\n"
                    "    'seeded': 'execute_python produced no result' in "
                    "open(e.__file__, encoding='utf-8').read(),\n"
                    "}\n"
                ),
            },
            60,
        )
        print("td bridge identity:", json.dumps(probe.get("result", probe))[:300])
        loaded = (probe.get("result") or {}).get("seeded")
        if loaded is not True:
            print(
                "\nSTALE BRIDGE IN TD -- this probe would describe code that is "
                "not the code in the repo.\nInstall it first: cargo run -p "
                "xtask -- install  (or re-run `ensure` against the install dir).\n"
            )
            session.stop_td()
            return 2
        print("TD is running the repo's bridge")

        # `includeLogs` left to its DEFAULT on purpose. Passing it explicitly
        # is what made the first two probe runs fail with
        # "cannot access local variable 'out'", and pinning WHY is the point:
        # bridge/tdmcp_bridge/execute.py:315 assigns out["logs"] in a `finally`
        # that runs even when `includeLogs` is false, so `out` is read while
        # unbound. Live TD's `td` import partly fails under Wine (the bare
        # name is absent), which skips the only assignment and reaches that
        # read. The unbound name is caught, turned into
        # `tdmcp.script.execution_failed`, and the log says "bridge error",
        # which is why it first read as a bridge failure rather than a script
        # failure.
        armed = session.call(
            "execute_python",
            {"pid": td_pid, "script": PATCH},
            60,
        )
        print("arm result:", json.dumps(armed)[:400])
        if not armed.get("ok"):
            return 2

        print(f"observing for {seconds:.0f}s while issuing the scenario's calls")
        deadline = time.monotonic() + seconds
        # The calls that precede the supersede in both clean runs: two reads,
        # a compare REFUSAL, then a capture.
        sequence = [
            ("fleet", {}),
            ("inspect", {"paths": ["/project1"], "include": ["nodes"]}),
            ("compare_nodes", {"paths": ["/project1"], "fields": ["par.name"]}),
            ("compare_nodes", {"paths": ["/project1"], "fields": ["par.name"], "capture": True}),
            ("compare_nodes", {"paths": ["/project1"], "fields": ["par.name"], "capture": True}),
        ]
        index = 0
        while time.monotonic() < deadline:
            tool, tool_args = sequence[min(index, len(sequence) - 1)]
            reply = session.call(tool, dict(tool_args, pid=td_pid), 60)
            summary = reply.get("summary") or reply.get("ok")
            print(f"  {index:02d} {tool}: ok={reply.get('ok')} {str(summary)[:90]}")
            index += 1
            time.sleep(2.0)

        teardown = session.stop_td()
        print(f"teardown: {json.dumps(teardown)}")
        print(f"record: {LOG}")
    finally:
        session.stop_daemon()

    if not LOG.read_text(encoding="utf-8").strip():
        print("\nNO RECORD -- the probe never ran inside TD")
        return 2
    print(f"\n{len(LOG.read_text(encoding='utf-8').splitlines())} record(s)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
