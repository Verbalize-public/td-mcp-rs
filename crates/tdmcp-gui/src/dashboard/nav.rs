//! Dashboard sidebar: brand block (LED · name · role), nav items with
//! attention badge, compact health-word footer.

use eframe::egui::{self, Color32, Sense};

use super::DashTab;
use crate::app::DashboardApp;
use crate::theme::{
    badge, font_label, font_meta, font_title, status_led, ACCENT, BG_ACTIVE, BG_HOVER, ERR, OK,
    TEXT, TEXT_DIM, WARN, BadgeKind,
};

/// Sidebar width (px).
pub(crate) const SIDEBAR_W: f32 = 172.0;

pub(crate) fn sidebar(app: &mut DashboardApp, ui: &mut egui::Ui) {
    let snap = &app.prev_snapshot;
    let attention = snap.disconnected + snap.resurrected + snap.cancelled;
    let offline = app.status.is_none() || app.error.is_some();

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
    ui.horizontal(|ui| {
        ui.add_space(14.0);
        let role = app
            .status
            .as_ref()
            .map(|s| s.role.to_ascii_lowercase())
            .unwrap_or_else(|| "offline".to_owned());
        let _ = badge(
            ui,
            &role,
            if offline {
                BadgeKind::Error
            } else {
                BadgeKind::Neutral
            },
        );
    });
    ui.add_space(18.0);

    for tab in DashTab::ALL {
        let selected = app.dash_tab == tab;
        // The Overview nav item carries the live attention count.
        let count = if tab == DashTab::Overview && attention > 0 && !offline {
            Some(attention)
        } else {
            None
        };
        if nav_item(ui, tab.label(), selected, count).clicked() {
            app.dash_tab = tab;
        }
    }

    ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
        // Status sticks to bottom with symmetric 12px breathing room — no separator line.
        ui.add_space(12.0);
        ui.horizontal(|ui| {
            ui.add_space(14.0);
            let (color, word) = if offline {
                (ERR, "offline")
            } else if attention > 0 {
                (WARN, "attention")
            } else {
                (OK, "all good")
            };
            status_led(ui, color);
            ui.add_space(6.0);
            ui.label(egui::RichText::new(word).font(font_meta()).color(TEXT_DIM))
                .on_hover_text(format!(
                    "attention: {} disconnected · {} resurrected · {} cancelled",
                    snap.disconnected, snap.resurrected, snap.cancelled
                ));
        });
        ui.add_space(4.0);
    });
}

fn nav_item(
    ui: &mut egui::Ui,
    label: &str,
    selected: bool,
    badge_count: Option<usize>,
) -> egui::Response {
    let size = egui::vec2(ui.available_width(), 30.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    let hovered = response.hovered();
    // Smooth 120ms hover fill instead of a hard swap.
    let t = ui
        .ctx()
        .animate_bool_with_time(egui::Id::new(("dash_nav", label)), hovered, 0.12);
    let fill = if selected {
        BG_ACTIVE
    } else {
        blend(BG_HOVER, Color32::TRANSPARENT, 1.0 - t)
    };
    ui.painter().rect_filled(rect, egui::CornerRadius::same(6), fill);
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
    if let Some(n) = badge_count {
        // Right-aligned amber count pill inside the item.
        let text = n.to_string();
        let galley = ui
            .painter()
            .layout_no_wrap(text.clone(), font_meta(), WARN);
        let w = galley.size().x + 10.0;
        let pill = egui::Rect::from_min_size(
            egui::pos2(rect.right() - w - 8.0, rect.center().y - 8.0),
            egui::vec2(w, 16.0),
        );
        ui.painter()
            .rect_filled(pill, pill.height() * 0.5, crate::theme::WARN_BG);
        ui.painter().galley(
            egui::pos2(
                pill.center().x - galley.size().x * 0.5,
                pill.center().y - galley.size().y * 0.5,
            ),
            galley,
            WARN,
        );
    }
    response
}

/// Linear blend of two colors; `t` = amount of `b`.
fn blend(a: Color32, b: Color32, t: f32) -> Color32 {
    let lerp = |x: u8, y: u8| -> u8 { (x as f32 + (y as f32 - x as f32) * t).round() as u8 };
    Color32::from_rgb(lerp(a.r(), b.r()), lerp(a.g(), b.g()), lerp(a.b(), b.b()))
}
