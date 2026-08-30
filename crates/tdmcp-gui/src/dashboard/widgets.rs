//! Dashboard shared painted components: stat tiles, fleet/MCP rows,
//! section captions, headered cards, federation modal shell.

use std::time::{SystemTime, UNIX_EPOCH};

use eframe::egui::{self, Color32};

use crate::app::DashboardApp;
use crate::theme::sp;
use crate::theme::{
    action_button, font_label, font_meta, font_mono, font_stat, row_between, status_led,
    ActionTone, ACCENT, BG_CARD, BG_HOVER, BORDER, CARD_PAD, OK, RADIUS_MD, RADIUS_SM, ROW_H,
    SIDE_MARGIN, TEXT, TEXT_DIM, TEXT_FAINT, WARN,
};
use crate::wire::{id_tail, FleetProc, SessionRow};

/// Daemon lifecycle actions — the one definition, rendered by both the
/// dashboard top bar and the tray popup footer.
///
/// Laid out for an RTL parent (`row_between`'s right slot / a right-to-left
/// child), so the first button added lands rightmost. Semantics match the
/// pre-pass-10 Overview card exactly:
///
/// * `Reveal .tox` is always offered, including while the daemon is unreachable.
/// * Stop / Restart are **omitted** when offline rather than shown dead.
/// * Stop is two-step: it swaps itself for a Confirm / Cancel pair.
pub(crate) fn daemon_actions(app: &mut DashboardApp, ui: &mut egui::Ui) {
    // Shared Open/Create are always visible when daemon is reachable; Reveal stays even offline.
    if app.status.is_none() {
        if action_button(ui, "Reveal .tox", ActionTone::Neutral).clicked() {
            app.reveal_tox();
        }
        // Still allow browsing when offline (spawn will fail with snack, but not hidden).
        ui.add_space(sp::XS);
        ui.add_enabled_ui(!app.spawn_busy, |ui| {
            if action_button(ui, "New", ActionTone::Neutral)
                .on_hover_text("Create a new project from template")
                .clicked()
            {
                app.create_project_dialog();
            }
        });
        ui.add_space(sp::XS);
        draw_open_split(app, ui);
        return;
    }

    if app.confirm_stop {
        if action_button(ui, "Cancel", ActionTone::Neutral).clicked() {
            app.confirm_stop = false;
        }
        ui.add_space(sp::XS);
        if action_button(ui, "Confirm stop", ActionTone::Danger).clicked() {
            app.shutdown_daemon();
        }
        return;
    }

    if action_button(ui, "Stop", ActionTone::Danger)
        .on_hover_text("Shut the daemon down (real exit)")
        .clicked()
    {
        app.confirm_stop = true;
    }
    ui.add_space(sp::XS);
    if action_button(ui, "Restart", ActionTone::Accent)
        .on_hover_text("Restart the daemon process")
        .clicked()
    {
        app.restart_daemon();
    }
    ui.add_space(sp::XS);
    if action_button(ui, "Reveal .tox", ActionTone::Neutral)
        .on_hover_text("Show the bootstrap .tox in the file manager")
        .clicked()
    {
        app.reveal_tox();
    }
    ui.add_space(sp::XS);
    ui.add_enabled_ui(!app.spawn_busy, |ui| {
        if action_button(ui, "New", ActionTone::Neutral)
            .on_hover_text("Create a new project from template")
            .clicked()
        {
            app.create_project_dialog();
        }
    });
    ui.add_space(sp::XS);
    draw_open_split(app, ui);
    if app.spawn_busy {
        ui.add_space(sp::XS);
        ui.spinner();
    }
    // Render dropdown if open (foreground area anchored to last arrow rect).
    draw_recent_menu(app, ui.ctx());
}

/// Split button: main "Open" opens file picker, arrow "▾" toggles recent menu.
fn draw_open_split(app: &mut DashboardApp, ui: &mut egui::Ui) {
    let enabled = !app.spawn_busy;
    let id = ui.id().with("open_split");
    let total_w = 68.0_f32;
    let arrow_w = 22.0_f32;
    let h = 22.0_f32;
    let (rect, _) = ui.allocate_exact_size(egui::vec2(total_w, h), egui::Sense::hover());

    let main_rect = egui::Rect::from_min_size(rect.min, egui::vec2(rect.width() - arrow_w, h));
    let arrow_rect = egui::Rect::from_min_size(
        egui::pos2(main_rect.right(), rect.top()),
        egui::vec2(arrow_w, h),
    );

    let main_resp = ui.interact(main_rect, id.with("main"), egui::Sense::click());
    let arrow_resp = ui.interact(arrow_rect, id.with("arrow"), egui::Sense::click());

    let main_hover = main_resp.hovered() || main_resp.is_pointer_button_down_on();
    let arrow_hover = arrow_resp.hovered() || arrow_resp.is_pointer_button_down_on();
    let any_hover = main_hover || arrow_hover || app.show_recent_menu;

    let stroke = if any_hover { TEXT } else { crate::theme::BORDER_STRONG };
    let fill_main = if main_hover { BG_HOVER } else { BG_CARD };
    let fill_arrow = if arrow_hover || app.show_recent_menu {
        BG_HOVER
    } else {
        BG_CARD
    };
    let text_color = if enabled { TEXT } else { TEXT_DIM };

    // Paint split background (two rects sharing edge, rounded outer corners only).
    let main_corner = egui::CornerRadius {
        nw: RADIUS_SM as u8,
        sw: RADIUS_SM as u8,
        ne: 0,
        se: 0,
    };
    let arrow_corner = egui::CornerRadius {
        nw: 0,
        sw: 0,
        ne: RADIUS_SM as u8,
        se: RADIUS_SM as u8,
    };
    ui.painter().rect_filled(main_rect, main_corner, fill_main);
    ui.painter().rect_stroke(
        main_rect,
        main_corner,
        egui::Stroke::new(1.0, stroke),
        egui::StrokeKind::Inside,
    );
    ui.painter().rect_filled(arrow_rect, arrow_corner, fill_arrow);
    ui.painter().rect_stroke(
        arrow_rect,
        arrow_corner,
        egui::Stroke::new(1.0, stroke),
        egui::StrokeKind::Inside,
    );
    // Divider line between halves.
    ui.painter().vline(
        main_rect.right(),
        main_rect.y_range(),
        egui::Stroke::new(1.0, crate::theme::BORDER),
    );
    // Centered labels.
    let main_galley = ui.painter().layout_no_wrap("Open".to_owned(), font_label(), text_color);
    ui.painter().galley(
        egui::pos2(
            main_rect.center().x - main_galley.size().x * 0.5,
            main_rect.center().y - main_galley.size().y * 0.5,
        ),
        main_galley,
        text_color,
    );
    let arrow_galley = ui
        .painter()
        .layout_no_wrap("▾".to_owned(), font_label(), text_color);
    ui.painter().galley(
        egui::pos2(
            arrow_rect.center().x - arrow_galley.size().x * 0.5,
            arrow_rect.center().y - arrow_galley.size().y * 0.5 + 1.0,
        ),
        arrow_galley,
        text_color,
    );

    if enabled && main_resp.clicked() {
        app.show_recent_menu = false;
        app.recent_menu_anchor = None;
        app.open_project_dialog();
    }
    if enabled && arrow_resp.clicked() {
        app.show_recent_menu = !app.show_recent_menu;
        if app.show_recent_menu {
            // Anchor right-aligned under the split so the 260px menu stays inside the
            // window even when the split sits at the far right (RTL top bar / popup).
            app.recent_menu_anchor = Some(egui::pos2(rect.right() - 260.0, rect.bottom() + 4.0));
        } else {
            app.recent_menu_anchor = None;
        }
    }
    // Close on Esc or click outside (handled in draw_recent_menu as well).
    if app.show_recent_menu && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        app.show_recent_menu = false;
        app.recent_menu_anchor = None;
    }
    main_resp.on_hover_text("Open an existing .toe/.tox");
    arrow_resp.on_hover_text("Recent projects");
}

fn draw_recent_menu(app: &mut DashboardApp, ctx: &egui::Context) {
    let Some(anchor) = app.recent_menu_anchor else {
        return;
    };
    if !app.show_recent_menu {
        return;
    }
    let pointer = ctx.input(|i| i.pointer.interact_pos());
    let recents = app.recent_projects.clone();
    let mut chosen: Option<std::path::PathBuf> = None;
    let mut close = false;
    egui::Area::new(egui::Id::new("recent_projects_menu"))
        .order(egui::Order::Foreground)
        .fixed_pos(anchor)
        .show(ctx, |ui| {
            let frame = egui::Frame::NONE
                .fill(BG_CARD)
                .stroke(egui::Stroke::new(1.0, crate::theme::BORDER))
                .corner_radius(egui::CornerRadius::same(RADIUS_SM as u8))
                .inner_margin(egui::Margin::same(6));
            frame.show(ui, |ui| {
                ui.set_min_width(260.0);
                ui.set_max_width(360.0);
                if recents.is_empty() {
                    ui.label(
                        egui::RichText::new("No recent projects — open a .toe/.tox to populate this list")
                            .font(font_meta())
                            .color(TEXT_DIM),
                    );
                    ui.add_space(sp::XS);
                    if ui
                        .add(egui::Button::new(
                            egui::RichText::new("Browse…").font(font_label()).color(ACCENT),
                        ))
                        .clicked()
                    {
                        close = true;
                        chosen = None;
                        // Signal browse: spawn dialog after close.
                        // We use a sentinel empty path to mean "browse".
                    }
                } else {
                    ui.label(
                        egui::RichText::new("RECENT PROJECTS")
                            .font(font_meta())
                            .color(TEXT_FAINT),
                    );
                    ui.add_space(sp::XS);
                    egui::ScrollArea::vertical()
                        .max_height(220.0)
                        .show(ui, |ui| {
                            for p in recents.iter().take(16) {
                                let name = p
                                    .file_name()
                                    .map(|n| n.to_string_lossy().to_string())
                                    .unwrap_or_else(|| p.display().to_string());
                                let dir = p
                                    .parent()
                                    .map(|d| d.display().to_string())
                                    .unwrap_or_default();
                                let resp = ui.add(
                                    egui::Button::new(
                                        egui::RichText::new(format!("{name}  ·  {dir}"))
                                            .font(font_meta())
                                            .color(TEXT),
                                    )
                                    .fill(egui::Color32::TRANSPARENT),
                                );
                                if resp.clicked() {
                                    chosen = Some(p.clone());
                                }
                                resp.on_hover_text(p.display().to_string());
                            }
                        });
                    ui.add_space(sp::XS);
                    if ui
                        .add(egui::Button::new(
                            egui::RichText::new("Browse for more…")
                                .font(font_label())
                                .color(TEXT_DIM),
                        ))
                        .clicked()
                    {
                        close = true;
                    }
                }
            });
        });
    // Handle outside-click dismiss via global pointer check + hover.
    if ctx.input(|i| i.pointer.any_pressed()) {
        if let Some(pos) = pointer {
            // Approximate menu rect: anchor + width 360 + height up to 300.
            let menu_rect = egui::Rect::from_min_size(anchor, egui::vec2(360.0, 300.0));
            if !menu_rect.contains(pos) {
                // Check if the click was on the split button itself — don't treat as outside.
                // That case is already toggled above. Here we just close.
                app.show_recent_menu = false;
                app.recent_menu_anchor = None;
                return;
            }
        }
    }
    if let Some(path) = chosen {
        app.show_recent_menu = false;
        app.recent_menu_anchor = None;
        app.spawn_project(path, false);
    } else if close {
        app.show_recent_menu = false;
        app.recent_menu_anchor = None;
        app.open_project_dialog();
    }
    // Esc already handled in draw_open_split; also handle here.
    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        app.show_recent_menu = false;
        app.recent_menu_anchor = None;
    }
}

/// Overview roster cap — long fleets/client lists are reference detail, not the
/// point of the page (pass 10). Same shape as the tray popup's own caps.
pub(crate) const ROSTER_CAP: usize = 4;

/// Render at most [`ROSTER_CAP`] rows, then a dim `+N more` line.
///
/// The overflow line is deliberately a plain label, not a button: the full list
/// is already one scroll away in the same card once the fleet grows, and an
/// extra click target here would compete with the actions this pass promotes.
pub(crate) fn capped_rows<T>(
    ui: &mut egui::Ui,
    items: &[T],
    mut row: impl FnMut(&mut egui::Ui, &T),
) {
    for item in items.iter().take(ROSTER_CAP) {
        row(ui, item);
    }
    let hidden = items.len().saturating_sub(ROSTER_CAP);
    if hidden > 0 {
        let (rect, _) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), ROW_H),
            egui::Sense::hover(),
        );
        ui.painter().text(
            egui::pos2(rect.left() + SIDE_MARGIN, rect.center().y),
            egui::Align2::LEFT_CENTER,
            format!("+{hidden} more"),
            font_meta(),
            TEXT_FAINT,
        );
    }
}

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
    let attention_row = p.resurrected || !p.cancelled_tasks.is_empty() || bridge == "disconnected";
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
        if p.resurrected { " · resurrected" } else { "" }
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
