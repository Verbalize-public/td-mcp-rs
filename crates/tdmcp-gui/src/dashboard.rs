//! Dashboard secondary viewport — Docker-Desktop-style rich window.
//!
//! Iteration 1 shell: sidebar navigation + Overview cards + latest errors +
//! embedded Fleet sections. Logs/Settings migrate here in later iterations.

use eframe::egui::{self, Color32, Sense};

use crate::theme::{
    self, chip, filled_button, font_label, font_meta, font_mono, font_title, ghost_button,
    row_between, status_led, ACCENT, BG_ACTIVE, BG_HOVER, BG_PANEL, BG_ROW, BG_ROW_ALT, BORDER,
    CARD_PAD, ERR, OK, RADIUS_MD, ROW_H, TEXT, TEXT_DIM, TEXT_FAINT, WARN,
};
use crate::{
    clip_line, level_color, level_letter, DashboardApp, FleetPanel, LogRecordView, ScanPurpose,
};

/// Sidebar width (px).
const SIDEBAR_W: f32 = 196.0;
/// Top bar height (px).
const TOPBAR_H: f32 = 44.0;
/// Modal dialog width (px).
const MODAL_W: f32 = 480.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DashTab {
    #[default]
    Overview,
    Fleet,
    Logs,
    Settings,
}

impl DashTab {
    fn label(self) -> &'static str {
        match self {
            DashTab::Overview => "Overview",
            DashTab::Fleet => "Fleet",
            DashTab::Logs => "Logs",
            DashTab::Settings => "Settings",
        }
    }

    const ALL: [DashTab; 4] = [
        DashTab::Overview,
        DashTab::Fleet,
        DashTab::Logs,
        DashTab::Settings,
    ];
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
        .with_inner_size([980.0, 660.0])
        .with_min_inner_size([860.0, 540.0])
        .with_icon(egui::IconData {
            width: window_icon.width,
            height: window_icon.height,
            rgba: window_icon.rgba.clone(),
        })
}

/// Render the dashboard into its viewport's root ui. Called every frame while open.
pub fn render(app: &mut DashboardApp, ui: &mut egui::Ui) {
    if ui.input(|i| i.viewport().close_requested()) {
        // Honor the OS close; the viewport simply stops being shown.
        app.dashboard_open = false;
    }

    egui::Panel::left("dash_sidebar")
        .exact_size(SIDEBAR_W)
        .frame(
            egui::Frame::NONE
                .fill(BG_PANEL)
                .stroke(egui::Stroke::new(1.0, BORDER))
                .inner_margin(egui::Margin {
                    left: 0,
                    right: 0,
                    top: 14,
                    bottom: 12,
                }),
        )
        .show(ui, |ui| {
            sidebar(app, ui);
        });

    egui::Panel::top("dash_topbar")
        .exact_size(TOPBAR_H)
        .frame(
            egui::Frame::NONE
                .fill(theme::BG_WINDOW)
                .stroke(egui::Stroke::new(1.0, BORDER))
                .inner_margin(egui::Margin::symmetric(20, 0)),
        )
        .show(ui, |ui| {
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                ui.label(
                    egui::RichText::new(app.dash_tab.label())
                        .font(font_title())
                        .color(TEXT),
                );
            });
        });

    egui::CentralPanel::default()
        .frame(
            egui::Frame::NONE
                .fill(theme::BG_WINDOW)
                .inner_margin(egui::Margin::same(20)),
        )
        .show(ui, |ui| match app.dash_tab {
            DashTab::Overview => overview(app, ui),
            DashTab::Fleet => fleet(app, ui),
            DashTab::Logs => logs(app, ui),
            DashTab::Settings => settings(app, ui),
        });
}

fn sidebar(app: &mut DashboardApp, ui: &mut egui::Ui) {
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
    ui.add_space(2.0);
    ui.horizontal(|ui| {
        ui.add_space(36.0);
        let role = app
            .status
            .as_ref()
            .map(|s| s.role.to_ascii_lowercase())
            .unwrap_or_else(|| "offline".to_owned());
        ui.label(
            egui::RichText::new(role)
                .font(font_meta())
                .color(TEXT_FAINT),
        );
    });
    ui.add_space(18.0);

    for tab in DashTab::ALL {
        let selected = app.dash_tab == tab;
        if nav_item(ui, tab.label(), selected).clicked() {
            app.dash_tab = tab;
        }
    }

    ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
        ui.add_space(8.0);
        ui.separator();
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.add_space(14.0);
            status_led(ui, health_color(app));
            ui.add_space(6.0);
            let meta = match app.status.as_ref() {
                Some(s) => format!("pid {} · {}", s.pid, s.bind_address),
                None => "daemon unreachable".to_owned(),
            };
            let snap = &app.prev_snapshot;
            ui.label(egui::RichText::new(meta).font(font_mono()).color(TEXT_DIM))
                .on_hover_text(format!(
                    "attention: {} disconnected · {} resurrected · {} cancelled",
                    snap.disconnected, snap.resurrected, snap.cancelled
                ));
        });
    });
}

fn nav_item(ui: &mut egui::Ui, label: &str, selected: bool) -> egui::Response {
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
    ui.painter().rect_filled(rect, 0.0, fill);
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
    response
}

/// Linear blend of two colors; `t` = amount of `b`.
fn blend(a: Color32, b: Color32, t: f32) -> Color32 {
    let lerp = |x: u8, y: u8| -> u8 { (x as f32 + (y as f32 - x as f32) * t).round() as u8 };
    Color32::from_rgb(lerp(a.r(), b.r()), lerp(a.g(), b.g()), lerp(a.b(), b.b()))
}

fn health_color(app: &DashboardApp) -> Color32 {
    if app.error.is_some() || app.status.is_none() {
        ERR
    } else if app.attention {
        WARN
    } else {
        OK
    }
}

fn overview(app: &mut DashboardApp, ui: &mut egui::Ui) {
    let mcp_n = app
        .status
        .as_ref()
        .map(|s| s.mcp_session_count)
        .unwrap_or(0);
    // Owned copies: daemon_card below takes `app` mutably.
    let connected = app.prev_snapshot.connected;
    let attention = {
        let snap = &app.prev_snapshot;
        snap.disconnected + snap.resurrected + snap.cancelled
    };
    let role = app
        .status
        .as_ref()
        .map(|s| s.role.to_ascii_lowercase())
        .unwrap_or_else(|| "offline".to_owned());

    ui.add_space(theme::sp::SM);

    daemon_card(app, ui);

    ui.add_space(theme::sp::MD);
    ui.columns(4, |cols| {
        stat_card(&mut cols[0], &mcp_n.to_string(), "MCP CLIENTS", OK);
        stat_card(&mut cols[1], &connected.to_string(), "TD CONNECTED", OK);
        // Compact label: keeps clear margin inside its 186px column even
        // under high DPI scaling.
        stat_card(&mut cols[2], &attention.to_string(), "ATTENTION", {
            if attention > 0 { WARN } else { TEXT_DIM }
        });
        stat_card(&mut cols[3], role.to_uppercase().as_str(), "ROLE", ACCENT);
    });

    ui.add_space(theme::sp::LG);

    // First-poll connecting hint (errors surface separately once known).
    if app.status.is_none() && app.error.is_none() {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.add_space(theme::sp::XS);
            ui.label(
                egui::RichText::new("connecting to daemon…")
                    .font(font_label())
                    .color(TEXT_FAINT),
            );
        });
        ui.add_space(theme::sp::SM);
    }

    // Latest errors card.
    let count = app.error_ring.len();
    theme::card(ui, |ui| {
        row_between(
            ui,
            18.0,
            |ui| {
                ui.label(
                    egui::RichText::new("LATEST ERRORS")
                        .font(font_meta())
                        .color(TEXT_DIM),
                );
            },
            |ui| {
                if count > 0 {
                    ui.label(
                        egui::RichText::new(count.to_string())
                            .font(font_meta())
                            .color(ERR),
                    );
                }
            },
        );
        ui.add_space(theme::sp::XS);
        if app.error_ring.is_empty() {
            ui.label(
                egui::RichText::new("No recent errors")
                    .font(font_label())
                    .color(TEXT_FAINT),
            );
        } else {
            egui::ScrollArea::vertical()
                .max_height(220.0)
                .auto_shrink(false)
                .show(ui, |ui| {
                    for msg in app.error_ring.clone().iter() {
                        error_row(ui, msg);
                    }
                });
        }
    });
}

/// Lifecycle card: identity line plus the Restart / Stop / Reveal-.tox actions
/// that moved out of the popup header in pass 5.
fn daemon_card(app: &mut DashboardApp, ui: &mut egui::Ui) {
    let info = app
        .status
        .as_ref()
        .map(|s| (s.role.to_ascii_lowercase(), s.version.clone(), s.pid));

    theme::card(ui, |ui| {
        // Right side is laid out right-to-left: add Stop first so it lands at
        // the trailing edge and the visual order reads Reveal · Restart · Stop.
        row_between(
            ui,
            ROW_H,
            |ui| {
                ui.label(
                    egui::RichText::new("DAEMON")
                        .font(font_meta())
                        .color(TEXT_FAINT),
                );
            },
            |ui| {
                if ghost_button(ui, "Stop", TEXT_DIM, ERR).clicked() {
                    app.shutdown_daemon();
                }
                if ghost_button(ui, "Restart", TEXT_DIM, ACCENT).clicked() {
                    app.restart_daemon();
                }
                if ghost_button(ui, "Reveal .tox", TEXT_DIM, ACCENT).clicked() {
                    app.reveal_tox();
                }
            },
        );
        ui.add_space(theme::sp::XS);
        let line = match &info {
            Some((role, version, pid)) => format!("{role} · v{version} · pid {pid}"),
            None => "not running".to_owned(),
        };
        ui.label(
            egui::RichText::new(line)
                .font(font_mono())
                .color(if info.is_some() { TEXT_DIM } else { TEXT_FAINT }),
        );
    });
}

fn stat_card(ui: &mut egui::Ui, value: &str, title: &str, value_color: Color32) {
    let size = egui::vec2(ui.available_width(), 68.0);
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    ui.painter()
        .rect_filled(rect, RADIUS_MD, BG_PANEL);
    ui.painter().rect_stroke(
        rect,
        RADIUS_MD,
        egui::Stroke::new(1.0, BORDER),
        egui::StrokeKind::Inside,
    );
    ui.painter().text(
        egui::pos2(rect.left() + CARD_PAD + 2.0, rect.top() + 26.0),
        egui::Align2::LEFT_CENTER,
        value,
        font_title(),
        value_color,
    );
    ui.painter().text(
        egui::pos2(rect.left() + CARD_PAD + 2.0, rect.bottom() - theme::sp::LG),
        egui::Align2::LEFT_CENTER,
        title,
        font_meta(),
        TEXT_FAINT,
    );
}

fn error_row(ui: &mut egui::Ui, msg: &str) {
    let h = ROW_H;
    let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), h), Sense::hover());
    ui.painter()
        .circle_filled(egui::pos2(rect.left() + 5.0, rect.center().y), 3.0, ERR);
    ui.painter().text(
        egui::pos2(rect.left() + 16.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        msg,
        font_mono(),
        TEXT_DIM,
    );
}

fn fleet(app: &mut DashboardApp, ui: &mut egui::Ui) {
    egui::ScrollArea::vertical()
        .auto_shrink(false)
        .show(ui, |ui| {
            app.draw_master_actions(ui);
            app.draw_mcp_section(ui);
            app.draw_td_section(ui);
        });

    // Federation overlays as centered modal cards; Esc or the panel's
    // "← Back" (fleet_panel = None) dismisses them.
    match app.fleet_panel {
        FleetPanel::None => {}
        FleetPanel::AddSlave => {
            modal_shell(ui.ctx(), "dash_add_slave", |ui| {
                app.draw_add_slave_panel(ui);
            });
        }
        FleetPanel::SlaveSettings => {
            modal_shell(ui.ctx(), "dash_slave_settings", |ui| {
                app.draw_slave_settings_panel(ui);
            });
        }
    }
    if app.fleet_panel != FleetPanel::None && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        app.fleet_panel = FleetPanel::None;
    }
}

fn modal_shell(ctx: &egui::Context, id: &str, add: impl FnOnce(&mut egui::Ui)) {
    egui::Modal::new(egui::Id::new(id))
        .frame(
            egui::Frame::NONE
                .fill(theme::BG_WINDOW)
                .stroke(egui::Stroke::new(1.0, BORDER))
                .corner_radius(egui::CornerRadius::same(RADIUS_MD as u8))
                .inner_margin(egui::Margin::same(theme::sp::LG as i8)),
        )
        .show(ctx, |ui| {
            ui.set_width(MODAL_W);
            add(ui);
        });
}

/// Wide log stream: server-side level/src filters (shared state), client-side
/// text search, follow/pause controls, click-to-expand detail with Ctrl+C copy.
fn logs(app: &mut DashboardApp, ui: &mut egui::Ui) {
    app.ensure_logs_dir();

    // Keyboard contract: F follow · Space pause · Esc back to Overview —
    // suppressed while the search box owns focus so typing is never hijacked.
    let search_id = egui::Id::new("dash_logs_search");
    let typing = ui.memory(|m| m.has_focus(search_id));
    if !typing {
        ui.input(|i| {
            if i.key_pressed(egui::Key::F) {
                app.logs_view.follow = !app.logs_view.follow;
            }
            if i.key_pressed(egui::Key::Space) {
                app.logs_view.paused = !app.logs_view.paused;
            }
            if i.key_pressed(egui::Key::Escape) {
                app.dash_tab = DashTab::Overview;
            }
        });
    }

    // Toolbar: level chips · source chips · right side follow/pause/folder.
    ui.horizontal(|ui| {
        for (label, level) in [("ALL", None), ("ERR", Some("error")), ("WRN", Some("warn"))] {
            let active = app.logs_view.min_level == level;
            if chip(ui, label, active).clicked() && !active {
                app.logs_view.min_level = level;
                app.reset_logs_filter_state();
            }
        }
        ui.add_space(theme::sp::SM);
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

    // Search row.
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        egui::TextEdit::singleline(&mut app.logs_view.text_filter)
            .id(search_id)
            .desired_width(ui.available_width() - 34.0)
            .hint_text("Filter message or target…")
            .show(ui);
        if !app.logs_view.text_filter.is_empty() && ghost_button(ui, "×", TEXT_DIM, TEXT).clicked()
        {
            app.logs_view.text_filter.clear();
        }
    });
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
                    ui.allocate_exact_size(egui::vec2(full, 18.0), Sense::click());
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

    // Detail drawer for the expanded record.
    if let Some(seq) = app.logs_view.expanded {
        if let Some(r) = app.logs_view.buf.iter().find(|r| r.seq == seq) {
            ui.add_space(6.0);
            egui::Frame::NONE
                .fill(BG_PANEL)
                .stroke(egui::Stroke::new(1.0, BORDER))
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

// ---------------------------------------------------------------------------
// Settings tab — wide section cards + two-column form rows.
// ---------------------------------------------------------------------------

/// Fixed label-column width for `row_wide`.
const LABEL_COL_W: f32 = 266.0;

fn settings(app: &mut DashboardApp, ui: &mut egui::Ui) {
    // Action toolbar: Reset · Discard left, Save right.
    ui.horizontal(|ui| {
        if ghost_button(ui, "Reset to defaults", TEXT_DIM, WARN).clicked() {
            app.reset_settings();
        }
        if ghost_button(ui, "Discard changes", TEXT_DIM, TEXT).clicked() {
            app.discard_settings();
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if filled_button(ui, "Save").clicked() {
                app.save_settings();
            }
        });
    });

    // Sticky one-click restart prompt after a restart-requiring save.
    if app.needs_restart {
        ui.add_space(6.0);
        let full = ui.available_width();
        let (rect, _) = ui.allocate_exact_size(egui::vec2(full, 34.0), egui::Sense::hover());
        ui.painter().rect_filled(rect, 4.0, BG_PANEL);
        ui.painter().rect_stroke(
            rect,
            4.0,
            egui::Stroke::new(1.0, WARN),
            egui::StrokeKind::Inside,
        );
        ui.painter().text(
            egui::pos2(rect.left() + 14.0, rect.center().y),
            egui::Align2::LEFT_CENTER,
            "Settings changed — a restart is needed for some values",
            font_label(),
            WARN,
        );
        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(rect.shrink2(egui::vec2(12.0, 5.0)))
                .layout(egui::Layout::right_to_left(egui::Align::Center)),
        );
        if filled_button(&mut child, "Restart to apply").clicked() {
            app.restart_daemon();
            app.needs_restart = false;
        }
        ui.add_space(2.0);
        if ghost_button(ui, "Dismiss", TEXT_DIM, TEXT).clicked() {
            app.needs_restart = false;
        }
    }
    ui.add_space(8.0);

    egui::ScrollArea::vertical()
        .auto_shrink(false)
        .show(ui, |ui| {
            section_card(ui, "GENERAL", |ui| {
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(app.config_path.display().to_string())
                            .font(font_mono())
                            .color(TEXT_FAINT),
                    )
                    .truncate(),
                );
                ui.label(
                    egui::RichText::new(
                        "Some changes need a restart — you'll get a one-click prompt after saving.",
                    )
                    .font(font_meta())
                    .color(TEXT_DIM),
                );
                if let Some(err) = &app.settings_error {
                    ui.colored_label(ERR, err.clone());
                }
            });

            section_card(ui, "SERVER", |ui| {
                row_wide(ui, "Port", DashboardApp::field_help("server.port"), |ui| {
                    ui.add(
                        egui::DragValue::new(&mut app.draft.server.port)
                            .range(1..=65535)
                            .speed(1),
                    );
                });
            });

            section_card(ui, "NETWORK", |ui| network_card(app, ui));
            section_card(ui, "FEDERATION", |ui| federation_card(app, ui));

            section_card(ui, "DAEMON", |ui| {
                row_wide(
                    ui,
                    "Keep alive",
                    DashboardApp::field_help("daemon.keep_alive"),
                    |ui| {
                        ui.checkbox(&mut app.draft.daemon.keep_alive, "");
                    },
                );
                row_wide(
                    ui,
                    "Always on",
                    DashboardApp::field_help("daemon.always_on"),
                    |ui| {
                        ui.checkbox(&mut app.draft.daemon.always_on, "");
                    },
                );
                row_wide(
                    ui,
                    "Show tray",
                    DashboardApp::field_help("daemon.show_tray"),
                    |ui| {
                        ui.checkbox(&mut app.draft.daemon.show_tray, "");
                    },
                );
            });

            section_card(ui, "BRIDGE", |ui| {
                row_wide(
                    ui,
                    "Call timeout (s)",
                    DashboardApp::field_help("bridge.call_timeout_secs"),
                    |ui| {
                        ui.add(secs_drag(&mut app.draft.bridge.call_timeout_secs, 600));
                    },
                );
                row_wide(
                    ui,
                    "Script timeout (s)",
                    DashboardApp::field_help("bridge.script_timeout_secs"),
                    |ui| {
                        ui.add(secs_drag(&mut app.draft.bridge.script_timeout_secs, 600));
                    },
                );
                row_wide(
                    ui,
                    "Heartbeat interval (s)",
                    DashboardApp::field_help("bridge.heartbeat_interval_secs"),
                    |ui| {
                        ui.add(secs_drag(
                            &mut app.draft.bridge.heartbeat_interval_secs,
                            120,
                        ));
                    },
                );
                row_wide(
                    ui,
                    "Pong timeout (s)",
                    DashboardApp::field_help("bridge.pong_timeout_secs"),
                    |ui| {
                        ui.add(secs_drag(&mut app.draft.bridge.pong_timeout_secs, 120));
                    },
                );
                row_wide(
                    ui,
                    "Idle dead (s)",
                    DashboardApp::field_help("bridge.idle_dead_secs"),
                    |ui| {
                        ui.add(secs_drag(&mut app.draft.bridge.idle_dead_secs, 300));
                    },
                );
            });

            section_card(ui, "ADVANCED", |ui| {
                row_wide(
                    ui,
                    "Data dir",
                    DashboardApp::field_help("advanced.data_dir"),
                    |ui| {
                        path_edit_wide(ui, &mut app.data_dir_edit, false);
                    },
                );
                row_wide(
                    ui,
                    "Bridge dir",
                    DashboardApp::field_help("advanced.bridge_dir"),
                    |ui| {
                        path_edit_wide(ui, &mut app.bridge_dir_edit, false);
                    },
                );
                row_wide(
                    ui,
                    "Catalog",
                    DashboardApp::field_help("advanced.catalog_path"),
                    |ui| {
                        path_edit_wide(ui, &mut app.catalog_path_edit, false);
                    },
                );
                if !app.daemon_bin_edit.is_empty() {
                    row_wide(ui, "Daemon bin", "", |ui| {
                        path_edit_wide(ui, &mut app.daemon_bin_edit, true);
                    });
                }
            });
        });
}

fn secs_drag<'a>(value: &'a mut u64, max: u64) -> egui::DragValue<'a> {
    egui::DragValue::new(value).range(1..=max).speed(1)
}

fn network_card(app: &mut DashboardApp, ui: &mut egui::Ui) {
    let sharing = !tdmcp_config::is_loopback_bind(&app.draft.server.bind_address);
    row_wide(
        ui,
        "Share on my network",
        "Make this daemon reachable on your local network — required for federation. Auth is a separate, optional choice.",
        |ui| {
            let mut sharing_now = sharing;
            if ui.checkbox(&mut sharing_now, "").changed() {
                if sharing_now {
                    app.set_sharing(true);
                } else if app.draft.federation.role != "standalone" {
                    app.confirm_turn_off_sharing = true;
                } else {
                    app.set_sharing(false);
                }
            }
        },
    );
    ui.label(
        egui::RichText::new(if sharing {
            format!(
                "{}:{} on your network",
                app.draft.server.bind_address, app.draft.server.port
            )
        } else {
            "Only this machine (127.0.0.1)".to_owned()
        })
        .font(font_meta())
        .color(TEXT_DIM),
    );
    if app.confirm_turn_off_sharing {
        ui.colored_label(WARN, "This will disconnect federation on this machine.");
        ui.horizontal(|ui| {
            if filled_button(ui, "Turn off anyway").clicked() {
                app.set_sharing(false);
            }
            ui.add_space(4.0);
            if ghost_button(ui, "Cancel", TEXT_DIM, TEXT).clicked() {
                app.confirm_turn_off_sharing = false;
            }
        });
    }
    row_wide(
        ui,
        "Auth PSK (optional)",
        "Leave blank to allow anyone on your network to connect. Set a PSK to require it.",
        |ui| {
            let resp = ui.add_sized(
                egui::vec2(ui.available_width().min(320.0), 22.0),
                egui::TextEdit::singleline(&mut app.draft.auth.psk)
                    .font(font_mono())
                    .password(!app.show_psk),
            );
            if resp.changed() {
                app.draft.auth.mode = if app.draft.auth.psk.trim().is_empty() {
                    "none"
                } else {
                    "psk"
                }
                .to_owned();
            }
            if ghost_button(
                ui,
                if app.show_psk { "hide" } else { "show" },
                TEXT_DIM,
                TEXT,
            )
            .clicked()
            {
                app.show_psk = !app.show_psk;
            }
            if ghost_button(ui, "copy", TEXT_DIM, ACCENT)
                .on_hover_text("Copy to clipboard — paste into another machine's Master PSK")
                .clicked()
            {
                ui.ctx().copy_text(app.draft.auth.psk.clone());
            }
        },
    );
    egui::CollapsingHeader::new("Advanced (manual bind & auth)")
        .id_salt("dash_network_advanced")
        .default_open(false)
        .show(ui, |ui| {
            row_wide(
                ui,
                "Bind address",
                DashboardApp::field_help("server.bind_address"),
                |ui| {
                    ui.add_sized(
                        egui::vec2(ui.available_width().min(280.0), 22.0),
                        egui::TextEdit::singleline(&mut app.draft.server.bind_address)
                            .font(font_mono()),
                    );
                },
            );
            row_wide(
                ui,
                "Auth mode",
                DashboardApp::field_help("auth.mode"),
                |ui| {
                    let mut mode_psk = app.draft.auth.mode == "psk";
                    if ui.selectable_label(!mode_psk, "none").clicked() {
                        app.draft.auth.mode = "none".to_owned();
                        mode_psk = false;
                    }
                    if ui.selectable_label(mode_psk, "psk").clicked() {
                        app.draft.auth.mode = "psk".to_owned();
                        if app.draft.auth.psk.trim().is_empty() {
                            app.draft.auth.psk = crate::generate_psk();
                        }
                    }
                },
            );
        });
}

fn federation_card(app: &mut DashboardApp, ui: &mut egui::Ui) {
    let sharing = !tdmcp_config::is_loopback_bind(&app.draft.server.bind_address);
    row_wide(
        ui,
        "Role",
        DashboardApp::field_help("federation.role"),
        |ui| {
            let current = app.draft.federation.role.clone();
            if ui
                .selectable_label(current == "standalone", "Solo")
                .clicked()
                && current != "standalone"
            {
                app.draft.federation.role = "standalone".to_owned();
                app.role_change_note = Some("role: standalone (restart to apply)".to_owned());
            }
            ui.add_enabled_ui(sharing, |ui| {
                if ui.selectable_label(current == "master", "Master").clicked()
                    && current != "master"
                {
                    app.draft.federation.role = "master".to_owned();
                    app.role_change_note = Some("role: master (restart to apply)".to_owned());
                }
                if ui
                    .selectable_label(current == "slave", "Join a master")
                    .clicked()
                    && current != "slave"
                {
                    app.draft.federation.role = "slave".to_owned();
                    app.role_change_note = Some("role: slave (restart to apply)".to_owned());
                }
            });
        },
    );
    if !sharing {
        ui.label(
            egui::RichText::new("Turn on sharing above to become a master or join one.")
                .font(font_meta())
                .color(TEXT_DIM),
        );
    }
    if let Some(note) = &app.role_change_note {
        ui.label(
            egui::RichText::new(note.clone())
                .font(font_meta())
                .color(WARN),
        );
    }
    if !app.draft.federation.daemon_id.is_empty() {
        row_wide(
            ui,
            "Daemon ID",
            DashboardApp::field_help("federation.daemon_id"),
            |ui| {
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(&app.draft.federation.daemon_id)
                            .font(font_mono())
                            .color(TEXT_FAINT),
                    )
                    .truncate(),
                );
            },
        );
    }
    if app.draft.federation.role == "master" {
        let (this_url, _) = app.master_federation_values();
        row_wide(
            ui,
            "This machine's URL",
            "Share with a machine that wants to join as a slave.",
            |ui| {
                ui.label(
                    egui::RichText::new(&this_url)
                        .font(font_mono())
                        .color(TEXT_DIM),
                );
                if ghost_button(ui, "copy", TEXT_DIM, ACCENT).clicked() {
                    ui.ctx().copy_text(this_url.clone());
                }
            },
        );
    }
    if app.draft.federation.role == "slave" {
        ui.horizontal(|ui| {
            if app.scan_busy && app.scan_purpose == ScanPurpose::JoinMaster {
                ui.label(
                    egui::RichText::new("scanning…")
                        .font(font_meta())
                        .color(TEXT_DIM),
                );
            } else if ghost_button(ui, "Scan for masters", TEXT_DIM, ACCENT).clicked() {
                app.start_scan(tdmcp_config::DEFAULT_PORT, ScanPurpose::JoinMaster);
            }
        });
        app.draw_scan_results(ui, ScanPurpose::JoinMaster);
        row_wide(
            ui,
            "Master URL",
            DashboardApp::field_help("federation.master_url"),
            |ui| {
                ui.add_sized(
                    egui::vec2(ui.available_width().min(360.0), 22.0),
                    egui::TextEdit::singleline(&mut app.draft.federation.master_url)
                        .font(font_mono()),
                );
            },
        );
        row_wide(
            ui,
            "Master PSK (optional)",
            DashboardApp::field_help("federation.master_psk"),
            |ui| {
                let resp = ui.add_sized(
                    egui::vec2(ui.available_width().min(280.0), 22.0),
                    egui::TextEdit::singleline(&mut app.draft.federation.master_psk)
                        .font(font_mono())
                        .password(!app.show_master_psk),
                );
                if app.focus_master_psk {
                    resp.request_focus();
                    app.focus_master_psk = false;
                }
                if ghost_button(
                    ui,
                    if app.show_master_psk { "hide" } else { "show" },
                    TEXT_DIM,
                    TEXT,
                )
                .clicked()
                {
                    app.show_master_psk = !app.show_master_psk;
                }
            },
        );
        ui.label(
            egui::RichText::new(
                "Only needed if the master requires a PSK — copy it from the master's Settings, Federation card.",
            )
            .font(font_meta())
            .color(TEXT_DIM),
        );
    }
}

/// Section card: rounded panel with a faint uppercase title and content.
fn section_card(ui: &mut egui::Ui, title: &str, add: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::NONE
        .fill(BG_ROW)
        .stroke(egui::Stroke::new(1.0, BORDER))
        .corner_radius(egui::CornerRadius::same(6))
        .inner_margin(egui::Margin::same(14))
        .show(ui, |ui| {
            ui.label(
                egui::RichText::new(title)
                    .font(font_meta())
                    .color(TEXT_FAINT),
            );
            ui.add_space(6.0);
            add(ui);
        });
    ui.add_space(10.0);
}

/// Wide settings row: fixed label column, control fills the rest.
fn row_wide(ui: &mut egui::Ui, label: &str, help: &str, add: impl FnOnce(&mut egui::Ui)) {
    let h = 30.0;
    let full = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(egui::vec2(full, h), egui::Sense::hover());
    if response.hovered() {
        ui.painter().rect_filled(rect, 4.0, BG_HOVER);
    }
    ui.painter().text(
        egui::pos2(rect.left() + 4.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        label,
        font_label(),
        TEXT,
    );
    let control = egui::Rect::from_min_max(
        egui::pos2(rect.left() + LABEL_COL_W, rect.top()),
        egui::pos2(rect.right() - 4.0, rect.bottom()),
    );
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(control)
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    add(&mut child);
    if !help.is_empty() {
        response.on_hover_text(help.to_owned());
    }
}

/// Full-width mono path input; `read_only` locks it (the resolved daemon bin).
fn path_edit_wide(ui: &mut egui::Ui, text: &mut String, read_only: bool) {
    ui.add_sized(
        egui::vec2(ui.available_width(), 22.0),
        egui::TextEdit::singleline(text)
            .font(font_mono())
            .interactive(!read_only),
    );
}
