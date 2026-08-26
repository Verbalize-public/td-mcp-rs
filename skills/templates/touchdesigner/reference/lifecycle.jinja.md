# Lifecycle — spawn, own, and stop TouchDesigner deterministically

## Spawn

`spawn_td` launches TD and waits for **that exact pid's** bridge
handshake — connections from other instances never satisfy the wait.

Outcomes:

| Payload | Meaning | Next step |
| --- | --- | --- |
| `ok:true` + `handshake` | Ready; pid is addressable everywhere | operate normally |
| `spawn.blocked_by_dialog` | Startup modal is blocking the handshake | see {{ skill("popups") }} |
| `spawn.wait_timeout` | No handshake, no popups | check licence / project path; retry |
| `spawn.exited_early` | Process died before connecting | read `exitCode`; fix cause |

Fleet rows for spawned pids carry `spawn: {startedAt, exePath}` and may show
`bridge:"starting"` pre-handshake. Human-opened instances never claim
provenance.

## Kill

`kill_td` refuses pids that are neither registered nor
`TouchDesigner.exe`. Ladder:

1. `graceful` posts WM_CLOSE and waits `graceMs` — clean projects exit in
   seconds with no prompt.
2. Lingering? The timeout payload lists open popups — dismiss via
   `dialogs`, then retry graceful.
3. `mode:"force"` terminates unconditionally. Last resort.

## Startup popups

Opening a foreign-build project pops version/compat warnings. They are always
surfaced in spawn payloads — never auto-dismissed. Prefer fixing the
install/project build skew over dismissing forever (see {{ skill("popups") }}).
