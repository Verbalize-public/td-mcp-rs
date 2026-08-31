# GUI Map — td-mcp-rs

Curated, hand-written map of the GUI. Last verified against the code and the
preview harness: 2026-08.

---

## 1. What the GUI is

Two surfaces in one process (inside `tdmcp-daemon`, disabled with `--no-gui` /
`TDMCP_NO_GUI`):

- **Dashboard window** (`dashboard.rs` + `dashboard/*`): a decorated,
  resizable secondary egui viewport (default 960×640, min 800×520) with
  sidebar navigation **Overview / Logs / Settings**. Overview carries the
  daemon strip, stat tiles, the TOUCHDESIGNER fleet card (grouped by machine,
  master actions, slave self-view), the MCP CLIENTS card, and the
  ACTIVITY/errors card; federation flows (add-slave, per-slave settings,
  role changes) run as modal overlays on top.
- **Tray popup** (`popup.rs`): a frameless glance card anchored near the tray
  (380 wide × 304 default, min 180, non-resizable). Header = logo mark ·
  title · version + `⛶` dashboard / `⚙` settings; body = ATTENTION strip,
  TOUCHDESIGNER mini list (capped), MCP client names; footer = the one
  mutation surface — Stop / Restart / Reveal .tox (Stop keeps its two-step
  confirm).
- Tray gestures: **left click opens the popup**, **double left click opens
  the dashboard**, **right click opens a context menu** (Dashboard · Stop ·
  Restart). On macOS (no status-item double click) the popup's `⛶` or the
  menu is the way in.
- Closing the popup or losing focus only **hides** it; the dashboard has a
  real close button. Real exit is Stop / `/admin/shutdown`.
- Stack: `eframe`/`egui 0.35` + `tray-icon` + `notify-rust` + `reqwest`
  (blocking calls with 2 s/3 s timeouts over throwaway current-thread tokio
  runtimes; `http.rs` isolates the seam).
- Dev/test hooks: `TDMCP_OPEN_DASH=overview|logs|settings` (`fleet` kept as a
  legacy alias for Overview) opens the dashboard on that tab; the `preview`
  feature renders fixture scenes for pixel verification (§2).

## 2. File inventory

| Path | Role |
| --- | --- |
| `crates/tdmcp-gui/Cargo.toml` | crate manifest; lib `tdmcp_gui` consumed by daemon under the `gui` feature; dev-only `preview` feature |
| `crates/tdmcp-gui/src/lib.rs` | entry point: `run()` + module map; public surface is exactly `run` + `toast` |
| `crates/tdmcp-gui/src/app.rs` | `DashboardApp` state/logic core: poll loop, fleet-snapshot diffing, settings save/dirty, snackbars, eframe tick |
| `crates/tdmcp-gui/src/tray.rs` | tray icon assets/build, click routing (popup/dashboard/menu), popup positioning near the tray |
| `crates/tdmcp-gui/src/popup.rs` | glance card (header + summary + action footer) |
| `crates/tdmcp-gui/src/recent.rs` | LRU of recently opened projects (≤16, deduped) persisted beside `data_dir`; feeds the recent-projects menu |
| `crates/tdmcp-gui/src/wire.rs` | admin-API DTOs + display mappers (level colors/letters, id tails, clip) |
| `crates/tdmcp-gui/src/http.rs` | blocking HTTP helpers (bounded timeouts), LAN subnet scan, local-IP helpers |
| `crates/tdmcp-gui/src/platform.rs` | OS toasts, file-manager reveal, per-OS pointer query (Linux glance close-on-outside-click) |
| `crates/tdmcp-gui/src/federation.rs` | add-slave one-click pipeline, per-slave settings, Go-standalone, scan-results UI |
| `crates/tdmcp-gui/src/theme.rs` + `theme/` | Ableton-dark tokens/fonts/visuals + widget kit (§6) |
| `crates/tdmcp-gui/src/dashboard.rs` | dashboard shell + viewport; `DashTab {Overview, Logs, Settings}` router |
| `crates/tdmcp-gui/src/dashboard/{nav,overview,fleet,logs,settings,widgets}.rs` | sidebar nav, pages, and shared painted pieces (`stat_card`, `fleet_row`, `card_with_header`, `daemon_actions`, `capped_rows`) |
| `crates/tdmcp-gui/src/preview.rs` + `examples/dashboard_preview.rs` | fixture harness (§7); scenes: `overview-empty` · `overview-populated` · `overview-offline` · `overview-many` · `overview-narrow` · `modal-add-slave` · `stop-confirm` · `logs-filtered` · `settings-dirty` · `popup` · `popup-stop-confirm` |
| `crates/tdmcp-gui/assets/logo-mark.png` | sidebar/popup brand mark (cropped node from `logo.svg`, rendered by `packaging/gen_icons.py`) |
| `crates/tdmcp-gui/assets/icon-normal.png` / `icon-attention.png` | tray icons (rendered from `logo.svg`; attention = orange corner badge) |

Daemon-side launch point: `crates/tdmcp-daemon/src/main.rs` spawns the GUI
thread after the admin listener is up; passes
`(admin_base, data_dir, quit, config_path)`.

## 3. Runtime topology & data flow

```
tdmcp-daemon ── spawns ──> GUI thread (eframe event loop, main thread)
                              │  polls every 2 s
                              ▼
                    Admin HTTP API (axum, 127.0.0.1:9860)
                      GET  /admin/status                  → StatusView
                      GET  /admin/fleet                   → FleetView (TD processes)
                      GET  /admin/mcp-sessions            → SessionsView
                      POST /admin/mcp-sessions/annotate   → session label
                      GET  /admin/logs                    → log ring records (cursor)
                      GET  /admin/logs/path               → log dir (reveal button)
                      POST /admin/logs/ingest             → proxy uplink
                      POST /admin/shutdown | /admin/restart
```

Federation routes live in `crates/tdmcp-daemon/src/federation.rs`
(`/admin/federation/status|register|fleet-push|slaves`); the GUI calls some
of these directly during add-slave / join-master flows.

Other threads/flows:

- **Subnet scan**: `std::thread` + `mpsc` channel, results drained each
  frame; shared between "find slaves" and "find masters" via `ScanPurpose`.
- **Notifications**: OS toasts on bridge loss / resurrect / cancelled tasks /
  slave joined / startup reachability.
- **Tray attention**: icon swaps normal↔attention + tooltip text derived
  from fleet snapshot diffing (`apply_fleet_status`).
- **Crash reports**: the daemon's panic hook writes `{data_dir}/crash/`;
  the GUI scans that directory (throttled ≥5 s off the poll tick) and
  surfaces `Previous run crashed — open report` (popup) and a crash-reports
  row on the Overview errors card.

## 4. State model (`DashboardApp`)

Grouped by concern:

- **Identity/wiring**: `admin_base`, `data_dir`, `config_path`, `quit`.
- **Navigation**: `dash_tab: DashTab {Overview, Logs, Settings}`,
  `fleet_panel: FleetPanel {None, AddSlave, SlaveSettings}` (overlay state).
- **Poll snapshots**: `status: Option<StatusView>`, `fleet_json`,
  `sessions_json`, `slaves_json`, `prev_snapshot: FleetSnapshot`,
  `last_poll`, `error`, `fail_polls`.
- **Settings editing**: `draft: ConfigFile` + path text buffers,
  `settings_loaded_snapshot` (restart-needed diff), `needs_restart`,
  `settings_error`, PSK visibility toggles.
- **Window/tray lifecycle**: `visible`, `dashboard_open`, `pending_tray`,
  `tray: Option<TrayIcon>`, icon pair, `attention`, tray rect/debounce/
  focus-grace timestamps, recent-projects LRU.
- **Federation flows**: add-slave host/port/psk/probe/message,
  `slave_settings_target` + timeouts, confirmations (Go standalone, turn off
  sharing), `role_change_note`, `focus_master_psk`, known-slave id set +
  seen-once flag (join-toast suppression).
- **Scans**: `scan_results`, `scan_busy`, `scan_rx`, `scan_purpose`.
- **Logs**: `logs_view: LogsViewState` — client ring capped at 2048 rows,
  fetch limit 512/poll while visible, level filter + text search, follow/pause.
- **Errors/crashes**: `error_ring` (attention strip + errors card),
  `crash_count`, crash-report ack state.

Wire DTOs (camelCase serde): `StatusView`, `SessionsView`/`SessionRow`,
`FleetView`/`FleetProc`, `LogRecordView`, plus local `SlaveRow`,
`FederationProbe`, `ScanHit`.

## 5. Views (what the user sees)

### Dashboard top bar
Page title left; right (RTL): health LED (tooltip: pid + bind) · daemon actions
(`widgets::daemon_actions`) — New / Open / Reveal .tox, Stop / Restart as
bordered `action_button`s. Identity meta (role / version / uptime) lives in the
sidebar footer. Present on every tab.

### Sidebar
Centered brand mark (`theme::logo_texture`, cached per context; falls back to
the text brand if decode fails) → nav items (Overview carries the live
attention count pill) → bottom footer stack: health LED + word
(`all good` / `attention` / `offline`, tooltip: attention breakdown), role
badge (`standalone` / `master` / `slave` / `offline`), and a mono
`v<ver> · up <t>` meta line (hidden while unreachable).

### Overview tab
- Attention strip + errors card (recent daemon errors, crash reports; header
  Clear empties the client-side error ring).
- DAEMON card (identity: role, version, uptime; actions live in the top bar).
- TOUCHDESIGNER card: fleet process rows grouped by owning daemon
  (`fleet.rs` — group rows / flat fallback), LED per bridge state,
  resurrect/cancelled badges, task counts + hover summary; master actions
  (Add Slave → `federation.rs` pipeline with probe + embedded LAN scan);
  slave self-view (master url/id, Go-standalone confirm).
- MCP CLIENTS card: session rows — client name/version, connected-for.
- Empty/loading states with guidance + CTA.

### Logs tab
Toolbar: level chips + text filter + pause/auto-scroll + clear + reveal-dir;
keyboard shortcuts (F follow, Space pause, Esc close). Rows render LED
letter + time + clipped message; click-to-expand detail with Ctrl+C copy.
Client ring 2048, fetch 512 per poll while visible.

### Settings tab
Toolbar (Save/Discard/Restart-needed bar), then section cards of
label-left/control-right rows (help tooltips from `FIELD_DESCS`): SETTINGS
(tray toggle), SERVER (port), NETWORK (bind, auth mode), FEDERATION (role
switcher, daemon_id, master_url, master_psk), DAEMON (keep_alive, always_on,
show_tray), BRIDGE (call/script/heartbeat/pong/idle timeouts), ADVANCED
(data_dir, bridge_dir, catalog_path, daemon bin). Draft-vs-loaded diff
drives the "restart required" bar. Save/Discard are dirty-gated.

### Tray popup
Header (logo mark · title · version · ⛶/⚙), ATTENTION strip (≤2 rows from the
error ring), TOUCHDESIGNER mini list (capped 4 + "+N more"), MCP client
names, pinned action footer (Stop / Restart / Reveal .tox). Body is a
strictly read-only glance.

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
`RADIUS_SM=4` / `RADIUS_MD=6`, row height `ROW_H`, card padding `CARD_PAD`.

Fonts: title/label/meta proportional + mono for ids/durations. Widgets:
`status_led` (+pulsing variant), `badge` pills, `banner`, `segmented`
control, `empty_state`, `filled_button` (accent Save), `action_button`
(tone: Neutral/Accent/Danger), `ghost_button` (borderless icon actions),
`card()` (bordered rounded container), `row_between()` (justify-between
flex row), `chip()` (filter toggle pill). No shadows.

Platform glue: macOS `ActivationPolicy::Accessory` (menu-bar-only),
per-OS `reveal_in_file_manager` (explorer/open/xdg-open), `toast()`/
`notify()` wrappers.

## 7. Verify changes

`TDMCP_PREVIEW_SCENE=<scene> cargo run -p tdmcp-gui --features preview
--example dashboard_preview` renders the real dashboard with fixture data
(see scene list in §2). `TDMCP_OPEN_DASH=overview|logs|settings` opens the
real dashboard against a live daemon.

## 8. Remaining open items

1. macOS/Linux parity unverified (Windows-first development session);
   macOS Accessory policy may affect secondary viewports.
2. Blocking HTTP on the UI thread per poll — bounded by 2 s/3 s client
   timeouts so dead hosts can't freeze the GUI; full async worker remains
   available as a future improvement (`http.rs` isolates the seam).
3. Logs toolbar at narrow widths (< ~900 px) leaves little slack between
   the chip row and the right-aligned controls — acceptable today; revisit
   if more toolbar items land.
