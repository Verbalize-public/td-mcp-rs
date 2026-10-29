# Development with real TouchDesigner

Use a scratch project for live testing. Automated tests use fake TD peers and
cannot prove that an actual network cooks or renders correctly.

## Choose the right validation

For a docs-only change, start with the documentation checks in [Testing](TESTING.md#documentation-only-changes).
For code, run the workspace build and relevant automated tests before live TD
checks. For behavior that depends on cooking, rendering, dialogs, or timing,
use an isolated TD project and record what you actually observed.

## Rebuild and install

Build and test without stopping user-owned processes:

```sh
cargo build --workspace
python scripts/package_smoke.py target/debug/tdmcp-daemon
```

The package smoke uses its own temporary data/config directories and listener
ports. For live testing, similarly isolate data/config, HTTP and bridge ports,
and use a task-owned scratch project. Never stop all daemons or TD instances by
executable name. Before deployment to an existing installation, identify the exact
PID, executable, listener and connected projects; coordinate consequential
interruption and preserve configuration/customized templates. Use a scoped
`install --force` only when that installation is authorized for refresh.

Record source/build/installed identities separately. Rebuilding does not refresh
an already-running process, and embedded skill resources may remain stale in an
old proxy even after files change. Verify the actual served resources and loaded
bridge after any authorized restart. Stop after three failed probes with no new
evidence; inspect ownership, logs and file locks rather than broadening cleanup.

## Probe through MCP

Use your connected assistant, or the standard-library client:

```sh
python scripts/mcp_probe.py fleet
python scripts/mcp_probe.py inspect '{"pid":123,"paths":["/project1"]}'
python scripts/mcp_probe.py --resource tdmcp://docs/operate
```

Replace 123 with the discovered PID. `TDMCP_DAEMON_URL` selects the HTTP origin
and `TDMCP_PSK` supplies a key. The client uses real MCP initialization,
sessions, and tool calls—not the daemon's compatibility tools endpoint.

## Manual stdio when the harness connection is unavailable

Use the executable directly, without changing the harness profile:

```sh
python scripts/mcp_stdio_probe.py --binary target/debug/tdmcp-daemon fleet
python scripts/mcp_stdio_probe.py --binary target/debug/tdmcp-daemon capture '{"pid":123,"path":"/project1/out1","maxSize":256}' --out capture-result.json
```

Replace the example PID with the confirmed owned target. This uses the actual
MCP initialize/notification/tools-call protocol over a short-lived proxy process;
the daemon is retained. Calls are sequential and never automatically replayed
after a timeout. An existing output path is rejected before dispatch. The JSON
result preserves image content but saving JSON alone is not visual verification.
Use this route only when authorized for the target and operation.

The known-issues regression sources and per-stage expectations are in
[scripts/fixtures/known_issues](../scripts/fixtures/known_issues/README.md).

## Live acceptance

Start a scratch .toe, confirm its bridge connects, then inspect, mutate a small
owned subtree, read it back, and capture a non-uniform output. Check operator
errors and clean up the test subtree. For lifecycle changes, test stop/restart
and ensure there are no duplicate surviving processes.

For federation changes, use two daemon instances with separate config/data,
HTTP ports, and bridge ports. Verify a remote tool call with both PID and
daemon ID; restore roles and terminate test daemons afterward.

For an already-connected scratch PID, the repeatable test is
`python scripts/live_federation_smoke.py 123 /project1/output`. It temporarily
joins the local standalone daemon to an isolated coordinator, checks key
rejection and remote inspect/Python/capture, then restores the original role.

Record what was actually observed, including failures and untested platforms.
The [E2E checklist](E2E_CHECKLIST.md) covers less common operations.

## Embedded assets

`bridge/tdmcp_bridge/` loads from the installed filesystem on bridge startup;
refresh the install and reconnect TD after edits.

`bridge/bootstrap.py` and `bridge/tox_callbacks.py` are different: they are
baked into the opaque embedded tox. Read and follow
[pack_bootstrap_tox.md](../scripts/pack_bootstrap_tox.md) before changing them.
Do not bypass its hash drift test.

Skills have a checked-in plugin render; follow [skills/README.md](../skills/README.md).
For Linux launchers and PID behavior, see [Linux/Wine](LINUX_SUPPORT.md).
