//! Dashboard secondary viewport — Docker-Desktop-style rich window.
//!
//! Iteration 1 shell: sidebar navigation + Overview cards + latest errors +
//! embedded Fleet sections. Logs/Settings migrate here in later iterations.

use eframe::egui::{self, Color32, FontId, Sense};

use crate::theme::{
    self, font_label, font_meta, font_mono, font_title, status_led, ACCENT, BG_ACTIVE, BG_HOVER,
    BG_PANEL, BG_ROW, BORDER, ERR, OK, TEXT, TEXT_DIM, TEXT_FAINT, WARN,
};
use crate::DashboardApp;

/// Sidebar width (px).
const SIDEBAR_W: f32 = 196.0;
/// Top bar height (px).
const TOPBAR_H: f32 = 44.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DashTab {
    #[default]
    Overview,
    Fleet,
    Logs,
    Settings,
}

impl DashTab {
    fn label(self) -> &'static str {
        match self {
            DashTab::Overview => "Overview",
            DashTab::Fleet => "Fleet",
            DashTab::Logs => "Logs",
            DashTab::Settings => "Settings",
        }
    }

    const ALL: [DashTab; 4] = [
        DashTab::Overview,
        DashTab::Fleet,
        DashTab::Logs,
        DashTab::Settings,
    ];
}

/// Stable id for the dashboard viewport.
#[must_use]
pub fn viewport_id() -> egui::ViewportId {
    egui::ViewportId(egui::Id::new("tdmcp-dashboard"))
}

/// Window builder for the dashboard viewport (decorated, resizable, real taskbar entry).
#[must_use]
pub fn builder(window_icon: &egui::IconData) -> egui::ViewportBuilder {
    egui::ViewportBuilder::default()
        .with_title("td-mcp-rs — Dashboard")
        .with_inner_size([980.0, 660.0])
        .with_min_inner_size([860.0, 540.0])
        .with_icon(egui::IconData {
            width: window_icon.width,
            height: window_icon.height,
            rgba: window_icon.rgba.clone(),
        })
}

/// Render the dashboard into its viewport's root ui. Called every frame while open.
pub fn render(app: &mut DashboardApp, ui: &mut egui::Ui) {
    if ui.input(|i| i.viewport().close_requested()) {
        // Honor the OS close; the viewport simply stops being shown.
        app.dashboard_open = false;
    }

    egui::Panel::left("dash_sidebar")
        .exact_size(SIDEBAR_W)
        .frame(
            egui::Frame::NONE
                .fill(BG_PANEL)
                .stroke(egui::Stroke::new(1.0, BORDER))
                .inner_margin(egui::Margin {
                    left: 0,
                    right: 0,
                    top: 14,
                    bottom: 12,
                }),
        )
        .show(ui, |ui| {
            sidebar(app, ui);
        });

    egui::Panel::top("dash_topbar")
        .exact_size(TOPBAR_H)
        .frame(
            egui::Frame::NONE
                .fill(theme::BG_WINDOW)
                .stroke(egui::Stroke::new(1.0, BORDER))
                .inner_margin(egui::Margin::symmetric(20, 0)),
        )
        .show(ui, |ui| {
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                ui.label(
                    egui::RichText::new(app.dash_tab.label())
                        .font(font_title())
                        .color(TEXT),
                );
            });
        });

    egui::CentralPanel::default()
        .frame(
            egui::Frame::NONE
                .fill(theme::BG_WINDOW)
                .inner_margin(egui::Margin::same(20)),
        )
        .show(ui, |ui| match app.dash_tab {
            DashTab::Overview => overview(app, ui),
            DashTab::Fleet => fleet(app, ui),
            DashTab::Logs => placeholder(
                ui,
                "Logs",
                "Log streaming lands in iteration 2.\nUse the tray popup's ≡ view meanwhile.",
            ),
            DashTab::Settings => placeholder(
                ui,
                "Settings",
                "Full settings forms land in iteration 3.\nUse the tray popup's ⚙ view meanwhile.",
            ),
        });
}

fn sidebar(app: &mut DashboardApp, ui: &mut egui::Ui) {
    // Brand block.
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.add_space(14.0);
        status_led(ui, ACCENT);
        ui.add_space(6.0);
        ui.label(
            egui::RichText::new("td-mcp-rs")
                .font(font_title())
                .color(TEXT),
        );
    });
    ui.add_space(2.0);
    ui.horizontal(|ui| {
        ui.add_space(36.0);
        let role = app
            .status
            .as_ref()
            .map(|s| s.role.to_ascii_lowercase())
            .unwrap_or_else(|| "offline".to_owned());
        ui.label(
            egui::RichText::new(role)
                .font(font_meta())
                .color(TEXT_FAINT),
        );
    });
    ui.add_space(18.0);

    for tab in DashTab::ALL {
        let selected = app.dash_tab == tab;
        if nav_item(ui, tab.label(), selected).clicked() {
            app.dash_tab = tab;
        }
    }

    ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
        ui.add_space(8.0);
        ui.separator();
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.add_space(14.0);
            status_led(ui, health_color(app));
            ui.add_space(6.0);
            let meta = match app.status.as_ref() {
                Some(s) => format!("pid {} · {}", s.pid, s.bind_address),
                None => "daemon unreachable".to_owned(),
            };
            ui.label(egui::RichText::new(meta).font(font_mono()).color(TEXT_DIM));
        });
    });
}

fn nav_item(ui: &mut egui::Ui, label: &str, selected: bool) -> egui::Response {
    let size = egui::vec2(ui.available_width(), 30.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    let hovered = response.hovered();
    let fill = if selected {
        BG_ACTIVE
    } else if hovered {
        BG_HOVER
    } else {
        Color32::TRANSPARENT
    };
    ui.painter().rect_filled(rect, 0.0, fill);
    if selected {
        let bar = egui::Rect::from_min_size(
            rect.left_top() + egui::vec2(0.0, 5.0),
            egui::vec2(3.0, rect.height() - 10.0),
        );
        ui.painter().rect_filled(bar, 2.0, ACCENT);
    }
    ui.painter().text(
        egui::pos2(rect.left() + 16.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        label,
        font_label(),
        if selected { TEXT } else { TEXT_DIM },
    );
    response
}

fn health_color(app: &DashboardApp) -> Color32 {
    if app.error.is_some() || app.status.is_none() {
        ERR
    } else if app.attention {
        WARN
    } else {
        OK
    }
}

fn overview(app: &mut DashboardApp, ui: &mut egui::Ui) {
    let mcp_n = app
        .status
        .as_ref()
        .map(|s| s.mcp_session_count)
        .unwrap_or(0);
    let snap = &app.prev_snapshot;
    let attention = snap.disconnected + snap.resurrected + snap.cancelled;
    let role = app
        .status
        .as_ref()
        .map(|s| s.role.to_ascii_lowercase())
        .unwrap_or_else(|| "offline".to_owned());

    ui.add_space(4.0);
    ui.columns(4, |cols| {
        stat_card(&mut cols[0], &mcp_n.to_string(), "MCP CLIENTS", OK);
        stat_card(
            &mut cols[1],
            &snap.connected.to_string(),
            "TD CONNECTED",
            OK,
        );
        stat_card(
            &mut cols[2],
            &attention.to_string(),
            "NEEDS ATTENTION",
            if attention > 0 { WARN } else { TEXT_DIM },
        );
        stat_card(&mut cols[3], role.to_uppercase().as_str(), "ROLE", ACCENT);
    });

    ui.add_space(18.0);

    // Latest errors card.
    let count = app.error_ring.len();
    egui::Frame::NONE
        .fill(BG_ROW)
        .stroke(egui::Stroke::new(1.0, BORDER))
        .corner_radius(egui::CornerRadius::same(6))
        .inner_margin(egui::Margin::same(14))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new("LATEST ERRORS")
                        .font(font_meta())
                        .color(TEXT_DIM),
                );
                if count > 0 {
                    ui.label(
                        egui::RichText::new(count.to_string())
                            .font(font_meta())
                            .color(ERR),
                    );
                }
            });
            ui.add_space(6.0);
            if app.error_ring.is_empty() {
                ui.label(
                    egui::RichText::new("No recent errors")
                        .font(font_label())
                        .color(TEXT_FAINT),
                );
            } else {
                egui::ScrollArea::vertical()
                    .max_height(220.0)
                    .auto_shrink(false)
                    .show(ui, |ui| {
                        for msg in app.error_ring.clone().iter() {
                            error_row(ui, msg);
                        }
                    });
            }
        });
}

fn stat_card(ui: &mut egui::Ui, value: &str, title: &str, value_color: Color32) {
    let size = egui::vec2(ui.available_width(), 68.0);
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    ui.painter().rect_filled(rect, 6.0, BG_PANEL);
    ui.painter().rect_stroke(
        rect,
        6.0,
        egui::Stroke::new(1.0, BORDER),
        egui::StrokeKind::Inside,
    );
    ui.painter().text(
        egui::pos2(rect.left() + 14.0, rect.top() + 26.0),
        egui::Align2::LEFT_CENTER,
        value,
        FontId::new(19.0, egui::FontFamily::Proportional),
        value_color,
    );
    ui.painter().text(
        egui::pos2(rect.left() + 14.0, rect.bottom() - 16.0),
        egui::Align2::LEFT_CENTER,
        title,
        font_meta(),
        TEXT_FAINT,
    );
}

fn error_row(ui: &mut egui::Ui, msg: &str) {
    let h = 22.0;
    let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), h), Sense::hover());
    ui.painter()
        .circle_filled(egui::pos2(rect.left() + 5.0, rect.center().y), 3.0, ERR);
    ui.painter().text(
        egui::pos2(rect.left() + 16.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        msg,
        font_mono(),
        TEXT_DIM,
    );
}

fn fleet(app: &mut DashboardApp, ui: &mut egui::Ui) {
    egui::ScrollArea::vertical()
        .auto_shrink(false)
        .show(ui, |ui| {
            app.draw_master_actions(ui);
            app.draw_mcp_section(ui);
            app.draw_td_section(ui);
        });
}

fn placeholder(ui: &mut egui::Ui, title: &str, note: &str) {
    ui.centered_and_justified(|ui| {
        ui.vertical_centered(|ui| {
            ui.label(
                egui::RichText::new(title)
                    .font(FontId::new(17.0, egui::FontFamily::Proportional))
                    .color(TEXT_DIM),
            );
            ui.add_space(6.0);
            ui.label(
                egui::RichText::new(note)
                    .font(font_label())
                    .color(TEXT_FAINT),
            );
        });
    });
}
