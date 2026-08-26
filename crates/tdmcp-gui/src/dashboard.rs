//! Dashboard secondary viewport — Docker-Desktop-style rich window.
//!
//! Shell: sidebar navigation + top bar + central tab router. Page bodies live
//! in sibling modules (`overview`, `logs`, `settings`); shared painted pieces
//! in `widgets`; sidebar in `nav`.

mod fleet;
mod logs;
mod nav;
mod overview;
mod settings;
pub(crate) mod widgets;

use eframe::egui;

use crate::app::DashboardApp;
use crate::theme::{
    font_display, font_mono, status_led_pulse, ACCENT, BORDER, ERR, OK, TEXT, WARN,
};
/// Top bar height (px).
const TOPBAR_H: f32 = 38.0;
/// Page gutter (px) — central panel side/bottom margin.
const GUTTER: f32 = 16.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DashTab {
    #[default]
    Overview,
    Logs,
    Settings,
}

impl DashTab {
    fn label(self) -> &'static str {
        match self {
            DashTab::Overview => "Overview",
            DashTab::Logs => "Logs",
            DashTab::Settings => "Settings",
        }
    }

    const ALL: [DashTab; 3] = [DashTab::Overview, DashTab::Logs, DashTab::Settings];
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
        .with_inner_size([960.0, 640.0])
        .with_min_inner_size([800.0, 520.0])
        .with_icon(egui::IconData {
            width: window_icon.width,
            height: window_icon.height,
            rgba: window_icon.rgba.clone(),
        })
}

/// Render the dashboard into its viewport's root ui. Called every frame while open.
pub fn render(app: &mut DashboardApp, ui: &mut egui::Ui) {
    if ui.input(|i| i.viewport().close_requested()) {
        // Hide via visibility — do not destroy the pre-created viewport (reopen
        // breaks if the native window is allowed to close).
        let ctx = ui.ctx();
        let id = viewport_id();
        ctx.send_viewport_cmd_to(id, egui::ViewportCommand::CancelClose);
        ctx.send_viewport_cmd_to(id, egui::ViewportCommand::Visible(false));
        app.dashboard_open = false;
    }

    egui::Panel::left("dash_sidebar")
        .exact_size(nav::SIDEBAR_W)
        .frame(
            egui::Frame::NONE
                .fill(crate::theme::BG_PANEL)
                .stroke(egui::Stroke::new(1.0, BORDER))
                .inner_margin(egui::Margin {
                    left: 0,
                    right: 0,
                    top: 14,
                    bottom: 12,
                }),
        )
        .show(ui, |ui| {
            nav::sidebar(app, ui);
        });

    egui::Panel::top("dash_topbar")
        .exact_size(TOPBAR_H)
        .frame(
            egui::Frame::NONE
                .fill(crate::theme::BG_WINDOW)
                .stroke(egui::Stroke::new(1.0, BORDER))
                .inner_margin(egui::Margin::symmetric(GUTTER as i8, 0)),
        )
        .show(ui, |ui| {
            // Left: page title. Right (RTL, first-added lands rightmost):
            // health LED → identity meta → daemon lifecycle actions.
            let attention = app.attention;
            // Trimmed to `up <t> · v<ver>`: the three action buttons need the
            // width, and at the 800px minimum the old pid+bind form overflowed.
            // pid / bind live in the LED tooltip below instead.
            let meta = match app.status.as_ref() {
                Some(s) => {
                    let up = widgets::format_uptime(s.uptime_secs);
                    if up.is_empty() {
                        format!("v{}", s.version)
                    } else {
                        format!("up {} · v{}", up, s.version)
                    }
                }
                None => "daemon unreachable".to_owned(),
            };
            let led_tip = match app.status.as_ref() {
                Some(s) => format!("pid {} · {}", s.pid, s.bind_address),
                None => "daemon unreachable".to_owned(),
            };
            let led_color = if app.error.is_some() || app.status.is_none() {
                ERR
            } else if attention {
                WARN
            } else {
                OK
            };
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                ui.label(
                    egui::RichText::new(app.dash_tab.label())
                        .font(font_display())
                        .color(TEXT),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    status_led_pulse(ui, led_color, attention && app.error.is_none());
                    // `status_led_pulse` paints directly and returns nothing, so
                    // hang the tooltip on an explicitly allocated hover strip.
                    ui.add_space(crate::theme::sp::XS);
                    ui.label(
                        egui::RichText::new(meta)
                            .font(font_mono())
                            .color(crate::theme::TEXT_FAINT),
                    )
                    .on_hover_text(led_tip);
                    ui.add_space(crate::theme::sp::MD);
                    widgets::daemon_actions(app, ui);
                });
            });
        });

    egui::CentralPanel::default()
        .frame(
            egui::Frame::NONE
                .fill(crate::theme::BG_WINDOW)
                .inner_margin(egui::Margin {
                    left: GUTTER as i8,
                    right: GUTTER as i8,
                    top: GUTTER as i8,
                    bottom: GUTTER as i8,
                }),
        )
        .show(ui, |ui| match app.dash_tab {
            DashTab::Overview => overview::overview(app, ui),
            DashTab::Logs => logs::logs(app, ui),
            DashTab::Settings => settings::settings(app, ui),
        });

    draw_snacks(app, ui);
}

/// Bottom-right action-acknowledgment stack (≤3 items, ~3s TTL).
fn draw_snacks(app: &mut DashboardApp, ui: &mut egui::Ui) {
    let now = std::time::Instant::now();
    app.snacks
        .retain(|s| now.duration_since(s.at) < std::time::Duration::from_secs(3));
    if app.snacks.is_empty() {
        return;
    }
    egui::Area::new(egui::Id::new("dash_snacks"))
        .anchor(egui::Align2::RIGHT_BOTTOM, [-16.0, -16.0])
        .order(egui::Order::Foreground)
        .show(ui.ctx(), |ui| {
            for s in app.snacks.iter() {
                let (dot, text_color) = match s.tone {
                    crate::app::SnackTone::Info => (ACCENT, TEXT),
                    crate::app::SnackTone::Ok => (OK, TEXT),
                    crate::app::SnackTone::Warn => (WARN, TEXT),
                    crate::app::SnackTone::Error => (ERR, TEXT),
                };
                let galley = ui
                    .painter()
                    .layout_no_wrap(s.msg.clone(), font_mono(), text_color);
                let size = egui::vec2(galley.size().x + 30.0, 26.0);
                let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
                ui.painter()
                    .rect_filled(rect, egui::CornerRadius::same(6), crate::theme::BG_CARD);
                ui.painter().rect_stroke(
                    rect,
                    egui::CornerRadius::same(6),
                    egui::Stroke::new(1.0, BORDER),
                    egui::StrokeKind::Inside,
                );
                ui.painter().circle_filled(
                    egui::pos2(rect.left() + 12.0, rect.center().y),
                    3.0,
                    dot,
                );
                ui.painter().galley(
                    egui::pos2(rect.left() + 22.0, rect.center().y - galley.size().y * 0.5),
                    galley,
                    text_color,
                );
                ui.add_space(6.0);
            }
        });
}
