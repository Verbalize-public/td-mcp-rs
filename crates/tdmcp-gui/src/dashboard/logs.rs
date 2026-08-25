//! Dashboard Logs tab — server-side level/src filters (shared state),
//! client-side text search, follow/pause controls, click-to-expand detail
//! with Ctrl+C copy.

use eframe::egui;

use super::DashTab;
use crate::app::DashboardApp;
use crate::theme::{
    chip, font_label, font_meta, font_mono, ghost_button, ACCENT, BG_HOVER, BG_ROW, BG_ROW_ALT,
    TEXT, TEXT_DIM, TEXT_FAINT, WARN,
};
use crate::wire::{level_color, level_letter, clip_line, LogRecordView};

/// Wide log stream.
pub(crate) fn logs(app: &mut DashboardApp, ui: &mut egui::Ui) {
    app.ensure_logs_dir();

    // Keyboard contract: F follow · Space pause · / search · Esc back to
    // Overview — suppressed while the search box owns focus so typing is
    // never hijacked.
    let search_id = egui::Id::new("dash_logs_search");
    let typing = ui.memory(|m| m.has_focus(search_id));
    let mut focus_search = false;
    if !typing {
        ui.input(|i| {
            if i.key_pressed(egui::Key::F) {
                app.logs_view.follow = !app.logs_view.follow;
            }
            if i.key_pressed(egui::Key::Space) {
                app.logs_view.paused = !app.logs_view.paused;
            }
            if i.key_pressed(egui::Key::Slash) {
                focus_search = true;
            }
            if i.key_pressed(egui::Key::Escape) {
                app.dash_tab = DashTab::Overview;
            }
        });
    }

    // Toolbar: level chips · source chips · active-filter count · right side
    // follow/pause/folder.
    ui.horizontal(|ui| {
        for (label, level) in [("ALL", None), ("ERR", Some("error")), ("WRN", Some("warn"))] {
            let active = app.logs_view.min_level == level;
            if chip(ui, label, active).clicked() && !active {
                app.logs_view.min_level = level;
                app.reset_logs_filter_state();
            }
        }
        for src in ["daemon", "bridge", "proxy"] {
            let active = app.logs_view.srcs.contains(src);
            let label = src.to_ascii_uppercase();
            if chip(ui, &label, active).clicked() {
                if active {
                    app.logs_view.srcs.remove(src);
                } else {
                    app.logs_view.srcs.insert(src);
                }
                app.reset_logs_filter_state();
            }
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ghost_button(ui, "Open folder", TEXT_DIM, ACCENT).clicked() {
                app.reveal_logs_dir();
            }
            let paused = app.logs_view.paused;
            let pause_label = if paused { "▶ Resume" } else { "⏸ Pause" };
            let pause_color = if paused { WARN } else { TEXT_DIM };
            if ghost_button(ui, pause_label, pause_color, WARN).clicked() {
                app.logs_view.paused = !paused;
            }
            let follow_on = app.logs_view.follow;
            // U+25CF is covered by no bundled font — color carries the state.
            let follow_color = if follow_on { ACCENT } else { TEXT_DIM };
            if ghost_button(ui, "FOLLOW", follow_color, ACCENT).clicked() {
                app.logs_view.follow = !follow_on;
            }
        });
    });

    // Search row: input · clear · active-filter count.
    ui.add_space(4.0);
    let mut search_resp: Option<egui::Response> = None;
    ui.horizontal(|ui| {
        let resp = ui.add(
            egui::TextEdit::singleline(&mut app.logs_view.text_filter)
                .id(search_id)
                .desired_width(ui.available_width() - 34.0)
                .hint_text("Filter message or target…"),
        );
        if !app.logs_view.text_filter.is_empty() && ghost_button(ui, "×", TEXT_DIM, TEXT).clicked()
        {
            app.logs_view.text_filter.clear();
        }
        search_resp = Some(resp);
        let active_filters = usize::from(app.logs_view.min_level.is_some())
            + app.logs_view.srcs.len()
            + usize::from(!app.logs_view.text_filter.is_empty());
        if active_filters > 0 {
            let _ = crate::theme::badge(
                ui,
                &format!("{active_filters}"),
                crate::theme::BadgeKind::Accent,
            )
            .on_hover_text("active filters");
        }
    });
    if focus_search {
        if let Some(r) = &search_resp {
            r.request_focus();
        }
    }
    ui.label(
        egui::RichText::new("F follow · Space pause · / search · click a row to expand · Esc overview")
            .font(font_meta())
            .color(crate::theme::TEXT_FAINT),
    );
    ui.add_space(6.0);

    if let Some(err) = &app.logs_view.fetch_error {
        ui.label(
            egui::RichText::new(format!("daemon unreachable — retrying ({err})"))
                .font(font_meta())
                .color(TEXT_FAINT),
        );
    }

    // List.
    let follow = app.logs_view.follow;
    let expanded_seq = app.logs_view.expanded;
    let needle = app.logs_view.text_filter.trim().to_lowercase();
    let empty_before_filter = app.logs_view.buf.is_empty();
    let fetched_once = app.logs_view.last_fetch.is_some();
    let shown_total = std::cell::Cell::new(0usize);
    let mut clicked_seq: Option<u64> = None;
    egui::ScrollArea::vertical()
        .id_salt("dash_logs_scroll")
        .auto_shrink(false)
        .stick_to_bottom(follow)
        .show(ui, |ui| {
            if empty_before_filter {
                ui.vertical_centered(|ui| {
                    ui.add_space(32.0);
                    if fetched_once {
                        ui.label(
                            egui::RichText::new("( no logs )")
                                .font(font_meta())
                                .color(TEXT_FAINT),
                        );
                    } else {
                        ui.spinner();
                        ui.add_space(8.0);
                        ui.label(
                            egui::RichText::new("connecting…")
                                .font(font_label())
                                .color(TEXT_FAINT),
                        );
                    }
                });
                return;
            }
            let mut shown = 0usize;
            for r in app.logs_view.buf.iter() {
                if !matches_filter(r, &needle) {
                    continue;
                }
                shown += 1;
                let full = ui.available_width().max(120.0);
                let (rect, response) =
                    ui.allocate_exact_size(egui::vec2(full, 18.0), egui::Sense::click());
                let bg = if Some(r.seq) == expanded_seq {
                    BG_HOVER
                } else if shown.is_multiple_of(2) {
                    BG_ROW
                } else {
                    BG_ROW_ALT
                };
                ui.painter().rect_filled(rect, 0.0, bg);
                ui.painter().circle_filled(
                    egui::pos2(rect.left() + 9.0, rect.center().y),
                    2.5,
                    level_color(&r.level),
                );
                let time = r.ts.get(11..19).unwrap_or(&r.ts);
                let line = format!(
                    "{time} {} {} {}",
                    level_letter(&r.level),
                    r.target,
                    r.msg.replace('\n', " ")
                );
                // ~6.6 px per mono glyph at the 11px mono size.
                let max_chars = (((rect.width() - 26.0) / 6.6) as usize).max(20);
                ui.painter().text(
                    egui::pos2(rect.left() + 18.0, rect.center().y),
                    egui::Align2::LEFT_CENTER,
                    clip_line(&line, max_chars),
                    font_mono(),
                    TEXT,
                );
                if response.clicked() {
                    clicked_seq = Some(r.seq);
                }
            }
            shown_total.set(shown);
            if shown == 0 {
                ui.vertical_centered(|ui| {
                    ui.add_space(16.0);
                    ui.label(
                        egui::RichText::new("( no rows match the filter )")
                            .font(font_meta())
                            .color(TEXT_FAINT),
                    );
                });
            }
        });
    if let Some(seq) = clicked_seq {
        app.logs_view.expanded = if app.logs_view.expanded == Some(seq) {
            None
        } else {
            Some(seq)
        };
    }
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        ui.label(
            egui::RichText::new(format!("{} rows", shown_total.get()))
                .font(font_meta())
                .color(crate::theme::TEXT_FAINT),
        );
    });

    // Detail drawer for the expanded record.
    if let Some(seq) = app.logs_view.expanded {
        if let Some(r) = app.logs_view.buf.iter().find(|r| r.seq == seq) {
            ui.add_space(6.0);
            egui::Frame::NONE
                .fill(crate::theme::BG_PANEL)
                .stroke(egui::Stroke::new(1.0, crate::theme::BORDER))
                .corner_radius(egui::CornerRadius::same(6))
                .inner_margin(egui::Margin::same(12))
                .show(ui, |ui| {
                    let kvs = if r.kvs.is_empty() {
                        "{}".to_owned()
                    } else {
                        r.kvs
                            .iter()
                            .map(|(k, v)| format!("{k}={v}"))
                            .collect::<Vec<_>>()
                            .join(" ")
                    };
                    let detail = format!(
                        "target {}\ncode {} · kvs {kvs}",
                        r.target,
                        r.code.as_deref().unwrap_or("null")
                    );
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(detail.clone())
                                .font(font_mono())
                                .color(TEXT_DIM),
                        )
                        .wrap(),
                    );
                    ui.label(
                        egui::RichText::new("Ctrl+C copies the full record")
                            .font(font_meta())
                            .color(TEXT_FAINT),
                    );
                    if ui.input(|i| i.key_pressed(egui::Key::C) && i.modifiers.command) {
                        ui.ctx().copy_text(format!(
                            "{} {} {} pid={} {} {detail}",
                            r.ts, r.level, r.src, r.pid, r.msg
                        ));
                    }
                });
        }
    }
}

/// Client-side substring filter over msg/target; empty matches everything.
fn matches_filter(r: &LogRecordView, needle: &str) -> bool {
    needle.is_empty()
        || r.msg.to_lowercase().contains(needle)
        || r.target.to_lowercase().contains(needle)
}
