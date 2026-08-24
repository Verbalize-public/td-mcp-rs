//! Ableton-dark design tokens + shared widget kit (popup + dashboard).

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

/// Spacing scale — every margin/gap in the GUI comes from here.
pub mod sp {
    /// Tight intra-row gaps.
    pub const XS: f32 = 4.0;
    /// Default item spacing.
    pub const SM: f32 = 8.0;
    /// Side margins, row padding.
    pub const MD: f32 = 12.0;
    /// Block separation inside cards.
    pub const LG: f32 = 16.0;
    /// Section separation.
    pub const XL: f32 = 24.0;
}

/// Corner radius — chips and small controls.
pub const RADIUS_SM: f32 = 4.0;
/// Corner radius — cards.
pub const RADIUS_MD: f32 = 6.0;
/// Standard list-row height.
pub const ROW_H: f32 = 26.0;
/// Card inner padding.
pub const CARD_PAD: f32 = 12.0;

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

    // Hack ships in the default font_data but is only wired to Monospace;
    // appending it to Proportional gives arrows (← → ↑) a real glyph instead
    // of a tofu box, since Ubuntu/NotoEmoji/emoji-icon cover none of them.
    let mut fonts = egui::FontDefinitions::default();
    fonts
        .families
        .entry(egui::FontFamily::Proportional)
        .or_default()
        .push("Hack".to_owned());
    ctx.set_fonts(fonts);
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
        ACCENT
    };
    let text_color = Color32::from_rgb(0x13, 0x13, 0x13);

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

/// Card container — the single surface primitive: row fill, hairline border,
/// soft corners, uniform padding. Every boxed panel sits on this so borders
/// and margins stay identical everywhere.
pub fn card(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::NONE
        .fill(BG_ROW)
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

/// Selected-chip fill — a warm accent tint, kept static to avoid runtime
/// color math.
const CHIP_SELECTED_BG: Color32 = Color32::from_rgb(0x36, 0x2a, 0x1c);

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
        CHIP_SELECTED_BG
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
