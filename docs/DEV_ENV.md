# TD Dev Environment (dual-MCP harness)

Interactive harness for testing the TouchDesigner side of td-mcp-rs.

- **Fixtures:** [`fixtures/dev/`](../fixtures/dev/) — committed `e2e_kit.tox`, gitignored `session/`
- **Pack recipe:** [`scripts/pack_e2e_kit.md`](../scripts/pack_e2e_kit.md)
- **Live host:** `td-sandbox/toe/_agent_tdmcprs_dev/` (owned; never lab `:9981`)
- **Formal gate:** still [`E2E_CHECKLIST.md`](E2E_CHECKLIST.md) (core rows + per-feature sections). This doc is the day-to-day loop.

> **Platform note:** host paths below are from the original Windows dev host.
> On macOS/Linux substitute the repo location and the data-dir equivalents
> (`~/Library/Application Support/tdmcp-rs/` · `~/.local/share/tdmcp-rs/` — see
> [`CONFIG.md`](CONFIG.md) / [`DELIVERY.md`](DELIVERY.md)). On Linux TouchDesigner
> runs under Wine — see [`LINUX_SUPPORT.md`](LINUX_SUPPORT.md).

## Dual-MCP roles

| Surface | Owns |
| --- | --- |
| Classic TD MCP (`user-touchdesigner`) | Lifecycle, `COMP.save` / `loadTox`, pack/rebuild baseline, session snapshots |
| `user-tdmcp-rs` | Daemon under test: `fleet` → `pid` → `execute_python` / `inspect` / `capture` |
| Daemon | `tdmcp-daemon ensure` / Cursor upsert — health `http://127.0.0.1:9860/mcp/health` |

Do not invent sticky `targetId` for rs calls — **pid only**.

### Rebuild when the binary is locked

Cursor leaves two `tdmcp-daemon` processes (`start` + stdio `mcp`). UI Stop
only ends `start`; the leftover `mcp` locks `target/release` or `target/dist`
and blocks `cargo build`. Unlock before rebuild:

```text
# Windows
pwsh -File scripts/kill-daemons.ps1
# Unix
bash scripts/kill-daemons.sh

# or package path (kills, then rebuilds + copies to target/dist)
cargo run -p xtask -- dist
```

If Cursor MCP is still connected it may respawn `mcp` immediately — pause or
reload the MCP server when the lock returns after kill.

After editing `bridge/tdmcp_bridge/`, sync the package into `%LOCALAPPDATA%/tdmcp-rs/bridge/` and reload `/project1/tdmcp_rs` (destroy + `loadTox` bootstrap) so live TD picks up the change — same idea as `pack_bootstrap_tox` force re-extract.

## Kit layout

After load, expect:

| Path | Role |
| --- | --- |
| `/project1/e2e_kit` | Baseline COMP from `fixtures/dev/e2e_kit.tox` |
| `/project1/e2e_kit/probe` | Non-black Constant TOP |
| `/project1/e2e_kit/out1` | Out TOP (`probe` → `out1`) |
| `/project1/e2e_kit/zone` | Empty mutation-zone shell (interactive edits + session tox) |
| `/project1/tdmcp_rs` | Bootstrap dialer — **separate** `loadTox` of `%LOCALAPPDATA%/tdmcp-rs/bootstrap.tox` |

### Bootstrap Operator Viewer (`tdmcp_rs`)

Custom page **Bridge** (created at Start by `ensure_ui`):

| Par | Role |
| --- | --- |
| `Connect` | Desired connection (Off → disconnect, stop retries) |
| `Autoconnect` | Connect on start + rate-limited reconnect after loss |
| `Status` | Phase string (`Disconnected` / `Connecting` / `Re-connecting` / `Connected` / `Connected ([N] Tasks)`) |
| `Cancelqueued` | Pulse — drop **bridge-queued** tasks only (not in-flight `dispatch`, not daemon queue) |

Daemon idle heartbeat (`ping` every 5s; dead after 20s silence — the
separate 15s fleet-eviction TTL is not this clock) is answered on
the bridge worker thread and does **not** appear in `task_table`. If the daemon
dies or stops probing, Autoconnect moves Status to `Re-connecting` after the
bridge idle-dead timeout.

Face: color-banded Text TOP `status_top` (Operator Viewer) + ASCII panel in `status_text` (TASKS + **LOGS**). Children include `task_table` and `debug` (stdio ring buffer; Global OP Shortcut **`Debug`** on the COMP → `op.Debug.op('debug')`). Provoke rows with rs `execute_python` / `inspect` / `capture` under load. See [`scripts/pack_bootstrap_tox.md`](../scripts/pack_bootstrap_tox.md).

## Cold start

1. Confirm daemon health (`GET http://127.0.0.1:9860/mcp/health` → `{"ok":true}`).
2. Classic MCP — empty dest (first time) or existing toe:

   ```text
   create_td_project({ destDir: "<…>/td-sandbox/toe/_agent_tdmcprs_dev", name: "_agent_tdmcprs_dev" })
   start_td_project({ toePath: "<…>/td-sandbox/toe/_agent_tdmcprs_dev/_agent_tdmcprs_dev.toe" })
   ```

   If the project already exists, skip `create` and only `start_td_project`.
3. `get_td_info` — `projectFolder` must match the destDir (not lab).
4. Load kit (idempotent destroy + load):

   ```python
   import os
   REPO = r"C:\Users\corbe\Documents\Derivative\Projects\td-mcp-rs"
   KIT = os.path.join(REPO, "fixtures", "dev", "e2e_kit.tox")
   root = op("/project1")
   old = root.op("e2e_kit")
   if old is not None:
   	old.destroy()
   loaded = root.loadTox(KIT)
   result = {"path": loaded.path if loaded else None, "kit": KIT}
   ```

5. **Resume session** (optional): if `fixtures/dev/session/latest.tox` exists and the
   user wants resume, load into the zone:

   ```python
   import os
   REPO = r"C:\Users\corbe\Documents\Derivative\Projects\td-mcp-rs"
   SNAP = os.path.join(REPO, "fixtures", "dev", "session", "latest.tox")
   zone = op("/project1/e2e_kit/zone")
   # clear children then loadTox into zone
   for c in list(zone.children):
   	c.destroy()
   loaded = zone.loadTox(SNAP) if os.path.isfile(SNAP) else None
   result = {"resumed": loaded.path if loaded else None, "snap": SNAP, "exists": os.path.isfile(SNAP)}
   ```

6. Drop bootstrap (sibling of kit, not inside it):

   ```python
   import os
   BOOT = os.path.join(os.environ["LOCALAPPDATA"], "tdmcp-rs", "bootstrap.tox")
   root = op("/project1")
   old = root.op("tdmcp_rs")
   if old is not None:
   	old.destroy()
   loaded = root.loadTox(BOOT)
   result = {"path": loaded.path if loaded else None, "boot": BOOT}
   ```

7. rs MCP: `fleet` → pick pid whose project path matches `_agent_tdmcprs_dev` → smoke asserts.

## Dev smoke (baseline green)

Cheapest probes before interactive work. Does **not** replace the full E2E gate.

| # | Surface | Check |
| --- | --- | --- |
| 1 | Classic | `get_td_node_errors` on `/project1/e2e_kit` clean |
| 2 | rs | `fleet`: bridge `connected` for host pid |
| 3 | rs | `execute_python` with `result = 1` (scripts get `tdmcp_resolve`, not bare `op`) |
| 4 | rs | `capture` mode `top` on `/project1/e2e_kit/probe` — non-black |
| 5 | rs | `inspect` `paths:["/project1/e2e_kit"]` summary — child `{name, opType}` roster present |

## Interactive session

1. Mutate only under `/project1/e2e_kit/zone` (user in Designer or agent via classic MCP).
2. On “snapshot” / end of turn — classic `execute_python_script` using the session
   save snippet in [`scripts/pack_e2e_kit.md`](../scripts/pack_e2e_kit.md)
   → `fixtures/dev/session/latest.tox` + `latest.json`.
3. Do **not** `project.save()` unless asked.
4. Do **not** overwrite committed `e2e_kit.tox` unless regenerating the baseline on purpose
   (`scripts/pack_e2e_kit.md`).

## Resuming a session

1. Cold start steps 1–4.
2. Prefer `session/latest.tox` over an empty zone when the sidecar exists and
   resume is intended.
3. Re-drop bootstrap if `tdmcp_rs` is missing or `fleet` shows disconnected.
4. Re-run the dev smoke before making new claims.

## Safety

- Never lab `:9981` for this harness.
- Distinct from `_agent_tdmcprs_e2e*` (formal E2E hosts).
- Stop after 3 failed probes with no new evidence.
- Never edit `mcp_webserver_base.tox`, `modules/`, or `import_modules.py` in the owned host.
