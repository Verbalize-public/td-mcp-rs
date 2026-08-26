//! Ableton-dark design tokens + shared widget kit (popup + dashboard).
//!
//! Painted widgets live in [`widgets`] and are re-exported here so call
//! sites keep using `theme::ghost_button`, `theme::card`, …

mod widgets;

// `banner`/`segmented`/`card` are wired up by the settings/fleet polish pass.
#[allow(unused_imports)]
pub use widgets::{
    action_button, badge, banner, card, chip, empty_state, filled_button, ghost_button,
    row_between, segmented, status_led, status_led_pulse, ActionTone, BadgeKind, BannerTone,
};

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
pub const TEXT_DIM: Color32 = Color32::from_rgb(0x8a, 0x8a, 0x8a);
/// Tertiary / metadata (id tails, mono meta).
pub const TEXT_FAINT: Color32 = Color32::from_rgb(0x60, 0x60, 0x60);
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

/// Card surface — one step above the window for quiet elevation.
pub const BG_CARD: Color32 = Color32::from_rgb(0x19, 0x19, 0x19);
/// Selected-chip fill — warm accent tint, static to avoid runtime color math.
pub const ACCENT_BG: Color32 = Color32::from_rgb(0x33, 0x26, 0x1a);
/// Faint red tint — error banners/badges.
pub const ERR_BG: Color32 = Color32::from_rgb(0x30, 0x1d, 0x1d);
/// Faint amber tint — warning banners/badges.
pub const WARN_BG: Color32 = Color32::from_rgb(0x30, 0x27, 0x18);

/// Fixed popup width (px).
pub const WINDOW_WIDTH: f32 = 380.0;
/// Hard max popup height (px); sections scroll beyond this.
pub const WINDOW_MAX_HEIGHT: f32 = 600.0;
/// Status LED diameter (px).
pub const LED_SIZE: f32 = 6.0;
/// Symmetric side inset for rows/lists shared by both surfaces (px).
pub const SIDE_MARGIN: f32 = 12.0;

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
pub const RADIUS_MD: f32 = 8.0;
/// Standard list-row height.
pub const ROW_H: f32 = 24.0;
/// Card inner padding.
pub const CARD_PAD: f32 = 10.0;

/// Wordmark / page title.
#[must_use]
pub fn font_display() -> FontId {
    FontId::new(15.0, FontFamily::Proportional)
}

/// Stat-tile numerals.
#[must_use]
pub fn font_stat() -> FontId {
    FontId::new(22.0, FontFamily::Proportional)
}

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
