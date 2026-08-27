# GUI Map — td-mcp-rs

Curated, hand-written map of the GUI. **Refactor complete** (iterations 1–4
shipped, then the 2026-08 overhaul: Overview+Fleet merged into one Overview
page, monolith split into modules, design-system v2 — spec in
`docs/GUI_OVERHAUL_PLAN.md`). Last verified live: 2026-08
(preview-harness screenshots + real daemon smoke).

---

## 1. What the GUI is today

Two surfaces in one process:

- **Dashboard window** (`src/dashboard.rs` + `src/dashboard/*`, secondary
  egui viewport, 960×640 resizable): sidebar nav **Overview / Logs /
  Settings** (Fleet was merged into Overview). Overview = daemon strip,
  stat tiles, TOUCHDESIGNER fleet card (groups by machine, master actions,
  slave self-view), MCP CLIENTS card, ACTIVITY/errors card, federation
  modals on top.
- **Tray popup** (`src/popup.rs`, frameless glance card): launcher +
  compact summary — `⛶` dashboard / `⚙` settings, ATTENTION strip,
  TOUCHDESIGNER mini list, MCP client names, plus a pinned **action footer**
  (Stop / Restart / Reveal .tox) since pass 10. The body stays read-only; the
  footer is the one mutation surface.
- Stack: `eframe`/`egui 0.35` + `tray-icon` + `notify-rust` + `reqwest`
  (blocking calls with 2 s/3 s timeouts over throwaway current-thread tokio
  runtimes).
- Process: lives inside `tdmcp-daemon`; disabled with `--no-gui` /
  `TDMCP_NO_GUI`. Dev/test hook: `TDMCP_OPEN_DASH=1|logs|settings`
  (`fleet` kept as a legacy alias for Overview) opens the dashboard.
- Closing the popup or losing focus only **hides** it; the dashboard has
  a real close button. Real exit is Stop (two-step confirm) /
  `/admin/shutdown`.

## 2. File inventory

| Path | Role |
| --- | --- |
| `crates/tdmcp-gui/Cargo.toml` | crate manifest; lib `tdmcp_gui` consumed by daemon under `gui` feature; dev-only `preview` feature |
| `crates/tdmcp-gui/src/lib.rs` | entry point: `run()`, module map; public surface is exactly `run` + `toast` |
| `crates/tdmcp-gui/src/app.rs` | `DashboardApp` state/logic core: poll loop, fleet-snapshot diffing, settings save/dirty, snackbars, eframe App tick |
| `crates/tdmcp-gui/src/tray.rs` | tray icon assets/build/click routing, popup positioning near tray |
| `crates/tdmcp-gui/src/popup.rs` | glance card (header + summary + action footer) |
| `crates/tdmcp-gui/src/wire.rs` | admin-API DTOs + display mappers (level colors/letters, id tails, clip) |
| `crates/tdmcp-gui/src/http.rs` | blocking HTTP helpers (bounded timeouts), LAN subnet scan, local-IP helpers |
| `crates/tdmcp-gui/src/platform.rs` | OS toasts, file-manager reveal |
| `crates/tdmcp-gui/src/federation.rs` | add-slave one-click pipeline, per-slave settings, Go-standalone, scan results UI |
| `crates/tdmcp-gui/src/theme.rs` + `theme/widgets.rs` | Ableton-dark tokens/fonts/visuals + widget kit (`badge`, `banner`, `segmented`, `empty_state`, LEDs w/ pulse, `filled_button`/`action_button`/`ghost_button`, cards, chips) |
| `crates/tdmcp-gui/src/dashboard.rs` + `dashboard/{nav,overview,fleet,logs,settings,widgets}.rs` | dashboard shell (sidebar/topbar/router/snackbars) + pages + shared painted pieces (`stat_card`, `fleet_row`, `card_with_header`, `daemon_actions`, `capped_rows`) |
| `crates/tdmcp-gui/src/preview.rs` + `examples/dashboard_preview.rs` | fixture harness for pixel verification (`--features preview`; scenes listed in the plan doc) |
| `crates/tdmcp-gui/assets/icon-normal.png` / `icon-attention.png` | tray icons |

Daemon-side launch point: `crates/tdmcp-daemon/src/main.rs` spawns the GUI
thread after the admin listener is up; passes
`(admin_base, data_dir, quit, config_path)`.

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

### Dashboard top bar (`dashboard.rs`)
Page title left; right (RTL) health LED · `up <t> · v<ver>` meta (tooltip:
pid + bind) · **daemon actions** (`widgets::daemon_actions`) — Stop / Restart /
Reveal .tox as bordered `action_button`s. Present on every tab.

### Popup header (`draw_header`)
Orange LED · "td-mcp-rs · master|slave" · version (tooltip: pid + bind).
Right-anchored ghost buttons: `⛶` dashboard, `⚙` Settings. Lifecycle actions
are in the popup footer, not here.

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
   to the Proportional fallback so `← Back` renders.
7. ✅ Pass 7 tray interaction: context menu removed entirely —
   **left click opens the dashboard**, **right click toggles the glance
   panel** near the tray (`on_tray_popup_toggle`, Up-event filtered).
   Daemon Stop/Restart live in the dashboard DAEMON card; no MenuItem /
   MenuEvent plumbing remains. *(Superseded by pass 11.)* Verification gotcha: at >100% display scale a non-DPI-aware PrintWindow captures
   only the top-left logical crop of the physical window — size the
   bitmap from a DPI-aware GetWindowRect before trusting "missing UI".
8. ✅ Pass 8 glance-card read-only + crash reports: the popup's last
   mutation affordance (federation "Share this daemon" nudge +
   `share_applicable()`) is deleted — the tray panel is now a strictly
   read-only glance card whose only actions are navigation (`⛶`/`⚙`),
   `Locate .tox` (`reveal_tox()`), and report links. New daemon module
   `tdmcp_daemon::crashreport`: a panic hook installed in the Start
   branch of `main.rs` writes `{data_dir}/crash/crash-<ts>-p<pid>.log`
   (payload, location, forced backtrace, 30-line ring tail; newest 10
   kept) for every panicking thread — daemon runtime *and* GUI render.
   The GUI polls that directory (`scan_crash_reports`, throttled ≥5s off
   the 2s poll tick, filename-sorted): an unacknowledged previous crash
   shows as `Previous run crashed — open report` in the popup
   (ack'd per session on click), and the dashboard Overview errors card
   gains a `CRASH REPORTS · N — Open folder` row when any exist. No new
   admin endpoint: same-machine data-dir contract.
9. ✅ Pass 9 — overhaul (spec: `docs/GUI_OVERHAUL_PLAN.md`):
   **Fleet tab merged into Overview** (sidebar is now Overview/Logs/
   Settings; `TDMCP_OPEN_DASH=fleet` aliases Overview); monolith split into
   15 focused modules; design-system v2 (brightened text tiers, BG_CARD,
   ACCENT_BG/ERR_BG/WARN_BG tints, radius-8 cards, ROW_H 24 / CARD_PAD 10 /
   GUTTER 16 density, stat/display font sizes); widget kit additions
   (badge pills, pulsing LEDs, segmented control, banner, empty-state);
   top bar gained health LED + identity meta incl. daemon uptime
   (`uptimeSecs` added to `/admin/status`); fleet rows label task counts +
   colored bridge words + hover summary; empty states got guidance + CTA;
   Stop is two-step; Add-Slave became one probe→configure pipeline with an
   embedded network scan; Settings Save/Discard are dirty-gated with
   per-section `restart` chips and a segmented role picker that auto-enables
   sharing; snackbars acknowledge async actions; HTTP clients got bounded
   timeouts (fixes multi-second UI freezes on dead hosts); dev-only
   `preview` harness renders fixture scenes for pixel verification.

10. ✅ Pass 10 — daemon actions promoted, rosters demoted. Stop/Restart were
    `ghost_button`s inside the Overview `daemon_card`, so they vanished on the
    Logs and Settings tabs and carried the faintest styling in the kit. They now
    live in the **dashboard top bar**, rendered from one shared
    `dashboard::widgets::daemon_actions` that the **tray popup footer** also
    calls — which **reverses the pass-8 read-only lock** above at the user's
    explicit request: the popup body stays a read-only glance, but its footer
    carries Stop / Restart / Reveal .tox. Stop keeps the two-step confirm in
    both surfaces (the popup hides on focus loss; a one-click exit there would
    be too easy to trigger by accident). New `theme::action_button` +
    `ActionTone {Neutral, Accent, Danger}` fills the gap between `filled_button`
    and `ghost_button`. The Overview DAEMON card is identity-only, which also
    deleted a **duplicate `Reveal .tox`** that rendered twice whenever the
    daemon was online. Fleet/MCP rosters cap at `widgets::ROSTER_CAP` (4) rows
    with a dim `+N more` line. Two layout fixes found by the preview harness:
    the top-bar meta dropped `pid`/`bind` into the LED tooltip (three buttons
    overflowed the 800px minimum width otherwise), and the popup's scroll area
    now sizes from `ui.available_height() - FOOTER_H` instead of a
    `WINDOW_MAX_HEIGHT`-derived cap — with `auto_shrink(false)` the old cap made
    the area fill and pushed the new footer off-screen. Popup default height
    260 → 304. Preview harness gained `popup`, `popup-stop-confirm`,
    `overview-narrow` (800px) and `overview-many` (7 procs) scenes.

11. ✅ Pass 11 — tray gestures reshuffled to the platform norm: **single left
    click opens the glance popup**, **double left click opens the dashboard**,
    and **right click opens a real context menu** (Dashboard · Stop · Restart)
    — reinstating the MenuItem/MenuEvent plumbing pass 7 removed. Windows
    sends `Down, Up, DoubleClick, Up` for a double click, so the single-click
    open is armed on Up and fires `TRAY_DOUBLE_CLICK_GRACE` (300 ms) later
    unless a `DoubleClick` claims the gesture; the trailing `Up` is swallowed
    so it cannot re-arm the popup behind the dashboard. macOS emits no
    `DoubleClick` for status items — there the popup's `⛶` or the menu is the
    way into the dashboard. Three unit tests in `tray.rs` cover the sequence.
    Footer fix: the popup reserved only `FOOTER_H` out of the scroll budget
    while `draw_action_footer` also drew a leading gap, so the action row sat
    flush against the window edge — `FOOTER_BLOCK_H` now reserves the gap and
    the trailing `sp::SM` breathing room too.

## 8. Remaining open items

1. macOS/Linux parity unverified (Windows-first development session);
   macOS Accessory policy may affect secondary viewports.
2. Blocking HTTP on the UI thread per poll — now bounded by 2 s/3 s client
   timeouts so dead hosts can't freeze the GUI; full async worker still
   available as a future improvement (`http.rs` isolates the seam).
3. Logs toolbar at narrow widths (< ~900 px) leaves little slack between
   the chip row and the right-aligned controls — acceptable today; revisit
   if more toolbar items land.
