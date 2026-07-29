# GUI0 wireframe — td-mcp-rs dashboard

Low-fidelity layout for Gate GUI0 sign-off **before** polishing.

```text
┌──────────────────────────────────────────────────────────────┐
│ td-mcp-rs          Admin: [http://127.0.0.1:9860] [Refresh] │
│                     [Stop daemon]                             │
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
├──────────────────────────────────────────────────────────────┤
│ Task history = live queue + cancelled stack (per selected    │
│ row in a later iteration). Kill/restart: Stop daemon for v0; │
│ per-connection kill deferred until lifecycle P2.             │
└──────────────────────────────────────────────────────────────┘
 Tray: Show / Hide window · Quit
```

## Controls (v0)

| Control | Behavior |
| --- | --- |
| Refresh | Poll `/admin/status` + `/admin/fleet` |
| Stop daemon | `POST /admin/shutdown` |
| Auto-refresh | ~2s |

## Out of scope for first visual pass

- Per-pid process kill (needs OS APIs + safety)
- Full tray menu (crate dependency present; UX follow-up)
- Dark/light theme polish

## Sign-off

- [ ] Layout matches this wireframe closely enough
- [ ] Status + fleet table populate from a running daemon
- [ ] Stop daemon works
