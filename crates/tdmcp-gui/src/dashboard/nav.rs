//! Dashboard sidebar: centered logo brand block, nav items with attention
//! badge, health footer carrying role / version / uptime meta.

use eframe::egui::{self, Color32, Sense};

use super::DashTab;
use crate::app::DashboardApp;
use crate::theme::{
    badge, font_label, font_meta, font_mono, font_title, status_led, BadgeKind, ACCENT, BG_ACTIVE,
    BG_HOVER, ERR, OK, TEXT, TEXT_DIM, TEXT_FAINT, WARN,
};

/// Sidebar width (px).
pub(crate) const SIDEBAR_W: f32 = 172.0;

/// Footer text inset (px) — matches the nav labels' 16px text origin so the
/// whole sidebar shares one left edge.
const FOOTER_INSET: f32 = 16.0;

/// Brand-mark display size (px).
const LOGO_SIZE: f32 = 44.0;

pub(crate) fn sidebar(app: &mut DashboardApp, ui: &mut egui::Ui) {
    let snap = &app.prev_snapshot;
    let attention = snap.disconnected + snap.resurrected + snap.cancelled;
    let offline = app.status.is_none() || app.error.is_some();

    // Brand block: the logo mark alone, well centered — the app name lives in
    // the window title, and role / version / uptime moved to the footer.
    ui.add_space(10.0);
    match crate::theme::logo_texture(ui.ctx()) {
        Some(tex) => {
            ui.vertical_centered(|ui| {
                ui.add(egui::Image::new(&tex).fit_to_exact_size(egui::vec2(LOGO_SIZE, LOGO_SIZE)));
            });
        }
        // Decode-failure fallback: keep the old text brand rather than an
        // empty header.
        None => {
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
        }
    }
    ui.add_space(14.0);

    // Palette's own attention count: components that failed to probe or are
    // wedge suspects. It is the one palette state that wants a human — a large
    // undescribed roster is normal, a component that hung TouchDesigner is not.
    let palette_attention = app.palette.stats.failed;

    for tab in DashTab::ALL {
        let selected = app.dash_tab == tab;
        // Nav items carry their own live attention count.
        let count = match tab {
            DashTab::Overview if attention > 0 && !offline => Some(attention),
            DashTab::Palette if palette_attention > 0 => Some(palette_attention),
            _ => None,
        };
        if nav_item(ui, tab.label(), selected, count).clicked() {
            app.dash_tab = tab;
        }
    }

    ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
        // Footer stack (painted bottom-up): version/uptime meta, role badge,
        // then the health word. Every row shares the 16px text inset; the
        // only gap to the window bottom is the panel's own 12px margin — the
        // footer adds none on top of it.
        // Version + uptime (hidden while the daemon is unreachable — the
        // role badge already says "offline").
        if let Some(s) = app.status.as_ref() {
            let up = super::widgets::format_uptime(s.uptime_secs);
            let meta = if up.is_empty() {
                format!("v{}", s.version)
            } else {
                format!("v{} · up {}", s.version, up)
            };
            ui.horizontal(|ui| {
                ui.add_space(FOOTER_INSET);
                ui.label(
                    egui::RichText::new(meta)
                        .font(font_mono())
                        .color(TEXT_FAINT),
                );
            });
            ui.add_space(6.0);
        }
        ui.horizontal(|ui| {
            // badge() pads its text 6px inside the pill — shift the box left
            // so the pill's *text* aligns with the rows above and below.
            ui.add_space(FOOTER_INSET - 6.0);
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
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.add_space(FOOTER_INSET);
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
        ui.add_space(10.0);
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
    ui.painter()
        .rect_filled(rect, egui::CornerRadius::same(6), fill);
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
        let galley = ui.painter().layout_no_wrap(text.clone(), font_meta(), WARN);
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
