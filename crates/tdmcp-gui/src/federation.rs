//! Federation flows: add-slave pipeline (probe + configure), per-slave
//! settings, slave self-view (Go standalone), and the LAN subnet scan UI.

use eframe::egui;
use serde_json::json;

use crate::app::{local_master_psk, nonempty_opt, DashboardApp, FleetPanel, SnackTone};
use crate::dashboard::widgets::section_caption;
use crate::http::{http_get_blocking, http_post_blocking, ip_prefix, local_ip, scan_subnet};
use crate::platform::notify;
use crate::theme::{
    filled_button, font_label, font_meta, font_mono, ghost_button, status_led, ACCENT, BG_HOVER,
    ERR, OK, RADIUS_SM, ROW_H, SIDE_MARGIN, TEXT, TEXT_DIM, TEXT_FAINT, WARN,
};
use crate::wire::{id_tail, FederationProbe, ScanHit, ScanPurpose, SlaveSettingsTarget};

/// Settings-row height for the federation panels (px).
const SETTINGS_ROW_H: f32 = 26.0;

/// Outcome state for the one-click add-slave pipeline (probe → configure).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AddSlaveStep {
    Idle,
    Working,
    Configured,
    ConfigureFailed(String),
}

/// One settings row: label left (measured width), control right-aligned.
/// Fixed height, symmetric side margins and an explicit control column, so
/// controls never touch the window edge, wrap under the label, or clip on
/// any platform. The help tooltip rides the whole row when one is given.
pub(crate) fn settings_row(
    ui: &mut egui::Ui,
    label: &str,
    help: &str,
    add: impl FnOnce(&mut egui::Ui),
) {
    // The window is a fixed width; bound the row to it so a reported
    // available_width larger than the actually-painted viewport (seen on some
    // macOS DPI paths) can never push controls past the window's right edge.
    let full = ui.available_width().min(crate::theme::WINDOW_WIDTH);
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

impl DashboardApp {
    /// Renders `self.scan_results` only when they belong to `purpose` — the
    /// master's "find a slave" scan and a joiner's "find a master" scan share one
    /// result set (see [`ScanPurpose`]) but must never bleed into each other's UI.
    pub(crate) fn draw_scan_results(&mut self, ui: &mut egui::Ui, purpose: ScanPurpose) {
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
        section_caption(ui, &format!("SCAN · {} hit(s)", hits.len()));
        let mut use_hit: Option<(String, u16)> = None;
        for hit in hits.iter() {
            let full = ui.available_width();
            let (rect, response) =
                ui.allocate_exact_size(egui::vec2(full, ROW_H), egui::Sense::hover());
            if response.hovered() {
                ui.painter().rect_filled(rect, RADIUS_SM, BG_HOVER);
            }

            let mut child = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(rect.shrink2(egui::vec2(SIDE_MARGIN, 0.0)))
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

    pub(crate) fn start_scan(&mut self, port: u16, purpose: ScanPurpose) {
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

    /// Probe and configure without blocking window input or repaint.
    pub(crate) fn run_add_pipeline(&mut self) {
        if self.add_slave_job.is_running() {
            return;
        }
        let host = self.add_slave_host.trim().to_owned();
        if host.is_empty() {
            self.add_slave_step = AddSlaveStep::ConfigureFailed("Enter a host".to_owned());
            return;
        }
        let port = self.add_slave_port;
        let bearer = nonempty_opt(&self.add_slave_psk);
        let (master_url, master_psk) = self.master_federation_values();
        let result = self.add_slave_job.start("tdmcp-join", move || {
            let base = format!("http://{host}:{port}");
            let response = http_get_blocking(&format!("{base}/admin/federation/status"), None)
                .map_err(|e| format!("Probe failed: {e}"))?;
            let probe: FederationProbe = serde_json::from_str(&response)
                .map_err(|_| "This host did not return a federation status".to_owned())?;
            if probe.role == "master" {
                return Err(
                    "This computer already coordinates a fleet. Change its role locally first."
                        .into(),
                );
            }
            http_post_blocking(
                &format!("{base}/admin/config"),
                bearer.as_deref(),
                Some(&json!({"federation": {
                    "role": "slave", "masterUrl": master_url, "masterPsk": master_psk
                }})),
            )?;
            Ok(probe)
        });
        self.add_slave_step = match result {
            Ok(()) => AddSlaveStep::Working,
            Err(e) => AddSlaveStep::ConfigureFailed(e),
        };
    }

    pub(crate) fn poll_add_pipeline(&mut self) {
        if let Some(result) = self.add_slave_job.poll() {
            match result {
                Ok(probe) => {
                    self.add_slave_probe = Some(probe);
                    self.add_slave_step = AddSlaveStep::Configured;
                    self.last_poll = None;
                    self.snack(
                        "Computer configured — waiting for it to join",
                        SnackTone::Ok,
                    );
                }
                Err(error) => self.add_slave_step = AddSlaveStep::ConfigureFailed(error),
            }
        }
    }

    /// URL + psk to advertise to a new slave (hostname + local port).
    pub(crate) fn master_federation_values(&self) -> (String, String) {
        let hostname = local_ip()
            .or_else(|| {
                self.status
                    .as_ref()
                    .map(|s| s.hostname.clone())
                    .filter(|h| !h.is_empty())
            })
            .unwrap_or_else(|| "localhost".to_owned());
        (
            format!(
                "http://{hostname}:{}",
                crate::http::port_from_base(&self.admin_base)
            ),
            local_master_psk(&self.settings_loaded_snapshot).unwrap_or_default(),
        )
    }

    pub(crate) fn slave_count(&self) -> usize {
        crate::wire::parse_slaves(&self.slaves_json).len()
    }

    pub(crate) fn open_slave_settings(&mut self, target: SlaveSettingsTarget) {
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
        let body = json!({
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
                notify("Slave settings", &format!("{hostname}: timeouts applied"));
                self.snack("Slave settings saved", SnackTone::Ok);
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

    pub(crate) fn draw_add_slave_panel(&mut self, ui: &mut egui::Ui) {
        use AddSlaveStep as S;
        section_caption(ui, "ADD SLAVE");
        ui.horizontal(|ui| {
            ui.add_space(SIDE_MARGIN);
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

        // Embedded network scan — results pick straight into the Host field.
        ui.add_space(2.0);
        ui.horizontal(|ui| {
            ui.add_space(SIDE_MARGIN);
            let scanning = self.scan_busy && self.scan_purpose == ScanPurpose::AddSlave;
            if scanning {
                ui.label(
                    egui::RichText::new("scanning…")
                        .font(font_meta())
                        .color(TEXT_DIM),
                );
            } else if ghost_button(ui, "Scan network", TEXT_DIM, ACCENT)
                .on_hover_text("Probe the /24 subnet for federation daemons")
                .clicked()
            {
                self.start_scan(self.add_slave_port, ScanPurpose::AddSlave);
            }
        });
        self.draw_scan_results(ui, ScanPurpose::AddSlave);

        // Probe preview + pipeline outcome.
        if let Some(probe) = &self.add_slave_probe {
            let led = if probe.role == "master" { ACCENT } else { OK };
            ui.horizontal(|ui| {
                ui.add_space(SIDE_MARGIN);
                status_led(ui, led);
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
        }
        match &self.add_slave_step {
            S::Idle => {}
            S::Working => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Connecting and configuring…");
                });
            }
            S::ConfigureFailed(e) => {
                ui.horizontal(|ui| {
                    ui.add_space(SIDE_MARGIN);
                    ui.colored_label(ERR, e.clone());
                });
            }
            S::Configured => {
                ui.horizontal(|ui| {
                    ui.add_space(SIDE_MARGIN);
                    status_led(ui, OK);
                    ui.colored_label(
                        OK,
                        "Configured. It will appear in the fleet when connected.",
                    );
                });
            }
        }

        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.add_space(SIDE_MARGIN);
            if filled_button(ui, "Add as slave")
                .on_hover_text("Probes the host, then writes role=slave via /admin/config")
                .clicked()
            {
                self.run_add_pipeline();
            }
        });
    }

    pub(crate) fn draw_slave_settings_panel(&mut self, ui: &mut egui::Ui) {
        section_caption(ui, "SLAVE SETTINGS");
        ui.horizontal(|ui| {
            ui.add_space(SIDE_MARGIN);
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
            ui.add_space(SIDE_MARGIN);
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
                ui.add_space(SIDE_MARGIN);
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
            ui.add_space(SIDE_MARGIN);
            ui.label(
                egui::RichText::new("Call timeouts apply to new requests when saved.")
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
    pub(crate) fn draw_slave_self_view(&mut self, ui: &mut egui::Ui) {
        ui.add_space(6.0);
        section_caption(ui, "FEDERATION");
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
                ui.add_space(SIDE_MARGIN);
                ui.colored_label(WARN, msg.clone());
            });
        }
        ui.add_space(2.0);
        ui.horizontal(|ui| {
            ui.add_space(SIDE_MARGIN);
            if self.confirm_go_standalone {
                ui.label(
                    egui::RichText::new("Leave the coordinator and control this computer locally?")
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
                .on_hover_text(
                    "Disconnect from the coordinator; local TouchDesigner stays connected",
                )
                .clicked()
            {
                self.confirm_go_standalone = true;
            }
        });
    }

    fn go_standalone(&mut self) {
        self.ensure_base();
        let bearer = local_master_psk(&self.settings_loaded_snapshot);
        let body = json!({ "federation": { "role": "standalone" } });
        let url = format!("{}/admin/config", self.admin_base);
        match http_post_blocking(&url, bearer.as_deref(), Some(&body)) {
            Ok(v)
                if v.get("ok")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false) =>
            {
                self.confirm_go_standalone = false;
                self.slave_self_message = Some("Disconnected from coordinator".to_owned());
                self.open_settings();
                self.last_poll = None;
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
