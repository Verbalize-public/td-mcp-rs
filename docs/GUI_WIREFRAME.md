# GUI wireframe — td-mcp-rs dashboard

Low-fidelity layout for the operator tray dashboard. The UI runs **in-process**
with `tdmcp-daemon` when the default `gui` Cargo feature is enabled (disable
with `--no-gui` / `TDMCP_NO_GUI=1`, or build `--no-default-features`).

## Design chart (Ableton vibe)

Dark-only. Flat. Sharp corners (0px radius). Orange is a *signal*, never fill mass
(≤5% of any frame). Status is conveyed by **LED dots**, not colored text.

| Token | Value | Role |
| --- | --- | --- |
| `bg_window` | `#131313` | popup fill |
| `bg_panel` | `#1c1c1c` | section header strips / chips |
| `bg_row` / `bg_row_alt` | `#1a1a1a` / `#1f1f1f` | zebra rows |
| `bg_hover` | `#262626` | hover only |
| `text` / `text_dim` / `text_faint` | `#e6e6e6` / `#7a7a7a` / `#555555` | primary / secondary / meta |
| `accent` / `accent_dim` | `#ff7a1a` / `#b85416` | brand + Restart |
| `ok` / `warn` / `err` | `#5fd35f` / `#f0a830` / `#e85d5d` | LED + Stop |
| `border` | `#2a2a2a` | hairlines |

Typography: title 13 bold · label 12 · meta/mono 11. Spacing on a 4px grid.
Row height 22–24px; section strip 20px; footer 36px. No shadows / elevation.

Native tray menu + OS toasts are unthemeable — keep the tray menu to Restart /
Stop only. Tray PNGs stay cyan mark + amber attention badge (brand, not UI).

## Layout

```text
┌─● td-mcp-rs · v0.1.0 · pid 4218        updated 2s ago ┐
│ ● MCP 2          ● TD 1 connected                     │
├──────────────────────────────────────────────────────┤
│ MCP CLIENTS                                           │
│ ●  a1b2…  Cursor              0.42.1      12m         │
│ ●  c3d4…  tdmcp-stdio-proxy   0.1.0       3m          │
├──────────────────────────────────────────────────────┤
│ TOUCHDESIGNER                                         │
│ ●  33   TD: project.toe     connected    1     0      │
│ ●  34   TD: project2.toe    resurrected  0     1      │
├──────────────────────────────────────────────────────┤
│                    [ Restart ]   [ Stop ]              │
└──────────────────────────────────────────────────────┘
 380px wide · content-height (max 600px) · 0px radius · flat
```

Empty states (centered meta text): `No MCP clients connected` /
`No TouchDesigner bridges`.

## Controls

| Control | Behavior |
| --- | --- |
| Auto-refresh | ~2s polls `/admin/status` + `/admin/fleet` + `/admin/mcp-sessions` |
| “updated Xs ago” | Replaces Refresh; dim meta text in header |
| Restart | `POST /admin/restart` (accent outline → fill on hover) |
| Stop | `POST /admin/shutdown` (err outline → fill on hover) |
| Startup | Tray icon + OS toast only — dashboard starts **hidden** |
| Tray left-click / double-click | **Toggle** popup (Docker-style); anchored near tray icon |
| Tray right-click | Context menu: Restart / Stop only (left-click does **not** open menu) |
| Focus loss / click-away | Hide popup (transient always-on-top ~150ms on show only) |
| Window chrome | Borderless (no OS title bar / controls); 1px theme border |
| Taskbar | No taskbar button while visible — tray icon only |
| Hide | Tray left-click toggle or focus loss — does **not** stop the daemon |

## Admin surfaces

| Endpoint | Purpose |
| --- | --- |
| `GET /admin/status` | `ok`, `version`, `pid`, `mcpSessionCount`, `bridgeCount` |
| `GET /admin/fleet` | TD bridge fleet |
| `GET /admin/mcp-sessions` | Live MCP client rows (`id`, `clientName`, `clientVersion`, `connectedAt`) |
| `POST /admin/mcp-sessions/annotate` | Stdio proxy renames a lease with IDE `clientInfo` |

## Sign-off

- [x] Compact Ableton-dark layout (LED chips, MCP + TD sections, Restart/Stop footer)
- [x] Status + fleet + MCP sessions populate from a running daemon
- [x] Auto-refresh only (no Refresh button / no editable Admin URL)
- [x] Tray left-click toggles; right-click Restart / Stop (menu not on left)
- [x] Borderless popup; no taskbar button (tray only)
- [x] Focus-loss auto-hide (Docker-style)
- [x] Status-reflecting icon / tooltip
- [x] OS toast notifications on fleet edge transitions + startup
- [x] Restart daemon via admin API
- [x] Tray/toasts start with the daemon by default; dashboard opens on demand
