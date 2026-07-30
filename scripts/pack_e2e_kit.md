# Pack E2E kit `.tox`

Regenerate [`fixtures/dev/e2e_kit.tox`](../fixtures/dev/e2e_kit.tox) after changing
the kit graph (probe / out / zone layout). Agent runbook:
[`docs/DEV_ENV.md`](../docs/DEV_ENV.md).

## Graph

Inside a base COMP named `e2e_kit`:

| Operator | Type | Role |
| --- | --- | --- |
| `probe` | Constant TOP | Non-black solid (mid gray) for `capture` asserts |
| `out1` | Out TOP | Face / preview terminal (`probe` → `out1`) |
| `zone` | base COMP | Empty mutation-zone shell for interactive edits |

Do **not** bake `tdmcp_rs` (bootstrap) into this tox — drop it separately from
`%LOCALAPPDATA%/tdmcp-rs/bootstrap.tox`.

Clear `externaltox` on the COMP before save. Use `comp.save(path)` (not `saveTox`).

## Live pack (classic TD MCP)

1. Create/start an **owned** TD project (never lab `:9981`). Preferred host:
   `td-sandbox/toe/_agent_tdmcprs_dev/`.
2. Run the pack script below via `execute_python_script`.
3. Confirm the file is a binary tox (not ASCII placeholder). A minimal kit may be
   only a few hundred bytes; bootstrap tox is larger because it embeds Text DATs.
4. Commit `fixtures/dev/e2e_kit.tox` only when intentionally refreshing the baseline.

```python
import os

REPO = r"C:\Users\corbe\Documents\Derivative\Projects\td-mcp-rs"  # adjust
OUT = os.path.join(REPO, "fixtures", "dev", "e2e_kit.tox")

root = op("/project1")
old = root.op("e2e_kit")
if old is not None:
	old.destroy()

kit = root.create(baseCOMP, "e2e_kit")
try:
	kit.par.externaltox = ""
except Exception:
	pass

probe = kit.create(constantTOP, "probe")
probe.par.colorr = 0.45
probe.par.colorg = 0.45
probe.par.colorb = 0.5
probe.nodeX, probe.nodeY = -200, 0

out1 = kit.create(outTOP, "out1")
out1.nodeX, out1.nodeY = 0, 0
out1.inputConnectors[0].connect(probe)

zone = kit.create(baseCOMP, "zone")
zone.nodeX, zone.nodeY = 200, 0
try:
	zone.par.externaltox = ""
except Exception:
	pass

# COMP Operator Viewer face → out1
try:
	kit.viewer = True
	kit.par.opviewer = out1
except Exception:
	pass

os.makedirs(os.path.dirname(OUT), exist_ok=True)
result = {"saved": kit.save(OUT, createFolders=True), "size": os.path.getsize(OUT), "path": OUT}
```

## Session snapshot (not baseline)

Interactive user/agent work under `/project1/e2e_kit/zone` should save to
`fixtures/dev/session/latest.tox` (gitignored), not overwrite `e2e_kit.tox`:

```python
import json, os, time

REPO = r"C:\Users\corbe\Documents\Derivative\Projects\td-mcp-rs"
SESSION_DIR = os.path.join(REPO, "fixtures", "dev", "session")
TOX = os.path.join(SESSION_DIR, "latest.tox")
META = os.path.join(SESSION_DIR, "latest.json")
NOTE = "session snapshot"  # adjust

zone = op("/project1/e2e_kit/zone")
os.makedirs(SESSION_DIR, exist_ok=True)
ok = zone.save(TOX, createFolders=True)
with open(META, "w", encoding="utf-8") as f:
	json.dump(
		{"savedAt": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()), "note": NOTE, "sourcePath": zone.path},
		f,
		indent=2,
	)
result = {"saved": ok, "path": TOX, "size": os.path.getsize(TOX) if ok else 0}
```
