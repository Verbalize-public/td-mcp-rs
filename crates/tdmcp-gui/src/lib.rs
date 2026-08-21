//! td-mcp-rs tray dashboard (egui 0.35 + tray-icon + notify-rust).
//!
//! Consumed in-process by `tdmcp-daemon` when the `gui` feature is enabled.
//! Closing the window or losing focus only hides the UI — it does not stop
//! the daemon. Use Stop / `/admin/shutdown` to end the process (sets `quit`).

mod theme;

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

use theme::{
    font_label, font_meta, font_mono, font_title, ghost_button, section_header, status_led, ACCENT,
    BG_HOVER, BG_PANEL, BG_ROW, BG_ROW_ALT, BORDER, ERR, OK, TEXT, TEXT_DIM, TEXT_FAINT, WARN,
    WINDOW_MAX_HEIGHT, WINDOW_WIDTH,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum View {
    Fleet,
    Settings,
}

/// Coalesce tray left-clicks so burst/double events cannot flip twice.
const TRAY_TOGGLE_DEBOUNCE: Duration = Duration::from_millis(250);
/// Focus-loss hide within this window counts as the close half of a tray toggle.
const FOCUS_LOSS_CLOSE_GRACE: Duration = Duration::from_millis(400);

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
        rgba: icon_normal_full.rgba,
        width: icon_normal_full.width,
        height: icon_normal_full.height,
    };

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
    view: View,
    draft: ConfigFile,
    settings_error: Option<String>,
    /// Text buffers for optional advanced paths (empty = unset).
    data_dir_edit: String,
    bridge_dir_edit: String,
    catalog_path_edit: String,
    status: Option<StatusView>,
    fleet_json: String,
    sessions_json: String,
    last_poll: Option<Instant>,
    error: Option<String>,
    tray: Option<TrayIcon>,
    menu_restart: MenuItem,
    menu_stop: MenuItem,
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
    ) -> Result<Self> {
        let menu_restart = MenuItem::new("Restart daemon", true, None);
        let menu_stop = MenuItem::new("Stop daemon", true, None);

        let draft = cfgfile::load(&config_path).unwrap_or_default();
        let (data_dir_edit, bridge_dir_edit, catalog_path_edit) = path_edits_from(&draft);

        Ok(Self {
            admin_base,
            data_dir,
            config_path,
            view: View::Fleet,
            draft,
            settings_error: None,
            data_dir_edit,
            bridge_dir_edit,
            catalog_path_edit,
            status: None,
            fleet_json: String::new(),
            sessions_json: String::new(),
            last_poll: None,
            error: None,
            // Defer tray build to the first `logic` tick. Creating a status-item
            // inside eframe's creation callback can re-enter AppKit on macOS and
            // trip winit 0.30's "event while another event is handled" abort.
            tray: None,
            menu_restart,
            menu_stop,
            icon_normal,
            icon_attention,
            attention: false,
            prev_snapshot: FleetSnapshot::default(),
            visible: false,
            pending_initial_hide: true,
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
        })
    }

    fn ensure_tray(&mut self) {
        if !self.pending_tray || self.tray.is_some() {
            return;
        }
        self.pending_tray = false;
        let menu = Menu::new();
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
        let (d, b, c) = path_edits_from(&self.draft);
        self.data_dir_edit = d;
        self.bridge_dir_edit = b;
        self.catalog_path_edit = c;
        self.view = View::Settings;
    }

    fn apply_path_edits(&mut self) {
        self.draft.advanced.data_dir = nonempty_path(&self.data_dir_edit);
        self.draft.advanced.bridge_dir = nonempty_path(&self.bridge_dir_edit);
        self.draft.advanced.catalog_path = nonempty_path(&self.catalog_path_edit);
    }

    fn save_settings(&mut self) {
        self.apply_path_edits();
        match cfgfile::save(&self.config_path, &self.draft) {
            Ok(()) => {
                self.settings_error = None;
                self.view = View::Fleet;
            }
            Err(e) => self.settings_error = Some(format!("save failed: {e}")),
        }
    }

    fn discard_settings(&mut self) {
        self.settings_error = None;
        self.view = View::Fleet;
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

        // Estimate popup size; height is content-driven but we place with a typical height.
        let popup_w = f64::from(WINDOW_WIDTH);
        let popup_h = 360.0_f64;

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
        let status_ok = match http_get_blocking(&format!("{}/admin/status", self.admin_base)) {
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
        match http_get_blocking(&format!("{}/admin/fleet", self.admin_base)) {
            Ok(body) => {
                self.fleet_json = body;
                self.apply_fleet_status();
            }
            Err(e) => self.error = Some(e),
        }
        match http_get_blocking(&format!("{}/admin/mcp-sessions", self.admin_base)) {
            Ok(body) => self.sessions_json = body,
            Err(e) => {
                if self.error.is_none() {
                    self.error = Some(e);
                }
            }
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
                }
            }
            for pid in &self.prev_snapshot.connected_pids {
                if !snap.connected_pids.contains(pid) {
                    notify(
                        "Bridge disconnected",
                        &format!("pid {pid} lost IPC — tasks cancelled"),
                    );
                }
            }
            if snap.cancelled_total > self.prev_snapshot.cancelled_total {
                let delta = snap.cancelled_total - self.prev_snapshot.cancelled_total;
                notify(
                    "Tasks cancelled",
                    &format!("{delta} task(s) stacked on bridge loss"),
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
        let _ = http_post_blocking(&format!("{}/admin/shutdown", self.admin_base));
    }

    fn restart_daemon(&mut self) {
        self.ensure_base();
        let _ = http_post_blocking(&format!("{}/admin/restart", self.admin_base));
    }

    fn reveal_tox(&self) {
        if let Err(e) = reveal_in_file_manager(&self.data_dir.join("bootstrap.tox"), &self.data_dir)
        {
            warn!(error = %e, "reveal bootstrap.tox failed");
        }
    }

    fn reveal_skills(&self) {
        let skills = self.data_dir.join("skills");
        if let Err(e) = reveal_in_file_manager(&skills, &skills) {
            warn!(error = %e, "reveal skills failed");
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
        // Keep Settings open while editing — avoid discarding the draft on
        // accidental focus blips from text fields / OS chrome.
        if self.view == View::Settings {
            return;
        }
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

    fn draw_header(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            status_led(ui, ACCENT);
            ui.add_space(2.0);
            ui.label(
                egui::RichText::new("td-mcp-rs")
                    .font(font_title())
                    .color(TEXT),
            );
            if let Some(st) = &self.status {
                ui.label(
                    egui::RichText::new(format!("· v{}", st.version))
                        .font(font_meta())
                        .color(TEXT_DIM),
                );
                ui.label(
                    egui::RichText::new(format!("· pid {}", st.pid))
                        .font(font_mono())
                        .color(TEXT_FAINT),
                );
            }
            // Right-anchored ghost actions: Stop · Restart · .tox · skills · gear (RTL).
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let stop = ghost_button(ui, "■", TEXT_DIM, ERR).on_hover_text("Stop daemon");
                if stop.clicked() {
                    self.shutdown_daemon();
                }
                ui.add_space(2.0);
                let restart =
                    ghost_button(ui, "↻", TEXT_DIM, ACCENT).on_hover_text("Restart daemon");
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
                let skills_path = self.data_dir.join("skills");
                let tip = format!("Reveal {}", skills_path.display());
                let skills = ghost_button(ui, "skills", TEXT_DIM, ACCENT).on_hover_text(tip);
                if skills.clicked() {
                    self.reveal_skills();
                }
                ui.add_space(4.0);
                let gear = ghost_button(ui, "⚙", TEXT_DIM, ACCENT).on_hover_text("Settings");
                if gear.clicked() {
                    self.open_settings();
                }
            });
        });
    }

    fn field_help(key: &str) -> &'static str {
        FIELD_DESCS
            .iter()
            .find(|f| f.key == key)
            .map(|f| f.help)
            .unwrap_or("")
    }

    fn draw_settings(&mut self, ui: &mut egui::Ui) {
        section_header(ui, "SETTINGS");
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.add_space(12.0);
            ui.label(
                egui::RichText::new(self.config_path.display().to_string())
                    .font(font_mono())
                    .color(TEXT_FAINT),
            );
        });
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.add_space(12.0);
            ui.label(
                egui::RichText::new("Changes apply after the next restart.")
                    .font(font_meta())
                    .color(TEXT_DIM),
            );
        });
        if let Some(err) = &self.settings_error {
            ui.horizontal(|ui| {
                ui.add_space(12.0);
                ui.colored_label(ERR, err.clone());
            });
        }

        ui.add_space(8.0);
        section_header(ui, "SERVER");
        ui.horizontal(|ui| {
            ui.add_space(12.0);
            ui.label(egui::RichText::new("Port").font(font_label()).color(TEXT));
            ui.add(
                egui::DragValue::new(&mut self.draft.server.port)
                    .range(1..=65535)
                    .speed(1),
            )
            .on_hover_text(Self::field_help("server.port"));
        });

        ui.add_space(4.0);
        section_header(ui, "DAEMON");
        ui.horizontal(|ui| {
            ui.add_space(12.0);
            ui.checkbox(&mut self.draft.daemon.keep_alive, "Keep alive")
                .on_hover_text(Self::field_help("daemon.keep_alive"));
        });
        ui.horizontal(|ui| {
            ui.add_space(12.0);
            ui.checkbox(&mut self.draft.daemon.always_on, "Always on")
                .on_hover_text(Self::field_help("daemon.always_on"));
        });
        ui.horizontal(|ui| {
            ui.add_space(12.0);
            ui.checkbox(&mut self.draft.daemon.show_tray, "Show tray")
                .on_hover_text(Self::field_help("daemon.show_tray"));
        });

        ui.add_space(4.0);
        section_header(ui, "BRIDGE");
        ui.horizontal(|ui| {
            ui.add_space(12.0);
            ui.label(
                egui::RichText::new("Call timeout (s)")
                    .font(font_label())
                    .color(TEXT),
            );
            ui.add(
                egui::DragValue::new(&mut self.draft.bridge.call_timeout_secs)
                    .range(1..=600)
                    .speed(1),
            )
            .on_hover_text(Self::field_help("bridge.call_timeout_secs"));
        });
        ui.horizontal(|ui| {
            ui.add_space(12.0);
            ui.label(
                egui::RichText::new("Script timeout (s)")
                    .font(font_label())
                    .color(TEXT),
            );
            ui.add(
                egui::DragValue::new(&mut self.draft.bridge.script_timeout_secs)
                    .range(1..=600)
                    .speed(1),
            )
            .on_hover_text(Self::field_help("bridge.script_timeout_secs"));
        });
        ui.horizontal(|ui| {
            ui.add_space(12.0);
            ui.label(
                egui::RichText::new("Heartbeat interval (s)")
                    .font(font_label())
                    .color(TEXT),
            );
            ui.add(
                egui::DragValue::new(&mut self.draft.bridge.heartbeat_interval_secs)
                    .range(1..=120)
                    .speed(1),
            )
            .on_hover_text(Self::field_help("bridge.heartbeat_interval_secs"));
        });
        ui.horizontal(|ui| {
            ui.add_space(12.0);
            ui.label(
                egui::RichText::new("Pong timeout (s)")
                    .font(font_label())
                    .color(TEXT),
            );
            ui.add(
                egui::DragValue::new(&mut self.draft.bridge.pong_timeout_secs)
                    .range(1..=120)
                    .speed(1),
            )
            .on_hover_text(Self::field_help("bridge.pong_timeout_secs"));
        });
        ui.horizontal(|ui| {
            ui.add_space(12.0);
            ui.label(
                egui::RichText::new("Idle dead (s)")
                    .font(font_label())
                    .color(TEXT),
            );
            ui.add(
                egui::DragValue::new(&mut self.draft.bridge.idle_dead_secs)
                    .range(1..=300)
                    .speed(1),
            )
            .on_hover_text(Self::field_help("bridge.idle_dead_secs"));
        });

        ui.add_space(4.0);
        section_header(ui, "ADVANCED");
        ui.horizontal(|ui| {
            ui.add_space(12.0);
            ui.label(
                egui::RichText::new("Data dir")
                    .font(font_label())
                    .color(TEXT),
            );
            ui.add(
                egui::TextEdit::singleline(&mut self.data_dir_edit)
                    .desired_width(220.0)
                    .font(font_mono()),
            )
            .on_hover_text(Self::field_help("advanced.data_dir"));
        });
        ui.horizontal(|ui| {
            ui.add_space(12.0);
            ui.label(
                egui::RichText::new("Bridge dir")
                    .font(font_label())
                    .color(TEXT),
            );
            ui.add(
                egui::TextEdit::singleline(&mut self.bridge_dir_edit)
                    .desired_width(220.0)
                    .font(font_mono()),
            )
            .on_hover_text(Self::field_help("advanced.bridge_dir"));
        });
        ui.horizontal(|ui| {
            ui.add_space(12.0);
            ui.label(
                egui::RichText::new("Catalog")
                    .font(font_label())
                    .color(TEXT),
            );
            ui.add(
                egui::TextEdit::singleline(&mut self.catalog_path_edit)
                    .desired_width(220.0)
                    .font(font_mono()),
            )
            .on_hover_text(Self::field_help("advanced.catalog_path"));
        });

        ui.add_space(12.0);
        ui.painter().rect_filled(
            ui.available_rect_before_wrap()
                .with_max_y(ui.cursor().top() + 28.0),
            0.0,
            BG_PANEL,
        );
        ui.horizontal(|ui| {
            ui.add_space(12.0);
            if ghost_button(ui, "← Back", TEXT_DIM, TEXT).clicked() {
                self.discard_settings();
            }
            ui.add_space(8.0);
            if ghost_button(ui, "Discard", TEXT_DIM, WARN).clicked() {
                self.discard_settings();
            }
            ui.add_space(8.0);
            if ghost_button(ui, "Reset", TEXT_DIM, WARN).clicked() {
                self.reset_settings();
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_space(12.0);
                if ghost_button(ui, "Save", TEXT_DIM, ACCENT).clicked() {
                    self.save_settings();
                }
            });
        });
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
            let bg = if i % 2 == 0 { BG_ROW } else { BG_ROW_ALT };
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

    fn draw_td_section(&self, ui: &mut egui::Ui) {
        section_header(ui, "TOUCHDESIGNER");
        let Ok(fleet) = serde_json::from_str::<FleetView>(&self.fleet_json) else {
            ui.vertical_centered(|ui| {
                ui.add_space(8.0);
                ui.label(
                    egui::RichText::new("No TouchDesigner bridges")
                        .font(font_meta())
                        .color(TEXT_DIM),
                );
                ui.add_space(8.0);
            });
            return;
        };
        if fleet.processes.is_empty() {
            ui.vertical_centered(|ui| {
                ui.add_space(8.0);
                ui.label(
                    egui::RichText::new("No TouchDesigner bridges")
                        .font(font_meta())
                        .color(TEXT_DIM),
                );
                ui.add_space(8.0);
            });
            return;
        }
        for (i, p) in fleet.processes.iter().enumerate() {
            let bridge = p.bridge.as_str().unwrap_or("?");
            let led = if p.resurrected || !p.cancelled_tasks.is_empty() || bridge == "disconnected"
            {
                WARN
            } else if bridge == "connected" {
                OK
            } else {
                TEXT_FAINT
            };
            let bg = if i % 2 == 0 { BG_ROW } else { BG_ROW_ALT };
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
            // Right: counts; middle remaining: bridge status right-aligned.
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
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.add_space(12.0);
                    ui.allocate_ui_with_layout(
                        egui::vec2(ui.available_width() - 12.0, 24.0),
                        egui::Layout::left_to_right(egui::Align::Center),
                        |ui| {
                            self.draw_header(ui);
                        },
                    );
                });
                ui.add_space(8.0);
                match self.view {
                    View::Settings => {
                        egui::ScrollArea::vertical()
                            .max_height(WINDOW_MAX_HEIGHT - 48.0)
                            .show(ui, |ui| {
                                self.draw_settings(ui);
                            });
                    }
                    View::Fleet => {
                        if let Some(err) = &self.error {
                            ui.horizontal(|ui| {
                                ui.add_space(12.0);
                                ui.colored_label(ERR, err);
                            });
                        }
                        egui::ScrollArea::vertical()
                            .max_height(WINDOW_MAX_HEIGHT - 80.0)
                            .show(ui, |ui| {
                                self.draw_mcp_section(ui);
                                self.draw_td_section(ui);
                            });
                    }
                }
                ui.add_space(8.0);
            });
    }
}

fn path_edits_from(cfg: &ConfigFile) -> (String, String, String) {
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
}

fn http_get_blocking(url: &str) -> Result<String, String> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    rt.block_on(async {
        let client = reqwest::Client::new();
        let resp = client.get(url).send().await.map_err(|e| e.to_string())?;
        resp.text().await.map_err(|e| e.to_string())
    })
}

fn http_post_blocking(url: &str) -> Result<(), String> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    rt.block_on(async {
        let client = reqwest::Client::new();
        client.post(url).send().await.map_err(|e| e.to_string())?;
        Ok(())
    })
}
