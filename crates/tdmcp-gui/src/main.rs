//! td-mcp-rs tray dashboard (egui 0.35).

#![allow(clippy::exit, reason = "process boundary")]

use std::time::{Duration, Instant};

use anyhow::Result;
use eframe::egui;
use serde::Deserialize;

fn main() -> Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([720.0, 480.0])
            .with_title("td-mcp-rs"),
        ..Default::default()
    };
    eframe::run_native(
        "td-mcp-rs",
        options,
        Box::new(|_cc| Ok(Box::new(DashboardApp::default()))),
    )
    .map_err(|e| anyhow::anyhow!("eframe: {e}"))
}

#[derive(Default)]
struct DashboardApp {
    admin_base: String,
    status_text: String,
    fleet_json: String,
    last_poll: Option<Instant>,
    error: Option<String>,
}

impl DashboardApp {
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
            Ok(body) => self.fleet_json = body,
            Err(e) => self.error = Some(e),
        }
        self.last_poll = Some(Instant::now());
    }

    fn shutdown_daemon(&mut self) {
        self.ensure_base();
        let _ = http_post_blocking(&format!("{}/admin/shutdown", self.admin_base));
    }
}

impl eframe::App for DashboardApp {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let due = self
            .last_poll
            .is_none_or(|t| t.elapsed() > Duration::from_secs(2));
        if due {
            self.poll();
        }
        ctx.request_repaint_after(Duration::from_secs(1));
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
                        ui.label(format!("{:?}", p.bridge));
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
            ui.label("See docs/GUI_WIREFRAME.md. Tray menu lands with tray-icon in a follow-up.");
        });
    }
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
