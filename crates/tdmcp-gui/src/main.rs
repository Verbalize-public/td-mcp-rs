//! td-mcp-rs tray dashboard (egui 0.35 + tray-icon + notify-rust).

#![allow(clippy::exit, reason = "process boundary")]

use std::time::{Duration, Instant};

use anyhow::Result;
use eframe::egui;
use serde::Deserialize;
use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder, TrayIconEvent};

fn main() -> Result<()> {
    let icon_normal_full = load_rgba(include_bytes!("../assets/icon-normal.png"), None)?;
    let icon_normal = load_rgba(include_bytes!("../assets/icon-normal.png"), Some(32))?;
    let icon_attention = load_rgba(include_bytes!("../assets/icon-attention.png"), Some(32))?;
    let window_icon = egui::IconData {
        rgba: icon_normal_full.rgba,
        width: icon_normal_full.width,
        height: icon_normal_full.height,
    };

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([720.0, 480.0])
            .with_title("td-mcp-rs")
            .with_icon(window_icon),
        ..Default::default()
    };
    eframe::run_native(
        "td-mcp-rs",
        options,
        Box::new(move |_cc| Ok(Box::new(DashboardApp::new(icon_normal, icon_attention)?))),
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
            // Cap window icons at 256 to keep IconData reasonable.
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
    status_text: String,
    fleet_json: String,
    last_poll: Option<Instant>,
    error: Option<String>,
    tray: Option<TrayIcon>,
    menu_show: MenuItem,
    menu_hide: MenuItem,
    menu_restart: MenuItem,
    menu_stop: MenuItem,
    menu_quit: MenuItem,
    icon_normal: RgbaIcon,
    icon_attention: RgbaIcon,
    attention: bool,
    prev_snapshot: FleetSnapshot,
    visible: bool,
}

#[derive(Debug, Default, Clone)]
struct FleetSnapshot {
    connected: usize,
    disconnected: usize,
    resurrected: usize,
    cancelled: usize,
    /// Pids that were connected last poll (for edge-triggered toasts).
    connected_pids: Vec<u32>,
    resurrected_pids: Vec<u32>,
    cancelled_total: usize,
}

impl DashboardApp {
    fn new(icon_normal: RgbaIcon, icon_attention: RgbaIcon) -> Result<Self> {
        let menu = Menu::new();
        let menu_show = MenuItem::new("Show dashboard", true, None);
        let menu_hide = MenuItem::new("Hide dashboard", true, None);
        let menu_restart = MenuItem::new("Restart daemon", true, None);
        let menu_stop = MenuItem::new("Stop daemon", true, None);
        let menu_quit = MenuItem::new("Quit", true, None);
        menu.append(&menu_show)?;
        menu.append(&menu_hide)?;
        menu.append(&PredefinedMenuItem::separator())?;
        menu.append(&menu_restart)?;
        menu.append(&menu_stop)?;
        menu.append(&PredefinedMenuItem::separator())?;
        menu.append(&menu_quit)?;

        let tray = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_tooltip("td-mcp-rs")
            .with_icon(tray_icon_from(&icon_normal)?)
            .build()
            .map_err(|e| anyhow::anyhow!("tray: {e}"))?;

        Ok(Self {
            admin_base: "http://127.0.0.1:9860".into(),
            status_text: String::new(),
            fleet_json: String::new(),
            last_poll: None,
            error: None,
            tray: Some(tray),
            menu_show,
            menu_hide,
            menu_restart,
            menu_stop,
            menu_quit,
            icon_normal,
            icon_attention,
            attention: false,
            prev_snapshot: FleetSnapshot::default(),
            visible: true,
        })
    }

    fn ensure_base(&mut self) {
        if self.admin_base.is_empty() {
            self.admin_base = "http://127.0.0.1:9860".into();
        }
    }

    fn poll(&mut self) {
        self.ensure_base();
        match http_get_blocking(&format!("{}/admin/status", self.admin_base)) {
            Ok(body) => {
                self.status_text = body;
                self.error = None;
            }
            Err(e) => {
                self.status_text.clear();
                self.error = Some(e);
            }
        }
        match http_get_blocking(&format!("{}/admin/fleet", self.admin_base)) {
            Ok(body) => {
                self.fleet_json = body;
                self.apply_fleet_status();
            }
            Err(e) => self.error = Some(e),
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

        // Edge-triggered toasts.
        if self.last_poll.is_some() {
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
        let tooltip = if snap.connected + snap.disconnected == 0 {
            "td-mcp-rs — no connections".to_owned()
        } else if needs_attention {
            format!(
                "td-mcp-rs — {} connected, {} disconnected, {} resurrected, {} cancelled",
                snap.connected, snap.disconnected, snap.resurrected, snap.cancelled
            )
        } else {
            format!("td-mcp-rs — {} connected", snap.connected)
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

    fn handle_tray_events(&mut self, ctx: &egui::Context) {
        while let Ok(event) = TrayIconEvent::receiver().try_recv() {
            if let TrayIconEvent::DoubleClick { .. } | TrayIconEvent::Click { .. } = event {
                self.visible = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            }
        }
        while let Ok(event) = MenuEvent::receiver().try_recv() {
            if event.id == self.menu_show.id() {
                self.visible = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            } else if event.id == self.menu_hide.id() {
                self.visible = false;
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
            } else if event.id == self.menu_restart.id() {
                self.restart_daemon();
            } else if event.id == self.menu_stop.id() {
                self.shutdown_daemon();
            } else if event.id == self.menu_quit.id() {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
    }
}

impl eframe::App for DashboardApp {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.handle_tray_events(ctx);
        let due = self
            .last_poll
            .is_none_or(|t| t.elapsed() > Duration::from_secs(2));
        if due {
            self.poll();
        }
        ctx.request_repaint_after(Duration::from_millis(250));
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.heading("td-mcp-rs");
                ui.separator();
                ui.label("Admin:");
                ui.text_edit_singleline(&mut self.admin_base);
                if ui.button("Refresh").clicked() {
                    self.poll();
                }
                if ui.button("Restart daemon").clicked() {
                    self.restart_daemon();
                }
                if ui.button("Stop daemon").clicked() {
                    self.shutdown_daemon();
                }
            });
            ui.separator();

            if let Some(err) = &self.error {
                ui.colored_label(egui::Color32::RED, err);
            }
            ui.collapsing("Daemon status", |ui| {
                ui.monospace(&self.status_text);
            });
            ui.separator();
            ui.heading("Connections (fleet)");
            if let Ok(fleet) = serde_json::from_str::<FleetView>(&self.fleet_json) {
                egui::Grid::new("conn_grid").striped(true).show(ui, |ui| {
                    ui.label("pid");
                    ui.label("title");
                    ui.label("bridge");
                    ui.label("tasks");
                    ui.label("cancelled");
                    ui.end_row();
                    for p in &fleet.processes {
                        ui.label(p.pid.to_string());
                        ui.label(p.title.as_deref().unwrap_or(""));
                        ui.label(p.bridge.as_str().unwrap_or("?"));
                        ui.label(
                            p.tasks
                                .as_ref()
                                .map(|t| t.len().to_string())
                                .unwrap_or_else(|| "-".into()),
                        );
                        ui.label(p.cancelled_tasks.len().to_string());
                        ui.end_row();
                    }
                });
            } else {
                ui.monospace(&self.fleet_json);
            }
            ui.separator();
            ui.label("Tray: Show / Hide · Restart · Stop · Quit. Icon turns amber on attention.");
        });
    }
}

fn notify(summary: &str, body: &str) {
    let _ = notify_rust::Notification::new()
        .summary(summary)
        .body(body)
        .appname("td-mcp-rs")
        .show();
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
