# GUI Map — td-mcp-rs

Curated, hand-written map of the GUI. **Refactor complete** (iterations 1–4
shipped): tray popup + second-viewport dashboard. Last verified live:
2026-08 (debug daemon, real TD bridge attached).

---

## 1. What the GUI is today

Two surfaces in one process:

- **Dashboard window** (`src/dashboard.rs`, secondary egui viewport,
  980×660 resizable, decorated): sidebar nav Overview / Fleet / Logs /
  Settings. Everything lives here — status cards + latest errors, fleet
  sections with federation modals (`egui::Modal`), full log stream with
  filters/search/follow/pause, wide settings cards with sticky
  restart-needed bar.
- **Tray popup** (`lib.rs`, 380×320 frameless): launcher + compact
  summary — header actions (Stop/Restart/.tox/⤢ dashboard/≡ logs/⚙
  settings all open the dashboard), ATTENTION error strip (latest 3 from
  the error ring), share banner, MCP CLIENTS + TOUCHDESIGNER mini lists.
- Stack: `eframe`/`egui 0.35` + `tray-icon` + `notify-rust` + `reqwest`
  (blocking calls over throwaway current-thread tokio runtimes — known
  smell, unchanged).
- Process: lives inside `tdmcp-daemon`; disabled with `--no-gui` /
  `TDMCP_NO_GUI`. Dev/test hook: `TDMCP_OPEN_DASH=1|logs|fleet|settings`
  opens the dashboard on launch.
- Closing the popup or losing focus only **hides** it; the dashboard has
  a real close button. Real exit is Stop / `/admin/shutdown`.

## 2. File inventory

| Path | Role |
| --- | --- |
| `crates/tdmcp-gui/Cargo.toml` | crate manifest; lib `tdmcp_gui` consumed by daemon under `gui` feature |
| `crates/tdmcp-gui/src/lib.rs` | **everything else — 3498 lines in one file**: app struct, all views, tray, polling, scans, federation flows, wire types, platform shims, unit tests |
| `crates/tdmcp-gui/src/theme.rs` | 239 lines: Ableton-dark tokens, egui `Visuals`, shared widgets (`status_led`, `section_header`, `filled_button`, `ghost_button`) |
| `crates/tdmcp-gui/assets/icon-normal.png` / `icon-attention.png` | tray icons (32px variants + full-res window icon) |

Daemon-side launch point: `crates/tdmcp-daemon/src/main.rs` (~lines 584–623)
spawns the GUI thread after the admin listener is up; passes
`(admin_base, data_dir, quit, config_path)`.

## 3. Runtime topology & data flow

```
tdmcp-daemon ── spawns ──> GUI thread (eframe event loop, main thread)
                              │  polls every 2 s (lib.rs poll(), L777)
                              ▼
                    Admin HTTP API (axum, 127.0.0.1:9860)
                      GET  /admin/status            → StatusView
                      GET  /admin/fleet             → FleetView (TD processes)
                      GET  /admin/mcp-sessions      → SessionsView
                      GET  /admin/federation/slaves (master-only, bearer PSK)
                      POST /admin/shutdown | /admin/restart
                      GET  /admin/logs?limit=…      → log ring records
```

Federation routes live in `crates/tdmcp-daemon/src/federation.rs`
(`/admin/federation/status|register|fleet-push|slaves`). The GUI itself
calls some of these directly during add-slave / join-master flows
(`http_get_blocking` / `http_post_blocking`, lib.rs L3105–L3147).

Other threads/flows:

- **Subnet scan**: `std::thread` + `mpsc` channel (`scan_rx`), results
  drained each frame (L2734). Shared between "find slaves" and
  "find masters" via `ScanPurpose`.
- **Notifications**: OS toasts via `notify-rust` on bridge loss /
  resurrect / cancelled tasks / slave joined / startup reachability.
- **Tray attention**: icon swaps normal↔attention + tooltip text derived
  from fleet snapshot diffing (`apply_fleet_status`, L864).

### Smells worth fixing during the refactor

1. `poll()` does **blocking HTTP on the UI thread** — a fresh tokio
   current-thread runtime *per request*. Works because endpoints are fast;
   will jank under load.
2. Raw JSON kept as `String` (`fleet_json`, `sessions_json`, `slaves_json`)
   and re-parsed on demand.
3. One 3.5 k-file; views, transport, domain logic, platform glue all mixed.
4. Popup window management relies on timing hacks: click debounce (250 ms),
   focus-loss grace (400 ms), always-on-top clear-at, ignore-focus-loss-until.

## 4. State model (`DashboardApp`, lib.rs L174)

Grouped by concern:

- **Identity/wiring**: `admin_base`, `data_dir`, `config_path`, `quit`.
- **Navigation**: `view: View {Fleet, Settings, Logs}`, `fleet_panel:
  FleetPanel {None, AddSlave, SlaveSettings}`.
- **Poll snapshots**: `status: Option<StatusView>`, `fleet_json`,
  `sessions_json`, `slaves_json`, `prev_snapshot: FleetSnapshot`,
  `last_poll`, `error`, `fail_polls`.
- **Settings editing**: `draft: ConfigFile` + four path text buffers,
  `settings_loaded_snapshot` (for restart-needed diff), `needs_restart`,
  `settings_error`, PSK visibility toggles.
- **Window/tray lifecycle**: `visible`, `pending_initial_hide`,
  `pending_tray`, `tray: Option<TrayIcon>`, menu items, icon pair,
  `attention`, tray rect/debounce/focus-grace timestamps.
- **Federation flows**: add-slave host/port/psk/probe/message,
  `slave_settings_target` + timeouts, `confirm_go_standalone`,
  `confirm_turn_off_sharing`, `role_change_note`, `focus_master_psk`,
  known-slave id set + seen-once flag (join-toast suppression).
- **Scans**: `scan_results`, `scan_busy`, `scan_rx`, `scan_purpose`.
- **Logs**: `logs_view: LogsViewState` — client ring capped at 2048 rows,
  fetch limit 512/poll, level filter + text search state.

Wire DTOs (camelCase serde): `StatusView`, `SessionsView`/`SessionRow`,
`FleetView`/`FleetProc`, `LogRecordView`, plus local `SlaveRow`,
`FederationProbe`, `ScanHit`.

## 5. Views (what the user actually sees)

All views share the top header; body switches on `View`.

### Header (`draw_header`, L1034)
Orange LED · "td-mcp-rs · master|slave" · version (tooltip: pid + bind).
Right-anchored ghost buttons: `■` Stop, `↻` Restart, `.tox` reveal,
`⚙` Settings, `≡` Logs toggle.

### Fleet (default view)
- Error strip + share banner (`draw_share_banner`).
- `MCP CLIENTS` section (L1846): session rows — client name/version,
  connected-for duration.
- `TOUCHDESIGNER` section (L1918): fleet process rows grouped by owning
  daemon (`draw_fleet_groups` L1964, flat fallback `draw_flat_fleet`),
  LED per bridge state, resurrect/cancelled badges.
- Master actions (`draw_master_actions` L1936): Add Slave → overlay panel
  (`draw_add_slave_panel` L2416) with host/port/PSK + probe + LAN scan.
- Slave self-view (`draw_slave_self_view` L2609): master url/id, Go
  standalone confirm.
- Overlay: per-slave settings panel (`draw_slave_settings_panel` L2527).
- Empty state (`draw_empty_fleet` L2111), scan results panel
  (`draw_scan_results` L2126).

### Settings (`draw_settings`, L1139)
Toolbar (Save/Discard/Restart-needed bar L1617), then sections of
label-left/control-right rows (`settings_row` L2801, help tooltips from
`FIELD_DESCS`): SETTINGS (tray toggle), SERVER (port), NETWORK (bind,
auth mode), FEDERATION (role switcher, daemon_id, master_url, master_psk),
DAEMON (keep_alive, always_on, show_tray), BRIDGE (call/script/
heartbeat/pong/idle timeouts), ADVANCED (data_dir, bridge_dir,
catalog_path, daemon bin). Draft-vs-loaded diff drives the
"restart required" bar.

### Logs (`draw_logs`, L1697)
Toolbar (L1658): level chips + text filter + pause/auto-scroll +
reveal-dir; keyboard shortcuts (L1644). Rows render LED letter + time +
clipped message; client ring 2048, fetch 512 per poll while visible.

## 6. Design system (`theme.rs`)

"Ableton-dark": flat surfaces, hairline borders, orange used sparingly as
signal. Rounding is tokenized, not zero-everywhere.

| Token | Value | Use |
| --- | --- | --- |
| `BG_WINDOW` | `#131313` | popup background |
| `BG_PANEL` | `#1c1c1c` | stat cards / strips |
| `BG_ROW` / `BG_ROW_ALT` | `#1a1a1a` / `#1f1f1f` | cards / zebra rows (logs) |
| `BG_HOVER` / `BG_ACTIVE` | `#262626` / `#2e2e2e` | hover / pressed |
| `TEXT` / `TEXT_DIM` / `TEXT_FAINT` | `#e6e6e6` / `#7a7a7a` / `#555555` | text tiers |
| `ACCENT` | `#ff7a1a` | Ableton orange, ≤5% of frame |
| `OK` / `WARN` / `ERR` | `#5fd35f` / `#f0a830` / `#e85d5d` | status LEDs |
| `BORDER` / `BORDER_STRONG` | `#2a2a2a` / `#3a3a3a` | hairlines |

Scale: spacing `sp::{XS=4, SM=8, MD=12, LG=16, XL=24}`, radius
`RADIUS_SM=4` / `RADIUS_MD=6`, row height `ROW_H=26`, card padding
`CARD_PAD=12`.

Fonts: title 13 / label 12 / meta 11 proportional, mono 11 for ids/durations.
Widgets: `status_led`, `filled_button` (accent Save), `ghost_button`
(borderless icon actions), `card()` (bordered rounded container),
`row_between()` (justify-between flex row), `chip()` (filter toggle pill).
No shadows.

Platform glue: macOS `ActivationPolicy::Accessory` (menu-bar-only),
per-OS `reveal_in_file_manager` (explorer/open/xdg-open),
`applescript_escape` remnant, `toast()`/`notify()` wrappers.

## 7. Refactor north star (agreed direction)

1. **Keep the small tray UI** as the quick-glance surface (status, jump
   actions) — its size/behavior stays roughly as-is.
2. **Add a proper dashboard window**, opened like Docker Desktop's
   dashboard: a real resizable window with room for a much richer layout —
   sidebar navigation, real tables, detail panes, charts/log streams,
   settings forms that aren't 26-px-tall rows.
3. **Polish + modern feel** across both surfaces: consistent spacing scale,
   rounded corners where appropriate, hover/transition states, empty/loading
   states, keyboard access.
4. **Better UX**: non-blocking data fetching, clearer federation flows,
   searchable logs, actionable errors instead of a single red error line.

### Decisions (locked with user)

- **Architecture: (A) second egui viewport.** Dashboard is a decorated,
  resizable native window in the same process (`ctx.show_viewport_*`),
  reusing `theme.rs` tokens/widgets. No web stack.
- **Scope: dashboard gets everything** (Status+Fleet+Logs+Settings
  migrate there); the tray popup stays as launcher + **short summary**
  + **latest errors always visible** in the popup.

### Iteration order — ALL SHIPPED

1. ✅ Dashboard viewport scaffold (sidebar nav, Overview cards, error
   ring surfaced in popup ATTENTION strip + dashboard card); openers:
   tray menu item + header ⤢.
2. ✅ Logs tab: shared `LogsViewState`, level/src chips + client-side
   text search, follow/pause, click-to-expand detail with Ctrl+C copy,
   keyboard F/Space/Esc; popup Logs view deleted.
3. ✅ Settings tab: section cards + wide two-column rows
   (`row_wide`/`section_card`), toolbar Reset/Discard/Save, sticky
   restart-needed bar; federation flows (add-slave, slave settings) as
   `egui::Modal` overlays on Fleet tab; popup Settings deleted;
   `View` enum removed — popup is unconditional summary.
4. ✅ Polish: 120ms nav hover animation (`animate_bool_with_time`),
   first-poll spinner states, health LED tooltip with attention
   breakdown, share banner opens dashboard settings.
5. ✅ Pass 5 streamlining: popup rebuilt as a glance card — header is
   LED·title·version + ⚙/⤢ only (Stop/Restart/.tox actions moved into a
   new `daemon_card()` atop dashboard Overview); attention capped at 2
   rows from the ring, TD fleet capped at 4 (+N more), MCP reduced to a
   one-line name list; share banner replaced by conditional
   `share_applicable()` hint button; transient red error line, section
   strips and zebra deleted; default popup height 320→260. Theme gained
   the spacing/radius scale (`sp::*`, `RADIUS_SM/MD`, `ROW_H`,
   `CARD_PAD`) and shared helpers (`card()`, `row_between()`,
   `chip()`); dashboard stat cards, logs toolbar, errors card, modals
   and master-actions row moved onto them; `section_header` deleted.
6. ✅ Pass 6 glyph hygiene + tray: the launcher glyph `⤢` (U+2922) and
   arrows `→`/`←`, bullet `●` have **no glyph** in egui's bundled
   proportional fonts (Ubuntu-Light → NotoEmoji → emoji-icon-font; Hack
   only covers some, and only in Monospace) — they rendered as tofu
   squares. Launcher is now `⛶` U+26F6 (covered), arrow/bullet strings
   reworded or color-coded, and `theme::apply()` appends bundled "Hack"
   to the Proportional fallback so `← Back` renders. Tray builder sets
   `.with_menu_on_right_click(true)` explicitly (right-click context
   menu opens with 'Open dashboard' as its first item). Verification
   gotcha: at >100% display scale a non-DPI-aware PrintWindow captures
   only the top-left logical crop of the physical window — size the
   bitmap from a DPI-aware GetWindowRect before trusting "missing UI".

## 8. Remaining open items

1. macOS/Linux parity unverified (Windows-first development session);
   macOS Accessory policy may affect secondary viewports.
2. Blocking HTTP on the UI thread per poll (pre-existing smell) — worth
   moving to a background worker if jank ever shows under load.
3. Modal visuals verified compile-level + code-path only; one manual
   glance at Add-Slave/Slave-Settings modals recommended on next use.
