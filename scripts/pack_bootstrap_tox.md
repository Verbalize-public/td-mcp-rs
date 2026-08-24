# Pack bootstrap `.tox`

Regenerate [`crates/tdmcp-daemon/embedded/bootstrap.tox`](../crates/tdmcp-daemon/embedded/bootstrap.tox) after changing [`bridge/bootstrap.py`](../bridge/bootstrap.py) or [`bridge/tox_callbacks.py`](../bridge/tox_callbacks.py).

## The four copies (read this before touching either `.py` file)

`bootstrap.tox` is TouchDesigner's opaque binary component format — nothing
outside TD can open, diff, or patch it. It is a **frozen snapshot** of the two
`.py` files above, baked in by the live-TD script below. That snapshot then
propagates through several more copies, each one only refreshed by an
explicit step — there is no auto-sync between any of them:

| # | What | Where | Refreshed by |
| --- | --- | --- | --- |
| 1 | Source | [`bridge/bootstrap.py`](../bridge/bootstrap.py), [`bridge/tox_callbacks.py`](../bridge/tox_callbacks.py) | You editing them (git-tracked, plain text, the only one worth reading a diff of) |
| 2 | Packed blob | [`crates/tdmcp-daemon/embedded/bootstrap.tox`](../crates/tdmcp-daemon/embedded/bootstrap.tox) | Re-running "Live pack" below, **inside TD** — the only place a `.tox` can be produced |
| 3 | Installed copy | `{data_dir}/bootstrap.tox` (e.g. `%LOCALAPPDATA%/tdmcp-rs/bootstrap.tox`) | `tdmcp-daemon install --force` / `ensure --force`, which re-extracts #2 |
| 4 | Baked into a project | Inside whatever `.toe` file a human dragged #3 into | Manually dragging a fresh #3 into that project again — **TD never re-reads this on its own**, not on reconnect, not on daemon restart |

**#1 and #2 silently drifting apart is exactly the failure mode `cargo test`
now catches**: `install::tests::bootstrap_tox_matches_packed_source_hash` in
`crates/tdmcp-daemon/src/install.rs` hashes #1 and compares it against a
sidecar file (`embedded/bootstrap.tox.source-hash`) recording what #1 looked
like the last time #2 was packed. If you change either `.py` file, that test
goes red until you repack (#2) and run `cargo run -p xtask -- stamp-tox`
(updates the sidecar). It cannot verify #2's *content* is correct — no tool
outside TD can — only that someone repacked *after* the source last changed.

**#4 has no automated guard and can't have one** — it lives inside a user's
own project file, which this repo doesn't and shouldn't touch. If you change
`bootstrap.py`/`tox_callbacks.py` in a way that matters at runtime (not just
comments), say so explicitly and tell the user which of their open TD
projects need a fresh `.tox` drag-in — don't assume `install --force` alone
fixes an already-open session.

## Graph

Inside a base COMP named `tdmcp_rs`:

| Operator | Type | Role |
| --- | --- | --- |
| `bootstrap` | Text DAT | Body = `bridge/bootstrap.py` |
| `callbacks` | Text DAT | Body = `bridge/tox_callbacks.py` (source mirror) |
| `tdmcp_exec` | Execute DAT | Same text as `callbacks`; pulses: Start, Create, Frame Start, Exit; Active on |

Runtime face (created by `ensure_ui` on Start — not required pre-baked):

| Operator | Type | Role |
| --- | --- | --- |
| `status_bg` | Constant TOP | Phase color band |
| `status_text` | Text DAT | Curated ASCII panel |
| `status_top` | Text TOP | Renders `status_text` |
| `status_face` | Composite TOP | bg under text — COMP Operator Viewer |
| `task_table` | Table DAT | `state \| method \| summarize \| age_s \| id` |
| `debug` | Text DAT | execute_python stdout/stderr ring buffer; face LOGS section; `op.Debug.op('debug')` |
| `capture_viewer` | OP Viewer TOP | Shared, retargeted per `capture` (`preview` / aliases); not the COMP face |

Global OP Shortcut **`Debug`** is claimed on the `tdmcp_rs` COMP by `ensure_ui` (skipped if another COMP already owns it).

Custom page **Bridge** (also created by `ensure_ui`):

| Par | Type | Role |
| --- | --- | --- |
| `Connect` | Toggle | Desired connection |
| `Autoconnect` | Toggle | Connect on start + reconnect after loss |
| `Status` | String (read-only) | Phase string |
| `Cancelqueued` | Pulse | Drop bridge-queued tasks (not in-flight) |

Do not name the Execute DAT `exec` (Python builtin / MCP result-key clash).

Clear `externaltox` on the COMP before save. Use `comp.save(path)` (not `saveTox`).

## Live pack (TD MCP)

1. Create/start an **owned** TD project (lease port, not lab `:9981`). Preferred: `td-sandbox/toe/_agent_tdmcprs_dev/`.
2. Run the pack script below via `execute_python_script`.
3. Confirm output size ≫ 1KB and file is binary (not the old ASCII placeholder).
4. Rebuild `tdmcp-daemon` so `include_bytes!` / `include_dir!` pick up the new tox and `bridge/`.
5. `cargo run -p xtask -- stamp-tox` — records the source hash so the
   drift-check test (`bootstrap_tox_matches_packed_source_hash` in
   `crates/tdmcp-daemon/src/install.rs`) goes green again. Skipping this
   step leaves the test correctly red.
6. Force re-extract (same semver skips refresh by default): `tdmcp-daemon install --force` (or `ensure --force`). Restart any running daemon so TD loads the new bridge.
7. If a TD project already has the **old** tox baked in (copy #4 in the table
   above — see e.g. a project you were live-testing against), drag the
   freshly-installed `.tox` into it again. `install --force` / a daemon
   restart does **not** reach an already-open project on its own.

```python
import os

REPO = r"C:\Users\corbe\Documents\Derivative\Projects\td-mcp-rs"  # adjust
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

# Bake face ops into the tox (Execute DAT onStart may not run while a
# blocking script is in-flight — call ensure_ui explicitly before save).
ns = {"parent": (lambda: comp), "__name__": "__pack_ensure__"}
exec(cb_text, ns, ns)
ns["ensure_ui"](comp)

result = {"saved": comp.save(OUT, createFolders=True), "size": os.path.getsize(OUT)}
```

## Drop-in use

After `tdmcp-daemon ensure` / `install`, drag `%LOCALAPPDATA%/tdmcp-rs/bootstrap.tox` (or the embedded copy) into a project. The tox only dials IPC and loads `bridge/` from disk — it does not embed the bridge package. On Start it builds the Operator Viewer face and Bridge parameters.
