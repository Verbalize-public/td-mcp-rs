# Lifecycle — spawn, own, and stop TouchDesigner deterministically

## Spawn

`spawn_td` launches TD and waits for **that exact pid's** bridge
handshake — connections from other instances never satisfy the wait.

Args are all optional: `exePath` (wins) / `installId` (from `td_installs`) /
`projectPath` / `args[]` / `waitTimeoutMs` (default **60000**).

Outcomes. Non-handshake outcomes come back as a **successful tool result with
`ok:false`** — read the `outcome` field, not a diagnostic code:

| Payload | Meaning | Next step |
| --- | --- | --- |
| `{ok:true, pid, handshake:{title,toePath}, startupDialogs?}` | Ready; pid is addressable everywhere | operate normally |
| `{ok:false, outcome:"wait_timeout", stillAlive, startupDialogs}` **with** `startupDialogs` non-empty | A startup modal is blocking the handshake | dismiss via `dialogs`, then poll `fleet` — see [`popups`](./popups.md) |
| `{ok:false, outcome:"wait_timeout", stillAlive}` with no dialogs | Process exists but has not connected | Check licence, project path and launcher; poll this pid instead of spawning again |
| `{ok:false, outcome:"exited_early", exitCode, pid}` | Process died before connecting | read `exitCode`; fix cause |

Hard failures never reach the table above — they are real diagnostics:
`tdmcp.spawn.exe_incomplete` (no complete install, or the chosen one is a stub
without toeexpand/toecollapse) and `tdmcp.spawn.spawn_failed` (OS refused the
launch, or that pid is already registered connected).

After a `wait_timeout` with `stillAlive:true`, **do not call `spawn_td` again** —
that TD is still running and you would start a second one. Dismiss the blocker
with `dialogs`, then poll `fleet` until that pid reads `bridge:"connected"`.

Fleet rows for spawned pids carry `spawn: {startedAt, exePath}` and may show
`bridge:"starting"` pre-handshake. Human-opened instances never claim
provenance.

## Kill

`kill_td` refuses pids that are neither registered nor
`TouchDesigner.exe` (`tdmcp.kill.not_td_pid`). Args: `pid` (required),
`mode` (default **`graceful`**), `graceMs` (default **5000**).

Ladder:

1. `graceful` requests an OS close and waits `graceMs` — clean projects exit in
   seconds with no prompt. Returns `{ok:true, pid, how:"graceful"}`.
2. Lingering? `tdmcp.kill.graceful_timeout`; the payload lists open popups —
   an unsaved-work prompt is the usual cause. Decide about the work, dismiss
   via `dialogs`, then retry graceful.
3. `mode:"force"` terminates unconditionally — `{ok:true, pid, how:"force"}`.
   Last resort; unsaved work is lost. `tdmcp.kill.access_denied` means
   elevation/UIPI blocked it.

On Linux, TD runs under Wine. Instances started through `spawn_td` report
their Linux process id; use that returned pid. Graceful stop sends SIGTERM,
and force stop sends SIGKILL. Dialog automation is unavailable under Wine.
If an installation needs a custom Wine runner or environment, configure
`official_tools.wine_exe` with that runner or a wrapper accepting
`<TouchDesigner.exe> [project]` arguments.

## Startup popups

Opening a foreign-build project pops version/compat warnings. They are always
surfaced in spawn payloads — never auto-dismissed. Prefer fixing the
install/project build skew over dismissing forever (see [`popups`](./popups.md)).

## Related

- Startup / save-prompt modals: [`popups`](./popups.md)
- Preparing the project you spawn: [`project-io`](./project-io.md)
- Busy / queue errors once connected: [`tooling-concurrency`](./tooling-concurrency.md)

---

**Canonical:** [`lifecycle`](./lifecycle.md)