# GUI wireframe — td-mcp-rs dashboard

Low-fidelity layout for the operator tray dashboard. The UI runs **in-process**
with `tdmcp-daemon` when the default `gui` Cargo feature is enabled (disable
with `--no-gui` / `TDMCP_NO_GUI=1`, or build `--no-default-features`).

```text
┌──────────────────────────────────────────────────────────────┐
│ td-mcp-rs          Admin: [http://127.0.0.1:9860] [Refresh] │
│                     [Restart daemon] [Stop daemon]            │
├──────────────────────────────────────────────────────────────┤
│ Daemon status                                                │
│   ok / version / daemon pid                                  │
├──────────────────────────────────────────────────────────────┤
│ Connections (fleet)                                          │
│ ┌─────┬────────────────────┬─────────────┬───────┬─────────┐ │
│ │ pid │ title              │ bridge      │ tasks │ cancel  │ │
│ ├─────┼────────────────────┼─────────────┼───────┼─────────┤ │
│ │ 33  │ TD: project.toe    │ connected   │ 1     │ 0       │ │
│ │ 34  │ TD: project2.toe   │ resurrected │ 0     │ 1       │ │
│ └─────┴────────────────────┴─────────────┴───────┴─────────┘ │
└──────────────────────────────────────────────────────────────┘
 Tray menu: Show / Hide · Restart daemon · Stop daemon
 Tray icon: normal (cyan) · attention (amber badge) when
 disconnected / resurrected / cancelled tasks present
 OS toasts: startup, bridge disconnect, resurrection, cancelled tasks
```

## Controls

| Control | Behavior |
| --- | --- |
| Refresh | Poll `/admin/status` + `/admin/fleet` |
| Restart daemon | `POST /admin/restart` (clear lock → respawn → exit) |
| Stop daemon | `POST /admin/shutdown` (ends the whole process) |
| Auto-refresh | ~2s (while process is alive; window may be hidden) |
| Startup | Tray icon + OS toast only — dashboard window starts **hidden** |
| Tray Show / click | Open dashboard |
| Tray Hide / window close (X) | Hide only — does **not** stop the daemon |

## Sign-off

- [x] Layout matches this wireframe closely enough
- [x] Status + fleet table populate from a running daemon
- [x] Stop daemon works
- [x] Tray icon + context menu
- [x] Status-reflecting icon / tooltip
- [x] OS toast notifications on fleet edge transitions + startup
- [x] Restart daemon via admin API
- [x] Tray/toasts start with the daemon by default; dashboard opens on demand
