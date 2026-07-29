# GUI wireframe — td-mcp-rs dashboard

Low-fidelity layout for the operator tray dashboard.

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
 Tray menu: Show / Hide · Restart daemon · Stop daemon · Quit
 Tray icon: normal (cyan) · attention (amber badge) when
 disconnected / resurrected / cancelled tasks present
 OS toasts: bridge disconnect, resurrection, cancelled tasks
```

## Controls

| Control | Behavior |
| --- | --- |
| Refresh | Poll `/admin/status` + `/admin/fleet` |
| Restart daemon | `POST /admin/restart` (respawn then exit) |
| Stop daemon | `POST /admin/shutdown` |
| Auto-refresh | ~2s |
| Tray Show / Hide | Toggle dashboard window visibility |
| Tray Quit | Close the GUI (daemon keeps running) |

## Sign-off

- [x] Layout matches this wireframe closely enough
- [x] Status + fleet table populate from a running daemon
- [x] Stop daemon works
- [x] Tray icon + context menu
- [x] Status-reflecting icon / tooltip
- [x] OS toast notifications on fleet edge transitions
- [x] Restart daemon via admin API
