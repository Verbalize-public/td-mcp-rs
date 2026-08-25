# GUI Overhaul Plan (v3) — Agent Handoff Spec

Status: **approved, not started**. Owner: any agent with repo access.
Companion docs: [`docs/GUI_MAP.md`](GUI_MAP.md) (curated map of the *current*
GUI — read it first), [`CONSTITUTION.md`](../CONSTITUTION.md) (never-panic and
lint rules apply to all code written here).

Origin: planning session 2026-08-25; plan audited once (findings baked in).
Execute the phases **in order**. Every phase must end with:
`cargo clippy --workspace --all-targets -- -D warnings` clean,
`cargo test --workspace` green, and the app in a shippable state.

---

## 1. Directives (from the user — do not renegotiate)

1. Merge the dashboard **Overview** and **Fleet** tabs into a single Overview page.
2. Modern look & feel, great UX overall.
3. Simplicity of use — avoid complex/intricate workflows.
4. Hint/info badges everywhere they are required.
5. Nice margins; overall a little more compact.
6. Break the monolithic GUI file into more files as needed.

## 2. Current state (verified facts — trust these over memory)

- Crate: `crates/tdmcp-gui` (egui **0.35** + eframe + tray-icon + notify-rust).
  Three files: `src/lib.rs` (3030 ln: app state, polling, tray, popup glance
  card, federation flows, scans, wire DTOs, HTTP helpers, platform shims,
  tests), `src/dashboard.rs` (1241 ln: dashboard viewport shell + 4 tabs),
  `src/theme.rs` (327 ln: tokens + widget kit).
- Dashboard sidebar tabs: `DashTab { Overview, Fleet, Logs, Settings }`
  (`dashboard.rs` L25–49). All reference sites when deleting `Fleet`:
  `lib.rs` L404–408 (`TDMCP_OPEN_DASH` env hook) and L1298 (popup
  "+N more — open dashboard" deep link); `dashboard.rs` render match
  L119–124.
- Fleet-tab content that must survive on Overview: master actions row +
  scan results, TD fleet groups (master) / flat list, slave self-view,
  Add-Slave and Slave-Settings modals (rendered post-ScrollArea today),
  MCP client section.
- Public crate surface consumed by the daemon (must not change):
  `tdmcp_gui::run(...)` (`crates/tdmcp-daemon/src/main.rs` ~L640) and
  `tdmcp_gui::toast(...)` (`idle.rs` ~L137).
- `/admin/status` body (`crates/tdmcp-daemon/src/admin.rs` `StatusBody`,
  camelCase): `ok, version, pid, mcp_session_count, bridge_count, no_gui,
  bind_address, role, daemon_id, hostname, slave_count?`. **No uptime field**
  yet (§10 adds one).
- **Latent bug (fix in phase 4):** `http_get_blocking` / `http_post_blocking`
  (`lib.rs` L2630–2668) build `reqwest::Client::new()` with **no timeout** —
  a probe against a dead host can freeze the UI thread for the OS TCP timeout
  (~20 s on Windows). `scan_subnet` by contrast uses 400 ms.
- Glyph constraint (pass-6 lesson in GUI_MAP.md): bundled fonts tofu most
  symbols. All indicators are painted shapes or text from the already-proven
  set (`← ⚙ ⛶ × ▶ ⏸`). Never introduce new icon glyphs.
- Popup glance card is deliberately read-only and stays as-is (locked prior
  decision); it inherits theme changes automatically.

## 3. Target module map

```
crates/tdmcp-gui/src/
├── lib.rs          ~200  run(), crate doc, pub use (run/toast preserved)
├── app.rs          ~700  DashboardApp (pub(crate) fields), new(), poll(),
│                          apply_fleet_status + FleetSnapshot::derive (pure, tested),
│                          settings save/load/dirty, eframe::App impl
├── tray.rs         ~180  tray build/icon-swap/tooltip, position_near_tray, debounce
├── popup.rs        ~330  glance card: draw_header/draw_summary/attention_row/captions
├── wire.rs         ~220  DTOs: StatusView, FleetView/FleetProc, SessionsView/
│                          SessionRow, SlavesView/SlaveRow, LogRecordView(s),
│                          FederationProbe; parse_slaves, id_tail, ScanPurpose
├── http.rs         ~240  http_get/post_blocking (**now with timeouts**), scan_subnet,
│                          local_ip, ip_prefix, port_from_base
├── platform.rs     ~170  toast/notify, reveal_in_file_manager, applescript_escape
├── federation.rs   ~420  impl DashboardApp flows: add-slave pipeline (+AddSlaveStep),
│                          probe, configure-as-slave, slave settings load/save,
│                          go_standalone, scan start/drain/results; owns FleetPanel
├── theme/
│   ├── mod.rs      ~190  palette, spacing/sizes, fonts, apply()
│   └── widgets.rs  ~470  led(+glow+pulse), ghost/filled buttons, card, card_with_header,
│                          row_between, chip, badge(Kind), segmented, banner, empty_state
└── dashboard/
    ├── mod.rs      ~150  viewport_id/builder, shell render(): sidebar+topbar+routing,
    │                      modal layer, snackbar overlay
    ├── nav.rs      ~160  brand block, nav items + Overview attention badge, health word
    ├── overview.rs ~400  daemon strip, stat row, MCP clients card, activity/errors card
    ├── fleet.rs    ~400  TD Instances card: master-actions header, groups/flat,
    │                      slave self-view, empty state
    ├── logs.rs     ~270  logs page (+filter-count/result-count/hint line, `/` focus)
    ├── settings.rs ~430  dirty-gated Save/Discard, restart-needed section chips
    └── widgets.rs  ~300  stat_card, fleet_row v2 (popup shares it), modal_shell,
                           snackbar stack
```

Mechanics: multiple `impl DashboardApp` blocks across modules are legal within
the defining crate; struct fields become `pub(crate)`; `fleet_row` lives in
`dashboard/widgets.rs` because both the popup and the fleet card render it.
Existing unit tests move with their subjects. Phase 1 is a **pure move** —
zero behavior change.

## 4. Design system v2

Palette (hues kept): brighten secondary text `TEXT_DIM #7a7a7a → #8a8a8a`,
`TEXT_FAINT #555555 → #606060`; rename card fill `BG_ROW → BG_CARD #191919`
(zebra keeps BG_ROW/BG_ROW_ALT); unify selected tint as `ACCENT_BG #33261a`;
add faint `ERR_BG` / `WARN_BG` tints for banners/badges. Radius: cards 6→8,
controls stay 4, pills = height/2. Shadow NONE everywhere except modals
(subtle elevation).

Type scale: `display 15` (page titles) · title 13 · label 12 · meta 11 ·
mono 11 · **stat 22** (big numerals).

Density (exact values):

| Token | Now | New | Token | Now | New |
|---|---|---|---|---|---|
| SIDEBAR_W | 196 | **172** | CARD_PAD | 12 | **10** |
| TOPBAR_H | 44 | **38** | ROW_H | 26 | **24** |
| Central margin | 20 | **16** (`GUTTER`) | Stat card h | 68 | **62** |
| Window default | 980×660 | **960×640** | MODAL_W | 480 | **440** |
| Window min | 860×540 | **800×520** | | | |

Fits-checked: 4 stat cards ≈149 px each at min width; egui min-sizes are
logical points so DPI does not shrink layout. Log rows keep their intentional
18 px density (they do not use ROW_H).

## 5. Widget kit additions (`theme/widgets.rs`, `dashboard/widgets.rs`)

- `badge(ui, text, Kind)` — painted pill; Kind ∈ {Neutral, Ok, Warn, Error, Accent}.
- `led(ui, color, glow, pulse)` — LED with soft halo; pulse via
  `animate_value_with_time` (attention only).
- `segmented(ui, options, selected)` — proper segmented control (role picker).
- `banner(ui, tone, text, action)` — attention / restart-needed strips.
- `empty_state(title, subtitle, cta)` — guidance + single CTA button.
- `card_with_header(title, right_actions, body)` — section cards with count
  badges and header actions.
- Snackbar stack (bottom-right, ≤3 items, ~3 s fade) + `app.snack(msg, tone)`.

No new dependencies; no shadows except modal; no animation beyond nav hover
(existing) and the topbar attention pulse.

## 6. Badge & hint inventory (required placements)

| Location | Badge/Hint | Trigger |
|---|---|---|
| Nav "Overview" | amber count pill (static) | attention > 0 |
| Topbar right | pulsing health LED + mono `pid N · bind · vX.Y[ · up Hh Mm]` | always; ERR steady when offline |
| Daemon strip | role chip · loopback/network chip · version chip · last-error line when unreachable | — |
| Stat cards | definition tooltips ("TD processes with a live bridge", …) | hover |
| ATTENTION stat | value turns amber (no pulse — de-noised) | attention > 0 |
| TD header | neutral `(N)` count | — |
| Slave group header | reachability badge (reachable/disconnected/unreachable) + proc count + `⚙` | per slave |
| Fleet row | bridge word colored Ok/Err/faint · `tasks N` · amber `cancelled M`(>0) · resurrected badge · summary tooltip | per row |
| MCP row | client name lead · `name · v · id-tail · duration` | — |
| Activity header | error-count pill; amber left edge while attention unacked | — |
| Logs toolbar | active-filter count chip · result-count meta · hint line `F follow · Space pause · / search · Esc overview` | filters active |
| Settings | amber `restart` chip on SERVER/NETWORK/FEDERATION iff their restart-required fields differ (port/bind/auth.mode/auth.psk/role/master_url/master_psk; BRIDGE timeouts hot-reload → unbadged) · "unsaved changes" pill · Save/Discard disabled when clean | draft dirty |
| Modals | standard title row + `×` close (Esc already wired) | open |
| Empty states | guidance line + CTA | no content |

Attention signals deliberately limited to three surfaces: topbar pulse, nav
pill, ATTENTION stat color.

## 7. Page specs

### Shell
Sidebar (172 px): brand block (LED + name + role chip) → nav items
(Overview/Logs/Settings; keep hover animation + accent bar) → footer = health
word (`all good` / `attention` / `offline`) + LED only. Topbar (38 px): left =
page title in display size; right = health LED + identity meta. No duplicated
pid/bind between sidebar and topbar.

### Merged Overview (order = question order)
1. **Daemon strip**: role/version/network chips, listening line, `up …`,
   actions `[Reveal .tox] [Restart] [Stop ⚠]`. Offline variant: ERR LED +
   last-error mono line; actions disabled except Reveal .tox; stats show `–`.
2. **Stats row** (4 × 62 px): MCP CLIENTS / TD CONNECTED / ATTENTION / ROLE;
   tooltips define each metric.
3. **TOUCHDESIGNER (N)** card: master gets `[+ Add slave…]` header action;
   body = collapsible LOCAL/SLAVE groups (default open) or flat rows;
   `role=slave` adds FEDERATION sub-card (master url, id tail, Go-standalone
   confirm). Empty state: "No TouchDesigner instances yet — open bootstrap.tox
   in TouchDesigner to bridge it." + `[Reveal .tox]`.
4. **MCP CLIENTS (N)** card: name-first rows; empty hint "connect an MCP client
   to this daemon's `/mcp` endpoint".
5. **ACTIVITY** card: error ring rows (click = copy + snackbar), crash-reports
   row when present.

All former Fleet content lives here. Federation modals render post-ScrollArea
(as today), Esc-dismiss kept. `TDMCP_OPEN_DASH=fleet` becomes an alias for
Overview; popup "+N more" opens Overview.

### Logs
Keep chips/search/follow/pause/click-expand/Ctrl+C. Add: active-filter count
chip, result-count meta, shortcut hint line, `/` focuses the filter input.

### Settings
Dirty model: `config_dirty(draft, settings_loaded_snapshot)` gates Save
(filled accent only when dirty) and Discard; sticky restart banner restyled as
`banner()`. Restart chips per §6. Role picker becomes `segmented`.

### Popup glance card
Untouched by design (read-only glance surface); benefits from theme only.

## 8. Workflow simplifications

1. **Add-Slave = one pipeline.** Modal holds Host/Port/PSK + embedded
   `[Scan network]` with pickable results (the floating SCAN section leaves
   the main page). One action runs probe → configure sequentially with an
   inline step indicator (`probing… ✓ configuring… ✓ restart the slave to
   apply`); failures surface at the failed step with retry. Separate Probe
   button deleted. New `AddSlaveStep` state enum. Safe because HTTP timeouts
   land first (§2 bug): dead host costs ≤2 s.
2. **Role implies sharing.** Selecting Master/Join-master while bind is
   loopback auto-enables sharing (with note) — kills the interlock foot-gun.
   Turn-off confirm flow unchanged.
3. **Stop two-step.** `[Stop]` swaps to `Stop the daemon? [Confirm][Cancel]`
   (same pattern as Go-standalone confirm).
4. **Honest Save.** See Settings above.

## 9. Feedback system

Snackbars acknowledge async outcomes: settings saved/failed · restart/shutdown
issued · slave configured · copied-to-clipboard · scan finished (N found).

## 10. Backend touch (tiny, additive)

Add `uptime_secs: u64` to `/admin/status`: capture a process-start `Instant`
(`std::sync::OnceLock<Instant>` at router build) in
`crates/tdmcp-daemon/src/admin.rs`, add field to `StatusBody`. GUI
`StatusView` gains `#[serde(default)] uptime_secs: u64`. **Guard:** before
relying on additive compatibility, grep daemon tests for anything pinning the
exact status JSON shape. The rebuild/install cycle (kill daemons →
`cargo build --workspace` → `tdmcp-daemon ensure`) is part of verification.

## 11. Preview harness (dev-only)

Non-default cargo feature `preview` exposing one pub entry:
`tdmcp_gui::preview::run(scene)`. Internally builds `DashboardApp` with JSON
fixtures injected into `status / fleet_json / sessions_json / slaves_json /
error_ring / scan_results / add_slave_step / confirm_stop / draft`, suppresses
tray/polling, and renders the real `dashboard::render` into a plain viewport
titled `td-mcp-rs — Dashboard` (so `.ua/gui-shot.ps1`'s Win32 capture works
unchanged). Scenes: `overview-empty · overview-populated (master + slave group
+ 3 clients + errors + crash + attention>0) · overview-offline ·
modal-add-slave (+scan hits) · stop-confirm · logs-filtered · settings-dirty`.
One screenshot per scene-process — zero click simulation.

## 12. Tests & quality gates

New unit tests: `FleetSnapshot::derive` fixture test · `config_dirty` test ·
restart-chip section mapping test. Moved tests (logs fixture, level mapping,
error ring, clip) stay green with their modules. Gates: clippy `-D warnings`
(workspace, all targets) + `cargo test --workspace`. Never-panic rule: no
`unwrap`/`expect`/`panic!` outside tests.

## 13. Verification (PASS criteria)

1. Clippy/tests green.
2. Seven scene screenshots inspected at pixel level (badges, density, pulse
   visible across two captures, empty/offline/modal/dirty variants).
3. Real-daemon smoke: kill stray `tdmcp-daemon.exe` → rebuild → run with
   `TDMCP_OPEN_DASH=1` → screenshot real empty Overview + tray alive → kill.
   Confirms wiring fixtures can't (incl. `uptime_secs` end-to-end).
4. User eyeball pass for interactions (tab switches, modal open/cancel,
   Stop-confirm cancel, dirty Save gating).

## 14. Phase order (each ends compiling + clippy-clean + tests green)

1. **Split** — pure module move, zero behavior change (§3).
2. **Merge** — DashTab −Fleet, Overview composition, env alias, popup link (§7).
3. **Theme v2 + kit + shell** — tokens/density, widget kit, topbar/sidebar (§4–6).
4. **Page polish** — daemon strip, fleet rows, activity/MCP cards, logs/settings
   upgrades, workflow simplifications, snackbars, **HTTP timeout fix** (§8–9, §2 bug).
5. **Harness + backend** — preview scenes, `uptime_secs` + shape-test guard (§10–11).
6. **Verify + docs** — screenshot matrix, real-daemon smoke, update
   `docs/GUI_MAP.md` (§1/§2 module inventory, §5 views, append Pass 9).

## 15. Locked & rejected decisions (do not relitigate without user)

Rejected: command palette (3 tabs suffice) · sparklines/charts (no history
data) · two-column masonry (rows stretch fine full-width) · light theme
(doubles QA) · icon/SVG fonts (glyph tofu lesson) · web stack (locked decision:
second egui viewport) · unread/read-state tracking for errors (stateful
clutter) · extracting federation fields into a substruct (~60 ref sites, zero
behavior gain — method-move only) · background poll worker (deferred P3; local
requests <5 ms; `http.rs` isolation makes the future swap trivial).

Risks watched: split churn (phase-gated, pure-move commit) · egui 0.35 API
drift (in-tree-proven APIs only) · glyph tofu (painted shapes only) ·
blocking-HTTP freezes now bounded by timeouts (full async worker deferred) ·
Windows-first verification (macOS parity remains an open item, see
GUI_MAP.md §8).
