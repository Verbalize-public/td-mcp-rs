//! Dashboard shared painted components: stat tiles, fleet/MCP rows,
//! section captions, headered cards, federation modal shell.

use std::time::{SystemTime, UNIX_EPOCH};

use eframe::egui::{self, Color32};

use crate::theme::{
    font_label, font_meta, font_mono, font_stat, row_between, status_led, BG_CARD, BG_HOVER,
    BORDER, CARD_PAD, OK, RADIUS_MD, RADIUS_SM, ROW_H, SIDE_MARGIN, TEXT, TEXT_DIM, TEXT_FAINT,
    WARN,
};
use crate::theme::sp;
use crate::wire::{id_tail, FleetProc, SessionRow};

/// Quiet section caption — dim meta text, no strip, no rule.
pub(crate) fn section_caption(ui: &mut egui::Ui, title: &str) {
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 18.0), egui::Sense::hover());
    ui.painter().text(
        egui::pos2(rect.left() + SIDE_MARGIN, rect.center().y),
        egui::Align2::LEFT_CENTER,
        title,
        font_meta(),
        TEXT_FAINT,
    );
}

/// Card with an uppercase caption header and a right-aligned action slot.
/// `accent` optionally paints a tone-colored edge on the left of the card.
pub(crate) fn card_with_header(
    ui: &mut egui::Ui,
    title: &str,
    accent: Option<Color32>,
    right: impl FnOnce(&mut egui::Ui),
    body: impl FnOnce(&mut egui::Ui),
) {
    let inner = egui::Frame::NONE
        .fill(BG_CARD)
        .stroke(egui::Stroke::new(1.0, BORDER))
        .corner_radius(egui::CornerRadius::same(RADIUS_MD as u8))
        .inner_margin(egui::Margin::same(CARD_PAD as i8))
        .show(ui, |ui| {
            row_between(
                ui,
                18.0,
                |ui| {
                    ui.label(
                        egui::RichText::new(title)
                            .font(font_meta())
                            .color(TEXT_FAINT),
                    );
                },
                right,
            );
            ui.add_space(sp::XS);
            body(ui);
        });
    if let Some(color) = accent {
        let r = inner.response.rect;
        ui.painter().rect_filled(
            egui::Rect::from_min_size(
                r.left_top() + egui::vec2(1.0, 1.0),
                egui::vec2(3.0, r.height() - 2.0),
            ),
            2.0,
            color,
        );
    }
    ui.add_space(sp::MD);
}

/// One TouchDesigner instance row — LED, pid, title, bridge/task counts.
/// Hover-only highlight; hover carries a summary tooltip.
pub(crate) fn fleet_row(ui: &mut egui::Ui, p: &FleetProc) {
    let bridge = p.bridge.as_str().unwrap_or("?");
    let attention_row =
        p.resurrected || !p.cancelled_tasks.is_empty() || bridge == "disconnected";
    let led = if attention_row {
        WARN
    } else if bridge == "connected" {
        OK
    } else {
        TEXT_FAINT
    };
    let full = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(egui::vec2(full, ROW_H), egui::Sense::hover());
    let fill = if response.hovered() {
        BG_HOVER
    } else {
        Color32::TRANSPARENT
    };
    ui.painter().rect_filled(rect, RADIUS_SM, fill);

    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect.shrink2(egui::vec2(SIDE_MARGIN, 0.0)))
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    status_led(&mut child, led);
    child.add_space(sp::XS);
    child.label(
        egui::RichText::new(p.pid.to_string())
            .font(font_mono())
            .color(TEXT_FAINT),
    );
    child.add_space(sp::SM);
    child.label(
        egui::RichText::new(p.title.as_deref().unwrap_or(""))
            .font(font_label())
            .color(TEXT),
    );
    child.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        if !p.cancelled_tasks.is_empty() {
            ui.label(
                egui::RichText::new(p.cancelled_tasks.len().to_string())
                    .font(font_mono())
                    .color(WARN),
            );
            ui.add_space(sp::XS);
        }
        ui.label(
            egui::RichText::new(format!(
                "tasks {}",
                p.tasks.as_ref().map(|t| t.len()).unwrap_or(0)
            ))
            .font(font_mono())
            .color(TEXT_DIM),
        );
        ui.add_space(sp::SM);
        ui.label(
            egui::RichText::new(bridge)
                .font(font_meta())
                .color(if bridge == "connected" {
                    OK
                } else if bridge == "disconnected" {
                    WARN
                } else {
                    TEXT_FAINT
                }),
        );
    });
    response.on_hover_text(format!(
        "pid {} · bridge {bridge} · {} active task(s) · {} cancelled{}",
        p.pid,
        p.tasks.as_ref().map(|t| t.len()).unwrap_or(0),
        p.cancelled_tasks.len(),
        if p.resurrected {
            " · resurrected"
        } else {
            ""
        }
    ));
}

/// One MCP session row: client name first, id tail demoted to trailing meta.
pub(crate) fn mcp_row(ui: &mut egui::Ui, s: &SessionRow) {
    let full = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(egui::vec2(full, ROW_H), egui::Sense::hover());
    if response.hovered() {
        ui.painter().rect_filled(rect, RADIUS_SM, BG_HOVER);
    }
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect.shrink2(egui::vec2(SIDE_MARGIN, 0.0)))
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    status_led(&mut child, OK);
    child.add_space(sp::XS);
    child.label(
        egui::RichText::new(&s.client_name)
            .font(font_label())
            .color(TEXT),
    );
    if !s.client_version.is_empty() {
        child.label(
            egui::RichText::new(&s.client_version)
                .font(font_mono())
                .color(TEXT_DIM),
        );
    }
    child.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        ui.label(
            egui::RichText::new(id_tail(&s.id))
                .font(font_mono())
                .color(TEXT_FAINT),
        );
        ui.add_space(sp::SM);
        ui.label(
            egui::RichText::new(format_duration_since(s.connected_at))
                .font(font_mono())
                .color(TEXT_DIM),
        );
    });
}

#[must_use]
pub(crate) fn format_duration_since(connected_at_ms: u64) -> String {
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let secs = now_ms.saturating_sub(connected_at_ms) / 1000;
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else {
        format!("{}h", secs / 3600)
    }
}

/// Compact human duration (`3h 12m`, `5m`, `42s`); empty when zero.
#[must_use]
pub(crate) fn format_uptime(secs: u64) -> String {
    if secs == 0 {
        return String::new();
    }
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else {
        format!("{}h {}m", secs / 3600, (secs % 3600) / 60)
    }
}

/// Big-number metric tile (MCP clients · TD connected · attention · role).
pub(crate) fn stat_card(ui: &mut egui::Ui, value: &str, title: &str, value_color: Color32) {
    let size = egui::vec2(ui.available_width(), 62.0);
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    ui.painter().rect_filled(rect, RADIUS_MD, BG_CARD);
    ui.painter().rect_stroke(
        rect,
        RADIUS_MD,
        egui::Stroke::new(1.0, BORDER),
        egui::StrokeKind::Inside,
    );
    ui.painter().text(
        egui::pos2(rect.left() + CARD_PAD, rect.top() + 26.0),
        egui::Align2::LEFT_CENTER,
        value,
        font_stat(),
        value_color,
    );
    ui.painter().text(
        egui::pos2(rect.left() + CARD_PAD, rect.bottom() - 12.0),
        egui::Align2::LEFT_CENTER,
        title,
        font_meta(),
        TEXT_FAINT,
    );
}

/// Federation overlays as centered modal cards; Esc or a panel's
/// "← Back" (`FleetPanel::None`) dismisses them.
pub(crate) fn modal_shell(ctx: &egui::Context, id: &str, add: impl FnOnce(&mut egui::Ui)) {
    egui::Modal::new(egui::Id::new(id))
        .frame(
            egui::Frame::NONE
                .fill(crate::theme::BG_WINDOW)
                .stroke(egui::Stroke::new(1.0, BORDER))
                .corner_radius(egui::CornerRadius::same(RADIUS_MD as u8))
                .inner_margin(egui::Margin::same(sp::LG as i8)),
        )
        .show(ctx, |ui| {
            ui.set_width(MODAL_W);
            add(ui);
        });
}

/// Modal dialog width (px).
pub(crate) const MODAL_W: f32 = 440.0;
