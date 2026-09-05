//! Dashboard Settings tab — wide section cards + two-column form rows,
//! network sharing, federation role editing, advanced paths.

use eframe::egui;

use crate::app::{generate_psk, DashboardApp};
use crate::theme::{
    banner, filled_button, font_label, font_meta, font_mono, ghost_button, segmented, BannerTone,
    ACCENT, BG_HOVER, BG_ROW, BORDER, ERR, TEXT, TEXT_DIM, TEXT_FAINT, WARN,
};
use crate::wire::ScanPurpose;

/// Fixed label-column width for `row_wide`.
const LABEL_COL_W: f32 = 190.0;

fn settings_actions(app: &mut DashboardApp, ui: &mut egui::Ui) {
    if app.settings_save.is_running() {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label("Saving settings…");
        });
    }
    let dirty = app.config_dirty();

    // Save first, with wrapping: critical controls must stay reachable even
    // when the OS gives us less than the requested minimum window width.
    ui.horizontal_wrapped(|ui| {
        ui.add_enabled_ui(dirty, |ui| {
            if filled_button(ui, "Save").clicked() {
                app.save_settings();
            }
        });
        ui.add_enabled_ui(dirty, |ui| {
            if ghost_button(ui, "Discard changes", TEXT_DIM, TEXT).clicked() {
                app.discard_settings();
                app.snack("Changes discarded", crate::app::SnackTone::Info);
            }
        });
        if dirty {
            let _ = crate::theme::badge(ui, "unsaved changes", crate::theme::BadgeKind::Warn);
        }
        if ghost_button(ui, "Reset to defaults", TEXT_DIM, WARN).clicked() {
            app.reset_settings();
        }
    });

    // Sticky one-click restart prompt after a restart-requiring save.
    if app.needs_restart {
        ui.add_space(6.0);
        banner(
            ui,
            BannerTone::Warn,
            "A restart is needed for some saved values",
        );
        ui.horizontal(|ui| {
            if filled_button(ui, "Restart to apply").clicked() {
                app.restart_daemon();
            }
        });
    }
    if let Some(err) = &app.settings_error {
        ui.colored_label(ERR, err);
    }
    ui.add_space(8.0);
}

pub(crate) fn federation(app: &mut DashboardApp, ui: &mut egui::Ui) {
    settings_actions(app, ui);
    if let Some(status) = &app.status {
        if !status.federation_connection.is_empty() {
            ui.label(
                egui::RichText::new(&status.federation_connection)
                    .font(font_label())
                    .color(TEXT),
            );
            ui.add_space(8.0);
        }
    } else {
        banner(
            ui,
            BannerTone::Warn,
            "Daemon unreachable — connection status unavailable",
        );
    }
    egui::ScrollArea::vertical().id_salt("federation_settings").auto_shrink(false).show(ui, |ui| {
        section_card(ui, "CONNECT COMPUTERS", false, |ui| federation_card(app, ui));
        let restart = app.draft.server != app.settings_loaded_snapshot.server
            || app.draft.auth != app.settings_loaded_snapshot.auth;
        section_card(ui, "NETWORK ACCESS", restart, |ui| network_card(app, ui));
        ui.label(egui::RichText::new(
            "Joined computers and their TouchDesigner processes appear in Overview. Sharing gives remote clients control of this computer: use a trusted network and an access key."
        ).font(font_meta()).color(TEXT_DIM));
    });
}

pub(crate) fn settings(app: &mut DashboardApp, ui: &mut egui::Ui) {
    settings_actions(app, ui);

    // Which sections hold restart-required fields that differ from the
    // loaded snapshot (mirrors restart_required_fields_changed).
    let (d, l) = (&app.draft, &app.settings_loaded_snapshot);
    let server_restart = d.server.port != l.server.port;
    let network_restart = d.server.bind_address != l.server.bind_address
        || d.auth.mode != l.auth.mode
        || d.auth.psk != l.auth.psk;

    egui::ScrollArea::vertical()
        .id_salt("general_settings")
        .auto_shrink(false)
        .show(ui, |ui| {
            section_card(ui, "GENERAL", false, |ui| {
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
                        "Federation, call timeouts and keep alive apply when saved. Network listeners and advanced process settings need a restart.",
                    )
                    .font(font_meta())
                    .color(TEXT_DIM),
                );
            });

            section_card(ui, "SERVER", server_restart, |ui| {
                row_wide(ui, "Port", DashboardApp::field_help("server.port"), |ui| {
                    ui.add(
                        egui::DragValue::new(&mut app.draft.server.port)
                            .range(1..=65535)
                            .speed(1),
                    );
                });
            });

            section_card(ui, "NETWORK", network_restart, |ui| network_card(app, ui));

            section_card(ui, "DAEMON", false, |ui| {
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

            section_card(ui, "BRIDGE", false, |ui| {
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

            section_card(ui, "DIALOGS", false, |ui| {
                row_wide(
                    ui,
                    "Enabled",
                    DashboardApp::field_help("dialogs.enabled"),
                    |ui| {
                        ui.checkbox(&mut app.draft.dialogs.enabled, "");
                    },
                );
                row_wide(
                    ui,
                    "Intercept bridged calls",
                    DashboardApp::field_help("dialogs.intercept"),
                    |ui| {
                        ui.checkbox(&mut app.draft.dialogs.intercept, "");
                    },
                );
                row_wide(
                    ui,
                    "Poll interval (ms)",
                    DashboardApp::field_help("dialogs.poll_ms"),
                    |ui| {
                        ui.add(
                            egui::DragValue::new(&mut app.draft.dialogs.poll_ms)
                                .range(250..=10_000)
                                .speed(50),
                        );
                    },
                );
                #[cfg(target_os = "macos")]
                if app.draft.dialogs.enabled {
                    ui.add_space(6.0);
                    banner(
                        ui,
                        BannerTone::Warn,
                        "macOS: enable Accessibility for tdmcp-daemon in System Settings → Privacy & Security → Accessibility (required for describe/dismiss; list still works via CGWindowList)",
                    );
                }
            });

            section_card(ui, "PROJECT", false, |ui| {
                ui.label(
                    egui::RichText::new("Template for spawn_td createIfMissing — copied when the target does not yet exist. Per-call templatePath can override this.")
                        .font(font_meta())
                        .color(TEXT_DIM),
                );
                ui.add_space(4.0);
                row_wide(
                    ui,
                    "Template .toe",
                    DashboardApp::field_help("project.template_path"),
                    |ui| {
                        path_edit_wide(ui, &mut app.template_path_edit, false);
                    },
                );
                ui.horizontal(|ui| {
                    if ghost_button(ui, "Locate…", TEXT_DIM, TEXT).clicked() {
                        app.locate_template();
                    }
                    if ghost_button(ui, "Reveal", TEXT_DIM, TEXT).clicked() {
                        app.reveal_template();
                    }
                    if ghost_button(ui, "Open", TEXT_DIM, TEXT)
                        .on_hover_text("Open the template with its default app (TD) — accept any build upgrade prompt then Save over it")
                        .clicked()
                    {
                        app.open_template();
                    }
                });
                // Show effective path hint when empty (falls back to {data_dir}/template.toe).
                if app.template_path_edit.trim().is_empty() {
                    let effective = app.effective_template_path();
                    ui.label(
                        egui::RichText::new(format!("effective: {}", effective.display()))
                            .font(font_mono())
                            .color(TEXT_FAINT),
                    );
                }
            });

            section_card(ui, "OFFICIAL TOOLS", false, |ui| {
                ui.label(
                    egui::RichText::new(
                        "Pin the TouchDesigner install used for spawn and offline project I/O. Leave everything empty for auto-detection.",
                    )
                    .font(font_meta())
                    .color(TEXT_DIM),
                );
                ui.add_space(4.0);
                row_wide(
                    ui,
                    "TouchDesigner exe",
                    DashboardApp::field_help("official_tools.td_exe"),
                    |ui| {
                        path_edit_wide(ui, &mut app.official_td_exe_edit, false);
                    },
                );
                row_wide(
                    ui,
                    "toeexpand path",
                    DashboardApp::field_help("official_tools.expand_path"),
                    |ui| {
                        path_edit_wide(ui, &mut app.official_expand_edit, false);
                    },
                );
                row_wide(
                    ui,
                    "toecollapse path",
                    DashboardApp::field_help("official_tools.collapse_path"),
                    |ui| {
                        path_edit_wide(ui, &mut app.official_collapse_edit, false);
                    },
                );
                #[cfg(all(not(windows), not(target_os = "macos")))]
                {
                    row_wide(
                        ui,
                        "Wine binary (Linux)",
                        DashboardApp::field_help("official_tools.wine_exe"),
                        |ui| {
                            path_edit_wide(ui, &mut app.official_wine_exe_edit, false);
                        },
                    );
                    row_wide(
                        ui,
                        "Wine prefix (Linux)",
                        DashboardApp::field_help("official_tools.wine_prefix"),
                        |ui| {
                            path_edit_wide(ui, &mut app.official_wine_prefix_edit, false);
                        },
                    );
                    ui.label(
                        egui::RichText::new(
                            "Auto-detect scans $WINEPREFIX, ~/.wine, ~/.local/share/wineprefixes/* and the touchdesigner-linux (AUR) prefix; the prefix to run tools in is derived from the install path. Set the Wine binary/prefix here only if your setup differs.",
                        )
                        .font(font_meta())
                        .color(TEXT_FAINT),
                    );
                }
            });

            section_card(ui, "ADVANCED", false, |ui| {
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
            let resp = secret_edit(ui, &mut app.draft.auth.psk, &mut app.show_psk, true);
            if resp.changed() {
                app.draft.auth.mode = if app.draft.auth.psk.trim().is_empty() {
                    "none"
                } else {
                    "psk"
                }
                .to_owned();
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
                            app.draft.auth.psk = generate_psk();
                        }
                    }
                },
            );
        });
}

fn federation_card(app: &mut DashboardApp, ui: &mut egui::Ui) {
    row_wide(
        ui,
        "Role",
        DashboardApp::field_help("federation.role"),
        |ui| {
            const OPTIONS: [&str; 3] = ["This computer", "Coordinate", "Join"];
            const ROLES: [&str; 3] = ["standalone", "master", "slave"];
            let current = app.draft.federation.role.clone();
            let selected = ROLES.iter().position(|r| *r == current).unwrap_or(0);
            if let Some(i) = segmented(ui, &OPTIONS, selected) {
                app.draft.federation.role = ROLES[i].to_owned();
                // Federation needs LAN reachability — enable it automatically
                // so the old interlock foot-gun disappears.
                if i > 0 && tdmcp_config::is_loopback_bind(&app.draft.server.bind_address) {
                    app.set_sharing(true);
                    app.role_change_note = Some(
                        "Network sharing enabled. Restart once to open the LAN listener; later federation changes apply on Save.".to_owned(),
                    );
                } else {
                    app.role_change_note = Some("Federation changes apply when saved.".to_owned());
                }
            }
        },
    );
    ui.label(
        egui::RichText::new(match app.draft.federation.role.as_str() {
            "master" => {
                "Connect your AI client here to control TouchDesigner on every joined computer."
            }
            "slave" => "Share this computer's TouchDesigner instances with a coordinator.",
            _ => "Control TouchDesigner on this computer only.",
        })
        .font(font_meta())
        .color(TEXT_DIM),
    );
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
            "Share with a computer that wants to join this coordinator.",
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
            } else if ghost_button(ui, "Find coordinators", TEXT_DIM, ACCENT).clicked() {
                app.start_scan(tdmcp_config::DEFAULT_PORT, ScanPurpose::JoinMaster);
            }
        });
        app.draw_scan_results(ui, ScanPurpose::JoinMaster);
        row_wide(
            ui,
            "Coordinator URL",
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
            "Coordinator key",
            DashboardApp::field_help("federation.master_psk"),
            |ui| {
                let resp = secret_edit(
                    ui,
                    &mut app.draft.federation.master_psk,
                    &mut app.show_master_psk,
                    false,
                );
                if app.focus_master_psk {
                    resp.request_focus();
                    app.focus_master_psk = false;
                }
            },
        );
        ui.label(
            egui::RichText::new(
                "Only needed if the coordinator requires a key. Copy its incoming key from Federation → Network access.",
            )
            .font(font_meta())
            .color(TEXT_DIM),
        );
    }
}

/// Section card: rounded panel with a faint uppercase title (plus an amber
/// `restart` chip when that section's restart-required fields changed) and content.
fn section_card(
    ui: &mut egui::Ui,
    title: &str,
    restart_chip: bool,
    add: impl FnOnce(&mut egui::Ui),
) {
    egui::Frame::NONE
        .fill(BG_ROW)
        .stroke(egui::Stroke::new(1.0, BORDER))
        .corner_radius(egui::CornerRadius::same(6))
        .inner_margin(egui::Margin::same(14))
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(title)
                        .font(font_meta())
                        .color(TEXT_FAINT),
                );
                if restart_chip {
                    let _ = crate::theme::badge(ui, "restart", crate::theme::BadgeKind::Warn)
                        .on_hover_text("This section has saved values needing a daemon restart");
                }
            });
            ui.add_space(6.0);
            add(ui);
        });
    ui.add_space(10.0);
}

/// Two columns when there is room; stack label/control on narrow windows.
fn row_wide(ui: &mut egui::Ui, label: &str, help: &str, add: impl FnOnce(&mut egui::Ui)) {
    if ui.available_width() < 480.0 {
        ui.vertical(|ui| {
            ui.label(egui::RichText::new(label).font(font_label()).color(TEXT))
                .on_hover_text(help);
            ui.horizontal_wrapped(add);
        });
        ui.add_space(8.0);
        return;
    }
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

/// Reserve action widths before sizing the input, so keys remain editable on
/// narrow windows and high-DPI desktops without clipping show/copy controls.
fn secret_edit(
    ui: &mut egui::Ui,
    value: &mut String,
    shown: &mut bool,
    copy: bool,
) -> egui::Response {
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        if copy && ghost_button(ui, "copy", TEXT_DIM, ACCENT).clicked() {
            ui.ctx().copy_text(value.clone());
        }
        if ghost_button(ui, if *shown { "hide" } else { "show" }, TEXT_DIM, TEXT).clicked() {
            *shown = !*shown;
        }
        ui.add_sized(
            egui::vec2(ui.available_width().max(24.0), 22.0),
            egui::TextEdit::singleline(value)
                .font(font_mono())
                .password(!*shown),
        )
    })
    .inner
}

#[cfg(test)]
mod layout_tests {
    use super::*;

    #[test]
    fn form_controls_stay_inside_narrow_and_wide_rows() {
        for width in [260.0, 320.0, 480.0, 800.0] {
            let ctx = egui::Context::default();
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(width, 400.0),
                )),
                ..Default::default()
            };
            let _ = ctx.run_ui(input, |ui| {
                egui::CentralPanel::default().show(ui, |ui| {
                    let bounds = ui.available_rect_before_wrap();
                    row_wide(ui, "Wine executable", "Runner path", |ui| {
                        let rect = ui
                            .add_sized(
                                egui::vec2(ui.available_width(), 22.0),
                                egui::TextEdit::singleline(&mut String::new()),
                            )
                            .rect;
                        assert!(rect.width() > 0.0);
                        assert!(rect.left() >= bounds.left());
                        assert!(rect.right() <= bounds.right() + 0.1);
                        if bounds.width() < 480.0 {
                            assert!(
                                rect.left() - bounds.left() < 1.0,
                                "narrow form did not stack"
                            );
                        }
                    });
                });
            });
        }
    }
}
