//! Tray popup glance card — frameless always-near-tray surface.
//!
//! The body stays a read-only glance (pass-8): navigation (`⛶`/`⚙`) and report
//! links only. Pass 10 added a footer carrying the daemon lifecycle actions
//! (Stop / Restart / Reveal .tox), so the controls a user reaches for most are
//! one tray click away instead of buried in a dashboard tab.

use eframe::egui;

use crate::app::DashboardApp;
use crate::dashboard::widgets::{fleet_row, section_caption};
use crate::dashboard::{self};
use crate::platform::reveal_in_file_manager;
use crate::theme::{
    font_meta, font_mono, font_title, ghost_button, status_led, ACCENT, BG_HOVER, BG_PANEL, BORDER,
    ERR, RADIUS_SM, SIDE_MARGIN, TEXT, TEXT_DIM, TEXT_FAINT, WARN,
};
use crate::wire::{clip_line, FleetView, SessionsView};

/// Top chrome strip height (px).
pub(crate) const HEADER_H: f32 = 34.0;
/// Width reserved for the header's right-anchored actions (px).
pub(crate) const HEADER_ACTIONS_W: f32 = 64.0;
/// Bottom action-footer height (px).
pub(crate) const FOOTER_H: f32 = 38.0;
/// What the pinned footer actually costs the column: symmetric `sp::SM`
/// breathing room above and below the footer row. Callers reserve this — not
/// bare [`FOOTER_H`] — out of the scroll budget, or the actions sit flush
/// against the window edge.
pub(crate) const FOOTER_BLOCK_H: f32 = crate::theme::sp::SM + FOOTER_H + crate::theme::sp::SM;
/// Popup glance caps — depth lives in the dashboard, not the tray window.
const POPUP_ATTENTION_ROWS: usize = 2;
const POPUP_FLEET_ROWS: usize = 4;

impl DashboardApp {
    /// Top chrome: LED + identity (title · version) left, dashboard/gear right.
    /// Daemon lifecycle actions sit in the footer ([`Self::draw_action_footer`]).
    pub(crate) fn draw_header(&mut self, ui: &mut egui::Ui) {
        let full = ui.available_width();
        let (rect, _) = ui.allocate_exact_size(egui::vec2(full, HEADER_H), egui::Sense::hover());
        ui.painter().rect_filled(rect, 0.0, BG_PANEL);
        ui.painter().hline(
            rect.x_range(),
            rect.bottom(),
            egui::Stroke::new(1.0, BORDER),
        );

        // Overlapping LTR/RTL children over one rect (see theme::row_between):
        // a sequential with_layout here clipped the actions out of the render.
        let inset = rect.shrink2(egui::vec2(SIDE_MARGIN, 0.0));
        let mut left_ui = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(inset)
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        status_led(&mut left_ui, ACCENT);
        left_ui.add_space(crate::theme::sp::XS);

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
            } else if tdmcp_config::is_loopback_bind(bind) {
                format!(" · {bind} (loopback)")
            } else {
                format!(" · {bind} (remote)")
            };
            format!("pid {}{bind}", st.pid)
        });
        let id_w = (left_ui.available_width() - HEADER_ACTIONS_W).max(64.0);
        left_ui.allocate_ui_with_layout(
            egui::vec2(id_w, HEADER_H),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                ui.set_clip_rect(ui.max_rect());
                ui.label(egui::RichText::new(title).font(font_title()).color(TEXT));
                if !version.is_empty() {
                    ui.add_space(crate::theme::sp::XS);
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

        // Right-anchored ghost actions: dashboard launcher · gear (RTL).
        // First widget added lands at the RIGHT edge.
        let mut right_ui = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(inset)
                .layout(egui::Layout::right_to_left(egui::Align::Center)),
        );
        let gear = ghost_button(&mut right_ui, "⚙", TEXT_DIM, ACCENT).on_hover_text("Settings");
        if gear.clicked() {
            self.open_settings();
            self.dash_tab = dashboard::DashTab::Settings;
            self.dashboard_open = true;
        }
        right_ui.add_space(crate::theme::sp::XS);
        let dash_active = self.dashboard_open;
        let dash_color = if dash_active { ACCENT } else { TEXT_DIM };
        // U+26F6 FOUR CORNERS — the only expand-ish glyph covered by the
        // bundled proportional fonts (U+2922 is in none of them → tofu box).
        let dash =
            ghost_button(&mut right_ui, "⛶", dash_color, ACCENT).on_hover_text("Open dashboard");
        if dash.clicked() {
            self.dashboard_open = true;
        }
    }

    /// Glance-card body: attention, TouchDesigner instances, MCP clients,
    /// share hint — each compact, with depth one click away in the dashboard.
    pub(crate) fn draw_summary(&mut self, ui: &mut egui::Ui) {
        let procs = serde_json::from_str::<FleetView>(&self.fleet_json)
            .map(|f| f.processes)
            .unwrap_or_default();
        let sessions = serde_json::from_str::<SessionsView>(&self.sessions_json)
            .map(|v| v.sessions)
            .unwrap_or_default();

        if self.status.is_none() && self.error.is_none() {
            ui.vertical_centered(|ui| {
                ui.add_space(crate::theme::sp::XL);
                ui.spinner();
                ui.add_space(crate::theme::sp::SM);
                ui.label(
                    egui::RichText::new("Connecting to daemon…")
                        .font(font_meta())
                        .color(TEXT_DIM),
                );
                ui.add_space(crate::theme::sp::XL);
            });
            return;
        }

        // Attention: transient action/poll error first, then ring history.
        // The transient line is not in the ring, so skip a duplicate head.
        let mut shown = 0usize;
        if let Some(err) = &self.error {
            attention_row(ui, err);
            shown += 1;
        }
        for msg in &self.error_ring {
            if shown >= POPUP_ATTENTION_ROWS {
                break;
            }
            if self.error.as_deref() == Some(msg.as_str()) {
                continue;
            }
            attention_row(ui, msg);
            shown += 1;
        }
        let hidden = self.error_ring.len() + usize::from(self.error.is_some()) - shown;
        if hidden > 0 && shown > 0 {
            ui.horizontal(|ui| {
                ui.add_space(SIDE_MARGIN);
                if ghost_button(
                    ui,
                    &format!("+{hidden} more — open dashboard"),
                    TEXT_FAINT,
                    ACCENT,
                )
                .clicked()
                {
                    self.dash_tab = dashboard::DashTab::Overview;
                    self.dashboard_open = true;
                }
            });
        }

        // Crash from a previous run — one-click reveal; ack'd per session.
        if !self.crash_seen {
            if let Some(crash) = self.last_crash.clone() {
                ui.horizontal(|ui| {
                    ui.add_space(SIDE_MARGIN);
                    if ghost_button(ui, "Previous run crashed — open report", WARN, ACCENT)
                        .clicked()
                    {
                        self.crash_seen = true;
                        let _ = reveal_in_file_manager(&crash, &self.data_dir);
                    }
                });
            }
        }

        if !procs.is_empty() {
            section_caption(ui, &format!("TOUCHDESIGNER · {}", procs.len()));
            for p in procs.iter().take(POPUP_FLEET_ROWS) {
                fleet_row(ui, p);
            }
            if procs.len() > POPUP_FLEET_ROWS {
                ui.horizontal(|ui| {
                    ui.add_space(SIDE_MARGIN);
                    if ghost_button(
                        ui,
                        &format!("+{} more — open dashboard", procs.len() - POPUP_FLEET_ROWS),
                        TEXT_FAINT,
                        ACCENT,
                    )
                    .clicked()
                    {
                        self.dash_tab = dashboard::DashTab::Overview;
                        self.dashboard_open = true;
                    }
                });
            }
        }

        if !sessions.is_empty() {
            let names: Vec<String> = sessions
                .iter()
                .take(2)
                .map(|s| s.client_name.clone())
                .collect();
            let more = sessions.len().saturating_sub(names.len());
            let names = if more > 0 {
                format!("{}, … +{more}", names.join(", "))
            } else {
                names.join(", ")
            };
            section_caption(ui, &format!("MCP CLIENTS · {}", sessions.len()));
            ui.horizontal(|ui| {
                ui.add_space(SIDE_MARGIN);
                ui.label(egui::RichText::new(names).font(font_meta()).color(TEXT_DIM));
            });
        }

        if procs.is_empty() && sessions.is_empty() && shown == 0 {
            ui.vertical_centered(|ui| {
                ui.add_space(crate::theme::sp::LG);
                ui.label(
                    egui::RichText::new("Waiting for TouchDesigner…")
                        .font(font_meta())
                        .color(TEXT_DIM),
                );
                ui.add_space(crate::theme::sp::SM);
            });
        }

        ui.add_space(crate::theme::sp::XS);
    }

    /// Bottom chrome: daemon lifecycle actions, hairline-separated from the
    /// glance body so it reads as chrome rather than content.
    ///
    /// Rendered by the caller *outside* the scroll area — it is pinned chrome,
    /// and `draw_summary` returns early on the connecting path where these
    /// actions are still wanted.
    ///
    /// Shares [`dashboard::widgets::daemon_actions`] with the dashboard top bar
    /// — including the two-step Stop, which matters more here: the popup hides
    /// on focus loss, so a one-click exit would be easy to trigger by accident.
    pub(crate) fn draw_action_footer(&mut self, ui: &mut egui::Ui) {
        // Symmetric top/bottom breathing room (sp::SM both sides).
        ui.add_space(crate::theme::sp::SM);
        let full = ui.available_width();
        let (rect, _) = ui.allocate_exact_size(egui::vec2(full, FOOTER_H), egui::Sense::hover());
        ui.painter().rect_filled(rect, 0.0, BG_PANEL);
        ui.painter()
            .hline(rect.x_range(), rect.top(), egui::Stroke::new(1.0, BORDER));
        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(rect.shrink2(egui::vec2(SIDE_MARGIN, 0.0)))
                .layout(egui::Layout::right_to_left(egui::Align::Center)),
        );
        dashboard::widgets::daemon_actions(self, &mut child);
        ui.add_space(crate::theme::sp::SM);
    }
}

/// One attention line: ERR dot + clipped message, hover highlight only.
fn attention_row(ui: &mut egui::Ui, msg: &str) {
    let full = ui.available_width();
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(full, crate::theme::ROW_H), egui::Sense::hover());
    if response.hovered() {
        ui.painter().rect_filled(rect, RADIUS_SM, BG_HOVER);
    }
    let center = egui::pos2(rect.left() + SIDE_MARGIN + 3.0, rect.center().y);
    ui.painter().circle_filled(center, 3.0, ERR);
    ui.painter().text(
        egui::pos2(rect.left() + SIDE_MARGIN + 14.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        clip_line(msg, 46),
        font_mono(),
        TEXT_DIM,
    );
}
