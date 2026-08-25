//! TD Instances + MCP Clients cards for the merged Overview page:
//! master actions live in the card header, federation self-view and
//! scan results flow inside the body.

use eframe::egui;

use super::widgets::{card_with_header, fleet_row, mcp_row};
use crate::app::{DashboardApp, FleetPanel};
use crate::theme::{
    badge, empty_state, font_label, ghost_button, ACCENT, TEXT, TEXT_DIM, TEXT_FAINT,
    BadgeKind,
};
use crate::wire::{id_tail, parse_slaves, FleetView, SlaveSettingsTarget};

impl DashboardApp {
    /// MCP clients card.
    pub(crate) fn draw_mcp_section(&mut self, ui: &mut egui::Ui) {
        let sessions = serde_json::from_str::<crate::wire::SessionsView>(&self.sessions_json)
            .map(|v| v.sessions)
            .unwrap_or_default();
        let count = sessions.len();
        card_with_header(
            ui,
            &format!("MCP CLIENTS ({count})"),
            None,
            |_| {},
            |ui| {
                if count == 0 {
                    ui.label(
                        egui::RichText::new(
                            "Connect an MCP client to this daemon's /mcp endpoint.",
                        )
                        .font(crate::theme::font_meta())
                        .color(TEXT_DIM),
                    );
                } else {
                    for s in &sessions {
                        mcp_row(ui, s);
                    }
                }
            },
        );
    }

    /// TouchDesigner instances card: master gets Add-slave/Scan actions in
    /// the header; slaves see their federation link; everyone sees rows.
    pub(crate) fn draw_td_section(&mut self, ui: &mut egui::Ui) {
        let count = serde_json::from_str::<FleetView>(&self.fleet_json)
            .map(|f| f.processes.len())
            .unwrap_or(0);
        let role = self
            .status
            .as_ref()
            .map(|s| s.role.clone())
            .unwrap_or_default();
        let is_master = role == "master";
        let is_slave = role == "slave";
        let slaves = if is_master { self.slave_count() } else { 0 };
        let mut add_slave = false;

        card_with_header(
            ui,
            &format!("TOUCHDESIGNER ({count})"),
            None,
            |ui| {
                if !is_master {
                    return;
                }
                if slaves > 0 {
                    let _ = badge(ui, &format!("{slaves} slave(s)"), crate::theme::BadgeKind::Neutral);
                    ui.add_space(crate::theme::sp::XS);
                }
                if ghost_button(ui, "+ Add slave…", TEXT_DIM, ACCENT)
                    .on_hover_text("Configure another machine's daemon as a slave")
                    .clicked()
                {
                    add_slave = true;
                }
            },
            |ui| {
                if is_master {
                    self.draw_fleet_groups(ui);
                } else {
                    self.draw_flat_fleet(ui);
                }
                if is_slave {
                    self.draw_slave_self_view(ui);
                }
            },
        );
        if add_slave {
            self.fleet_panel = FleetPanel::AddSlave;
        }
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
        let mut groups: std::collections::HashMap<String, Vec<&crate::wire::FleetProc>> =
            std::collections::HashMap::new();
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
            let (led_kind, reach) = if is_local {
                (BadgeKind::Accent, "local".to_owned())
            } else if let Some(s) = slave {
                match s.reachability.as_str() {
                    "reachable" => (BadgeKind::Ok, "reachable".to_owned()),
                    "disconnected" => (BadgeKind::Warn, "disconnected".to_owned()),
                    _ => (BadgeKind::Error, "unreachable".to_owned()),
                }
            } else {
                (BadgeKind::Neutral, "unknown".to_owned())
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
            let proc_count = slave.map(|s| s.process_count).unwrap_or(procs.len());

            ui.horizontal(|ui| {
                // No ● prefix: U+25CF has no glyph in the bundled fonts — the
                // LOCAL/SLAVE word plus a tone badge carry the state.
                ui.label(
                    egui::RichText::new(if is_local { "LOCAL" } else { "SLAVE" })
                        .font(font_label())
                        .color(if is_local { TEXT } else { TEXT_FAINT }),
                );
                let _ = badge(ui, &hostname, led_kind);
                if !tail.is_empty() {
                    ui.label(
                        egui::RichText::new(tail)
                            .font(crate::theme::font_mono())
                            .color(TEXT_FAINT),
                    );
                }
                if !is_local {
                    let _ = badge(ui, reach.as_str(), led_kind);
                    ui.label(
                        egui::RichText::new(format!("{proc_count} proc"))
                            .font(crate::theme::font_meta())
                            .color(TEXT_FAINT),
                    );
                }
                if let Some(s) = slave {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            egui::RichText::new(s.base_url.as_str())
                                .font(crate::theme::font_mono())
                                .color(TEXT_FAINT),
                        );
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
                    });
                }
            });
            ui.add_space(2.0);
            for p in procs.iter() {
                fleet_row(ui, p);
            }
            ui.add_space(6.0);
        }
        if let Some(target) = pending_settings {
            self.open_slave_settings(target);
        }
    }

    /// Standalone / slave: local processes as one flat list.
    fn draw_flat_fleet(&mut self, ui: &mut egui::Ui) {
        let Ok(fleet) = serde_json::from_str::<FleetView>(&self.fleet_json) else {
            self.draw_empty_fleet(ui);
            return;
        };
        if fleet.processes.is_empty() {
            self.draw_empty_fleet(ui);
            return;
        }
        for p in fleet.processes.iter() {
            fleet_row(ui, p);
        }
    }

    fn draw_empty_fleet(&mut self, ui: &mut egui::Ui) {
        if empty_state(
            ui,
            "No TouchDesigner instances yet",
            "Open bootstrap.tox in TouchDesigner to bridge it.",
            Some("Reveal .tox"),
        ) {
            self.reveal_tox();
        }
    }
}
