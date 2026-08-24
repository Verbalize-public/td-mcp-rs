//! td-mcp-rs tray dashboard (egui 0.35 + tray-icon + notify-rust).
//!
//! Consumed in-process by `tdmcp-daemon` when the `gui` feature is enabled.
//! Closing the window or losing focus only hides the UI — it does not stop
//! the daemon. Use Stop / `/admin/shutdown` to end the process (sets `quit`).

mod dashboard;
mod theme;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::Result;
use eframe::egui;
use serde::Deserialize;
use tdmcp_config::{self as cfgfile, ConfigFile, FIELD_DESCS};
use tracing::{info, warn};
use tray_icon::menu::{Menu, MenuEvent, MenuItem};
use tray_icon::{
    Icon, MouseButton, MouseButtonState, Rect, TrayIcon, TrayIconBuilder, TrayIconEvent,
};
use uuid::Uuid;

use theme::{
    filled_button, font_label, font_meta, font_mono, font_title, ghost_button, section_header,
    status_led, ACCENT, BG_HOVER, BG_PANEL, BG_ROW, BG_ROW_ALT, BORDER, ERR, OK, TEXT, TEXT_DIM,
    TEXT_FAINT, WARN, WINDOW_MAX_HEIGHT, WINDOW_WIDTH,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FleetPanel {
    None,
    AddSlave,
    SlaveSettings,
}

/// Which UI triggered the shared subnet scan — keeps the master's "find a
/// slave" scan and a joiner's "find a master" scan from showing each other's
/// stale results, without duplicating the scan state/thread/mpsc plumbing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScanPurpose {
    AddSlave,
    JoinMaster,
}

/// Coalesce tray left-clicks so burst/double events cannot flip twice.
const TRAY_TOGGLE_DEBOUNCE: Duration = Duration::from_millis(250);
/// Focus-loss hide within this window counts as the close half of a tray toggle.
const FOCUS_LOSS_CLOSE_GRACE: Duration = Duration::from_millis(400);

/// Top chrome strip height (px).
const HEADER_H: f32 = 34.0;
/// Settings row height (px).
const SETTINGS_ROW_H: f32 = 26.0;
/// Symmetric side inset for content (px).
const SIDE_MARGIN: f32 = 12.0;
/// Width reserved for the header's right-anchored actions (px).
const HEADER_ACTIONS_W: f32 = 186.0;
/// Newest-first error/warning entries kept for popup + dashboard surfaces.
const ERROR_RING_CAP: usize = 50;
/// Rendered log rows kept client-side (evict oldest; matches the daemon ring cap).
const LOGS_RENDER_CAP: usize = 2048;
/// Records requested per `/admin/logs` fetch while the Logs view is active.
const LOGS_FETCH_LIMIT: u32 = 512;

/// Run the tray dashboard on the calling thread (must be the process main thread).
///
/// Polls `admin_base` (e.g. `http://127.0.0.1:9860`) for status/fleet/sessions.
/// `data_dir` is the install root (contains `bootstrap.tox`) for the reveal button.
/// `config_path` is the TOML settings file edited by the Settings view.
/// When `quit` is set (Stop / idle / admin shutdown), the event loop closes for real.
pub fn run(
    admin_base: String,
    data_dir: PathBuf,
    quit: Arc<AtomicBool>,
    config_path: PathBuf,
) -> Result<()> {
    let icon_normal_full = load_rgba(include_bytes!("../assets/icon-normal.png"), None)?;
    let icon_normal = load_rgba(include_bytes!("../assets/icon-normal.png"), Some(32))?;
    let icon_attention = load_rgba(include_bytes!("../assets/icon-attention.png"), Some(32))?;
    let window_icon = egui::IconData {
        rgba: icon_normal_full.rgba.clone(),
        width: icon_normal_full.width,
        height: icon_normal_full.height,
    };
    let dash_icon = egui::IconData {
        rgba: window_icon.rgba.clone(),
        width: window_icon.width,
        height: window_icon.height,
    };

    #[allow(unused_mut)]
    let mut options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([WINDOW_WIDTH, 320.0])
            .with_min_inner_size([WINDOW_WIDTH, 200.0])
            .with_max_inner_size([WINDOW_WIDTH, WINDOW_MAX_HEIGHT])
            .with_title("td-mcp-rs")
            .with_icon(window_icon)
            .with_decorations(false)
            .with_taskbar(false)
            .with_resizable(false)
            // Tray + toast only at startup; user opens the dashboard via the tray.
            .with_visible(false),
        ..Default::default()
    };
    #[cfg(target_os = "macos")]
    {
        // Menu-bar-only process: Accessory keeps us out of the Dock and is the
        // policy tray-icon / NSStatusItem expect for a persistent status item.
        options.event_loop_builder = Some(Box::new(|builder| {
            use winit::platform::macos::{ActivationPolicy, EventLoopBuilderExtMacOS};
            builder.with_activation_policy(ActivationPolicy::Accessory);
        }));
    }
    eframe::run_native(
        "td-mcp-rs",
        options,
        Box::new(move |cc| {
            theme::apply(&cc.egui_ctx);
            Ok(Box::new(DashboardApp::new(
                admin_base,
                data_dir,
                icon_normal,
                icon_attention,
                quit,
                config_path,
                dash_icon,
            )?))
        }),
    )
    .map_err(|e| anyhow::anyhow!("eframe: {e}"))
}

struct RgbaIcon {
    rgba: Vec<u8>,
    width: u32,
    height: u32,
}

fn load_rgba(bytes: &[u8], max_side: Option<u32>) -> Result<RgbaIcon> {
    let img = image::load_from_memory(bytes)?.into_rgba8();
    let (width, height) = img.dimensions();
    let resized = match max_side {
        Some(side) if width > side || height > side => {
            image::imageops::resize(&img, side, side, image::imageops::FilterType::Lanczos3)
        }
        _ => {
            if width > 256 || height > 256 {
                image::imageops::resize(&img, 256, 256, image::imageops::FilterType::Lanczos3)
            } else {
                img
            }
        }
    };
    let (width, height) = resized.dimensions();
    Ok(RgbaIcon {
        rgba: resized.into_raw(),
        width,
        height,
    })
}

fn tray_icon_from(rgba: &RgbaIcon) -> Result<Icon> {
    Icon::from_rgba(rgba.rgba.clone(), rgba.width, rgba.height)
        .map_err(|e| anyhow::anyhow!("tray icon: {e}"))
}

struct DashboardApp {
    admin_base: String,
    data_dir: PathBuf,
    config_path: PathBuf,
    draft: ConfigFile,
    settings_error: Option<String>,
    /// Text buffers for optional advanced paths (empty = unset).
    data_dir_edit: String,
    bridge_dir_edit: String,
    catalog_path_edit: String,
    daemon_bin_edit: String,
    status: Option<StatusView>,
    fleet_json: String,
    sessions_json: String,
    /// Master-only: `/admin/federation/slaves` body.
    slaves_json: String,
    last_poll: Option<Instant>,
    error: Option<String>,
    tray: Option<TrayIcon>,
    menu_restart: MenuItem,
    menu_stop: MenuItem,
    menu_dashboard: MenuItem,
    icon_normal: RgbaIcon,
    icon_attention: RgbaIcon,
    attention: bool,
    prev_snapshot: FleetSnapshot,
    visible: bool,
    /// Apply `Visible(false)` once after the first frame.
    pending_initial_hide: bool,
    /// Build the status-item on the first `logic` tick (see `ensure_tray`).
    pending_tray: bool,
    /// Fired once after the first successful `/admin/status` poll.
    startup_notified: bool,
    /// Fired once when polls fail before any success.
    startup_fail_notified: bool,
    fail_polls: u32,
    /// Drop always-on-top after this instant (transient focus grab).
    clear_always_on_top_at: Option<Instant>,
    /// Suppress focus-loss hide briefly after show (tray click focus race).
    ignore_focus_loss_until: Option<Instant>,
    /// When focus-loss hid the popup (coalesce with tray click close).
    hidden_by_focus_loss_at: Option<Instant>,
    /// Last tray toggle gesture (debounce burst events).
    last_tray_toggle_at: Option<Instant>,
    /// Last tray icon rect for anchoring.
    last_tray_rect: Option<Rect>,
    /// Shared with the daemon thread — when set, close the event loop for real.
    quit: Arc<AtomicBool>,
    /// Show auth PSK in Settings.
    show_psk: bool,
    /// Show master PSK in Settings.
    show_master_psk: bool,
    /// Confirm label after federation role change in Settings.
    role_change_note: Option<String>,
    /// Slave: awaiting confirm for Go standalone.
    confirm_go_standalone: bool,
    /// Fleet overlay panel.
    fleet_panel: FleetPanel,
    add_slave_host: String,
    add_slave_port: u16,
    add_slave_psk: String,
    add_slave_message: Option<String>,
    add_slave_probe: Option<FederationProbe>,
    scan_results: Vec<ScanHit>,
    scan_busy: bool,
    /// Target slave for settings panel.
    slave_settings_target: Option<SlaveSettingsTarget>,
    slave_settings_call_timeout: u64,
    slave_settings_script_timeout: u64,
    slave_settings_error: Option<String>,
    show_add_slave_psk: bool,
    /// Pending scan result receiver (None when idle).
    scan_rx: Option<std::sync::mpsc::Receiver<Vec<ScanHit>>>,
    /// Slave self-view transient message (Go standalone outcome).
    slave_self_message: Option<String>,
    /// Which flow the current `scan_results` belong to.
    scan_purpose: ScanPurpose,
    /// Awaiting confirm for turning off network sharing while federated.
    confirm_turn_off_sharing: bool,
    /// One-shot: focus the Master PSK field next time it draws.
    focus_master_psk: bool,
    /// A saved setting needs a daemon restart to take effect.
    needs_restart: bool,
    /// Config snapshot as of the last `open_settings()`, for restart-needed diffing.
    settings_loaded_snapshot: ConfigFile,
    /// Slave daemon ids seen on the last `/admin/federation/slaves` poll.
    known_slave_ids: HashSet<String>,
    /// Suppresses a join-toast burst for slaves already connected at GUI start.
    slaves_seen_once: bool,
    /// Tray Logs view (T4.2).
    logs_view: LogsViewState,
    /// Dashboard secondary viewport is open.
    dashboard_open: bool,
    /// Selected dashboard tab.
    dash_tab: dashboard::DashTab,
    /// Latest errors/warnings, newest first (tray strip + dashboard card).
    error_ring: Vec<String>,
    /// Window icon reused by the dashboard viewport builder.
    window_icon: egui::IconData,
}

/// One record as returned by `/admin/logs` (camelCase over the wire).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LogRecordView {
    seq: u64,
    ts: String,
    level: String,
    src: String,
    pid: u32,
    target: String,
    msg: String,
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    kvs: std::collections::BTreeMap<String, String>,
}

#[derive(Debug, Deserialize)]
struct LogsResponse {
    records: Vec<LogRecordView>,
    next: u64,
}

/// Dot color per record level (ERR/WRN colored, everything else plain text).
fn level_color(level: &str) -> egui::Color32 {
    match level {
        "error" => ERR,
        "warn" => WARN,
        _ => TEXT_FAINT,
    }
}

fn level_letter(level: &str) -> &'static str {
    match level {
        "trace" => "T",
        "debug" => "D",
        "info" => "I",
        "warn" => "W",
        "error" => "E",
        _ => "?",
    }
}

/// Clip to `max_chars`, char-boundary safe (targets/messages may be UTF-8).
fn clip_line(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_owned();
    }
    s.chars().take(max_chars).collect()
}

/// Tray Logs view state (T4.2). Polling only runs while this view is the
/// active one, the window is visible, and `paused` is false.
struct LogsViewState {
    buf: std::collections::VecDeque<LogRecordView>,
    /// Cursor for the next `/admin/logs?after=` fetch.
    next: u64,
    /// Auto-scroll to the newest row (only when already at the bottom).
    follow: bool,
    paused: bool,
    /// `None` = ALL; cycles ALL -> WARN -> ERROR -> ALL via the filter chips.
    min_level: Option<&'static str>,
    /// Empty = ALL sources; otherwise only these `src` values are kept.
    srcs: HashSet<&'static str>,
    /// Client-side substring filter over msg/target (dashboard search box).
    text_filter: String,
    /// `seq` of the expanded row, if any.
    expanded: Option<u64>,
    fetch_error: Option<String>,
    /// Resolved `/admin/logs/path` directory (fetched lazily, once).
    dir: Option<String>,
    last_fetch: Option<Instant>,
}

impl Default for LogsViewState {
    fn default() -> Self {
        Self {
            buf: std::collections::VecDeque::new(),
            next: 0,
            follow: true,
            paused: false,
            min_level: None,
            srcs: HashSet::new(),
            text_filter: String::new(),
            expanded: None,
            fetch_error: None,
            dir: None,
            last_fetch: None,
        }
    }
}

#[derive(Debug, Default, Clone)]
struct FleetSnapshot {
    connected: usize,
    disconnected: usize,
    resurrected: usize,
    cancelled: usize,
    connected_pids: Vec<u32>,
    resurrected_pids: Vec<u32>,
    cancelled_total: usize,
}

impl DashboardApp {
    fn new(
        admin_base: String,
        data_dir: PathBuf,
        icon_normal: RgbaIcon,
        icon_attention: RgbaIcon,
        quit: Arc<AtomicBool>,
        config_path: PathBuf,
        window_icon: egui::IconData,
    ) -> Result<Self> {
        let menu_restart = MenuItem::new("Restart daemon", true, None);
        let menu_stop = MenuItem::new("Stop daemon", true, None);
        let menu_dashboard = MenuItem::new("Open dashboard", true, None);

        let draft = cfgfile::load(&config_path).unwrap_or_default();
        // Dev/test hook: TDMCP_OPEN_DASH=1|logs|fleet|settings opens the
        // dashboard (optionally on a tab) instead of staying tray-only.
        let dash_env = std::env::var("TDMCP_OPEN_DASH").unwrap_or_default();
        let dash_tab = match dash_env.as_str() {
            "logs" => dashboard::DashTab::Logs,
            "fleet" => dashboard::DashTab::Fleet,
            "settings" => dashboard::DashTab::Settings,
            _ => dashboard::DashTab::default(),
        };
        let dash_open = !dash_env.is_empty() && dash_env != "0";
        let (data_dir_edit, bridge_dir_edit, catalog_path_edit, daemon_bin_edit) =
            path_edits_from(&draft);
        let settings_loaded_snapshot = draft.clone();

        Ok(Self {
            admin_base,
            data_dir,
            config_path,
            draft,
            settings_error: None,
            data_dir_edit,
            bridge_dir_edit,
            catalog_path_edit,
            daemon_bin_edit,
            status: None,
            fleet_json: String::new(),
            sessions_json: String::new(),
            slaves_json: String::new(),
            last_poll: None,
            error: None,
            // Defer tray build to the first `logic` tick. Creating a status-item
            // inside eframe's creation callback can re-enter AppKit on macOS and
            // trip winit 0.30's "event while another event is handled" abort.
            tray: None,
            menu_restart,
            menu_stop,
            menu_dashboard,
            icon_normal,
            icon_attention,
            attention: false,
            prev_snapshot: FleetSnapshot::default(),
            visible: false,
            pending_initial_hide: !dash_open,
            pending_tray: true,
            startup_notified: false,
            startup_fail_notified: false,
            fail_polls: 0,
            clear_always_on_top_at: None,
            ignore_focus_loss_until: None,
            hidden_by_focus_loss_at: None,
            last_tray_toggle_at: None,
            last_tray_rect: None,
            quit,
            show_psk: false,
            show_master_psk: false,
            role_change_note: None,
            confirm_go_standalone: false,
            fleet_panel: FleetPanel::None,
            add_slave_host: String::new(),
            add_slave_port: tdmcp_config::DEFAULT_PORT,
            add_slave_psk: String::new(),
            add_slave_message: None,
            add_slave_probe: None,
            scan_results: Vec::new(),
            scan_busy: false,
            slave_settings_target: None,
            slave_settings_call_timeout: 45,
            slave_settings_script_timeout: 120,
            slave_settings_error: None,
            show_add_slave_psk: false,
            scan_rx: None,
            slave_self_message: None,
            scan_purpose: ScanPurpose::AddSlave,
            confirm_turn_off_sharing: false,
            focus_master_psk: false,
            needs_restart: false,
            settings_loaded_snapshot,
            known_slave_ids: HashSet::new(),
            slaves_seen_once: false,
            logs_view: LogsViewState::default(),
            dashboard_open: dash_open,
            dash_tab,
            error_ring: Vec::new(),
            window_icon,
        })
    }

    fn ensure_tray(&mut self) {
        if !self.pending_tray || self.tray.is_some() {
            return;
        }
        self.pending_tray = false;
        let menu = Menu::new();
        if let Err(e) = menu.append(&self.menu_dashboard) {
            warn!(error = %e, "tray menu append dashboard failed");
            return;
        }
        if let Err(e) = menu.append(&self.menu_restart) {
            warn!(error = %e, "tray menu append restart failed");
            return;
        }
        if let Err(e) = menu.append(&self.menu_stop) {
            warn!(error = %e, "tray menu append stop failed");
            return;
        }
        let icon = match tray_icon_from(&self.icon_normal) {
            Ok(i) => i,
            Err(e) => {
                warn!(error = %e, "tray icon decode failed");
                return;
            }
        };
        match TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_menu_on_left_click(false)
            .with_tooltip("td-mcp-rs")
            .with_icon(icon)
            // Do not set template mode: our PNGs are full-color opaque RGB.
            // macOS template icons need black+alpha shapes; template+opaque
            // color assets often render as an invisible menu-bar item.
            .build()
        {
            Ok(tray) => {
                info!("tray status item created");
                self.tray = Some(tray);
            }
            Err(e) => warn!(error = %e, "tray icon build failed"),
        }
    }

    fn open_settings(&mut self) {
        match cfgfile::load(&self.config_path) {
            Ok(draft) => {
                self.draft = draft;
                self.settings_error = None;
            }
            Err(e) => {
                self.draft = ConfigFile::default();
                self.settings_error = Some(format!("load failed: {e}"));
            }
        }
        let (d, b, c, bin) = path_edits_from(&self.draft);
        self.data_dir_edit = d;
        self.bridge_dir_edit = b;
        self.catalog_path_edit = c;
        self.daemon_bin_edit = bin;
        self.settings_loaded_snapshot = self.draft.clone();
        self.confirm_turn_off_sharing = false;
    }

    /// Lazily resolve the log directory once, from either surface.
    fn ensure_logs_dir(&mut self) {
        if self.logs_view.dir.is_some() {
            return;
        }
        self.ensure_base();
        let bearer = local_master_psk(&self.draft);
        if let Ok(body) = http_get_blocking(
            &format!("{}/admin/logs/path", self.admin_base),
            bearer.as_deref(),
        ) {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&body) {
                self.logs_view.dir = v["dir"].as_str().map(str::to_owned);
            }
        }
    }

    /// Logs streaming is wanted while the dashboard sits on its Logs tab —
    /// even when the popup itself is hidden.
    fn logs_surface_active(&self) -> bool {
        self.dashboard_open && self.dash_tab == dashboard::DashTab::Logs
    }

    fn reveal_logs_dir(&self) {
        let Some(dir) = self.logs_view.dir.as_ref() else {
            return;
        };
        let path = PathBuf::from(dir);
        if let Err(e) = reveal_in_file_manager(&path, &path) {
            warn!(error = %e, "reveal logs dir failed");
        }
    }

    /// Fetch the next page of `/admin/logs` when due (either surface wants
    /// logs, not paused) — piggybacks the existing repaint tick, same
    /// throttle style as `poll()`.
    fn fetch_logs_if_due(&mut self) {
        if !self.logs_surface_active() || self.logs_view.paused {
            return;
        }
        let due = self
            .logs_view
            .last_fetch
            .is_none_or(|t| t.elapsed() > Duration::from_millis(250));
        if !due {
            return;
        }
        self.logs_view.last_fetch = Some(Instant::now());
        self.ensure_base();
        let mut url = format!(
            "{}/admin/logs?after={}&limit={LOGS_FETCH_LIMIT}",
            self.admin_base, self.logs_view.next
        );
        if let Some(level) = self.logs_view.min_level {
            url.push_str("&level=");
            url.push_str(level);
        }
        if !self.logs_view.srcs.is_empty() {
            url.push_str("&src=");
            url.push_str(
                &self
                    .logs_view
                    .srcs
                    .iter()
                    .copied()
                    .collect::<Vec<_>>()
                    .join(","),
            );
        }
        let bearer = local_master_psk(&self.draft);
        match http_get_blocking(&url, bearer.as_deref()) {
            Ok(body) => match serde_json::from_str::<LogsResponse>(&body) {
                Ok(page) => {
                    self.logs_view.fetch_error = None;
                    self.logs_view.next = page.next;
                    for r in page.records {
                        self.logs_view.buf.push_back(r);
                        if self.logs_view.buf.len() > LOGS_RENDER_CAP {
                            self.logs_view.buf.pop_front();
                        }
                    }
                }
                Err(e) => self.logs_view.fetch_error = Some(e.to_string()),
            },
            Err(e) => self.logs_view.fetch_error = Some(e),
        }
    }

    /// Changing filters resets the client buffer + cursor to 0 so the next
    /// fetch re-derives the tail from whatever the ring still holds, under
    /// the new filter — matches the plan's "changing filters resets cursor
    /// and refetches the tail" contract.
    fn reset_logs_filter_state(&mut self) {
        self.logs_view.buf.clear();
        self.logs_view.next = 0;
        self.logs_view.expanded = None;
        self.logs_view.last_fetch = None;
    }

    /// Toggle LAN reachability. Auth is a separate, optional choice (see the
    /// Auth PSK row) — sharing does not require or imply a PSK. Turning sharing
    /// off also drops federation role back to standalone — leaving
    /// `role=master`/`slave` on a loopback-only bind would silently produce a dead
    /// federation link with no error surfaced anywhere.
    fn set_sharing(&mut self, on: bool) {
        self.draft.server.bind_address = if on { "0.0.0.0" } else { "127.0.0.1" }.to_owned();
        if !on {
            self.draft.federation.role = "standalone".to_owned();
        }
        self.confirm_turn_off_sharing = false;
    }

    fn apply_path_edits(&mut self) {
        self.draft.advanced.data_dir = nonempty_path(&self.data_dir_edit);
        self.draft.advanced.bridge_dir = nonempty_path(&self.bridge_dir_edit);
        self.draft.advanced.catalog_path = nonempty_path(&self.catalog_path_edit);
        self.draft.advanced.daemon_bin = nonempty_path(&self.daemon_bin_edit);
    }

    fn save_settings(&mut self) {
        self.apply_path_edits();
        if let Err(e) = cfgfile::validate_remote_auth(&self.draft) {
            self.settings_error = Some(e.to_string());
            return;
        }
        let restart_needed =
            restart_required_fields_changed(&self.settings_loaded_snapshot, &self.draft);
        match cfgfile::save(&self.config_path, &self.draft) {
            Ok(()) => {
                self.settings_error = None;
                self.role_change_note = None;
                self.needs_restart = self.needs_restart || restart_needed;
                self.settings_loaded_snapshot = self.draft.clone();
            }
            Err(e) => self.settings_error = Some(format!("save failed: {e}")),
        }
    }

    fn discard_settings(&mut self) {
        self.settings_error = None;
    }

    fn reset_settings(&mut self) {
        match cfgfile::ensure_default(&self.config_path, true) {
            Ok(_) => self.open_settings(),
            Err(e) => self.settings_error = Some(format!("reset failed: {e}")),
        }
    }

    fn quitting(&self) -> bool {
        self.quit.load(Ordering::SeqCst)
    }

    fn ensure_base(&mut self) {
        if self.admin_base.is_empty() {
            self.admin_base = format!("http://127.0.0.1:{}", tdmcp_config::DEFAULT_PORT);
        }
    }

    fn hide_window(&mut self, ctx: &egui::Context) {
        self.visible = false;
        self.clear_always_on_top_at = None;
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
        ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(
            egui::WindowLevel::Normal,
        ));
    }

    fn show_window(&mut self, ctx: &egui::Context, tray_rect: Option<Rect>) {
        if let Some(r) = tray_rect {
            self.last_tray_rect = Some(r);
        }
        self.visible = true;
        self.hidden_by_focus_loss_at = None;
        self.position_near_tray(ctx);
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
        // Transient always-on-top to win z-order (Docker-style), then drop.
        ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(
            egui::WindowLevel::AlwaysOnTop,
        ));
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        let now = Instant::now();
        self.clear_always_on_top_at = Some(now + Duration::from_millis(150));
        self.ignore_focus_loss_until = Some(now + Duration::from_millis(400));
    }

    /// Tray left-click: open when closed; close when open (or when focus-loss
    /// already closed this gesture — avoids blink-reopen).
    fn on_tray_left_click(&mut self, ctx: &egui::Context, tray_rect: Rect) {
        let now = Instant::now();
        if self
            .last_tray_toggle_at
            .is_some_and(|t| now.duration_since(t) < TRAY_TOGGLE_DEBOUNCE)
        {
            return;
        }
        self.last_tray_toggle_at = Some(now);
        self.last_tray_rect = Some(tray_rect);

        let recently_closed_by_focus = self
            .hidden_by_focus_loss_at
            .is_some_and(|t| now.duration_since(t) < FOCUS_LOSS_CLOSE_GRACE);

        if self.visible || recently_closed_by_focus {
            if self.visible {
                self.hide_window(ctx);
            }
            // Stay hidden — focus-loss already satisfied the close half.
            self.hidden_by_focus_loss_at = None;
        } else {
            self.show_window(ctx, Some(tray_rect));
        }
    }

    fn position_near_tray(&self, ctx: &egui::Context) {
        let Some(rect) = self.last_tray_rect else {
            return;
        };

        // tray-icon reports the icon rect in *physical* pixels, but egui's
        // `OuterPosition` expects *logical* points. On HiDPI displays the raw
        // physical coords land the window off-screen (it still renders, so the
        // taskbar preview shows it, but it's invisible). Convert via the OS
        // scale, then clamp to the current monitor so it can never escape.
        let scale = ctx
            .input(|i| i.viewport().native_pixels_per_point)
            .unwrap_or(1.0) as f64;
        let scale = if scale > 0.0 { scale } else { 1.0 };
        let icon_x = rect.position.x / scale;
        let icon_y = rect.position.y / scale;
        let icon_w = f64::from(rect.size.width) / scale;
        let icon_h = f64::from(rect.size.height) / scale;

        // Use the window's real size (logical points) so the popup is placed with
        // its true footprint and can never walk off the monitor via a stale height.
        let (popup_w, popup_h) = ctx
            .input(|i| i.viewport().outer_rect)
            .map(|r| (f64::from(r.width()), f64::from(r.height())))
            .filter(|(w, h)| *w > 0.0 && *h > 0.0)
            .unwrap_or((f64::from(WINDOW_WIDTH), 360.0));

        // Current monitor bounds in logical points. `monitor_size` is the size of
        // the monitor the window is on (the window was hidden near the tray last,
        // so this is normally the tray's monitor). Derive its origin by tiling the
        // tray position into the monitor grid — good enough for the common
        // same-sized-monitor case, and far better than landing off-screen.
        let mon = ctx
            .input(|i| i.viewport().monitor_size)
            .map(|m| (m.x as f64, m.y as f64));
        let (mon_x, mon_y, mon_w, mon_h) = match mon {
            Some((w, h)) if w > 0.0 && h > 0.0 => {
                let ox = (icon_x / w).floor() * w;
                let oy = (icon_y / h).floor() * h;
                (ox, oy, w, h)
            }
            _ => (0.0, 0.0, f64::INFINITY, f64::INFINITY),
        };

        // Bottom taskbar: grow up-left from icon. Top taskbar: grow down-left.
        // Flush to the dock/taskbar edge (no gap).
        let taskbar_bottom = icon_y > mon_y + mon_h / 2.0;
        let mut x = icon_x + icon_w - popup_w;
        let mut y = if taskbar_bottom {
            icon_y - popup_h
        } else {
            icon_y + icon_h
        };

        // Clamp so the whole popup stays on the monitor.
        x = x.max(mon_x).min(mon_x + mon_w - popup_w);
        y = y.max(mon_y).min(mon_y + mon_h - popup_h);

        ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(egui::pos2(
            x as f32, y as f32,
        )));
    }

    fn poll(&mut self) {
        self.ensure_base();
        let status_ok = match http_get_blocking(&format!("{}/admin/status", self.admin_base), None)
        {
            Ok(body) => {
                self.status = serde_json::from_str(&body).ok();
                self.error = None;
                true
            }
            Err(e) => {
                self.status = None;
                self.error = Some(e);
                false
            }
        };
        match http_get_blocking(&format!("{}/admin/fleet", self.admin_base), None) {
            Ok(body) => {
                self.fleet_json = body;
                self.apply_fleet_status();
            }
            Err(e) => {
                self.error = Some(e.clone());
                push_error_ring(&mut self.error_ring, e);
            }
        }
        match http_get_blocking(&format!("{}/admin/mcp-sessions", self.admin_base), None) {
            Ok(body) => self.sessions_json = body,
            Err(e) => {
                if self.error.is_none() {
                    self.error = Some(e.clone());
                }
                push_error_ring(&mut self.error_ring, e);
            }
        }

        let is_master = self
            .status
            .as_ref()
            .is_some_and(|s| s.role.eq_ignore_ascii_case("master"));
        if is_master {
            let bearer = local_master_psk(&self.draft);
            match http_get_blocking(
                &format!("{}/admin/federation/slaves", self.admin_base),
                bearer.as_deref(),
            ) {
                Ok(body) => {
                    self.slaves_json = body;
                    let slaves = parse_slaves(&self.slaves_json);
                    if self.slaves_seen_once {
                        for s in &slaves {
                            if !self.known_slave_ids.contains(&s.daemon_id) {
                                notify("Slave joined", &s.hostname);
                            }
                        }
                    }
                    self.known_slave_ids = slaves.iter().map(|s| s.daemon_id.clone()).collect();
                    self.slaves_seen_once = true;
                }
                Err(_) => {
                    // Keep last snapshot; auth may be unset on old daemons.
                }
            }
        } else {
            self.slaves_json.clear();
            self.known_slave_ids.clear();
            self.slaves_seen_once = false;
        }

        if status_ok {
            self.fail_polls = 0;
            if !self.startup_notified {
                self.startup_notified = true;
                notify(
                    "td-mcp-rs",
                    &format!("listening on {}", self.admin_base.trim_end_matches('/')),
                );
            }
        } else if !self.startup_notified {
            self.fail_polls = self.fail_polls.saturating_add(1);
            if self.fail_polls >= 2 && !self.startup_fail_notified {
                self.startup_fail_notified = true;
                notify(
                    "td-mcp-rs",
                    "daemon not reachable — check bind / already running",
                );
            }
        }

        self.last_poll = Some(Instant::now());
    }

    fn apply_fleet_status(&mut self) {
        let Ok(fleet) = serde_json::from_str::<FleetView>(&self.fleet_json) else {
            return;
        };
        let mut snap = FleetSnapshot::default();
        for p in &fleet.processes {
            let bridge = p.bridge.as_str().unwrap_or("");
            match bridge {
                "connected" => {
                    snap.connected += 1;
                    snap.connected_pids.push(p.pid);
                }
                "disconnected" => snap.disconnected += 1,
                _ => {}
            }
            if p.resurrected {
                snap.resurrected += 1;
                snap.resurrected_pids.push(p.pid);
            }
            snap.cancelled += p.cancelled_tasks.len();
        }
        snap.cancelled_total = snap.cancelled;

        if self.startup_notified {
            for pid in &snap.resurrected_pids {
                if !self.prev_snapshot.resurrected_pids.contains(pid) {
                    notify(
                        "Bridge resurrected",
                        &format!("pid {pid} reconnected — check cancelled tasks"),
                    );
                    push_error_ring(
                        &mut self.error_ring,
                        format!("bridge resurrected — pid {pid} reconnected"),
                    );
                }
            }
            for pid in &self.prev_snapshot.connected_pids {
                if !snap.connected_pids.contains(pid) {
                    notify(
                        "Bridge disconnected",
                        &format!("pid {pid} lost IPC — tasks cancelled"),
                    );
                    push_error_ring(
                        &mut self.error_ring,
                        format!("bridge disconnected — pid {pid} lost IPC"),
                    );
                }
            }
            if snap.cancelled_total > self.prev_snapshot.cancelled_total {
                let delta = snap.cancelled_total - self.prev_snapshot.cancelled_total;
                notify(
                    "Tasks cancelled",
                    &format!("{delta} task(s) stacked on bridge loss"),
                );
                push_error_ring(
                    &mut self.error_ring,
                    format!("{delta} task(s) cancelled on bridge loss"),
                );
            }
        }

        let needs_attention = snap.disconnected > 0 || snap.resurrected > 0 || snap.cancelled > 0;
        let mcp_n = self
            .status
            .as_ref()
            .map(|s| s.mcp_session_count)
            .unwrap_or(0);
        let tooltip = if snap.connected + snap.disconnected == 0 && mcp_n == 0 {
            "td-mcp-rs — no connections".to_owned()
        } else if needs_attention {
            format!(
                "td-mcp-rs — MCP {mcp_n}, TD {} connected, {} attention",
                snap.connected,
                snap.disconnected + snap.resurrected + snap.cancelled
            )
        } else {
            format!("td-mcp-rs — MCP {mcp_n}, TD {} connected", snap.connected)
        };

        if let Some(tray) = &self.tray {
            let _ = tray.set_tooltip(Some(&tooltip));
            if needs_attention != self.attention {
                let icon = if needs_attention {
                    &self.icon_attention
                } else {
                    &self.icon_normal
                };
                if let Ok(ti) = tray_icon_from(icon) {
                    let _ = tray.set_icon(Some(ti));
                }
                self.attention = needs_attention;
            }
        }

        self.prev_snapshot = snap;
    }

    fn shutdown_daemon(&mut self) {
        self.ensure_base();
        let _ = http_post_blocking(&format!("{}/admin/shutdown", self.admin_base), None, None);
    }

    fn restart_daemon(&mut self) {
        self.ensure_base();
        let _ = http_post_blocking(&format!("{}/admin/restart", self.admin_base), None, None);
    }

    fn reveal_tox(&self) {
        if let Err(e) = reveal_in_file_manager(&self.data_dir.join("bootstrap.tox"), &self.data_dir)
        {
            warn!(error = %e, "reveal bootstrap.tox failed");
        }
    }

    fn handle_tray_events(&mut self, ctx: &egui::Context) {
        while let Ok(event) = TrayIconEvent::receiver().try_recv() {
            match event {
                // Left Click{Up} only — ignore DoubleClick (would double-toggle).
                TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    rect,
                    ..
                } => {
                    self.on_tray_left_click(ctx, rect);
                }
                TrayIconEvent::Click {
                    button: MouseButton::Right,
                    rect,
                    ..
                } => {
                    // Keep rect for later show; OS menu handles Restart/Stop.
                    self.last_tray_rect = Some(rect);
                }
                _ => {}
            }
        }
        while let Ok(event) = MenuEvent::receiver().try_recv() {
            if event.id == self.menu_restart.id() {
                self.restart_daemon();
            } else if event.id == self.menu_stop.id() {
                self.shutdown_daemon();
            } else if event.id == self.menu_dashboard.id() {
                self.dashboard_open = true;
            }
        }
    }

    fn handle_close_request(&mut self, ctx: &egui::Context) {
        if !ctx.input(|i| i.viewport().close_requested()) {
            return;
        }
        if self.quitting() {
            // Real shutdown — allow the viewport to close.
            return;
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
        self.hide_window(ctx);
    }

    fn handle_focus_loss(&mut self, ctx: &egui::Context) {
        if !self.visible {
            return;
        }
        // Any focus loss hides the popup; editing happens in the dashboard.
        if self
            .ignore_focus_loss_until
            .is_some_and(|t| Instant::now() < t)
        {
            return;
        }
        let focused = ctx.input(|i| i.viewport().focused);
        if focused == Some(false) {
            self.hide_window(ctx);
            self.hidden_by_focus_loss_at = Some(Instant::now());
        }
    }

    /// Top chrome: LED + identity (title · version) left, Stop/Restart/.tox/gear right.
    /// The identity block is width-capped and clipped so a long title can never
    /// slide under the right-anchored actions; pid/bind live on the version tooltip.
    fn draw_header(&mut self, ui: &mut egui::Ui) {
        let full = ui.available_width();
        let (rect, _) = ui.allocate_exact_size(egui::vec2(full, HEADER_H), egui::Sense::hover());
        ui.painter().rect_filled(rect, 0.0, BG_PANEL);
        ui.painter().hline(
            rect.x_range(),
            rect.bottom(),
            egui::Stroke::new(1.0, BORDER),
        );

        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(rect.shrink2(egui::vec2(SIDE_MARGIN, 0.0)))
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        status_led(&mut child, ACCENT);
        child.add_space(6.0);

        let title = match self.status.as_ref().map(|s| s.role.as_str()) {
            Some("master") => "td-mcp-rs · master",
            Some("slave") => "td-mcp-rs · slave",
            _ => "td-mcp-rs",
        };
        let version = self
            .status
            .as_ref()
            .map(|s| s.version.clone())
            .unwrap_or_default();
        let meta_tip = self.status.as_ref().map(|st| {
            let bind = st.bind_address.as_str();
            let bind = if bind.is_empty() {
                String::new()
            } else if cfgfile::is_loopback_bind(bind) {
                format!(" · {bind} (loopback)")
            } else {
                format!(" · {bind} (remote)")
            };
            format!("pid {}{bind}", st.pid)
        });
        let id_w = (child.available_width() - HEADER_ACTIONS_W).max(64.0);
        child.allocate_ui_with_layout(
            egui::vec2(id_w, HEADER_H),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                ui.set_clip_rect(ui.max_rect());
                ui.label(egui::RichText::new(title).font(font_title()).color(TEXT));
                if !version.is_empty() {
                    ui.add_space(4.0);
                    let meta = ui.label(
                        egui::RichText::new(format!("v{version}"))
                            .font(font_meta())
                            .color(TEXT_DIM),
                    );
                    if let Some(tip) = &meta_tip {
                        let _ = meta.on_hover_text(tip.clone());
                    }
                }
            },
        );

        // Right-anchored ghost actions: Stop · Restart · .tox · gear (RTL).
        child.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let stop = ghost_button(ui, "■", TEXT_DIM, ERR).on_hover_text("Stop daemon");
            if stop.clicked() {
                self.shutdown_daemon();
            }
            ui.add_space(2.0);
            let restart = ghost_button(ui, "↻", TEXT_DIM, ACCENT).on_hover_text("Restart daemon");
            if restart.clicked() {
                self.restart_daemon();
            }
            ui.add_space(4.0);
            let tox_path = self.data_dir.join("bootstrap.tox");
            let tip = format!("Reveal {}", tox_path.display());
            let tox = ghost_button(ui, ".tox", TEXT_DIM, ACCENT).on_hover_text(tip);
            if tox.clicked() {
                self.reveal_tox();
            }
            ui.add_space(4.0);
            let gear = ghost_button(ui, "⚙", TEXT_DIM, ACCENT).on_hover_text("Settings");
            if gear.clicked() {
                self.open_settings();
                self.dash_tab = dashboard::DashTab::Settings;
                self.dashboard_open = true;
            }
            ui.add_space(2.0);
            let dash_logs = self.dashboard_open && self.dash_tab == dashboard::DashTab::Logs;
            let logs_color = if dash_logs { ACCENT } else { TEXT_DIM };
            let logs = ghost_button(ui, "≡", logs_color, ACCENT).on_hover_text("Logs");
            if logs.clicked() {
                self.dash_tab = dashboard::DashTab::Logs;
                self.dashboard_open = true;
            }
            ui.add_space(4.0);
            // Dashboard launcher (leftmost header action).
            let dash_active = self.dashboard_open;
            let dash_color = if dash_active { ACCENT } else { TEXT_DIM };
            let dash = ghost_button(ui, "⤢", dash_color, ACCENT).on_hover_text("Open dashboard");
            if dash.clicked() {
                self.dashboard_open = true;
            }
        });
    }

    fn field_help(key: &str) -> &'static str {
        FIELD_DESCS
            .iter()
            .find(|f| f.key == key)
            .map(|f| f.help)
            .unwrap_or("")
    }

    /// Fleet-view nudge toward federation for a not-yet-shared daemon — today
    /// nothing on the main screen hints the feature exists until Settings is opened.
    fn draw_share_banner(&mut self, ui: &mut egui::Ui) {
        let Some(status) = &self.status else { return };
        let role_ok = status.role.is_empty() || status.role == "standalone";
        if !role_ok || !cfgfile::is_loopback_bind(&status.bind_address) {
            return;
        }
        ui.horizontal(|ui| {
            ui.add_space(SIDE_MARGIN);
            if ghost_button(ui, "Share this daemon on your network →", TEXT_DIM, ACCENT).clicked()
            {
                self.open_settings();
                self.dash_tab = dashboard::DashTab::Settings;
                self.dashboard_open = true;
            }
        });
        ui.add_space(2.0);
    }

    fn draw_mcp_section(&self, ui: &mut egui::Ui) {
        section_header(ui, "MCP CLIENTS");
        let sessions = serde_json::from_str::<SessionsView>(&self.sessions_json)
            .map(|v| v.sessions)
            .unwrap_or_default();
        if sessions.is_empty() {
            ui.vertical_centered(|ui| {
                ui.add_space(8.0);
                ui.label(
                    egui::RichText::new("No MCP clients connected")
                        .font(font_meta())
                        .color(TEXT_DIM),
                );
                ui.add_space(8.0);
            });
            return;
        }
        for (i, s) in sessions.iter().enumerate() {
            let bg = if i.is_multiple_of(2) {
                BG_ROW
            } else {
                BG_ROW_ALT
            };
            let full = ui.available_width();
            let (rect, response) =
                ui.allocate_exact_size(egui::vec2(full, 24.0), egui::Sense::hover());
            let fill = if response.hovered() { BG_HOVER } else { bg };
            ui.painter().rect_filled(rect, 0.0, fill);
            ui.painter().hline(
                rect.x_range(),
                rect.bottom(),
                egui::Stroke::new(1.0, BORDER),
            );

            let mut child = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(rect.shrink2(egui::vec2(12.0, 0.0)))
                    .layout(egui::Layout::left_to_right(egui::Align::Center)),
            );
            status_led(&mut child, OK);
            child.add_space(6.0);
            let id_tail = id_tail(&s.id);
            child.label(
                egui::RichText::new(id_tail)
                    .font(font_mono())
                    .color(TEXT_FAINT),
            );
            child.add_space(8.0);
            child.label(
                egui::RichText::new(&s.client_name)
                    .font(font_label())
                    .color(TEXT),
            );
            child.add_space(8.0);
            if !s.client_version.is_empty() {
                child.label(
                    egui::RichText::new(&s.client_version)
                        .font(font_mono())
                        .color(TEXT_DIM),
                );
            }
            child.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    egui::RichText::new(format_duration_since(s.connected_at))
                        .font(font_mono())
                        .color(TEXT_DIM),
                );
            });
        }
    }

    fn draw_td_section(&mut self, ui: &mut egui::Ui) {
        section_header(ui, "TOUCHDESIGNER");
        let role = self
            .status
            .as_ref()
            .map(|s| s.role.clone())
            .unwrap_or_default();
        if role == "master" {
            self.draw_master_actions(ui);
            self.draw_fleet_groups(ui);
            self.draw_scan_results(ui, ScanPurpose::AddSlave);
        } else {
            self.draw_flat_fleet(ui);
        }
        if role == "slave" {
            self.draw_slave_self_view(ui);
        }
    }

    fn draw_master_actions(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.add_space(12.0);
            if ghost_button(ui, "+ Add slave", TEXT_DIM, ACCENT).clicked() {
                self.fleet_panel = FleetPanel::AddSlave;
            }
            ui.add_space(4.0);
            if self.scan_busy && self.scan_purpose == ScanPurpose::AddSlave {
                ui.label(
                    egui::RichText::new("scanning…")
                        .font(font_meta())
                        .color(TEXT_DIM),
                );
            } else if ghost_button(ui, "Scan", TEXT_DIM, ACCENT).clicked() {
                self.start_scan(self.add_slave_port, ScanPurpose::AddSlave);
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    egui::RichText::new(format!("{} slave(s)", self.slave_count()))
                        .font(font_meta())
                        .color(TEXT_DIM),
                );
            });
        });
        ui.add_space(2.0);
    }

    /// Master fleet: one collapsible group per daemon (local first, then slaves).
    fn draw_fleet_groups(&mut self, ui: &mut egui::Ui) {
        let Ok(fleet) = serde_json::from_str::<FleetView>(&self.fleet_json) else {
            self.draw_empty_fleet(ui);
            return;
        };
        if fleet.processes.is_empty() {
            self.draw_empty_fleet(ui);
            return;
        }
        let local_id = self
            .status
            .as_ref()
            .map(|s| s.daemon_id.clone())
            .unwrap_or_default();
        let slaves = parse_slaves(&self.slaves_json);

        let mut order: Vec<String> = Vec::new();
        let mut groups: HashMap<String, Vec<&FleetProc>> = HashMap::new();
        for p in &fleet.processes {
            let key = p
                .daemon_id
                .clone()
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| local_id.clone());
            if !groups.contains_key(&key) {
                order.push(key.clone());
            }
            groups.entry(key).or_default().push(p);
        }
        // Local group first, then slaves in first-seen order.
        order.sort_by_key(|k| k != &local_id);

        let mut pending_settings: Option<SlaveSettingsTarget> = None;
        for key in &order {
            let Some(procs) = groups.get(key) else {
                continue;
            };
            let is_local = key == &local_id;
            let slave = slaves.iter().find(|s| &s.daemon_id == key);
            let (led, reach) = if is_local {
                (ACCENT, "local".to_owned())
            } else if let Some(s) = slave {
                match s.reachability.as_str() {
                    "reachable" => (OK, "reachable".to_owned()),
                    "disconnected" => (WARN, "disconnected".to_owned()),
                    _ => (ERR, "unreachable".to_owned()),
                }
            } else {
                (TEXT_FAINT, "unknown".to_owned())
            };
            let hostname = if is_local {
                self.status
                    .as_ref()
                    .map(|s| s.hostname.clone())
                    .unwrap_or_default()
            } else {
                slave.map(|s| s.hostname.clone()).unwrap_or_else(|| {
                    procs
                        .first()
                        .and_then(|p| p.hostname.clone())
                        .unwrap_or_default()
                })
            };
            let tail = if key.is_empty() {
                String::new()
            } else {
                id_tail(key)
            };
            let count = if is_local {
                String::new()
            } else {
                format!(
                    " · {} proc",
                    slave.map(|s| s.process_count).unwrap_or(procs.len())
                )
            };
            let label = if is_local {
                format!("LOCAL · {hostname} · {tail}")
            } else {
                format!("SLAVE · {hostname} · {tail} · {reach}{count}")
            };
            let header = egui::RichText::new(format!("● {label}"))
                .font(font_label())
                .color(if is_local { TEXT } else { led });
            egui::CollapsingHeader::new(header)
                .id_salt(key.as_str())
                .default_open(true)
                .show(ui, |ui| {
                    ui.add_space(2.0);
                    if let Some(s) = slave {
                        ui.horizontal(|ui| {
                            ui.add_space(12.0);
                            ui.label(
                                egui::RichText::new(s.base_url.as_str())
                                    .font(font_mono())
                                    .color(TEXT_FAINT),
                            );
                            ui.add_space(4.0);
                            ui.label(
                                egui::RichText::new(format!("v{}", s.version))
                                    .font(font_meta())
                                    .color(TEXT_FAINT),
                            );
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if ghost_button(ui, "⚙", TEXT_DIM, ACCENT)
                                        .on_hover_text("Slave settings via /admin/config")
                                        .clicked()
                                    {
                                        pending_settings = Some(SlaveSettingsTarget {
                                            daemon_id: s.daemon_id.clone(),
                                            hostname: s.hostname.clone(),
                                            base_url: s.base_url.clone(),
                                            auth_token: s.auth_token.clone(),
                                        });
                                    }
                                },
                            );
                        });
                        ui.add_space(2.0);
                    }
                    for (i, p) in procs.iter().enumerate() {
                        fleet_row(ui, p, i);
                    }
                });
        }
        if let Some(target) = pending_settings {
            self.open_slave_settings(target);
        }
    }

    /// Standalone / slave: local processes as one flat list.
    fn draw_flat_fleet(&self, ui: &mut egui::Ui) {
        let Ok(fleet) = serde_json::from_str::<FleetView>(&self.fleet_json) else {
            self.draw_empty_fleet(ui);
            return;
        };
        if fleet.processes.is_empty() {
            self.draw_empty_fleet(ui);
            return;
        }
        for (i, p) in fleet.processes.iter().enumerate() {
            fleet_row(ui, p, i);
        }
    }

    fn draw_empty_fleet(&self, ui: &mut egui::Ui) {
        ui.vertical_centered(|ui| {
            ui.add_space(8.0);
            ui.label(
                egui::RichText::new("No TouchDesigner bridges")
                    .font(font_meta())
                    .color(TEXT_DIM),
            );
            ui.add_space(8.0);
        });
    }

    /// Renders `self.scan_results` only when they belong to `purpose` — the
    /// master's "find a slave" scan and a joiner's "find a master" scan share one
    /// result set (see [`ScanPurpose`]) but must never bleed into each other's UI.
    fn draw_scan_results(&mut self, ui: &mut egui::Ui, purpose: ScanPurpose) {
        if self.scan_purpose != purpose || self.scan_results.is_empty() {
            return;
        }
        let hits: Vec<&ScanHit> = self
            .scan_results
            .iter()
            .filter(|h| purpose != ScanPurpose::JoinMaster || h.role == "master")
            .collect();
        if hits.is_empty() {
            return;
        }
        ui.add_space(6.0);
        section_header(ui, &format!("SCAN · {} hit(s)", hits.len()));
        let mut use_hit: Option<(String, u16)> = None;
        for (i, hit) in hits.iter().enumerate() {
            let bg = if i.is_multiple_of(2) {
                BG_ROW
            } else {
                BG_ROW_ALT
            };
            let full = ui.available_width();
            let (rect, response) =
                ui.allocate_exact_size(egui::vec2(full, 24.0), egui::Sense::hover());
            let fill = if response.hovered() { BG_HOVER } else { bg };
            ui.painter().rect_filled(rect, 0.0, fill);
            ui.painter().hline(
                rect.x_range(),
                rect.bottom(),
                egui::Stroke::new(1.0, BORDER),
            );

            let mut child = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(rect.shrink2(egui::vec2(12.0, 0.0)))
                    .layout(egui::Layout::left_to_right(egui::Align::Center)),
            );
            let led = if hit.role == "slave" {
                OK
            } else if hit.role == "master" {
                ACCENT
            } else {
                TEXT_FAINT
            };
            status_led(&mut child, led);
            child.add_space(6.0);
            child.label(
                egui::RichText::new(&hit.host)
                    .font(font_mono())
                    .color(TEXT_FAINT),
            );
            child.add_space(8.0);
            child.label(
                egui::RichText::new(format!(
                    "{} · {} · v{}",
                    hit.role, hit.hostname, hit.version
                ))
                .font(font_label())
                .color(TEXT),
            );
            child.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let hover = match purpose {
                    ScanPurpose::AddSlave => "Open add-slave with this host",
                    ScanPurpose::JoinMaster => "Use this master's URL",
                };
                if ghost_button(ui, "use", TEXT_DIM, ACCENT)
                    .on_hover_text(hover)
                    .clicked()
                {
                    use_hit = Some((hit.host.clone(), hit.port));
                }
                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new(id_tail(&hit.daemon_id))
                        .font(font_mono())
                        .color(TEXT_DIM),
                );
            });
        }
        if let Some((host, port)) = use_hit {
            match purpose {
                ScanPurpose::AddSlave => {
                    self.add_slave_host = host;
                    self.add_slave_port = port;
                    self.fleet_panel = FleetPanel::AddSlave;
                }
                ScanPurpose::JoinMaster => {
                    self.draft.federation.master_url = format!("http://{host}:{port}");
                    self.focus_master_psk = true;
                }
            }
        }
    }

    fn start_scan(&mut self, port: u16, purpose: ScanPurpose) {
        if self.scan_busy && self.scan_purpose == purpose {
            return;
        }
        let Some(ip) = local_ip() else {
            self.error = Some("cannot determine local subnet for scan".to_owned());
            return;
        };
        let Some(prefix) = ip_prefix(&ip) else {
            self.error = Some(format!("unexpected local IP {ip}"));
            return;
        };
        self.scan_purpose = purpose;
        let (tx, rx) = std::sync::mpsc::channel::<Vec<ScanHit>>();
        let spawned = std::thread::Builder::new()
            .name("tdmcp-scan".to_owned())
            .spawn(move || {
                let hits = scan_subnet(&prefix, port);
                let _ = tx.send(hits);
            });
        if spawned.is_err() {
            return;
        }
        self.scan_rx = Some(rx);
        self.scan_busy = true;
        self.scan_results.clear();
    }

    fn probe_slave(&mut self) {
        let host = self.add_slave_host.trim().to_owned();
        if host.is_empty() {
            self.add_slave_message = Some("enter host".to_owned());
            return;
        }
        let url = format!(
            "http://{host}:{}/admin/federation/status",
            self.add_slave_port
        );
        match http_get_blocking(&url, None) {
            Ok(body) => match serde_json::from_str::<FederationProbe>(&body) {
                Ok(probe) => {
                    self.add_slave_probe = Some(probe);
                    self.add_slave_message = None;
                }
                Err(_) => {
                    self.add_slave_probe = None;
                    self.add_slave_message =
                        Some("probe reply is not a federation daemon".to_owned());
                }
            },
            Err(e) => {
                self.add_slave_probe = None;
                self.add_slave_message = Some(format!("probe failed: {e}"));
            }
        }
    }

    /// Configure the probed daemon as a slave of this master via its `/admin/config`.
    fn add_as_slave(&mut self) {
        let host = self.add_slave_host.trim().to_owned();
        if host.is_empty() {
            self.add_slave_message = Some("enter host".to_owned());
            return;
        }
        let (master_url, master_psk) = self.master_federation_values();
        let body = serde_json::json!({
            "federation": {
                "role": "slave",
                "masterUrl": master_url,
                "masterPsk": master_psk,
            }
        });
        let url = format!("http://{host}:{}/admin/config", self.add_slave_port);
        let bearer = nonempty_opt(&self.add_slave_psk);
        match http_post_blocking(&url, bearer.as_deref(), Some(&body)) {
            Ok(v)
                if v.get("ok")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false) =>
            {
                self.add_slave_message = Some("configured — restart the slave to apply".to_owned());
            }
            Ok(v) => {
                self.add_slave_message = Some(
                    v.get("error")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("config save rejected")
                        .to_owned(),
                );
            }
            Err(e) => self.add_slave_message = Some(format!("config failed: {e}")),
        }
    }

    /// URL + psk to advertise to a new slave (hostname + local port).
    fn master_federation_values(&self) -> (String, String) {
        let hostname = self
            .status
            .as_ref()
            .map(|s| s.hostname.clone())
            .filter(|h| !h.is_empty())
            .unwrap_or_else(|| "localhost".to_owned());
        (
            format!("http://{hostname}:{}", port_from_base(&self.admin_base)),
            self.draft.auth.psk.clone(),
        )
    }

    fn slave_count(&self) -> usize {
        parse_slaves(&self.slaves_json).len()
    }

    fn open_slave_settings(&mut self, target: SlaveSettingsTarget) {
        self.slave_settings_target = Some(target);
        self.slave_settings_error = None;
        self.fleet_panel = FleetPanel::SlaveSettings;
        self.load_slave_settings();
    }

    fn load_slave_settings(&mut self) {
        let Some(target) = &self.slave_settings_target else {
            return;
        };
        let bearer = nonempty_opt(&target.auth_token);
        match http_get_blocking(
            &format!("{}/admin/config", target.base_url),
            bearer.as_deref(),
        ) {
            Ok(body) => {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&body) {
                    if let Some(b) = v.get("bridge") {
                        if let Some(t) = b
                            .get("call_timeout_secs")
                            .or_else(|| b.get("callTimeoutSecs"))
                            .and_then(serde_json::Value::as_u64)
                        {
                            self.slave_settings_call_timeout = t;
                        }
                        if let Some(t) = b
                            .get("script_timeout_secs")
                            .or_else(|| b.get("scriptTimeoutSecs"))
                            .and_then(serde_json::Value::as_u64)
                        {
                            self.slave_settings_script_timeout = t;
                        }
                    }
                    self.slave_settings_error = None;
                } else {
                    self.slave_settings_error = Some("config reply is not JSON".to_owned());
                }
            }
            Err(e) => self.slave_settings_error = Some(format!("load failed: {e}")),
        }
    }

    fn save_slave_settings(&mut self) {
        let Some(target) = &self.slave_settings_target else {
            return;
        };
        let body = serde_json::json!({
            "bridge": {
                "call_timeout_secs": self.slave_settings_call_timeout,
                "script_timeout_secs": self.slave_settings_script_timeout,
            }
        });
        let bearer = nonempty_opt(&target.auth_token);
        match http_post_blocking(
            &format!("{}/admin/config", target.base_url),
            bearer.as_deref(),
            Some(&body),
        ) {
            Ok(v)
                if v.get("ok")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false) =>
            {
                let hostname = target.hostname.clone();
                self.slave_settings_error = None;
                self.fleet_panel = FleetPanel::None;
                notify(
                    "Slave settings",
                    &format!("{hostname} saved — applies after slave restart"),
                );
            }
            Ok(v) => {
                self.slave_settings_error = Some(
                    v.get("error")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("save rejected")
                        .to_owned(),
                );
            }
            Err(e) => self.slave_settings_error = Some(format!("save failed: {e}")),
        }
    }

    fn draw_add_slave_panel(&mut self, ui: &mut egui::Ui) {
        section_header(ui, "ADD SLAVE");
        ui.horizontal(|ui| {
            ui.add_space(12.0);
            if ghost_button(ui, "← Back", TEXT_DIM, TEXT).clicked() {
                self.fleet_panel = FleetPanel::None;
            }
        });
        ui.add_space(4.0);
        settings_row(ui, "Host", "IP or hostname of the slave daemon", |ui| {
            ui.add_sized(
                egui::vec2(ui.available_width().min(160.0), 20.0),
                egui::TextEdit::singleline(&mut self.add_slave_host).font(font_mono()),
            );
        });
        settings_row(ui, "Port", "slave daemon listen port", |ui| {
            ui.add(
                egui::DragValue::new(&mut self.add_slave_port)
                    .range(1..=65535)
                    .speed(1),
            );
        });
        settings_row(
            ui,
            "Slave PSK",
            "slave auth.psk (needed to write /admin/config)",
            |ui| {
                ui.add_sized(
                    egui::vec2(ui.available_width().min(140.0), 20.0),
                    egui::TextEdit::singleline(&mut self.add_slave_psk)
                        .font(font_mono())
                        .password(!self.show_add_slave_psk),
                );
                if ghost_button(
                    ui,
                    if self.show_add_slave_psk {
                        "hide"
                    } else {
                        "show"
                    },
                    TEXT_DIM,
                    TEXT,
                )
                .clicked()
                {
                    self.show_add_slave_psk = !self.show_add_slave_psk;
                }
            },
        );
        ui.add_space(2.0);
        if let Some(probe) = &self.add_slave_probe {
            let led = if probe.role == "slave" {
                OK
            } else if probe.role == "master" {
                ACCENT
            } else {
                TEXT_FAINT
            };
            ui.horizontal(|ui| {
                ui.add_space(12.0);
                status_led(ui, led);
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new(format!(
                        "{} · v{} · {} · {}",
                        probe.role,
                        probe.version,
                        probe.hostname,
                        id_tail(&probe.daemon_id)
                    ))
                    .font(font_meta())
                    .color(TEXT),
                );
            });
            if probe.role == "master" {
                ui.horizontal(|ui| {
                    ui.add_space(12.0);
                    ui.colored_label(WARN, "target is a master — cannot act as slave");
                });
            }
        }
        if let Some(msg) = &self.add_slave_message {
            let color = if msg.starts_with("configured") {
                OK
            } else {
                ERR
            };
            ui.horizontal(|ui| {
                ui.add_space(12.0);
                ui.colored_label(color, msg.clone());
            });
        }
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.add_space(12.0);
            if ghost_button(ui, "Probe", TEXT_DIM, ACCENT)
                .on_hover_text("GET /admin/federation/status (unauth)")
                .clicked()
            {
                self.probe_slave();
            }
            ui.add_space(4.0);
            if filled_button(ui, "Add as slave")
                .on_hover_text("Writes role=slave via the slave's /admin/config")
                .clicked()
            {
                self.add_as_slave();
            }
        });
    }

    fn draw_slave_settings_panel(&mut self, ui: &mut egui::Ui) {
        section_header(ui, "SLAVE SETTINGS");
        ui.horizontal(|ui| {
            ui.add_space(12.0);
            if ghost_button(ui, "← Back", TEXT_DIM, TEXT).clicked() {
                self.fleet_panel = FleetPanel::None;
            }
        });
        let Some((hostname, daemon_id, base_url)) = self.slave_settings_target.as_ref().map(|t| {
            (
                t.hostname.clone(),
                id_tail(&t.daemon_id),
                t.base_url.clone(),
            )
        }) else {
            return;
        };
        ui.add_space(2.0);
        ui.horizontal(|ui| {
            ui.add_space(12.0);
            ui.label(
                egui::RichText::new(format!("{hostname} · {daemon_id}"))
                    .font(font_label())
                    .color(TEXT),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    egui::RichText::new(&base_url)
                        .font(font_mono())
                        .color(TEXT_FAINT),
                );
            });
        });
        ui.add_space(4.0);
        if let Some(err) = &self.slave_settings_error {
            ui.horizontal(|ui| {
                ui.add_space(12.0);
                ui.colored_label(ERR, err.clone());
            });
        }
        ui.add_space(2.0);
        settings_row(
            ui,
            "Call timeout (s)",
            "bridge.call_timeout_secs on the slave",
            |ui| {
                ui.add(
                    egui::DragValue::new(&mut self.slave_settings_call_timeout)
                        .range(1..=600)
                        .speed(1),
                );
            },
        );
        settings_row(
            ui,
            "Script timeout (s)",
            "bridge.script_timeout_secs on the slave",
            |ui| {
                ui.add(
                    egui::DragValue::new(&mut self.slave_settings_script_timeout)
                        .range(1..=600)
                        .speed(1),
                );
            },
        );
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.add_space(12.0);
            ui.label(
                egui::RichText::new("Changes apply after the slave restarts.")
                    .font(font_meta())
                    .color(TEXT_DIM),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if filled_button(ui, "Save").clicked() {
                    self.save_slave_settings();
                }
            });
        });
    }

    /// Slave self-view: master link + Go standalone (saves role, restarts locally).
    fn draw_slave_self_view(&mut self, ui: &mut egui::Ui) {
        ui.add_space(6.0);
        section_header(ui, "FEDERATION");
        let master_url = self.draft.federation.master_url.clone();
        let daemon_id = id_tail(&self.draft.federation.daemon_id);
        settings_row(
            ui,
            "Master",
            Self::field_help("federation.master_url"),
            |ui| {
                ui.label(
                    egui::RichText::new(&master_url)
                        .font(font_mono())
                        .color(TEXT_DIM),
                );
            },
        );
        settings_row(
            ui,
            "Daemon ID",
            Self::field_help("federation.daemon_id"),
            |ui| {
                ui.label(
                    egui::RichText::new(daemon_id.clone())
                        .font(font_mono())
                        .color(TEXT_FAINT),
                );
            },
        );
        if let Some(msg) = &self.slave_self_message {
            ui.horizontal(|ui| {
                ui.add_space(12.0);
                ui.colored_label(WARN, msg.clone());
            });
        }
        ui.add_space(2.0);
        ui.horizontal(|ui| {
            ui.add_space(12.0);
            if self.confirm_go_standalone {
                ui.label(
                    egui::RichText::new("Go standalone? the daemon restarts to apply.")
                        .font(font_meta())
                        .color(WARN),
                );
                ui.add_space(4.0);
                if filled_button(ui, "Confirm").clicked() {
                    self.go_standalone();
                }
                ui.add_space(4.0);
                if ghost_button(ui, "Cancel", TEXT_DIM, TEXT).clicked() {
                    self.confirm_go_standalone = false;
                }
            } else if ghost_button(ui, "Go standalone", TEXT_DIM, WARN)
                .on_hover_text("role=standalone; saves config and restarts this daemon")
                .clicked()
            {
                self.confirm_go_standalone = true;
            }
        });
    }

    fn go_standalone(&mut self) {
        self.ensure_base();
        let bearer = local_master_psk(&self.draft);
        let body = serde_json::json!({ "federation": { "role": "standalone" } });
        let url = format!("{}/admin/config", self.admin_base);
        match http_post_blocking(&url, bearer.as_deref(), Some(&body)) {
            Ok(v)
                if v.get("ok")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false) =>
            {
                self.confirm_go_standalone = false;
                self.slave_self_message = Some("role saved — restarting".to_owned());
                self.restart_daemon();
            }
            Ok(v) => {
                self.confirm_go_standalone = false;
                self.slave_self_message = Some(
                    v.get("error")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("config save rejected")
                        .to_owned(),
                );
            }
            Err(e) => {
                self.confirm_go_standalone = false;
                self.slave_self_message = Some(format!("config failed: {e}"));
            }
        }
    }
}

impl eframe::App for DashboardApp {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if self.quitting() {
            // Drop the tray before closing so the icon does not linger.
            self.tray = None;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            self.handle_close_request(ctx);
            return;
        }
        self.ensure_tray();
        if self.pending_initial_hide {
            self.pending_initial_hide = false;
            self.hide_window(ctx);
        }
        if let Some(at) = self.clear_always_on_top_at {
            if Instant::now() >= at {
                ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(
                    egui::WindowLevel::Normal,
                ));
                self.clear_always_on_top_at = None;
            }
        }
        self.handle_close_request(ctx);
        self.handle_tray_events(ctx);
        self.handle_focus_loss(ctx);
        let due = self
            .last_poll
            .is_none_or(|t| t.elapsed() > Duration::from_secs(2));
        if due {
            self.poll();
        }
        self.fetch_logs_if_due();
        let scan_hits = self.scan_rx.as_ref().and_then(|rx| rx.try_recv().ok());
        if let Some(hits) = scan_hits {
            self.scan_results = hits;
            self.scan_busy = false;
            self.scan_rx = None;
        }
        if self.dashboard_open {
            let vb = dashboard::builder(&self.window_icon);
            let id = dashboard::viewport_id();
            ctx.show_viewport_immediate(id, vb, |ui, _class| {
                dashboard::render(self, ui);
            });
        }
        ctx.request_repaint_after(Duration::from_millis(250));
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default()
            .frame(
                egui::Frame::NONE
                    .fill(theme::BG_WINDOW)
                    .stroke(egui::Stroke::new(1.0, BORDER))
                    .inner_margin(0.0),
            )
            .show(ui, |ui| {
                self.draw_header(ui);
                if let Some(err) = &self.error {
                    ui.horizontal(|ui| {
                        ui.add_space(SIDE_MARGIN);
                        ui.colored_label(ERR, err);
                    });
                }
                error_strip(ui, &self.error_ring);
                self.draw_share_banner(ui);
                egui::ScrollArea::vertical()
                    .auto_shrink(false)
                    .max_height(WINDOW_MAX_HEIGHT - 60.0)
                    .show(ui, |ui| {
                        self.draw_mcp_section(ui);
                        self.draw_td_section(ui);
                    });
                ui.add_space(6.0);
            });
    }
}

/// One settings row: label left (measured width), control right-aligned.
/// Fixed height, symmetric side margins and an explicit control column, so
/// controls never touch the window edge, wrap under the label, or clip on
/// any platform. The help tooltip rides the whole row when one is given.
fn settings_row(ui: &mut egui::Ui, label: &str, help: &str, add: impl FnOnce(&mut egui::Ui)) {
    // The window is a fixed width; bound the row to it so a reported
    // available_width larger than the actually-painted viewport (seen on some
    // macOS DPI paths) can never push controls past the window's right edge.
    let full = ui.available_width().min(WINDOW_WIDTH);
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(full, SETTINGS_ROW_H), egui::Sense::hover());
    if response.hovered() {
        ui.painter().rect_filled(rect, 0.0, BG_HOVER);
    }

    let inner = egui::Rect::from_min_size(
        egui::pos2(rect.left() + SIDE_MARGIN, rect.top()),
        egui::vec2((full - SIDE_MARGIN * 2.0).max(0.0), SETTINGS_ROW_H),
    );
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(inner)
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );

    // Label column — exactly as wide as the text (+ gap), so short labels
    // leave more room for the control and long ones never wrap or overlap.
    let label_g = child
        .painter()
        .layout_no_wrap(label.to_owned(), font_label(), TEXT);
    let label_w = (label_g.size().x + 8.0).max(96.0);
    child.allocate_ui_with_layout(
        egui::vec2(label_w, SETTINGS_ROW_H),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            ui.label(egui::RichText::new(label).font(font_label()).color(TEXT));
        },
    );

    // Control column — fills the exact remaining inner width, right-aligned.
    // Computed from the row bounds rather than `available_width()` so a
    // platform width quirk can never make the column wider than the row.
    let control_w = (inner.width() - label_w).max(0.0);
    child.allocate_ui_with_layout(
        egui::vec2(control_w, SETTINGS_ROW_H),
        egui::Layout::right_to_left(egui::Align::Center),
        |ui| add(ui),
    );

    if !help.is_empty() {
        response.on_hover_text(help);
    }
}

fn path_edits_from(cfg: &ConfigFile) -> (String, String, String, String) {
    (
        cfg.advanced
            .data_dir
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_default(),
        cfg.advanced
            .bridge_dir
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_default(),
        cfg.advanced
            .catalog_path
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_default(),
        cfg.advanced
            .daemon_bin
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_default(),
    )
}

fn nonempty_path(s: &str) -> Option<PathBuf> {
    let t = s.trim();
    if t.is_empty() {
        None
    } else {
        Some(PathBuf::from(t))
    }
}

/// Show an OS toast (best-effort; failures are logged).
///
/// On macOS, `notify-rust` talks to AppKit/`NSUserNotification` and can re-enter
/// the run loop from inside a winit callback, aborting with
/// "tried to handle event while another event is currently being handled".
/// Fire a separate `osascript` process instead so the notification never shares
/// our event loop.
pub fn toast(summary: &str, body: &str) {
    #[cfg(target_os = "macos")]
    {
        let summary = summary.to_owned();
        let body = body.to_owned();
        let spawn = std::thread::Builder::new()
            .name("tdmcp-toast".into())
            .spawn(move || {
                let script = format!(
                    "display notification \"{}\" with title \"{}\"",
                    applescript_escape(&body),
                    applescript_escape(&summary),
                );
                match Command::new("osascript")
                    .args(["-e", &script])
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .status()
                {
                    Ok(status) if status.success() => {}
                    Ok(status) => warn!(?status, summary, "osascript toast non-zero"),
                    Err(e) => warn!(error = %e, summary, "osascript toast failed"),
                }
            });
        if let Err(e) = spawn {
            warn!(error = %e, "toast thread spawn failed");
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        match notify_rust::Notification::new()
            .summary(summary)
            .body(body)
            .appname("td-mcp-rs")
            .show()
        {
            Ok(_) => {}
            Err(e) => warn!(error = %e, summary, "OS toast failed"),
        }
    }
}

#[cfg(target_os = "macos")]
fn applescript_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

fn notify(summary: &str, body: &str) {
    toast(summary, body);
}

/// Open the file manager on `target` (select file when it is a file; else open dir).
/// `fallback_dir` is used when `target` is missing.
fn reveal_in_file_manager(target: &Path, fallback_dir: &Path) -> Result<()> {
    #[cfg(windows)]
    {
        if target.is_file() {
            Command::new("explorer")
                .arg(format!("/select,{}", target.display()))
                .spawn()
                .map_err(|e| anyhow::anyhow!("explorer /select: {e}"))?;
        } else {
            let dir = if target.is_dir() {
                target
            } else {
                fallback_dir
            };
            Command::new("explorer")
                .arg(dir.as_os_str())
                .spawn()
                .map_err(|e| anyhow::anyhow!("explorer: {e}"))?;
        }
        Ok(())
    }
    #[cfg(target_os = "macos")]
    {
        if target.is_file() {
            Command::new("open")
                .args(["-R", &target.to_string_lossy()])
                .spawn()
                .map_err(|e| anyhow::anyhow!("open -R: {e}"))?;
        } else {
            let dir = if target.is_dir() {
                target
            } else {
                fallback_dir
            };
            Command::new("open")
                .arg(dir)
                .spawn()
                .map_err(|e| anyhow::anyhow!("open: {e}"))?;
        }
        Ok(())
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let dir = if target.is_dir() {
            target
        } else if target.is_file() {
            target.parent().unwrap_or(fallback_dir)
        } else {
            fallback_dir
        };
        Command::new("xdg-open")
            .arg(dir)
            .spawn()
            .map_err(|e| anyhow::anyhow!("xdg-open: {e}"))?;
        Ok(())
    }
    #[cfg(not(any(windows, unix)))]
    {
        let _ = (target, fallback_dir);
        anyhow::bail!("reveal not supported on this platform");
    }
}

fn id_tail(id: &str) -> String {
    let compact: String = id.chars().filter(|c| *c != '-').collect();
    let tail = if compact.len() > 4 {
        &compact[compact.len() - 4..]
    } else {
        &compact
    };
    format!("{tail}…")
}

fn format_duration_since(connected_at_ms: u64) -> String {
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let secs = now_ms.saturating_sub(connected_at_ms) / 1000;
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else {
        format!("{}h", secs / 3600)
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StatusView {
    version: String,
    pid: u32,
    #[serde(default)]
    mcp_session_count: usize,
    /// Federation role (`standalone` | `master` | `slave`).
    #[serde(default)]
    role: String,
    /// Configured listen IP (`server.bind_address`).
    #[serde(default)]
    bind_address: String,
    /// Local hostname.
    #[serde(default)]
    hostname: String,
    /// Persistent daemon id.
    #[serde(default)]
    daemon_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SessionsView {
    sessions: Vec<SessionRow>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SessionRow {
    id: String,
    client_name: String,
    #[serde(default)]
    client_version: String,
    connected_at: u64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FleetView {
    processes: Vec<FleetProc>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FleetProc {
    pid: u32,
    title: Option<String>,
    bridge: serde_json::Value,
    tasks: Option<Vec<serde_json::Value>>,
    #[serde(default)]
    cancelled_tasks: Vec<serde_json::Value>,
    #[serde(default)]
    resurrected: bool,
    /// Owning daemon id (aggregated fleet; `None` before federation).
    #[serde(default)]
    daemon_id: Option<String>,
    /// Owning hostname.
    #[serde(default)]
    hostname: Option<String>,
}

/// Push a message onto the newest-first error ring, deduping a repeated head.
fn push_error_ring(ring: &mut Vec<String>, msg: String) {
    if ring.first().is_some_and(|m| *m == msg) {
        return;
    }
    ring.insert(0, msg);
    ring.truncate(ERROR_RING_CAP);
}

/// Compact newest-first attention strip under the tray header (≤3 rows).
fn error_strip(ui: &mut egui::Ui, ring: &[String]) {
    if ring.is_empty() {
        return;
    }
    section_header(ui, "ATTENTION");
    let shown = ring.len().min(3);
    for msg in ring.iter().take(shown) {
        let full = ui.available_width().min(WINDOW_WIDTH);
        let (rect, _) = ui.allocate_exact_size(egui::vec2(full, 20.0), egui::Sense::hover());
        let center = egui::pos2(rect.left() + 16.0, rect.center().y);
        ui.painter().circle_filled(center, 3.0, ERR);
        ui.painter().text(
            egui::pos2(rect.left() + 28.0, rect.center().y),
            egui::Align2::LEFT_CENTER,
            clip_line(msg, 46),
            font_mono(),
            TEXT_DIM,
        );
    }
    if ring.len() > shown {
        let full = ui.available_width().min(WINDOW_WIDTH);
        let (rect, _) = ui.allocate_exact_size(egui::vec2(full, 16.0), egui::Sense::hover());
        ui.painter().text(
            egui::pos2(rect.left() + 28.0, rect.center().y),
            egui::Align2::LEFT_CENTER,
            format!("+{} more — open dashboard", ring.len() - shown),
            font_meta(),
            TEXT_FAINT,
        );
    }
}

fn http_get_blocking(url: &str, bearer: Option<&str>) -> Result<String, String> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    rt.block_on(async {
        let client = reqwest::Client::new();
        let mut req = client.get(url);
        if let Some(b) = bearer {
            req = req.bearer_auth(b);
        }
        let resp = req.send().await.map_err(|e| e.to_string())?;
        resp.text().await.map_err(|e| e.to_string())
    })
}

fn http_post_blocking(
    url: &str,
    bearer: Option<&str>,
    body: Option<&serde_json::Value>,
) -> Result<serde_json::Value, String> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    rt.block_on(async {
        let client = reqwest::Client::new();
        let mut req = client.post(url);
        if let Some(b) = bearer {
            req = req.bearer_auth(b);
        }
        if let Some(v) = body {
            req = req.json(v);
        }
        let resp = req.send().await.map_err(|e| e.to_string())?;
        resp.json::<serde_json::Value>()
            .await
            .map_err(|e| e.to_string())
    })
}

/// True when a field that only takes effect after a daemon restart changed.
#[must_use]
fn restart_required_fields_changed(a: &ConfigFile, b: &ConfigFile) -> bool {
    a.server.bind_address != b.server.bind_address
        || a.server.port != b.server.port
        || a.auth.mode != b.auth.mode
        || a.auth.psk != b.auth.psk
        || a.federation.role != b.federation.role
        || a.federation.master_url != b.federation.master_url
        || a.federation.master_psk != b.federation.master_psk
}

/// Random PSK for `auth.psk` (32 hex chars) when the user switches to `psk`.
#[must_use]
fn generate_psk() -> String {
    Uuid::new_v4().simple().to_string()
}

/// Local daemon's own psk to call its psk-gated admin routes, if `mode = psk`.
#[must_use]
fn local_master_psk(cfg: &ConfigFile) -> Option<String> {
    if cfg.auth.mode == "psk" && !cfg.auth.psk.is_empty() {
        Some(cfg.auth.psk.clone())
    } else {
        None
    }
}

#[must_use]
fn nonempty_opt(s: &str) -> Option<String> {
    let t = s.trim();
    if t.is_empty() {
        None
    } else {
        Some(t.to_owned())
    }
}

/// Port parsed from an `http://host:port` admin base URL.
#[must_use]
fn port_from_base(base: &str) -> u16 {
    let rest = base
        .trim_start_matches("http://")
        .trim_start_matches("https://");
    match rest.rsplit(':').next() {
        Some(p) => p
            .trim_end_matches('/')
            .parse()
            .unwrap_or(tdmcp_config::DEFAULT_PORT),
        None => tdmcp_config::DEFAULT_PORT,
    }
}

/// Best local (LAN) IPv4 via the UDP connect trick — no packets are sent.
#[must_use]
fn local_ip() -> Option<String> {
    let sock = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    sock.connect("8.8.8.8:80").ok()?;
    sock.local_addr().ok().map(|a| a.ip().to_string())
}

/// First three octets of an IPv4 (the `/24` prefix); `None` for non-IPv4.
#[must_use]
fn ip_prefix(ip: &str) -> Option<String> {
    let mut parts = ip.split('.');
    let a: u8 = parts.next()?.parse().ok()?;
    let b: u8 = parts.next()?.parse().ok()?;
    let c: u8 = parts.next()?.parse().ok()?;
    Some(format!("{a}.{b}.{c}"))
}

/// Probe `prefix.1..254` on `port` via the unauth `/admin/federation/status` probe.
#[must_use]
fn scan_subnet(prefix: &str, port: u16) -> Vec<ScanHit> {
    let Ok(rt) = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    else {
        return Vec::new();
    };
    rt.block_on(async move {
        let Ok(client) = reqwest::Client::builder()
            .timeout(Duration::from_millis(400))
            .build()
        else {
            return Vec::new();
        };
        let mut set = tokio::task::JoinSet::new();
        for i in 1..=254u8 {
            let client = client.clone();
            let prefix = prefix.to_owned();
            set.spawn(async move {
                let host = format!("{prefix}.{i}");
                let url = format!("http://{host}:{port}/admin/federation/status");
                let Ok(resp) = client.get(&url).send().await else {
                    return None;
                };
                if !resp.status().is_success() {
                    return None;
                }
                let Ok(v) = resp.json::<serde_json::Value>().await else {
                    return None;
                };
                Some(ScanHit {
                    host,
                    port,
                    role: v
                        .get("role")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("?")
                        .to_owned(),
                    hostname: v
                        .get("hostname")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or_default()
                        .to_owned(),
                    daemon_id: v
                        .get("daemonId")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or_default()
                        .to_owned(),
                    version: v
                        .get("version")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or_default()
                        .to_owned(),
                })
            });
        }
        let mut results = Vec::new();
        while let Some(joined) = set.join_next().await {
            if let Ok(Some(hit)) = joined {
                results.push(hit);
            }
        }
        results.sort_by_key(|h| h.host.clone());
        results
    })
}

#[must_use]
fn parse_slaves(json: &str) -> Vec<SlaveRow> {
    serde_json::from_str::<SlavesView>(json)
        .map(|v| v.slaves)
        .unwrap_or_default()
}

/// One full-width fleet row (pid, title, counts, bridge status) shared by all views.
fn fleet_row(ui: &mut egui::Ui, p: &FleetProc, index: usize) {
    let bridge = p.bridge.as_str().unwrap_or("?");
    let led = if p.resurrected || !p.cancelled_tasks.is_empty() || bridge == "disconnected" {
        WARN
    } else if bridge == "connected" {
        OK
    } else {
        TEXT_FAINT
    };
    let bg = if index.is_multiple_of(2) {
        BG_ROW
    } else {
        BG_ROW_ALT
    };
    let full = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(egui::vec2(full, 24.0), egui::Sense::hover());
    let fill = if response.hovered() { BG_HOVER } else { bg };
    ui.painter().rect_filled(rect, 0.0, fill);
    ui.painter().hline(
        rect.x_range(),
        rect.bottom(),
        egui::Stroke::new(1.0, BORDER),
    );

    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect.shrink2(egui::vec2(12.0, 0.0)))
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    status_led(&mut child, led);
    child.add_space(6.0);
    child.label(
        egui::RichText::new(p.pid.to_string())
            .font(font_mono())
            .color(TEXT_FAINT),
    );
    child.add_space(8.0);
    child.label(
        egui::RichText::new(p.title.as_deref().unwrap_or(""))
            .font(font_label())
            .color(TEXT),
    );
    child.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        ui.label(
            egui::RichText::new(p.cancelled_tasks.len().to_string())
                .font(font_mono())
                .color(TEXT_DIM),
        );
        ui.add_space(8.0);
        ui.label(
            egui::RichText::new(
                p.tasks
                    .as_ref()
                    .map(|t| t.len().to_string())
                    .unwrap_or_else(|| "-".into()),
            )
            .font(font_mono())
            .color(TEXT_DIM),
        );
        ui.add_space(10.0);
        // Flexible middle: status takes remaining width, aligned right.
        let avail = ui.available_width();
        let (status_rect, _) =
            ui.allocate_exact_size(egui::vec2(avail, 20.0), egui::Sense::hover());
        ui.painter().text(
            egui::pos2(status_rect.right(), status_rect.center().y),
            egui::Align2::RIGHT_CENTER,
            bridge,
            font_meta(),
            TEXT_DIM,
        );
    });
}

/// `/admin/federation/slaves` body.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SlavesView {
    slaves: Vec<SlaveRow>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SlaveRow {
    daemon_id: String,
    #[serde(default)]
    hostname: String,
    #[serde(default)]
    version: String,
    #[serde(default)]
    base_url: String,
    #[serde(default)]
    auth_token: String,
    #[serde(default)]
    reachability: String,
    #[serde(default)]
    process_count: usize,
}

/// Minimal `/admin/federation/status` probe (unauth LAN scan oracle).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FederationProbe {
    #[serde(default)]
    role: String,
    #[serde(default)]
    version: String,
    #[serde(default)]
    hostname: String,
    #[serde(default)]
    daemon_id: String,
}

/// One scan hit (a reachable federation daemon on the scanned subnet).
#[derive(Debug, Clone)]
struct ScanHit {
    host: String,
    port: u16,
    role: String,
    hostname: String,
    daemon_id: String,
    version: String,
}

/// Slave identity for the settings panel (auth token from the registry).
#[derive(Debug, Clone)]
struct SlaveSettingsTarget {
    daemon_id: String,
    hostname: String,
    base_url: String,
    auth_token: String,
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, reason = "unit tests")]
mod tests {
    use super::*;

    /// Pins the `/admin/logs` contract (T4.1's `admin.rs` response shape,
    /// `crate::logrecord::Record` in `tdmcp-daemon`) against `LogRecordView`
    /// / `LogsResponse` — no codegen shares the type, so a drift on either
    /// side must fail here (T4.3).
    #[test]
    fn logs_response_round_trips_the_daemon_fixture() {
        let fixture = r#"{
            "records": [
                {
                    "seq": 41,
                    "ts": "2026-01-01T12:00:00.123Z",
                    "level": "warn",
                    "src": "bridge",
                    "pid": 12345,
                    "target": "bridge::tox_callbacks",
                    "msg": "heartbeat pong timeout",
                    "code": "tdmcp.bridge.pong_timeout",
                    "kvs": {"ms": "42"}
                },
                {
                    "seq": 42,
                    "ts": "2026-01-01T12:00:01.000Z",
                    "level": "info",
                    "src": "daemon",
                    "pid": 999,
                    "target": "tdmcp_daemon",
                    "msg": "no code, no kvs"
                }
            ],
            "next": 42
        }"#;
        let parsed: LogsResponse = serde_json::from_str(fixture).expect("parse fixture");
        assert_eq!(parsed.next, 42);
        assert_eq!(parsed.records.len(), 2);

        let first = &parsed.records[0];
        assert_eq!(first.seq, 41);
        assert_eq!(first.level, "warn");
        assert_eq!(first.src, "bridge");
        assert_eq!(first.pid, 12345);
        assert_eq!(first.target, "bridge::tox_callbacks");
        assert_eq!(first.code.as_deref(), Some("tdmcp.bridge.pong_timeout"));
        assert_eq!(first.kvs.get("ms").map(String::as_str), Some("42"));

        let second = &parsed.records[1];
        assert_eq!(second.code, None, "code omitted on the wire when absent");
        assert!(second.kvs.is_empty(), "kvs omitted on the wire when empty");
    }

    #[test]
    fn level_color_and_letter_cover_all_wire_levels() {
        for level in ["trace", "debug", "info", "warn", "error"] {
            let _ = level_color(level);
            assert_ne!(level_letter(level), "?", "missing mapping for {level}");
        }
        assert_eq!(level_letter("not-a-level"), "?");
    }

    #[test]
    fn error_ring_dedupes_head_and_caps() {
        let mut ring: Vec<String> = Vec::new();
        push_error_ring(&mut ring, "a".to_owned());
        push_error_ring(&mut ring, "a".to_owned());
        assert_eq!(ring.len(), 1);
        push_error_ring(&mut ring, "b".to_owned());
        assert_eq!(ring.first().map(String::as_str), Some("b"));
        for i in 0..(ERROR_RING_CAP + 10) {
            push_error_ring(&mut ring, format!("m{i}"));
        }
        assert_eq!(ring.len(), ERROR_RING_CAP);
    }

    #[test]
    fn clip_line_is_char_boundary_safe() {
        let s = "héllo wörld"; // multi-byte chars
        let clipped = clip_line(s, 3);
        assert_eq!(clipped.chars().count(), 3);
        assert_eq!(clipped, "hél");
        assert_eq!(clip_line("short", 100), "short");
    }
}
