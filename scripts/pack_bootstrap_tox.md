# Pack bootstrap `.tox`

Regenerate [`crates/tdmcp-daemon/embedded/bootstrap.tox`](../crates/tdmcp-daemon/embedded/bootstrap.tox) after changing [`bridge/bootstrap.py`](../bridge/bootstrap.py) or [`bridge/tox_callbacks.py`](../bridge/tox_callbacks.py).

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
5. Force re-extract (same semver does **not** refresh assets): delete `%LOCALAPPDATA%/tdmcp-rs/install.version`, then `tdmcp-daemon install`. Restart any running daemon so TD loads the new bridge. For a quick live check you can also copy `bridge/` into the data dir and reload the tox COMP.

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
