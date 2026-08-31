//! Shared painted widgets (status LEDs, buttons, cards, chips, badges,
//! segmented controls, banners, empty states).
//!
//! Split from `theme.rs`; re-exported at the `theme` root so call sites
//! keep using `theme::ghost_button` etc.

use eframe::egui::{self, Color32, CornerRadius, Stroke};

use super::{
    font_label, font_meta, sp, ACCENT_BG, BG_CARD, BG_HOVER, BORDER, CARD_PAD, ERR, LED_SIZE,
    RADIUS_MD, RADIUS_SM, TEXT, TEXT_DIM, WARN,
};

/// Paint a 6px status LED (Ableton-style colored dot). Returns the LED's
/// hover response so callers can hang a tooltip on it.
pub fn status_led(ui: &mut egui::Ui, color: Color32) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(LED_SIZE + 2.0, LED_SIZE + 2.0),
        egui::Sense::hover(),
    );
    let center = rect.center();
    ui.painter().circle_filled(center, LED_SIZE * 0.5, color);
    response
}

/// Status LED with a breathing halo while `active` (attention signal).
/// Without `active` this is exactly [`status_led`].
pub fn status_led_pulse(ui: &mut egui::Ui, color: Color32, active: bool) -> egui::Response {
    if !active {
        return status_led(ui, color);
    }
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(LED_SIZE * 3.0, LED_SIZE * 3.0),
        egui::Sense::hover(),
    );
    let t = ui.input(|i| i.time) as f32;
    let pulse = (t * 2.2).sin() * 0.5 + 0.5;
    ui.painter().circle_filled(
        rect.center(),
        LED_SIZE * (1.1 + 0.45 * pulse),
        Color32::from_rgba_unmultiplied(
            color.r(),
            color.g(),
            color.b(),
            (36.0 + 60.0 * pulse) as u8,
        ),
    );
    ui.painter()
        .circle_filled(rect.center(), LED_SIZE * 0.5, color);
    response
}

/// Filled accent button — the settings primary action (Save).
pub fn filled_button(ui: &mut egui::Ui, label: &str) -> egui::Response {
    let galley = ui.painter().layout_no_wrap(
        label.to_owned(),
        font_label(),
        Color32::from_rgb(0x13, 0x13, 0x13),
    );
    let pad = egui::vec2(10.0, 3.0);
    let size = egui::vec2((galley.size().x + pad.x * 2.0).max(48.0), 22.0);
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());

    let hovered = response.hovered();
    let pressed = response.is_pointer_button_down_on();
    let fill = if pressed {
        // Darker accent while held.
        Color32::from_rgb(0xc9, 0x5d, 0x0e)
    } else if hovered {
        // Lighter accent on hover.
        Color32::from_rgb(0xff, 0x8f, 0x3a)
    } else {
        super::ACCENT
    };
    let text_color = Color32::from_rgb(0x13, 0x13, 0x13);

    ui.painter().rect_filled(rect, RADIUS_SM, fill);
    let text_galley = ui
        .painter()
        .layout_no_wrap(label.to_owned(), font_label(), text_color);
    ui.painter().galley(
        egui::pos2(
            rect.center().x - text_galley.size().x * 0.5,
            rect.center().y - text_galley.size().y * 0.5,
        ),
        text_galley,
        text_color,
    );
    response
}

/// Tone of an [`action_button`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionTone {
    /// Ordinary action (Reveal, Cancel).
    Neutral,
    /// Primary-ish action (Restart).
    Accent,
    /// Destructive action (Stop, Confirm).
    Danger,
}

impl ActionTone {
    /// `(text, hover fill)` for this tone.
    fn colors(self) -> (Color32, Color32) {
        match self {
            ActionTone::Neutral => (super::TEXT, BG_HOVER),
            ActionTone::Accent => (super::ACCENT, super::ACCENT_BG),
            ActionTone::Danger => (super::ERR, super::ERR_BG),
        }
    }
}

/// Bordered, tone-colored button — the middle weight between [`filled_button`]
/// (one solid accent per screen) and [`ghost_button`] (borderless tertiary).
///
/// Used for repeated primary actions that must stay findable, i.e. the daemon
/// lifecycle row in the dashboard top bar and the tray popup footer.
pub fn action_button(ui: &mut egui::Ui, label: &str, tone: ActionTone) -> egui::Response {
    let (text_color, hover_fill) = tone.colors();
    let galley = ui
        .painter()
        .layout_no_wrap(label.to_owned(), font_label(), text_color);
    let pad = egui::vec2(10.0, 3.0);
    let size = egui::vec2(galley.size().x + pad.x * 2.0, 22.0);
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());

    let active = response.hovered() || response.is_pointer_button_down_on();
    let fill = if active { hover_fill } else { super::BG_CARD };
    let stroke = if active {
        text_color
    } else {
        super::BORDER_STRONG
    };

    ui.painter().rect_filled(rect, RADIUS_SM, fill);
    ui.painter().rect_stroke(
        rect,
        egui::CornerRadius::same(RADIUS_SM as u8),
        egui::Stroke::new(1.0, stroke),
        egui::StrokeKind::Inside,
    );
    ui.painter().galley(
        egui::pos2(
            rect.center().x - galley.size().x * 0.5,
            rect.center().y - galley.size().y * 0.5,
        ),
        galley,
        text_color,
    );
    response
}

/// Ghost (borderless) text/icon button — transparent at rest, hover fill only.
///
/// * `rest` — text color at rest
/// * `hot` — text color on hover/press
pub fn ghost_button(ui: &mut egui::Ui, label: &str, rest: Color32, hot: Color32) -> egui::Response {
    let galley = ui
        .painter()
        .layout_no_wrap(label.to_owned(), font_label(), rest);
    let pad = egui::vec2(6.0, 2.0);
    let size = egui::vec2((galley.size().x + pad.x * 2.0).max(22.0), 22.0);
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());

    let hovered = response.hovered();
    let pressed = response.is_pointer_button_down_on();
    let active = hovered || pressed;
    let fill = if active {
        BG_HOVER
    } else {
        Color32::TRANSPARENT
    };
    let text_color = if active { hot } else { rest };

    ui.painter().rect_filled(rect, RADIUS_SM, fill);
    let text_galley = ui
        .painter()
        .layout_no_wrap(label.to_owned(), font_label(), text_color);
    ui.painter().galley(
        egui::pos2(
            rect.center().x - text_galley.size().x * 0.5,
            rect.center().y - text_galley.size().y * 0.5,
        ),
        text_galley,
        text_color,
    );
    response
}

/// Card container — the single surface primitive: card fill, hairline border,
/// soft corners, uniform padding. Every boxed panel sits on this so borders
/// and margins stay identical everywhere.
// Pending phase-4 wiring (settings section cards migrate onto it).
#[allow(dead_code)]
pub fn card(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::NONE
        .fill(BG_CARD)
        .stroke(Stroke::new(1.0, BORDER))
        .corner_radius(CornerRadius::same(RADIUS_MD as u8))
        .inner_margin(egui::Margin::same(CARD_PAD as i8))
        .show(ui, add);
}

/// Flexbox-style justify-between row over one full-width line of `height`:
/// `left` lays out left-to-right, `right` is pinned to the trailing edge.
pub fn row_between(
    ui: &mut egui::Ui,
    height: f32,
    left: impl FnOnce(&mut egui::Ui),
    right: impl FnOnce(&mut egui::Ui),
) {
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), height),
        egui::Sense::hover(),
    );
    let mut l = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect)
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    left(&mut l);
    let mut r = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect)
            .layout(egui::Layout::right_to_left(egui::Align::Center)),
    );
    right(&mut r);
}

/// Toggle pill — filter chips. Selected gets the warm tint + bright text.
pub fn chip(ui: &mut egui::Ui, label: &str, selected: bool) -> egui::Response {
    let text_color = if selected { TEXT } else { TEXT_DIM };
    let galley = ui
        .painter()
        .layout_no_wrap(label.to_owned(), font_meta(), text_color);
    let size = egui::vec2(galley.size().x + sp::SM * 2.0, 20.0);
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());

    let fill = if response.is_pointer_button_down_on() || response.hovered() {
        BG_HOVER
    } else if selected {
        ACCENT_BG
    } else {
        Color32::TRANSPARENT
    };
    ui.painter().rect_filled(rect, RADIUS_SM, fill);
    ui.painter().galley(
        egui::pos2(
            rect.center().x - galley.size().x * 0.5,
            rect.center().y - galley.size().y * 0.5,
        ),
        galley,
        text_color,
    );
    response
}

// ---------------------------------------------------------------------------
// Design-system v2 additions
// ---------------------------------------------------------------------------

/// Badge tone — drives the tint/background pair of [`badge`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BadgeKind {
    Neutral,
    Ok,
    Warn,
    Error,
    Accent,
}

impl BadgeKind {
    fn colors(self) -> (Color32, Color32) {
        match self {
            Self::Neutral => (Color32::from_rgb(0x26, 0x26, 0x26), TEXT_DIM),
            Self::Ok => (
                Color32::from_rgb(0x17, 0x30, 0x17),
                Color32::from_rgb(0x8e, 0xe0, 0x8e),
            ),
            Self::Warn => (super::WARN_BG, Color32::from_rgb(0xf5, 0xc4, 0x6b)),
            Self::Error => (super::ERR_BG, Color32::from_rgb(0xf0, 0x93, 0x93)),
            Self::Accent => (ACCENT_BG, Color32::from_rgb(0xff, 0xa0, 0x4d)),
        }
    }
}

/// Small info/count pill (painted rect + text — no glyph dependency).
pub fn badge(ui: &mut egui::Ui, text: &str, kind: BadgeKind) -> egui::Response {
    let (bg, fg) = kind.colors();
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_owned(), font_meta(), fg);
    let h = 16.0;
    let size = egui::vec2(galley.size().x + 12.0, h);
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::hover());
    ui.painter().rect_filled(rect, h * 0.5, bg);
    ui.painter().galley(
        egui::pos2(
            rect.center().x - galley.size().x * 0.5,
            rect.center().y - galley.size().y * 0.5,
        ),
        galley,
        fg,
    );
    response
}

/// Connected segmented control (role picker). Returns the clicked index.
// Pending phase-4 wiring (settings federation role picker).
#[allow(dead_code)]
pub fn segmented(ui: &mut egui::Ui, options: &[&str], selected: usize) -> Option<usize> {
    const SEG_H: f32 = 22.0;
    let mut clicked = None;
    ui.horizontal(|ui| {
        for (i, opt) in options.iter().enumerate() {
            let sel = i == selected;
            let text_color = if sel { TEXT } else { TEXT_DIM };
            let galley = ui
                .painter()
                .layout_no_wrap((*opt).to_owned(), font_label(), text_color);
            let w = galley.size().x + 14.0;
            let (rect, response) =
                ui.allocate_exact_size(egui::vec2(w, SEG_H), egui::Sense::click());
            let n = options.len();
            // Per-corner rounding: outer edges of the joined control.
            let corner = CornerRadius {
                nw: if i == 0 { RADIUS_SM as u8 } else { 0 },
                sw: if i == 0 { RADIUS_SM as u8 } else { 0 },
                ne: if i + 1 == n { RADIUS_SM as u8 } else { 0 },
                se: if i + 1 == n { RADIUS_SM as u8 } else { 0 },
            };
            let fill = if response.hovered() && !sel {
                BG_HOVER
            } else if sel {
                ACCENT_BG
            } else {
                Color32::TRANSPARENT
            };
            ui.painter().rect_filled(rect, corner, fill);
            ui.painter().galley(
                egui::pos2(
                    rect.center().x - galley.size().x * 0.5,
                    rect.center().y - galley.size().y * 0.5,
                ),
                galley,
                text_color,
            );
            if response.clicked() && !sel {
                clicked = Some(i);
            }
        }
    });
    clicked
}

/// Full-width notice strip with a tone-colored left edge — wraps to multiple
/// lines so long hints (e.g. macOS Accessibility note) never clip.
pub fn banner(ui: &mut egui::Ui, tone: BannerTone, text: &str) -> egui::Response {
    let (bg, fg, edge) = match tone {
        BannerTone::Warn => (super::WARN_BG, Color32::from_rgb(0xf5, 0xc4, 0x6b), WARN),
        BannerTone::Error => (super::ERR_BG, Color32::from_rgb(0xf0, 0x93, 0x93), ERR),
    };
    let pad_x = 12.0;
    let pad_y = 8.0;
    // Width available for wrapped text (full - edge strip - horizontal padding).
    let avail_w = (ui.available_width() - pad_x * 2.0 - 8.0).max(80.0);
    let galley = ui
        .painter()
        .layout(text.to_owned(), font_label(), fg, avail_w);
    let h = (galley.size().y + pad_y * 2.0).max(32.0);
    let full = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(egui::vec2(full, h), egui::Sense::hover());
    ui.painter().rect_filled(rect, RADIUS_SM, bg);
    ui.painter().rect_filled(
        egui::Rect::from_min_size(rect.left_top(), egui::vec2(3.0, h)),
        2.0,
        edge,
    );
    let text_pos = egui::pos2(rect.left() + pad_x + 6.0, rect.top() + pad_y);
    ui.painter().galley(text_pos, galley, fg);
    response
}

/// Tone for [`banner`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum BannerTone {
    Warn,
    Error,
}

/// Guidance block for empty sections: quiet ring glyph, title, subtitle and
/// an optional single CTA. Returns true when the CTA was clicked.
#[must_use]
pub fn empty_state(ui: &mut egui::Ui, title: &str, subtitle: &str, cta: Option<&str>) -> bool {
    let mut clicked = false;
    ui.vertical_centered(|ui| {
        ui.add_space(sp::SM);
        // Painted ring instead of an icon glyph (font-coverage constraint).
        let (ring_rect, _) = ui.allocate_exact_size(egui::vec2(28.0, 28.0), egui::Sense::hover());
        ui.painter().circle_stroke(
            ring_rect.center(),
            10.0,
            Stroke::new(1.5, Color32::from_rgb(0x45, 0x45, 0x45)),
        );
        ui.add_space(sp::XS);
        ui.label(egui::RichText::new(title).font(font_label()).color(TEXT));
        ui.label(
            egui::RichText::new(subtitle)
                .font(font_meta())
                .color(TEXT_DIM),
        );
        if let Some(cta) = cta {
            ui.add_space(sp::XS);
            if ghost_button(ui, cta, TEXT_DIM, super::ACCENT).clicked() {
                clicked = true;
            }
        }
        ui.add_space(sp::XS);
    });
    clicked
}
