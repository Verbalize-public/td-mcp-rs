#!/usr/bin/env python3
"""Repack `embedded/bootstrap.tox` from the current sources, in a live TD.

A `.tox` is TouchDesigner's opaque component format: nothing outside TD can
produce one. `bridge/bootstrap.py` and `bridge/tox_callbacks.py` are packed
into `crates/tdmcp-daemon/embedded/bootstrap.tox` by hand, and nothing keeps
that snapshot in step with the sources except
`install::tests::bootstrap_tox_matches_packed_source_hash`, which fails
loudly when they drift.

It is currently red. The last pack was `d587565` on 2026-09-09; since then
`bridge/tox_callbacks.py` gained the teardown gate from `92cb509`
(`_bridge_shutting_down`, `draining`) and the packed blob contains neither of
those names -- measured by searching the blob, not by trusting the timestamp.

That matters more than a red test. The tox is copy #2 of four in
`scripts/pack_bootstrap_tox.md`; it is extracted to every user's data dir and
baked into every project, so a stale one ships pre-fix bridge code to everyone
who updates. This is the concrete reason that file insists on a live TD.

Uses a task-owned TouchDesigner on the campaign template, and writes ONLY the
packed blob plus the source-hash sidecar. It does not touch the user's daemons,
their open TouchDesigners, or anything in their data dir.
"""
from __future__ import annotations

import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import harness  # noqa: E402

REPO = Path(__file__).resolve().parents[2]
OUT = REPO / "crates/tdmcp-daemon/embedded/bootstrap.tox"
BOOT = REPO / "bridge/bootstrap.py"
CALLBACKS = REPO / "bridge/tox_callbacks.py"

# The pack script from scripts/pack_bootstrap_tox.md, verbatim in behaviour:
# one base COMP, three DATs, face ops baked in before the save (an Execute
# DAT's onStart may not run while a blocking script is in flight, so
# `ensure_ui` is called explicitly), then `comp.save`.
# `REPO` is the Wine-visible form: TD runs under Wine, where the repo is `z:\`.
PACK = r'''
import os

# `op`, `baseCOMP`, `textDAT`, `executeDAT` are injected as globals by
# TouchDesigner's own Text DAT / Execute DAT execution context, and NOT by
# execute_python: the first attempt failed with `name 'baseCOMP' is not
# defined`, and a second attempt's `from opdef import Op` failed with
# `No module named 'opdef'` -- there is no such module to import.
#
# The bridge's own scripts already solved this, in `ensure_ui` and
# `ensure_capture_viewer` (`bridge/tox_callbacks.py:167-176`, `:223-232`):
# try the bare global, and on NameError fall back to `getattr(td, name)`. The
# same fallback is used here rather than inventing a third mechanism.
import td

op = td.op
baseCOMP = getattr(td, "baseCOMP")
textDAT = getattr(td, "textDAT")
executeDAT = getattr(td, "executeDAT")

REPO = r"Z:\home\acorbeau\Repos\td-mcp-rs"
OUT = os.path.join(REPO, "crates", "tdmcp-daemon", "embedded", "bootstrap.tox")

with open(os.path.join(REPO, "bridge", "bootstrap.py"), encoding="utf-8") as f:
    boot_text = f.read()
with open(os.path.join(REPO, "bridge", "tox_callbacks.py"), encoding="utf-8") as f:
    cb_text = f.read()

root = op("/project1")
old = root.op("tdmcp_rs")
if old is not None:
    old.destroy()

comp = root.create(baseCOMP, "tdmcp_rs")
try:
    comp.par.externaltox = ""
except Exception:
    pass

boot = comp.create(textDAT, "bootstrap")
boot.text = boot_text
boot.nodeX, boot.nodeY = -200, 0

callbacks = comp.create(textDAT, "callbacks")
callbacks.text = cb_text
callbacks.nodeX, callbacks.nodeY = 0, 0

ex_dat = comp.create(executeDAT, "tdmcp_exec")
ex_dat.text = cb_text
ex_dat.nodeX, ex_dat.nodeY = 200, 0
ex_dat.par.active = False
ex_dat.par.start = True
ex_dat.par.create = True
ex_dat.par.framestart = True
ex_dat.par.exit = True
ex_dat.par.active = True

ns = {"parent": (lambda: comp), "__name__": "__pack_ensure__"}
exec(cb_text, ns, ns)
ns["ensure_ui"](comp)

result = {"saved": comp.save(OUT, createFolders=True), "size": os.path.getsize(OUT),
          "boot_len": len(boot_text), "cb_len": len(cb_text)}
'''


def main() -> int:
    before = OUT.stat().st_size if OUT.exists() else 0
    print(f"existing blob: {before} bytes")

    run_dir = REPO / "retroanalysis/campaign/pack-tox"
    session = harness.Session(
        data_dir=run_dir / "daemon-data", run_dir=run_dir
    )
    session.start_daemon()
    template = REPO / "crates/tdmcp-daemon/embedded/template.toe"
    import shutil

    staged = Path("/tmp/tdmcp-pack.toe")
    shutil.copyfile(template, staged)
    pid = session.spawn_td(project_path=str(staged))
    if not pid:
        print("spawn_td returned no pid")
        session.stop_daemon()
        return 2
    print(f"td pid {pid}; packing")
    reply = session.call("execute_python", {"pid": str(pid), "script": PACK}, timeout_s=120.0)
    print(json.dumps(reply)[:600])
    teardown = session.stop_td()
    session.stop_daemon()
    print(f"teardown: {json.dumps(teardown)}")

    if not OUT.exists() or OUT.stat().st_size <= 1024:
        print(f"FAIL: blob is {OUT.stat().st_size if OUT.exists() else 'absent'} bytes")
        return 1
    blob = OUT.read_bytes()
    print(f"wrote {OUT.stat().st_size} bytes (was {before})")
    for probe in (b"_bridge_shutting_down", b"_reconnect_watchdog", b"ensure_ui"):
        print(f"  contains {probe.decode()}: {probe in blob}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

#: What `scripts/pack_bootstrap_tox.md` says to build, and what the two shipped
#: blobs actually contained. The doc's "Live pack" recipe has never been run
#: against `template.toe`: the shipped template carried a 25655-byte
#: `callbacks.text` while the source was 31015 bytes, with none of the round-2
#: teardown gate in it. Every live campaign run therefore exercised pre-fix
#: callback code, and the trace hook added later could not fire from the
#: template's baked copy either.
#:
#: These are the checks that make the next stale template loud. They read the
#: EXPANDED form (`.toe.dir/.../*.text`), because a `.toe` is an opaque binary
#: whose bytes cannot be grepped -- measured: the 14522-byte shipped template
#: contains zero occurrences of the string "bootstrap".
def template_dat_sources(expanded: Path) -> dict[str, str]:
    """Each baked DAT body, keyed by DAT name. Only what was actually read.

    The KEYS are what the check iterates: a DAT missing from the expansion
    produces no entry, so "the template has no `callbacks` DAT at all" is a
    missing key rather than a silently-accepted empty body. Returning the
    SOURCE here instead would make every assertion vacuous -- a mapping from
    names to constants agrees with itself no matter what the template contains,
    which is exactly the failure this was written to prevent.
    """
    root = expanded / "project1" / "tdmcp_rs"
    bodies: dict[str, str] = {}
    for name in ("bootstrap.text", "callbacks.text", "tdmcp_exec.text"):
        path = root / name
        if path.is_file():
            bodies[name] = path.read_text(encoding="utf-8")
    return bodies


def template_is_current(expanded: Path) -> tuple[bool, str]:
    """Is every baked DAT body identical to the source it mirrors?

    The executable DAT is checked against the callbacks SOURCE rather than
    against the sibling DAT, because it is what actually runs: a template
    whose two DATs disagree executes code nobody reads.
    """
    bodies = template_dat_sources(expanded)
    expected = {
        "bootstrap.text": REPO / "bridge/bootstrap.py",
        "callbacks.text": REPO / "bridge/tox_callbacks.py",
        "tdmcp_exec.text": REPO / "bridge/tox_callbacks.py",
    }
    missing = sorted(name for name in expected if name not in bodies)
    if missing:
        return False, f"the template has no {', '.join(missing)}"
    stale = [
        f"{name} ({len(bodies[name])} bytes, source is {expected[name].stat().st_size})"
        for name in sorted(expected)
        if bodies[name] != expected[name].read_text(encoding="utf-8")
    ]
    if stale:
        return False, f"stale against the source: {', '.join(stale)}"
    return True, "every baked DAT mirrors its source"
