//! Merged Overview page: daemon strip → stat tiles → TouchDesigner fleet →
//! MCP clients → activity/errors, all in one scroll, with federation modals
//! layered on top (Esc / ← Back dismisses).

use eframe::egui;

use super::widgets::{modal_shell, stat_card};
use crate::app::{DashboardApp, FleetPanel};
use crate::platform::reveal_in_file_manager;
use crate::theme::sp;
use crate::theme::{
    badge, font_label, font_meta, font_mono, ghost_button, row_between, BadgeKind, ACCENT, ERR, OK,
    ROW_H, TEXT_DIM, TEXT_FAINT, WARN,
};

pub(crate) fn overview(app: &mut DashboardApp, ui: &mut egui::Ui) {
    egui::ScrollArea::vertical()
        .auto_shrink(false)
        .show(ui, |ui| {
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

            ui.add_space(sp::SM);
            daemon_card(app, ui);

            ui.add_space(sp::MD);
            ui.columns(4, |cols| {
                stat_card(&mut cols[0], &mcp_n.to_string(), "MCP CLIENTS", OK);
                stat_card(&mut cols[1], &connected.to_string(), "TD CONNECTED", OK);
                // Compact label: keeps clear margin inside its 186px column even
                // under high DPI scaling.
                stat_card(&mut cols[2], &attention.to_string(), "ATTENTION", {
                    if attention > 0 {
                        WARN
                    } else {
                        TEXT_DIM
                    }
                });
                stat_card(&mut cols[3], role.to_uppercase().as_str(), "ROLE", ACCENT);
            });

            ui.add_space(sp::LG);

            // First-poll connecting hint (errors surface in ACTIVITY once known).
            if app.status.is_none() && app.error.is_none() {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.add_space(sp::XS);
                    ui.label(
                        egui::RichText::new("connecting to daemon…")
                            .font(font_label())
                            .color(TEXT_FAINT),
                    );
                });
                ui.add_space(sp::SM);
            }

            // Workloads first, integrations second, problems last.
            app.draw_td_section(ui);
            ui.add_space(sp::MD);
            app.draw_mcp_section(ui);

            ui.add_space(sp::LG);
            activity_card(app, ui);
            ui.add_space(sp::SM);
        });

    // Federation overlays as centered modal cards; Esc or a panel's
    // "← Back" (`FleetPanel::None`) dismisses them.
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

/// Identity strip: role/version/bind chips + listening line. Lifecycle actions
/// live in the dashboard top bar (pass 10), reachable from every tab.
fn daemon_card(app: &mut DashboardApp, ui: &mut egui::Ui) {
    let info = app.status.as_ref().map(|s| {
        (
            s.role.clone(),
            s.version.clone(),
            s.pid,
            s.bind_address.clone(),
        )
    });
    let offline = info.is_none();
    let last_error = app.error.clone();
    let listen_port = crate::http::port_from_base(&app.admin_base);

    let role_disp = info
        .as_ref()
        .map(|(r, _, _, _)| r.to_ascii_uppercase())
        .unwrap_or_else(|| "OFFLINE".to_owned());

    super::widgets::card_with_header(
        ui,
        "DAEMON",
        None,
        // Identity only since pass 10 — lifecycle actions live in the top bar,
        // where they stay reachable from Logs and Settings too.
        |_ui| {},
        |ui| {
            ui.horizontal(|ui| {
                let _ = badge(
                    ui,
                    &role_disp,
                    if offline {
                        BadgeKind::Error
                    } else if role_disp == "MASTER" || role_disp == "SLAVE" {
                        BadgeKind::Accent
                    } else {
                        BadgeKind::Neutral
                    },
                );
                if let Some((_, version, pid, bind)) = &info {
                    let _ = badge(ui, &format!("v{version}"), BadgeKind::Neutral);
                    let networked = !tdmcp_config::is_loopback_bind(bind);
                    let _ = badge(
                        ui,
                        if networked { "network" } else { "loopback" },
                        if networked {
                            BadgeKind::Warn
                        } else {
                            BadgeKind::Ok
                        },
                    );
                    ui.label(
                        egui::RichText::new(format!("pid {pid}"))
                            .font(font_mono())
                            .color(TEXT_FAINT),
                    );
                }
            });
            ui.add_space(sp::XS);
            let line = match (&info, &last_error) {
                (Some((_, _, _, bind)), _) => format!("listening on {bind}:{listen_port}"),
                (None, Some(e)) => format!("unreachable — {e}"),
                (None, None) => "not running".to_owned(),
            };
            ui.label(
                egui::RichText::new(line)
                    .font(font_mono())
                    .color(match info.as_ref() {
                        Some(_) => TEXT_DIM,
                        None if last_error.is_some() => ERR,
                        None => TEXT_FAINT,
                    }),
            );
        },
    );
}

fn activity_card(app: &mut DashboardApp, ui: &mut egui::Ui) {
    let count = app.error_ring.len();
    let attention = app.attention;
    // The header's right slot can't borrow `app` (the body closure holds the
    // mutable borrow), so the click is ferried out through a cell.
    let clear_clicked = std::cell::Cell::new(false);
    super::widgets::card_with_header(
        ui,
        "ACTIVITY",
        attention.then_some(WARN),
        |ui| {
            if count > 0 {
                let _ = badge(ui, &count.to_string(), BadgeKind::Error);
                if ghost_button(ui, "Clear", TEXT_DIM, WARN)
                    .on_hover_text("Clear the activity list")
                    .clicked()
                {
                    clear_clicked.set(true);
                }
            }
        },
        |ui| {
            if app.error_ring.is_empty() {
                ui.label(
                    egui::RichText::new("No recent errors")
                        .font(font_label())
                        .color(TEXT_FAINT),
                );
            } else {
                let mut copied: Option<String> = None;
                egui::ScrollArea::vertical()
                    .max_height(180.0)
                    .auto_shrink(false)
                    .show(ui, |ui| {
                        for msg in app.error_ring.clone().iter() {
                            error_row(ui, msg, &mut copied);
                        }
                    });
                if let Some(m) = copied {
                    ui.ctx().copy_text(m);
                    app.snack("Copied to clipboard", crate::app::SnackTone::Info);
                }
            }
            if app.crash_count > 0 {
                ui.add_space(sp::XS);
                let crash_dir = app.data_dir.join("crash");
                let fallback = app.data_dir.clone();
                row_between(
                    ui,
                    ROW_H,
                    |ui| {
                        ui.label(
                            egui::RichText::new(format!("CRASH REPORTS · {}", app.crash_count))
                                .font(font_meta())
                                .color(WARN),
                        );
                    },
                    |ui| {
                        if ghost_button(ui, "Open folder", TEXT_DIM, ACCENT).clicked() {
                            let _ = reveal_in_file_manager(&crash_dir, &fallback);
                        }
                    },
                );
            }
        },
    );
    if clear_clicked.get() {
        app.error_ring.clear();
        app.snack("Activity cleared", crate::app::SnackTone::Ok);
    }
}

fn error_row(ui: &mut egui::Ui, msg: &str, copied: &mut Option<String>) {
    let h = ROW_H;
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), h), egui::Sense::click());
    let fill = if response.hovered() {
        crate::theme::BG_HOVER
    } else {
        egui::Color32::TRANSPARENT
    };
    ui.painter().rect_filled(rect, 4.0, fill);
    ui.painter()
        .circle_filled(egui::pos2(rect.left() + 5.0, rect.center().y), 3.0, ERR);
    ui.painter().text(
        egui::pos2(rect.left() + 16.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        msg,
        font_mono(),
        TEXT_DIM,
    );
    let clicked = response.clicked();
    response.on_hover_text("Click to copy");
    if clicked {
        *copied = Some(msg.to_owned());
    }
}
