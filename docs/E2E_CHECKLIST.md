# Live TD acceptance checks

Use a scratch project and an explicit PID. Follow [the dev setup](DEV_ENV.md)
after [automated checks](TESTING.md) pass. Reinstall edited bridge files with
`tdmcp-daemon install --force`; this preserves existing configuration.

Record the commit, OS, daemon/TD versions, PID, requests, results and screenshots
in the release or review evidence. Old pass marks are not evidence for a new
build. Run the sections affected by a change; a full release needs every
applicable section and native installer checks on each supported OS.

## Startup and lifecycle

- Install twice: configuration and custom template remain intact.
- Spawn a clean template; `fleet` reports the connected PID, title and project
  path. No unrelated fixtures or operator errors are present.
- Connect two TD processes and address each independently.
- Stop an owned TD process while idle and during a call. Fleet reports the
  loss, pending work gets a diagnostic, and stale entries expire.
- Reconnect the same surviving PID; a later read succeeds without spawning
  a duplicate. Test graceful close and force close only on scratch processes.

## Editing and inspection

- Create, set values/expressions/flags, connect, disconnect and delete nodes.
  Inspect the resulting type, parameter mode/expression, wiring and errors.
- Fail in the middle of a batch. Earlier changes remain, later dependent
  steps are skipped, and `applied`/`failedAt` identify the boundary.
- Create at an occupied path. Verify the renamed node is reported and later
  steps address that new node, leaving the original occupant untouched.
- Inspect several paths including a missing path. Check inline errors,
  direct-child rosters, parameter values and capped DAT/shader previews.
- Select nodes in two editor panes; `editor_context` identifies the focused
  pane, owner path, selection and current node.
- Run Python with a result, printed output, `includeLogs:false`, and an
  exception. Check results, optional logs and actionable error locations.

## Images and response limits

- Capture a changing TOP and a non-TOP viewer; inspect the actual images.
- Capture intentional black and solid-color TOPs. A valid PNG succeeds with
  an advisory classification; a uniform image is not automatically a failure.
- Capture CHOP data, an empty CHOP and a wrong-family target. Check shape,
  limits and diagnostics. No temporary conversion operators remain.
- Run [the live response-limit smoke](../scripts/live_bridge_limits_smoke.py):
  `python scripts/live_bridge_limits_smoke.py PID /path/to/output`.
  Invalid values and a 33 MiB reply must return errors while later calls work.

## Settings, federation and dashboard

- Save, discard and reset drafts. Validation errors remain visible; failed
  saves keep edits. Reset preserves this computer's identity.
- Save federation roles, coordinator address/key, call budgets and keep-alive.
  These apply without restarting. Listener/process changes show an explicit
  restart prompt; restart returns on the saved address without losing config.
- Check a rejected coordinator key, then repair it and join successfully.
  Remote fleet, inspect, Python and image capture target the intended computer.
- Run [the live federation smoke](../scripts/live_federation_smoke.py):
  `python scripts/live_federation_smoke.py PID /path/to/output`.
  Use a standalone development daemon; the script restores its settings.
- Click through Settings, Federation, Logs and Palette at normal and narrow
  window sizes/high DPI. Save and navigation remain reachable; slow requests
  do not freeze the interface or erase drafts.

## Reliability and logs

- Keep a slow script in flight while another client reads fleet. Session-busy
  and exclusive-queue errors must not freeze unrelated requests.
- Change a call budget live, exercise a timeout, then make a successful call.
  A timeout does not prove cancellation: inspect state before retrying writes.
- Restart the daemon while an MCP client remains open; reconnect and verify
  later calls. Routine disconnects should not produce repeated tracebacks.
- Print inside and outside a tool call. Check the TD face log and central
  Logs tab, source/PID attribution, filtering and cursor resume after hiding
  the window. Interleaved logs must not break a tool response.

## Project files, palette and OS integration

- Unpack, pack, lint and install the bridge into **copies** of both a project
  and component. Verify backups, existing-bridge replacement, missing-bridge
  insertion and a real handshake from the resulting project.
- Scan/list/probe palette entries, save/read a card, and place a component.
  Verify its interface, settings and wires; the probe leaves no scratch nodes.
- Check unknown IDs, ignored/corrupt entries and failed probes. Bulk analysis
  must advance through explicit IDs, and interruption must remain recoverable.
- Check palette thumbnails and filters. Missing/flat thumbnails do not erase
  a successful digest; flat palette thumbnails are not stored.
- On each native OS, install/upgrade/uninstall the packaged app; check paths,
  shortcuts/tray, autostart and configuration preservation. Verify signing or
  notarization when configured. Linux/Wine needs its configured runner.
- On Windows/macOS, check supported modal discovery, explicit dismissal and
  recovery. macOS requires Accessibility permission; Linux dialog automation
  is unavailable. Do not call TD UI APIs from worker threads to manufacture
  dialogs: this can deadlock TD and create undismissable windows.
