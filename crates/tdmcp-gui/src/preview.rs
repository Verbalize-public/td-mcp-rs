//! Dev-only preview harness (feature `preview`): renders the real dashboard
//! from injected fixtures so every state — populated fleets, modals, banners,
//! dirty settings — can be pixel-verified without a live daemon or TD.
//!
//! Scenes: `overview-empty · overview-populated · overview-offline ·
//! modal-add-slave · stop-confirm · logs-filtered · settings-dirty`.
//! Window title matches the production dashboard so the Win32 capture script
//! (`.ua/gui-shot.ps1` technique) finds it.

use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use eframe::egui;
use serde_json::json;

use crate::app::{DashboardApp, FleetPanel};
use crate::wire::{LogRecordView, ScanHit, ScanPurpose};

/// Run one scene in a native window; blocks until closed.
pub fn run(scene: &str) -> anyhow::Result<()> {
    let app = Box::new(build(scene)?);
    // `popup-*` scenes render the tray glance card at its real size instead of
    // the dashboard, so the footer actions can be verified without a daemon.
    let popup = scene.starts_with("popup");
    let (title, size) = if popup {
        ("td-mcp-rs", [crate::theme::WINDOW_WIDTH, 304.0])
    } else if scene == "overview-narrow" {
        ("td-mcp-rs — Dashboard", [800.0, 620.0])
    } else {
        ("td-mcp-rs — Dashboard", [1040.0, 760.0])
    };
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(title)
            .with_inner_size(size),
        ..Default::default()
    };
    eframe::run_native(
        "tdmcp-preview",
        options,
        Box::new(move |cc| {
            crate::theme::apply(&cc.egui_ctx);
            Ok(Box::new(PreviewApp { inner: app, popup }))
        }),
    )
    .map_err(|e| anyhow::anyhow!("eframe: {e}"))
}

struct PreviewApp {
    inner: Box<DashboardApp>,
    popup: bool,
}

impl eframe::App for PreviewApp {
    // eframe 0.35 drives apps through `ui`, matching the production popup path.
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(200));
        if self.popup {
            let app = self.inner.as_mut();
            egui::CentralPanel::default()
                .frame(
                    egui::Frame::NONE
                        .fill(crate::theme::BG_WINDOW)
                        .stroke(egui::Stroke::new(1.0, crate::theme::BORDER))
                        .inner_margin(0.0),
                )
                .show(ui, |ui| {
                    app.draw_header(ui);
                    egui::ScrollArea::vertical()
                        .auto_shrink(false)
                        .max_height((ui.available_height() - crate::popup::FOOTER_H).max(0.0))
                        .show(ui, |ui| {
                            ui.add_space(crate::theme::sp::SM);
                            app.draw_summary(ui);
                        });
                    app.draw_action_footer(ui);
                });
            return;
        }
        crate::dashboard::render(self.inner.as_mut(), ui);
    }
}

fn build(scene: &str) -> anyhow::Result<DashboardApp> {
    let tmp = std::env::temp_dir();
    let quit = Arc::new(AtomicBool::new(false));
    let blank_icon = crate::tray::RgbaIcon {
        rgba: vec![0, 0, 0, 255],
        width: 1,
        height: 1,
    };
    let mut app = DashboardApp::new(
        "http://127.0.0.1:9860".to_owned(),
        tmp.clone(),
        blank_icon,
        crate::tray::RgbaIcon {
            rgba: vec![0, 0, 0, 255],
            width: 1,
            height: 1,
        },
        quit,
        tmp.join("tdmcp-preview-config.toml"),
        egui::IconData {
            width: 1,
            height: 1,
            rgba: vec![0, 0, 0, 255],
        },
    )?;
    // Suppress tray + initial hide + OS toasts paths.
    app.pending_tray = false;
    app.pending_initial_hide = false;
    app.visible = true;
    app.dashboard_open = true;

    match scene {
        "overview-empty" => {
            inject_status(&mut app, "master", 0, 9860);
            app.fleet_json = r#"{"processes":[]}"#.to_owned();
            app.sessions_json = r#"{"sessions":[]}"#.to_owned();
            app.apply_fleet_status();
        }
        "overview-populated" | "modal-add-slave" | "stop-confirm" | "popup"
        | "popup-stop-confirm" | "overview-narrow" | "overview-many" => {
            inject_status(&mut app, "master", 3, 9860);
            app.fleet_json = json!({
                "processes": [
                    {"pid": 12045, "title": "tox-scene-01", "bridge": "connected",
                     "tasks": [{"id": 1}, {"id": 2}], "cancelledTasks": []},
                    {"pid": 12100, "title": "tox-render", "bridge": "disconnected",
                     "tasks": null, "cancelledTasks": [{"id": 9}]},
                    {"pid": 22001, "title": "tox-studio", "bridge": "connected",
                     "tasks": [{"id": 4}], "cancelledTasks": [],
                     "hostname": "studio-b", "daemonId": SLAVE_ID}
                ]
            })
            .to_string();
            if scene == "overview-many" {
                let procs: Vec<_> = (0..7)
                    .map(|i| {
                        json!({"pid": 13000 + i, "title": format!("tox-{i:02}"),
                               "bridge": "connected", "tasks": [], "cancelledTasks": []})
                    })
                    .collect();
                app.fleet_json = json!({ "processes": procs }).to_string();
            }
            app.sessions_json = sessions_fixture();
            app.slaves_json = slaves_fixture();
            app.error_ring
                .push("bridge disconnected — pid 12100 lost IPC".to_owned());
            app.error_ring
                .push("2 task(s) cancelled on bridge loss".to_owned());
            app.crash_count = 1;
            app.apply_fleet_status();
            if scene == "modal-add-slave" {
                app.fleet_panel = FleetPanel::AddSlave;
                app.add_slave_host = "192.168.1.50".to_owned();
                app.add_slave_probe =
                    Some(serde_json::from_str(PROBE_FIXTURE).map_err(anyhow::Error::msg)?);
                app.scan_results = scan_hits();
                app.scan_purpose = ScanPurpose::AddSlave;
            }
            if scene == "stop-confirm" || scene == "popup-stop-confirm" {
                app.confirm_stop = true;
            }
        }
        "overview-offline" => {
            app.status = None;
            app.error = Some("connection refused (os error 111)".to_owned());
        }
        "logs-filtered" => {
            app.dash_tab = crate::dashboard::DashTab::Logs;
            inject_status(&mut app, "standalone", 1, 9860);
            app.logs_view.buf = log_records().into();
            app.logs_view.min_level = Some("warn");
            app.logs_view.last_fetch = Some(std::time::Instant::now());
        }
        "settings-dirty" => {
            app.dash_tab = crate::dashboard::DashTab::Settings;
            inject_status(&mut app, "master", 2, 9860);
            app.fleet_json = r#"{"processes":[]}"#.to_owned();
            app.sessions_json = r#"{"sessions":[]}"#.to_owned();
            app.draft.server.port += 7;
            app.needs_restart = true;
        }
        other => anyhow::bail!(
            "unknown scene `{other}` — expected popup · popup-stop-confirm · overview-narrow · overview-many · overview-empty · overview-populated · \
             overview-offline · modal-add-slave · stop-confirm · logs-filtered · settings-dirty"
        ),
    }
    Ok(app)
}

const LOCAL_ID: &str = "d4f0a2c1-77bb-4c11-9f2e-a1b2c3d4e5f6";
const SLAVE_ID: &str = "91ac33dd-02ea-49ab-8d10-9988aabbccdd";

fn inject_status(app: &mut DashboardApp, role: &str, mcp: usize, _port: u16) {
    let body = json!({
        "ok": true,
        "version": "0.9.3",
        "pid": std::process::id(),
        "mcpSessionCount": mcp,
        "bridgeCount": 2,
        "noGui": false,
        "bindAddress": "127.0.0.1",
        "role": role,
        "daemonId": LOCAL_ID,
        "hostname": "DESKTOP-A",
        "slaveCount": if role == "master" { Some(1) } else { None },
        "uptimeSecs": 3 * 3600 + 12 * 60,
    });
    app.status = serde_json::from_str(&body.to_string()).ok();
}

fn sessions_fixture() -> String {
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    json!({
        "sessions": [
            {"id": "aa11bb22-cc33-4444-5555-6666777788889999", "clientName": "claude-code",
             "clientVersion": "1.4.2", "connectedAt": now_ms - 720_000},
            {"id": "b222c333-d444-4555-6666-7777888899990000", "clientName": "cursor",
             "clientVersion": "0.42.0", "connectedAt": now_ms - 90_000},
            {"id": "c333d444-e555-4666-7777-888899990000aaaa1111", "clientName": "codex-cli",
             "clientVersion": "", "connectedAt": now_ms - 15_000}
        ]
    })
    .to_string()
}

fn slaves_fixture() -> String {
    json!({
        "slaves": [
            {"daemonId": SLAVE_ID, "hostname": "studio-b", "version": "0.9.1",
             "baseUrl": "http://192.168.1.50:9860", "authToken": "",
             "reachability": "reachable", "processCount": 3}
        ]
    })
    .to_string()
}

const PROBE_FIXTURE: &str = r#"{
    "role": "slave",
    "version": "0.9.1",
    "hostname": "studio-c",
    "daemonId": "55667788-aabb-4cdd-8eef-001122334455"
}"#;

fn scan_hits() -> Vec<ScanHit> {
    vec![
        ScanHit {
            host: "192.168.1.50".to_owned(),
            port: 9860,
            role: "slave".to_owned(),
            hostname: "studio-b".to_owned(),
            daemon_id: SLAVE_ID.to_owned(),
            version: "0.9.1".to_owned(),
        },
        ScanHit {
            host: "192.168.1.62".to_owned(),
            port: 9860,
            role: "slave".to_owned(),
            hostname: "studio-c".to_owned(),
            daemon_id: "55667788-aabb-4cdd-8eef-001122334455".to_owned(),
            version: "0.9.1".to_owned(),
        },
    ]
}

fn log_records() -> Vec<LogRecordView> {
    let mut v = Vec::new();
    for i in 0..24u64 {
        let (level, src, target, msg) = match i % 6 {
            0 => (
                "error",
                "bridge",
                "tdmcp_bridge::ipc",
                "heartbeat pong timeout after 120s",
            ),
            1 => (
                "warn",
                "daemon",
                "tdmcp_daemon::middleware",
                "session chill engaged for 250ms",
            ),
            2 => (
                "info",
                "daemon",
                "tdmcp_daemon",
                "poll ok · fleet 3 processes",
            ),
            3 => (
                "debug",
                "proxy",
                "tdmcp_proxy::stdio",
                "forwarded tools/list",
            ),
            4 => (
                "info",
                "bridge",
                "tdmcp_bridge",
                "execute_python ok pid=12045",
            ),
            _ => ("trace", "daemon", "tdmcp_daemon::ring", "tick"),
        };
        v.push(LogRecordView {
            seq: 100 + i,
            ts: format!("2026-01-01T12:{:02}:{:02}.123Z", i / 60, i % 60),
            level: level.to_owned(),
            src: src.to_owned(),
            pid: 12045,
            target: target.to_owned(),
            msg: msg.to_owned(),
            code: if level == "error" {
                // Real registered code — the diagnostics catalog scan forbids
                // unregistered tdmcp.* literals outside #[cfg(test)].
                Some("tdmcp.bridge.timeout".to_owned())
            } else {
                None
            },
            kvs: Default::default(),
        });
    }
    v
}
