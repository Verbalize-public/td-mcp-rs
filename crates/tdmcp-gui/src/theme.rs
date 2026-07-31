//! Ableton-dark design tokens + egui Visuals for the tray popup.

use eframe::egui::{
    self, Color32, CornerRadius, FontFamily, FontId, Shadow, Stroke, Style, Visuals,
};

/// Popup background — everything sits on this.
pub const BG_WINDOW: Color32 = Color32::from_rgb(0x13, 0x13, 0x13);
/// Section header strip fill only.
pub const BG_PANEL: Color32 = Color32::from_rgb(0x1c, 0x1c, 0x1c);
/// List row base (zebra).
pub const BG_ROW: Color32 = Color32::from_rgb(0x1a, 0x1a, 0x1a);
/// List row alternate stripe.
pub const BG_ROW_ALT: Color32 = Color32::from_rgb(0x1f, 0x1f, 0x1f);
/// Hover fill (no shadow).
pub const BG_HOVER: Color32 = Color32::from_rgb(0x26, 0x26, 0x26);
/// Pressed / selected fill.
pub const BG_ACTIVE: Color32 = Color32::from_rgb(0x2e, 0x2e, 0x2e);
/// Primary text.
pub const TEXT: Color32 = Color32::from_rgb(0xe6, 0xe6, 0xe6);
/// Secondary text (section titles, empty state).
pub const TEXT_DIM: Color32 = Color32::from_rgb(0x7a, 0x7a, 0x7a);
/// Tertiary / metadata (id tails, mono meta).
pub const TEXT_FAINT: Color32 = Color32::from_rgb(0x55, 0x55, 0x55);
/// Ableton orange — signal only (≤5% of frame).
pub const ACCENT: Color32 = Color32::from_rgb(0xff, 0x7a, 0x1a);
/// Status LED — healthy.
pub const OK: Color32 = Color32::from_rgb(0x5f, 0xd3, 0x5f);
/// Status LED — attention (matches amber tray badge).
pub const WARN: Color32 = Color32::from_rgb(0xf0, 0xa8, 0x30);
/// Status LED — bad / Stop outline.
pub const ERR: Color32 = Color32::from_rgb(0xe8, 0x5d, 0x5d);
/// Hairline divider.
pub const BORDER: Color32 = Color32::from_rgb(0x2a, 0x2a, 0x2a);
/// Focused control border.
pub const BORDER_STRONG: Color32 = Color32::from_rgb(0x3a, 0x3a, 0x3a);

/// Fixed popup width (px).
pub const WINDOW_WIDTH: f32 = 380.0;
/// Hard max popup height (px); sections scroll beyond this.
pub const WINDOW_MAX_HEIGHT: f32 = 600.0;
/// Status LED diameter (px).
pub const LED_SIZE: f32 = 6.0;

/// Wordmark / title.
#[must_use]
pub fn font_title() -> FontId {
    FontId::new(13.0, FontFamily::Proportional)
}

/// Row labels, values, buttons.
#[must_use]
pub fn font_label() -> FontId {
    FontId::new(12.0, FontFamily::Proportional)
}

/// Section headers, empty state, meta labels.
#[must_use]
pub fn font_meta() -> FontId {
    FontId::new(11.0, FontFamily::Proportional)
}

/// Pid, session id, durations (tabular via monospace).
#[must_use]
pub fn font_mono() -> FontId {
    FontId::new(11.0, FontFamily::Monospace)
}

/// Apply dark-only Ableton visuals to the egui context (call once at startup).
pub fn apply(ctx: &egui::Context) {
    let mut style = Style {
        visuals: dark_visuals(),
        ..Style::default()
    };
    style.spacing.item_spacing = egui::vec2(8.0, 4.0);
    style.spacing.window_margin = egui::Margin::same(12);
    style.spacing.button_padding = egui::vec2(8.0, 4.0);
    style.interaction.selectable_labels = false;
    ctx.set_theme(egui::Theme::Dark);
    ctx.set_style_of(egui::Theme::Dark, style);
}

fn dark_visuals() -> Visuals {
    let mut v = Visuals::dark();
    v.dark_mode = true;
    v.window_fill = BG_WINDOW;
    v.panel_fill = BG_WINDOW;
    v.extreme_bg_color = BG_PANEL;
    v.faint_bg_color = BG_ROW;
    v.override_text_color = Some(TEXT);
    v.window_stroke = Stroke::new(1.0, BORDER);
    v.window_corner_radius = CornerRadius::ZERO;
    v.menu_corner_radius = CornerRadius::ZERO;
    v.window_shadow = Shadow::NONE;
    v.popup_shadow = Shadow::NONE;

    zero_rounding(&mut v.widgets.noninteractive);
    zero_rounding(&mut v.widgets.inactive);
    zero_rounding(&mut v.widgets.hovered);
    zero_rounding(&mut v.widgets.active);
    zero_rounding(&mut v.widgets.open);

    v.widgets.noninteractive.bg_fill = BG_WINDOW;
    v.widgets.noninteractive.weak_bg_fill = BG_PANEL;
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, BORDER);
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, TEXT_DIM);

    v.widgets.inactive.bg_fill = BG_PANEL;
    v.widgets.inactive.weak_bg_fill = BG_PANEL;
    v.widgets.inactive.bg_stroke = Stroke::new(1.0, BORDER);
    v.widgets.inactive.fg_stroke = Stroke::new(1.0, TEXT);

    v.widgets.hovered.bg_fill = BG_HOVER;
    v.widgets.hovered.weak_bg_fill = BG_HOVER;
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, BORDER_STRONG);
    v.widgets.hovered.fg_stroke = Stroke::new(1.0, TEXT);

    v.widgets.active.bg_fill = BG_ACTIVE;
    v.widgets.active.weak_bg_fill = BG_ACTIVE;
    v.widgets.active.bg_stroke = Stroke::new(1.0, BORDER_STRONG);
    v.widgets.active.fg_stroke = Stroke::new(1.0, TEXT);

    v.widgets.open.bg_fill = BG_ACTIVE;
    v.widgets.open.bg_stroke = Stroke::new(1.0, BORDER_STRONG);

    v.selection.bg_fill = BG_ACTIVE;
    v.selection.stroke = Stroke::new(1.0, ACCENT);

    v.hyperlink_color = ACCENT;
    v.warn_fg_color = WARN;
    v.error_fg_color = ERR;
    v
}

fn zero_rounding(w: &mut egui::style::WidgetVisuals) {
    w.corner_radius = CornerRadius::ZERO;
}

/// Paint a 6px status LED (Ableton-style colored dot).
pub fn status_led(ui: &mut egui::Ui, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(LED_SIZE + 2.0, LED_SIZE + 2.0),
        egui::Sense::hover(),
    );
    let center = rect.center();
    ui.painter().circle_filled(center, LED_SIZE * 0.5, color);
}

/// Section header strip: `bg_panel`, meta title, hairline below.
pub fn section_header(ui: &mut egui::Ui, title: &str) {
    let full = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(egui::vec2(full, 20.0), egui::Sense::hover());
    ui.painter().rect_filled(rect, 0.0, BG_PANEL);
    ui.painter().text(
        egui::pos2(rect.left() + 12.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        title,
        font_meta(),
        TEXT_DIM,
    );
    ui.painter()
        .hline(rect.x_range(), rect.bottom(), Stroke::new(1.0, BORDER));
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

    ui.painter().rect_filled(rect, 0.0, fill);
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
