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
| `bg_panel` | `#1c1c1c` | section header strips |
| `bg_row` / `bg_row_alt` | `#1a1a1a` / `#1f1f1f` | zebra rows |
| `bg_hover` | `#262626` | hover only (ghost buttons + rows) |
| `text` / `text_dim` / `text_faint` | `#e6e6e6` / `#7a7a7a` / `#555555` | primary / secondary / meta |
| `accent` | `#ff7a1a` | brand + Restart hover |
| `ok` / `warn` / `err` | `#5fd35f` / `#f0a830` / `#e85d5d` | LED + Stop hover |
| `border` | `#2a2a2a` | hairlines |

Typography: title 13 bold · label 12 · meta/mono 11. Spacing on a 4px grid.
Row height 22–24px; section strip 20px. No shadows / elevation.

Native tray menu + OS toasts are unthemeable — keep the tray menu to Restart /
Stop only. Tray PNGs stay cyan mark + amber attention badge (brand, not UI).

## Layout

```text
┌─● td-mcp-rs · v0.1.0 · pid 4218       ⚙  .tox  ↻  ■ ┐
├──────────────────────────────────────────────────────┤
│ MCP CLIENTS                                           │
│ ●  a1b2…  Cursor              0.42.1      12m         │
│ ●  c3d4…  tdmcp-stdio-proxy   0.1.0       3m          │
├──────────────────────────────────────────────────────┤
│ TOUCHDESIGNER                                         │
│ ●  33   TD: project.toe              connected  1  0 │
│ ●  34   TD: longer-name.toe          connected  0  1 │
└──────────────────────────────────────────────────────┘
 380px wide · content-height (max 600px) · 0px radius · flat
```

Empty states (centered meta text): `No MCP clients connected` /
`No TouchDesigner bridges`.

Header actions are **ghost** (borderless): transparent at rest, `bg_hover` +
accent/err text on hover. Right-anchored cluster: gear `⚙` · `.tox` · restart
`↻` · stop `■`.

Settings is an **in-popup view** (same viewport): Server / Daemon / Advanced
sections + Save / Discard / Reset / Back. Edits write `config.toml`; apply after
restart.

TD bridge status fills the space between title and task counts and is
**right-aligned** so its X does not shift with title length.

## Controls

| Control | Behavior |
| --- | --- |
| Auto-refresh | ~2s polls `/admin/status` + `/admin/fleet` + `/admin/mcp-sessions` |
| Gear `⚙` | Open Settings view (edit TOML-backed config; Save/Discard/Reset) |
| `.tox` | Reveal `data_dir/bootstrap.tox` in the file manager (Explorer `/select` on Windows) |
| Restart `↻` | `POST /admin/restart` (ghost; accent on hover) |
| Stop `■` | `POST /admin/shutdown` → quit flag + cancel serve (ghost; err on hover); process ends after drain |
| Startup | Tray icon + OS toast only — dashboard starts **hidden** |
| Tray left-click | **Toggle** popup (Docker-style); flush to taskbar edge; ignore DoubleClick |
| Tray right-click | Context menu: Restart / Stop only (left-click does **not** open menu) |
| Focus loss / click-away | Hide popup on Fleet view; Settings stays open while editing; tray click that caused focus-loss does **not** reopen |
| Window chrome | Borderless (no OS title bar / controls); 1px theme border |
| Taskbar | No taskbar button while visible — tray icon only |
| Hide | Tray left-click toggle or focus loss — does **not** stop the daemon |

`/admin/restart` spawns the replacement with the same detached flags as `ensure`
(null stdio + Windows `DETACHED_PROCESS`; `CREATE_NO_WINDOW` only when `--no-gui`)
so a console window does not flash.

## Admin surfaces

| Endpoint | Purpose |
| --- | --- |
| `GET /admin/status` | `ok`, `version`, `pid`, `mcpSessionCount`, `bridgeCount` |
| `GET /admin/fleet` | TD bridge fleet |
| `GET /admin/mcp-sessions` | Live MCP client rows (`id`, `clientName`, `clientVersion`, `connectedAt`) |
| `POST /admin/mcp-sessions/annotate` | Stdio proxy renames a lease with IDE `clientInfo` |

## Sign-off

- [x] Compact Ableton-dark layout (LED rows, MCP + TD sections, ghost header actions)
- [x] Status + fleet + MCP sessions populate from a running daemon
- [x] Auto-refresh only (no Refresh / “updated Xs ago” / summary chips)
- [x] Tray left-click toggles; right-click Restart / Stop (menu not on left)
- [x] Borderless popup; no taskbar button (tray only); flush to dock edge
- [x] Focus-loss auto-hide without tray-click reopen blink
- [x] Status-reflecting icon / tooltip
- [x] OS toast notifications on fleet edge transitions + startup
- [x] Restart daemon via admin API (detached spawn, no console flash)
- [x] `.tox` reveal opens install `bootstrap.tox` in the file manager
- [x] Tray/toasts start with the daemon by default; dashboard opens on demand
